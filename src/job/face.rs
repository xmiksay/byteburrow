use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::{info, warn};

use crate::entity::face_reference;
use crate::face_match::{bytes_to_floats, floats_to_bytes, match_embedding, Exemplar, MatchParams};
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
    params: MatchParams,
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

    // Load all confirmed face references for matching, decoding each embedding
    // once so it can be reused across every query face in this image.
    let confirmed_refs = face_reference::Entity::find()
        .filter(face_reference::Column::Confirmed.eq(true))
        .all(db)
        .await?;
    let decoded_refs: Vec<(i32, String, String, Vec<f32>)> = confirmed_refs
        .iter()
        .filter_map(|r| {
            r.contact_id.map(|contact_id| {
                (
                    contact_id,
                    r.model_id.clone(),
                    r.model_version.clone(),
                    bytes_to_floats(&r.embedding),
                )
            })
        })
        .collect();
    let exemplars: Vec<Exemplar> = decoded_refs
        .iter()
        .map(
            |(contact_id, model_id, model_version, embedding)| Exemplar {
                contact_id: *contact_id,
                model_id,
                model_version,
                embedding,
            },
        )
        .collect();

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

        // Match against confirmed references through the shared host-side
        // matcher (single threshold + margin guard). Cross-model references are
        // refused inside `match_embedding` and reported back for a warning.
        let outcome = match_embedding(
            &embedding_floats,
            &model_id,
            &model_version,
            &exemplars,
            params,
        );

        if outcome.skipped_cross_model > 0 {
            warn!(
                face_index,
                model_id = %model_id,
                model_version = %model_version,
                skipped = outcome.skipped_cross_model,
                "skipped confirmed references from a different embedding model; \
                 re-embed them to make matching work across the model change"
            );
        }

        if let Some(m) = outcome.best {
            if face_index < contact_matches.len() {
                contact_matches[face_index] = serde_json::Value::Number(m.contact_id.into());
            }
            info!(
                face_index,
                contact_id = m.contact_id,
                similarity = m.similarity,
                runner_up = ?m.runner_up,
                "Face matched to contact"
            );
        }
    }

    // Store the contact match array in meta.custom
    merged.custom.insert(
        "face_embeddings".to_string(),
        serde_json::Value::Array(contact_matches),
    );

    Ok(())
}
