use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::info;

use crate::entity::face_reference;
use crate::plugin::MergedClassification;

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
                    confirmed: Set(false),
                    ..Default::default()
                };
                active.insert(db).await?;
            }
        }

        // Match against confirmed references
        let mut best_contact_id: Option<i32> = None;
        let mut best_similarity: f32 = 0.0;

        for reference in &confirmed_refs {
            if let Some(contact_id) = reference.contact_id {
                let ref_floats = bytes_to_floats(&reference.embedding);
                let sim = cosine_similarity(&embedding_floats, &ref_floats);
                if sim > best_similarity {
                    best_similarity = sim;
                    best_contact_id = Some(contact_id);
                }
            }
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

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
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
