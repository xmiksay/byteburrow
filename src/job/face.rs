use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::{info, warn};

use crate::entity::face_reference;
use crate::plugin::MergedClassification;

/// Model identity assumed for embeddings that predate model metadata (rows
/// backfilled by migration `..._face_reference_model_meta`, or raw plugin
/// output from a plugin version that did not emit these fields). Must match the
/// migration's `LEGACY_MODEL_*` constants so backfilled rows and legacy plugin
/// output share one comparable vector space.
const LEGACY_MODEL_ID: &str = "faceonnx-recognition-resnet27";
const LEGACY_MODEL_VERSION: &str = "1";

/// Persist face embeddings extracted by the faces plugin and match them
/// against confirmed contacts. Sole caller: `classify::run_classification`.
pub(super) async fn process_face_embeddings(
    db: &DatabaseConnection,
    hash_bytes: &[u8],
    merged: &mut MergedClassification,
) -> anyhow::Result<()> {
    // Extract raw embeddings (temporary key, not persisted to meta)
    let raw_embeddings = match merged.custom.remove("face_embeddings_raw") {
        Some(v) => v,
        None => return Ok(()),
    };

    let raw_arr = match raw_embeddings.as_array() {
        Some(a) => a.clone(),
        None => return Ok(()),
    };

    // Get face bounding boxes from the faces plugin output
    let faces_data = merged.custom.get("faces").cloned();
    let rects = faces_data
        .as_ref()
        .and_then(|f| f.get("rects"))
        .and_then(|r| r.as_array());

    // Load all confirmed face references for matching
    let confirmed_refs = face_reference::Entity::find()
        .filter(face_reference::Column::Confirmed.eq(true))
        .all(db)
        .await?;

    let face_count = rects.map(|r| r.len()).unwrap_or(0);
    let mut contact_matches: Vec<serde_json::Value> = vec![serde_json::Value::Null; face_count];

    for entry in &raw_arr {
        let face_index = entry
            .get("face_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let embedding_floats: Vec<f32> = match entry.get("embedding").and_then(|v| v.as_array()) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect(),
            None => continue,
        };

        if embedding_floats.is_empty() {
            continue;
        }

        // Model identity of this embedding. Older plugin output omits these
        // fields; fall back to the legacy identity so it stays comparable with
        // migration-backfilled rows.
        let model_id = entry
            .get("model_id")
            .and_then(|v| v.as_str())
            .unwrap_or(LEGACY_MODEL_ID)
            .to_string();
        let model_version = entry
            .get("model_version")
            .and_then(|v| v.as_str())
            .unwrap_or(LEGACY_MODEL_VERSION)
            .to_string();
        let dim = embedding_floats.len() as i32;

        let embedding_bytes = floats_to_bytes(&embedding_floats);

        // Get bbox from faces data
        let (bbox_x, bbox_y, bbox_w, bbox_h) =
            if let Some(rect) = rects.and_then(|r| r.get(face_index)) {
                (
                    rect.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                )
            } else {
                (0, 0, 0, 0)
            };

        // Upsert face_reference row
        let existing = face_reference::Entity::find()
            .filter(face_reference::Column::Hash.eq(hash_bytes.to_vec()))
            .filter(face_reference::Column::FaceIndex.eq(face_index as i16))
            .one(db)
            .await?;

        match existing {
            Some(existing_ref) => {
                let active = face_reference::ActiveModel {
                    id: Set(existing_ref.id),
                    embedding: Set(embedding_bytes),
                    model_id: Set(model_id.clone()),
                    model_version: Set(model_version.clone()),
                    dim: Set(dim),
                    bbox_x: Set(bbox_x),
                    bbox_y: Set(bbox_y),
                    bbox_w: Set(bbox_w),
                    bbox_h: Set(bbox_h),
                    ..Default::default()
                };
                active.update(db).await?;
            }
            None => {
                let active = face_reference::ActiveModel {
                    hash: Set(hash_bytes.to_vec()),
                    face_index: Set(face_index as i16),
                    bbox_x: Set(bbox_x),
                    bbox_y: Set(bbox_y),
                    bbox_w: Set(bbox_w),
                    bbox_h: Set(bbox_h),
                    embedding: Set(embedding_bytes),
                    model_id: Set(model_id.clone()),
                    model_version: Set(model_version.clone()),
                    dim: Set(dim),
                    confirmed: Set(false),
                    ..Default::default()
                };
                active.insert(db).await?;
            }
        }

        // Match against confirmed references, but only those produced by the
        // same model. Comparing across models silently corrupts the vector
        // space, so incomparable references are skipped (and counted) rather
        // than scored 0.
        let mut best_contact_id: Option<i32> = None;
        let mut best_similarity: f32 = 0.0;
        let mut skipped_cross_model = 0usize;

        for reference in &confirmed_refs {
            let Some(contact_id) = reference.contact_id else {
                continue;
            };
            if reference.model_id != model_id || reference.model_version != model_version {
                skipped_cross_model += 1;
                continue;
            }
            let ref_floats = bytes_to_floats(&reference.embedding);
            let sim = match cosine_similarity(&embedding_floats, &ref_floats) {
                Ok(sim) => sim,
                Err(e) => {
                    warn!(
                        reference_id = reference.id,
                        error = %e,
                        "skipping incomparable face embedding"
                    );
                    continue;
                }
            };
            if sim > best_similarity {
                best_similarity = sim;
                best_contact_id = Some(contact_id);
            }
        }

        if skipped_cross_model > 0 {
            warn!(
                face_index,
                model_id = %model_id,
                model_version = %model_version,
                skipped = skipped_cross_model,
                "skipped confirmed references from a different embedding model; \
                 re-embed them to make matching work across the model change"
            );
        }

        if best_similarity > 0.8 {
            if let Some(contact_id) = best_contact_id {
                if face_index < contact_matches.len() {
                    contact_matches[face_index] = serde_json::Value::Number(contact_id.into());
                }
                info!(
                    face_index,
                    contact_id,
                    similarity = best_similarity,
                    "Face matched to contact"
                );
            }
        }
    }

    // Store the contact match array in meta.custom
    merged.custom.insert(
        "face_embeddings".to_string(),
        serde_json::Value::Array(contact_matches),
    );

    Ok(())
}

/// Cosine similarity of two same-length embeddings. Returns an explicit error
/// on a dimension mismatch (incomparable vectors) instead of silently scoring 0,
/// which would hide a model/version mismatch. Callers must only pass embeddings
/// they have already confirmed share a model identity.
fn cosine_similarity(a: &[f32], b: &[f32]) -> anyhow::Result<f32> {
    if a.len() != b.len() {
        anyhow::bail!("embedding dimension mismatch: {} vs {}", a.len(), b.len());
    }
    if a.is_empty() {
        anyhow::bail!("empty embedding");
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        Ok(dot / (norm_a * norm_b))
    } else {
        Ok(0.0)
    }
}

/// Sole caller: `process_face_embeddings` (encodes a new embedding for storage).
fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Sole caller: `process_face_embeddings` (decodes a stored reference embedding).
fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = [0.5f32, 0.5, 0.5, 0.5];
        let sim = cosine_similarity(&v, &v).unwrap();
        assert!((sim - 1.0).abs() < 1e-6, "got {sim}");
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        let sim = cosine_similarity(&a, &b).unwrap();
        assert!(sim.abs() < 1e-6, "got {sim}");
    }

    #[test]
    fn cosine_dimension_mismatch_is_error_not_zero() {
        // The core of the fix: incomparable vectors must error, never score 0.
        let a = [1.0f32, 2.0, 3.0];
        let b = [1.0f32, 2.0];
        let err = cosine_similarity(&a, &b).unwrap_err();
        assert!(err.to_string().contains("dimension mismatch"), "{err}");
    }

    #[test]
    fn cosine_empty_is_error() {
        let empty: [f32; 0] = [];
        assert!(cosine_similarity(&empty, &empty).is_err());
    }

    #[test]
    fn floats_bytes_roundtrip() {
        let floats = vec![0.0f32, -1.5, 3.5, 42.0];
        let bytes = floats_to_bytes(&floats);
        assert_eq!(bytes.len(), floats.len() * 4);
        assert_eq!(bytes_to_floats(&bytes), floats);
    }
}
