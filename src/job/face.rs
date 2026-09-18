use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::collections::HashSet;
use tracing::{info, warn};

use crate::entity::{face_reference, meta};
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
/// against confirmed contacts. Primary caller: `classify::run_classification`;
/// the re-match pass below is the backfill half of the confirmation loop.
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
                    // Fresh detection: no human label yet, so the re-match
                    // pass owns this row's `contact_id` until someone
                    // assigns one through the API.
                    pinned: Set(false),
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

// ---------------------------------------------------------------------------
// Backfill re-match (issue #27)
// ---------------------------------------------------------------------------

/// Rebuild `meta.custom["face_embeddings"]` for one file hash from the
/// current `face_reference` rows, so the per-face contact array the UI reads
/// stays in sync after any assignment change (classification, manual
/// assignment, or re-match). Shared by the re-match pass, the face-management
/// API (#26/#27), and the CLI face tools.
///
/// Returns `true` when a `meta` row was rewritten. A hash with no `meta` row
/// (deleted file meta) is skipped with a warning — nothing to sync.
pub async fn sync_face_meta(db: &DatabaseConnection, hash: &[u8]) -> anyhow::Result<bool> {
    let Some(meta_row) = meta::Entity::find_by_id(hash.to_vec()).one(db).await? else {
        warn!(hash = %hex::encode(hash), "face_reference has no meta row; skipping meta sync");
        return Ok(false);
    };

    // Re-read every face of this hash so unchanged faces keep their current
    // assignment in the rebuilt array.
    let faces_for_hash = face_reference::Entity::find()
        .filter(face_reference::Column::Hash.eq(hash.to_vec()))
        .all(db)
        .await?;
    let face_count = faces_for_hash
        .iter()
        .map(|f| f.face_index as usize + 1)
        .max()
        .unwrap_or(0);
    let mut contact_matches: Vec<serde_json::Value> = vec![serde_json::Value::Null; face_count];
    for f in &faces_for_hash {
        let idx = f.face_index as usize;
        if idx < contact_matches.len() {
            contact_matches[idx] = match f.contact_id {
                Some(cid) => serde_json::Value::Number(cid.into()),
                None => serde_json::Value::Null,
            };
        }
    }

    // `custom` must be an object; a non-object (legacy/corrupt) row gets a
    // fresh object rather than failing the whole pass.
    let mut custom = meta_row.custom.as_object().cloned().unwrap_or_default();
    custom.insert(
        "face_embeddings".to_string(),
        serde_json::Value::Array(contact_matches),
    );

    let mut active: meta::ActiveModel = meta_row.into();
    active.custom = Set(serde_json::Value::Object(custom));
    active.update(db).await?;
    Ok(true)
}

/// Summary of one backfill run, returned to the confirmation API so the UI
/// can tell the user what naming this person just changed.
#[derive(Debug, Default, Clone, Copy)]
pub struct RematchOutcome {
    /// Machine-suggested faces newly assigned to a contact.
    pub assigned: usize,
    /// Machine-suggested faces whose previous suggestion was withdrawn (no
    /// contact clears the threshold/margin any more).
    pub cleared: usize,
    /// Suggestions left exactly as they were.
    pub unchanged: usize,
    /// Faces skipped because their exemplar pool contains no embedding from
    /// their (model_id, model_version) — nothing comparable to match against,
    /// so no new decision can be made and the row keeps its assignment.
    pub skipped: usize,
    /// Distinct `meta` rows whose `custom["face_embeddings"]` was rewritten.
    pub metas_updated: usize,
    /// Machine-suggested faces examined this run.
    pub considered: usize,
}

impl std::fmt::Display for RematchOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} considered, {} assigned, {} cleared, {} unchanged, {} skipped, {} file metas updated",
            self.considered, self.assigned, self.cleared, self.unchanged, self.skipped,
            self.metas_updated
        )
    }
}

/// Restricts a re-match pass to one embedding model. `None` re-decides every
/// machine-suggested face; `Some((model_id, model_version))` only faces from
/// that model — the exact slice a single confirmation can change, since
/// [`match_embedding`] refuses cross-model comparisons.
pub type RematchScope = Option<(String, String)>;

/// Recompute contact assignments for every **machine-suggested** face
/// (`confirmed = false AND pinned = false`) against the current confirmed
/// exemplar pool, and rewrite each affected file's `meta.custom.face_embeddings`
/// contact array.
///
/// This is the backfill half of the confirmation flow (#26/#27): without it,
/// naming a person only affects files processed *after* the confirmation.
/// It deliberately does **not** re-run classification — the embeddings are
/// already stored in `face_reference`; re-matching is pure vector math plus
/// two writes per changed face.
///
/// Invariants:
/// - Confirmed exemplars are never touched (they define the pool).
/// - Human assignments (`pinned`) are never touched — only suggestions are
///   recomputed.
/// - A `meta` row is only rewritten when at least one of its faces changed,
///   so steady-state confirms (which change nothing outside the exemplar)
///   cost no writes.
pub async fn rematch_unconfirmed_faces(
    db: &DatabaseConnection,
    params: MatchParams,
    scope: RematchScope,
) -> anyhow::Result<RematchOutcome> {
    // The exemplar pool: every confirmed row with a contact. Decoded once.
    let confirmed = face_reference::Entity::find()
        .filter(face_reference::Column::Confirmed.eq(true))
        .all(db)
        .await?;
    let decoded_pool: Vec<(i32, String, String, Vec<f32>)> = confirmed
        .iter()
        .filter_map(|r| {
            r.contact_id.map(|cid| {
                (
                    cid,
                    r.model_id.clone(),
                    r.model_version.clone(),
                    bytes_to_floats(&r.embedding),
                )
            })
        })
        .collect();
    let exemplars: Vec<Exemplar> = decoded_pool
        .iter()
        .map(|(cid, mid, mv, emb)| Exemplar {
            contact_id: *cid,
            model_id: mid,
            model_version: mv,
            embedding: emb,
        })
        .collect();

    // Which model identities actually have comparable exemplars. A candidate
    // whose model has none can't be re-decided at all.
    let pool_models: HashSet<(String, String)> = decoded_pool
        .iter()
        .map(|(_, mid, mv, _)| (mid.clone(), mv.clone()))
        .collect();

    // Faces to re-decide: machine suggestions only.
    let mut candidate_query = face_reference::Entity::find()
        .filter(face_reference::Column::Confirmed.eq(false))
        .filter(face_reference::Column::Pinned.eq(false));
    if let Some((model_id, model_version)) = &scope {
        candidate_query = candidate_query
            .filter(face_reference::Column::ModelId.eq(model_id.clone()))
            .filter(face_reference::Column::ModelVersion.eq(model_version.clone()));
    }
    let candidates = candidate_query.all(db).await?;

    let mut outcome = RematchOutcome {
        considered: candidates.len(),
        ..Default::default()
    };
    // Hashes whose faces changed, so each affected file's meta row is
    // rewritten exactly once.
    let mut changed_hashes: HashSet<Vec<u8>> = HashSet::new();

    for face in &candidates {
        // No comparable exemplar for this face's model ⇒ no new decision is
        // possible; leave the row as-is rather than clearing on a technicality.
        if !pool_models.contains(&(face.model_id.clone(), face.model_version.clone())) {
            outcome.skipped += 1;
            continue;
        }

        let query = bytes_to_floats(&face.embedding);
        let result = match_embedding(
            &query,
            &face.model_id,
            &face.model_version,
            &exemplars,
            params,
        );

        let new_contact = result.best.map(|m| m.contact_id);
        let old_contact = face.contact_id;

        if new_contact == old_contact {
            outcome.unchanged += 1;
            continue;
        }

        match new_contact {
            Some(_) => outcome.assigned += 1,
            None => outcome.cleared += 1,
        }

        let mut active: face_reference::ActiveModel = face.clone().into();
        active.contact_id = Set(new_contact);
        active.update(db).await?;

        changed_hashes.insert(face.hash.clone());
    }

    // Rewrite meta.custom.face_embeddings for every touched file.
    for hash in &changed_hashes {
        if sync_face_meta(db, hash).await? {
            outcome.metas_updated += 1;
        }
    }

    if outcome.assigned + outcome.cleared > 0 {
        info!(%outcome, "Face backfill re-match complete");
    }

    Ok(outcome)
}

#[cfg(test)]
mod rematch_tests {
    use super::*;
    use crate::entity::contact;
    use crate::test_support::{runtime, test_db};
    use chrono::{FixedOffset, Utc};
    use sea_orm::EntityTrait;

    /// The rematch pass operates on the whole `face_reference` table (a
    /// `scope = None` pass re-decides every suggestion in the DB), and cargo
    /// runs tests in parallel against one shared database. Serialise these
    /// tests so one test's global pass can't sweep another's fixtures
    /// mid-assert.
    static SEQUENTIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Every test isolates itself with a unique model identity + contact id +
    /// hash prefix, because `rematch_unconfirmed_faces` operates on the whole
    /// `face_reference` table (like the production job would).
    fn uniq() -> i32 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i32
    }

    async fn make_contact(db: &DatabaseConnection, id: i32, name: &str) {
        contact::ActiveModel {
            id: Set(id),
            name: Set(name.to_string()),
            created_at: Set(Utc::now().with_timezone(&FixedOffset::east_opt(0).unwrap())),
        }
        .insert(db)
        .await
        .expect("insert contact");
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_face(
        db: &DatabaseConnection,
        hash: &[u8],
        face_index: i16,
        contact_id: Option<i32>,
        embedding: &[f32],
        model_id: &str,
        confirmed: bool,
        pinned: bool,
    ) -> face_reference::Model {
        face_reference::ActiveModel {
            hash: Set(hash.to_vec()),
            face_index: Set(face_index),
            contact_id: Set(contact_id),
            bbox_x: Set(0),
            bbox_y: Set(0),
            bbox_w: Set(10),
            bbox_h: Set(10),
            embedding: Set(floats_to_bytes(embedding)),
            model_id: Set(model_id.to_string()),
            model_version: Set("1".to_string()),
            dim: Set(embedding.len() as i32),
            confirmed: Set(confirmed),
            pinned: Set(pinned),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert face_reference")
    }

    async fn insert_meta(db: &DatabaseConnection, hash: &[u8]) {
        meta::ActiveModel {
            hash: Set(hash.to_vec()),
            tags: Set(vec![]),
            keywords: Set(vec![]),
            custom: Set(serde_json::json!({})),
        }
        .insert(db)
        .await
        .expect("insert meta");
    }

    async fn face_by_id(db: &DatabaseConnection, id: i32) -> face_reference::Model {
        face_reference::Entity::find_by_id(id)
            .one(db)
            .await
            .expect("query face")
            .expect("face row exists")
    }

    async fn face_embeddings_meta(db: &DatabaseConnection, hash: &[u8]) -> serde_json::Value {
        meta::Entity::find_by_id(hash.to_vec())
            .one(db)
            .await
            .expect("query meta")
            .expect("meta row exists")
            .custom
            .get("face_embeddings")
            .cloned()
            .expect("face_embeddings in meta.custom")
    }

    fn params() -> MatchParams {
        MatchParams {
            threshold: 0.8,
            margin: 0.05,
        }
    }

    /// The core #27 behavior: confirming an exemplar must retroactively
    /// assign already-processed faces and rewrite the file's meta.
    #[test]
    fn rematch_assigns_existing_faces_after_confirmation() {
        let _guard = SEQUENTIAL.lock().unwrap_or_else(|e| e.into_inner());
        runtime().block_on(async {
            let db = test_db().await;
            let n = uniq();
            let model = format!("rematch-assign-{n}");
            let contact_id = 710000 + (n % 100000);
            make_contact(db, contact_id, "Rematch Assign").await;
            let hash = format!("rematch-assign-{n}").into_bytes();

            // Confirmed exemplar: unit vector along axis 0.
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            insert_face(
                db,
                &hash,
                0,
                Some(contact_id),
                &exemplar,
                &model,
                true,
                true,
            )
            .await;

            // An already-processed face close to the exemplar, still unassigned.
            let mut query = exemplar.clone();
            query[0] = 0.99;
            query[1] = 0.01;
            let candidate = insert_face(db, &hash, 1, None, &query, &model, false, false).await;
            insert_meta(db, &hash).await;

            let outcome =
                rematch_unconfirmed_faces(db, params(), Some((model.clone(), "1".to_string())))
                    .await
                    .expect("rematch");

            assert_eq!(outcome.assigned, 1, "one suggestion must be assigned");
            assert_eq!(outcome.metas_updated, 1);

            let updated = face_by_id(db, candidate.id).await;
            assert_eq!(updated.contact_id, Some(contact_id));

            // meta.custom["face_embeddings"]: face 0 (the exemplar itself) and
            // face 1 both point at the contact.
            let arr = face_embeddings_meta(db, &hash).await;
            assert_eq!(
                arr,
                serde_json::json!([contact_id, contact_id]),
                "meta array must reflect the new assignment"
            );
        });
    }

    /// Human labels (`pinned`) must survive re-matching — the reason the
    /// column exists.
    #[test]
    fn rematch_never_overwrites_pinned_labels() {
        let _guard = SEQUENTIAL.lock().unwrap_or_else(|e| e.into_inner());
        runtime().block_on(async {
            let db = test_db().await;
            let n = uniq();
            let model = format!("rematch-pinned-{n}");
            let alice = 720000 + (n % 100000);
            let bob = alice + 1;
            make_contact(db, alice, "Pinned Alice").await;
            make_contact(db, bob, "Pinned Bob").await;
            let hash = format!("rematch-pinned-{n}").into_bytes();

            // Alice's confirmed exemplar.
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            insert_face(db, &hash, 0, Some(alice), &exemplar, &model, true, true).await;

            // A human pinned this face to Bob even though it is closest to
            // Alice's exemplar — the re-match must not touch it.
            let mut near_alice = exemplar.clone();
            near_alice[0] = 0.99;
            near_alice[1] = 0.01;
            let pinned =
                insert_face(db, &hash, 1, Some(bob), &near_alice, &model, false, true).await;
            insert_meta(db, &hash).await;

            let outcome = rematch_unconfirmed_faces(db, params(), Some((model, "1".to_string())))
                .await
                .expect("rematch");

            assert_eq!(outcome.considered, 0, "pinned faces are not candidates");
            let unchanged = face_by_id(db, pinned.id).await;
            assert_eq!(unchanged.contact_id, Some(bob));
            assert!(unchanged.pinned);
        });
    }

    /// When the pool shrinks (exemplar withdrawn / contact deleted), stale
    /// suggestions are withdrawn: contact_id cleared, meta array updated.
    #[test]
    fn rematch_clears_stale_suggestions() {
        let _guard = SEQUENTIAL.lock().unwrap_or_else(|e| e.into_inner());
        runtime().block_on(async {
            let db = test_db().await;
            let n = uniq();
            let model = format!("rematch-clear-{n}");
            let alice = 730000 + (n % 100000);
            make_contact(db, alice, "Clear Alice").await;
            let hash = format!("rematch-clear-{n}").into_bytes();

            // Exemplar: axis 0. Query: far away (below threshold).
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            insert_face(db, &hash, 0, Some(alice), &exemplar, &model, true, true).await;

            // A stale suggestion pointing at Alice that no longer clears the
            // threshold — e.g. left over from a different exemplar.
            let mut far = vec![0.0f32; 64];
            far[1] = 1.0; // orthogonal ⇒ similarity 0
            let stale = insert_face(db, &hash, 1, Some(alice), &far, &model, false, false).await;
            insert_meta(db, &hash).await;

            let outcome = rematch_unconfirmed_faces(db, params(), Some((model, "1".to_string())))
                .await
                .expect("rematch");

            assert_eq!(outcome.cleared, 1);
            let updated = face_by_id(db, stale.id).await;
            assert_eq!(updated.contact_id, None);

            let arr = face_embeddings_meta(db, &hash).await;
            assert_eq!(arr, serde_json::json!([alice, null]));
        });
    }

    /// Faces from an embedding model with no exemplars cannot be re-decided
    /// and are skipped (kept as-is) instead of being cleared.
    #[test]
    fn rematch_skips_models_without_exemplars() {
        let _guard = SEQUENTIAL.lock().unwrap_or_else(|e| e.into_inner());
        runtime().block_on(async {
            let db = test_db().await;
            let n = uniq();
            let pool_model = format!("rematch-skip-pool-{n}");
            let other_model = format!("rematch-skip-other-{n}");
            let alice = 740000 + (n % 100000);
            make_contact(db, alice, "Skip Alice").await;
            let hash = format!("rematch-skip-{n}").into_bytes();

            // Pool exemplar in pool_model.
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            insert_face(
                db,
                &hash,
                0,
                Some(alice),
                &exemplar,
                &pool_model,
                true,
                true,
            )
            .await;

            // A suggestion in other_model (no exemplars there), carrying a
            // label that must survive untouched.
            let mut v = vec![0.0f32; 64];
            v[2] = 1.0;
            let foreign =
                insert_face(db, &hash, 1, Some(alice), &v, &other_model, false, false).await;

            let outcome = rematch_unconfirmed_faces(db, params(), None)
                .await
                .expect("rematch");

            assert!(outcome.skipped >= 1, "foreign-model face must be skipped");
            let unchanged = face_by_id(db, foreign.id).await;
            assert_eq!(unchanged.contact_id, Some(alice));
        });
    }
}
