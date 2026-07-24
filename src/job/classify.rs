use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tokio::io::AsyncReadExt;
use tracing::info;

use crate::entity::{entry, meta, photo};
use crate::plugin::{MergedClassification, PluginRegistry};
use crate::storage::determine_content_type;

use super::{exif, face, is_image_file};

/// Classify a file (plugin path or EXIF fallback) and persist the results.
/// Sole caller: `JobRunner::process_file`.
pub(super) async fn run_classification(
    db: &DatabaseConnection,
    plugins: &PluginRegistry,
    entry: &entry::Model,
    hash_bytes: &[u8],
    full_path: &Path,
) -> anyhow::Result<()> {
    // Determine MIME from a bounded header read instead of slurping the whole
    // file. The header read also surfaces unreadable/missing files early
    // (issue #6) — propagating instead of silently classifying them as empty —
    // before we decide whether the full contents are actually needed.
    let header = read_mime_header(full_path).await?;
    let mime_type = determine_content_type(full_path, &header);
    info!(path = &entry.path, mime = mime_type, "Classifying file");

    // Honor `ClassifierPlugin::needs_file_data()`: only load the whole file
    // when a plugin that will run actually needs the bytes. Plugins that do
    // their own path-based I/O (and the EXIF fallback, which reads via its own
    // extractor) never touch this buffer, so reading it would be wasted I/O —
    // significant for large media. Async read keeps the job worker off a
    // blocking sync call.
    let data = if plugins.needs_file_data(mime_type) {
        tokio::fs::read(full_path)
            .await
            .with_context(|| format!("reading file for classification: {}", full_path.display()))?
    } else {
        Vec::new()
    };

    // Run classification (plugin path, or the inline-EXIF fallback).
    let result = classify_or_exif(plugins, entry, full_path, &data, mime_type).await?;

    if let Some(mut merged) = result {
        // Persist face embeddings; this mutates merged.custom in place so
        // the match results survive into meta persistence below.
        let config = crate::config::Config::get();
        let face_params = crate::face_match::MatchParams {
            threshold: config.face_match_threshold,
            margin: config.face_match_margin,
        };
        face::process_face_embeddings(db, hash_bytes, &mut merged, face_params).await?;

        persist_meta(db, hash_bytes, &merged).await?;
        persist_photo(db, hash_bytes, &merged).await?;
    }

    Ok(())
}

/// Magic-byte signatures need at most the first 12 bytes; read a little more
/// for headroom. Bounded so we never load large media just to sniff its type.
const MIME_HEADER_LEN: u64 = 64;

/// Read the leading bytes of a file for MIME sniffing, propagating open/read
/// errors so an unreadable file surfaces instead of being classified as empty.
async fn read_mime_header(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening file for classification: {}", path.display()))?;
    let mut buf = Vec::with_capacity(MIME_HEADER_LEN as usize);
    file.take(MIME_HEADER_LEN)
        .read_to_end(&mut buf)
        .await
        .with_context(|| format!("reading file header for classification: {}", path.display()))?;
    Ok(buf)
}

/// Run the plugin classification pipeline when plugins are loaded;
/// otherwise fall back to inline EXIF extraction for images.
///
/// Returns `Ok(Some(merged))` when the plugin path produced a
/// [`MergedClassification`], `Ok(None)` for the EXIF fallback (which writes
/// the photo row directly), so callers know not to re-persist.
async fn classify_or_exif(
    plugins: &PluginRegistry,
    entry: &entry::Model,
    full_path: &Path,
    data: &[u8],
    mime_type: &'static str,
) -> anyhow::Result<Option<MergedClassification>> {
    if !plugins.is_empty() {
        let existing_custom: HashMap<String, serde_json::Value> = HashMap::new();
        let ctx = byteburrow_plugin_api::FileContext {
            path: &entry.path,
            full_path,
            data,
            mime_type,
            size: entry.size as u64,
            custom: &existing_custom,
        };
        Ok(Some(plugins.classify_file(&ctx)))
    } else {
        // Fallback: inline EXIF extraction when no plugins loaded.
        if is_image_file(&entry.path) {
            info!(path = &entry.path, "Processing image (inline, no plugins)");
            let (latitude, longitude, date) = exif::extract_exif(full_path);
            let merged = MergedClassification {
                latitude,
                longitude,
                date_unix: date.map(|d| d.and_utc().timestamp()),
                ..Default::default()
            };
            Ok(Some(merged))
        } else {
            Ok(None)
        }
    }
}

/// Upsert the `meta` row (keywords + custom JSON) for `hash_bytes`.
async fn persist_meta(
    db: &DatabaseConnection,
    hash_bytes: &[u8],
    merged: &MergedClassification,
) -> anyhow::Result<()> {
    if merged.keywords.is_empty() && merged.custom.is_empty() {
        return Ok(());
    }

    let existing_meta = meta::Entity::find_by_id(hash_bytes.to_vec())
        .one(db)
        .await?;

    match existing_meta {
        Some(m) => {
            let mut kw = m.keywords.clone();
            kw.extend(merged.keywords.clone());
            kw.sort();
            kw.dedup();

            let custom = match m.custom {
                serde_json::Value::Object(mut map) => {
                    map.extend(merged.custom.clone());
                    serde_json::Value::Object(map)
                }
                _ => {
                    if merged.custom.is_empty() {
                        m.custom
                    } else {
                        serde_json::Value::Object(merged.custom.clone())
                    }
                }
            };

            let active = meta::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                keywords: Set(kw),
                custom: Set(custom),
                ..Default::default()
            };
            active.update(db).await?;
        }
        None => {
            let active = meta::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                tags: Set(vec![]),
                keywords: Set(merged.keywords.clone()),
                custom: Set(if merged.custom.is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::Value::Object(merged.custom.clone())
                }),
            };
            active.insert(db).await?;
        }
    }

    Ok(())
}

/// Upsert the `photo` row (geo + date) for `hash_bytes`.
async fn persist_photo(
    db: &DatabaseConnection,
    hash_bytes: &[u8],
    merged: &MergedClassification,
) -> anyhow::Result<()> {
    if merged.latitude.is_none() && merged.longitude.is_none() && merged.date_unix.is_none() {
        return Ok(());
    }

    let date = merged
        .date_unix
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.naive_utc()));

    let existing = photo::Entity::find_by_id(hash_bytes.to_vec())
        .one(db)
        .await?;

    match existing {
        Some(_) => {
            let active = photo::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                latitude: Set(merged.latitude),
                longitude: Set(merged.longitude),
                date: Set(date),
                ..Default::default()
            };
            active.update(db).await?;
        }
        None => {
            let active = photo::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                latitude: Set(merged.latitude),
                longitude: Set(merged.longitude),
                date: Set(date),
                keywords: Set(vec![]),
            };
            active.insert(db).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{runtime, test_db};
    use std::collections::HashMap;

    /// Regression test for issue #11: `run_classification` used to run
    /// `face::process_face_embeddings` against `&mut merged.clone()`, so the
    /// contact-match array it wrote into `custom["face_embeddings"]` landed
    /// on a throwaway clone and never reached `persist_meta`. This exercises
    /// the same two calls on the same `merged` binding that
    /// `run_classification` now uses, and checks the result actually lands
    /// in `meta.custom`.
    #[test]
    fn face_match_results_persist_to_meta_custom() {
        runtime().block_on(async {
            let db = test_db().await;
            let hash_bytes = b"classify-test-face-embeddings-hash".to_vec();

            let mut merged = MergedClassification {
                custom: serde_json::json!({
                    "faces": { "rects": [{ "x": 0, "y": 0, "width": 10, "height": 10 }] },
                    "face_embeddings_raw": [
                        { "face_index": 0, "embedding": [0.1, 0.2, 0.3] }
                    ],
                })
                .as_object()
                .expect("test fixture is a JSON object")
                .clone(),
                ..Default::default()
            };

            let face_params = crate::face_match::MatchParams {
                threshold: 0.8,
                margin: 0.05,
            };
            face::process_face_embeddings(db, &hash_bytes, &mut merged, face_params)
                .await
                .expect("process face embeddings");

            // Sanity check: face processing must mutate the same `merged`
            // that gets passed to `persist_meta` below, not a clone of it.
            assert!(merged.custom.contains_key("face_embeddings"));

            persist_meta(db, &hash_bytes, &merged)
                .await
                .expect("persist meta");

            let stored = meta::Entity::find_by_id(hash_bytes)
                .one(db)
                .await
                .expect("query meta")
                .expect("meta row must exist after persist_meta");

            assert!(
                stored.custom.get("face_embeddings").is_some(),
                "face_embeddings must reach meta.custom, got: {:?}",
                stored.custom
            );
        });
    }

    /// Regression test for issue #6: an unreadable file must surface a read
    /// error instead of being silently classified as empty data.
    #[test]
    fn unreadable_file_propagates_error() {
        runtime().block_on(async {
            let db = test_db().await;
            let plugins = PluginRegistry::load_from_directory(
                Path::new("/nonexistent-plugin-dir"),
                &HashMap::new(),
            );
            let epoch = chrono::DateTime::from_timestamp(0, 0)
                .expect("epoch timestamp")
                .naive_utc();
            let entry = entry::Model {
                id: 1,
                storage_id: 1,
                user_id: 1,
                group_id: 1,
                parent_id: None,
                path: "does-not-exist.jpg".to_string(),
                hash: None,
                entry_type: entry::EntryType::File,
                notify: false,
                skip_plugins: false,
                size: 0,
                modified_at: epoch,
                created_at: epoch,
            };

            let result = run_classification(
                db,
                &plugins,
                &entry,
                b"unreadable-file-hash",
                Path::new("/nonexistent/unreadable-file.bin"),
            )
            .await;

            assert!(
                result.is_err(),
                "unreadable file must produce an error, not empty classification"
            );
        });
    }

    // ── face::process_face_embeddings (Tier 4) ─────────────────────
    //
    // Exercises: multi-face matching, cross-model exemplar skip, and
    // empty-embedding skip — the three branches `process_face_embeddings`
    // takes through `face_match::match_embedding`.

    use crate::entity::{contact, face_reference};
    use crate::face_match::{floats_to_bytes, MatchParams};
    use chrono::{FixedOffset, Utc};

    /// Insert a contact with a fixed id so face_reference rows can point at it.
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

    /// Insert a confirmed face_reference row for a contact.
    async fn make_confirmed_ref(
        db: &DatabaseConnection,
        hash: &[u8],
        face_index: i16,
        contact_id: i32,
        embedding: &[f32],
        model_id: &str,
        model_version: &str,
    ) {
        face_reference::ActiveModel {
            hash: Set(hash.to_vec()),
            face_index: Set(face_index),
            contact_id: Set(Some(contact_id)),
            bbox_x: Set(0),
            bbox_y: Set(0),
            bbox_w: Set(10),
            bbox_h: Set(10),
            embedding: Set(floats_to_bytes(embedding)),
            model_id: Set(model_id.to_string()),
            model_version: Set(model_version.to_string()),
            dim: Set(embedding.len() as i32),
            confirmed: Set(true),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert confirmed face_reference");
    }

    #[test]
    fn process_face_embeddings_matches_a_known_face() {
        runtime().block_on(async {
            let db = test_db().await;
            // Unique contact id, hash, AND model id per run so accumulated rows
            // from other tests/runs can't interfere (process_face_embeddings
            // loads ALL confirmed refs; we must isolate by model identity).
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i32;
            let contact_id = 700000 + (n % 100000);
            let hash = format!("face-match-test-{n}").into_bytes();
            let model_id = format!("match-model-{n}");

            make_contact(db, contact_id, "Face Match Test").await;

            // A confirmed exemplar that the query will match closely. Use a
            // high-dimensional vector so no other test's low-dim exemplars can
            // be compared (cross-model skip) and no margin ambiguity arises.
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            make_confirmed_ref(db, &hash, 0, contact_id, &exemplar, &model_id, "1").await;

            let mut query = exemplar.clone();
            query[0] = 0.99;
            query[1] = 0.01;
            let raw_query: Vec<serde_json::Value> =
                query.iter().map(|f| serde_json::json!(f)).collect();

            let mut merged = MergedClassification {
                custom: serde_json::json!({
                    "faces": { "rects": [{ "x": 0, "y": 0, "width": 10, "height": 10 }] },
                    "face_embeddings_raw": [
                        { "face_index": 0, "embedding": raw_query,
                          "model_id": model_id, "model_version": "1" }
                    ],
                })
                .as_object()
                .unwrap()
                .clone(),
                ..Default::default()
            };

            face::process_face_embeddings(
                db,
                &hash,
                &mut merged,
                MatchParams {
                    threshold: 0.8,
                    margin: 0.05,
                },
            )
            .await
            .expect("process face embeddings");

            let matches = merged
                .custom
                .get("face_embeddings")
                .and_then(|v| v.as_array())
                .expect("face_embeddings present");
            assert_eq!(matches.len(), 1);
            assert_eq!(
                matches[0].as_i64(),
                Some(contact_id as i64),
                "matched the known contact"
            );
        });
    }

    #[test]
    fn process_face_embeddings_skips_cross_model_exemplars() {
        // A confirmed exemplar from a different model must be skipped (not
        // scored as a 0-similarity non-match), leaving no positive match.
        runtime().block_on(async {
            let db = test_db().await;
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i32;
            let contact_id = 710000 + (n % 100000);
            let hash = format!("face-crossmodel-test-{n}").into_bytes();
            // Exemplar model differs from the query model; both are unique so
            // no other test's rows share the query's model identity.
            let exemplar_model = format!("xmodel-exemplar-{n}");
            let query_model = format!("xmodel-query-{n}");

            make_contact(db, contact_id, "Cross Model Test").await;
            let mut exemplar = vec![0.0f32; 64];
            exemplar[0] = 1.0;
            make_confirmed_ref(db, &hash, 0, contact_id, &exemplar, &exemplar_model, "1").await;

            let raw_query: Vec<serde_json::Value> =
                exemplar.iter().map(|f| serde_json::json!(f)).collect();
            let mut merged = MergedClassification {
                custom: serde_json::json!({
                    "faces": { "rects": [{ "x": 0, "y": 0, "width": 10, "height": 10 }] },
                    "face_embeddings_raw": [
                        { "face_index": 0, "embedding": raw_query,
                          "model_id": query_model, "model_version": "1" }
                    ],
                })
                .as_object()
                .unwrap()
                .clone(),
                ..Default::default()
            };

            face::process_face_embeddings(
                db,
                &hash,
                &mut merged,
                MatchParams {
                    threshold: 0.8,
                    margin: 0.0,
                },
            )
            .await
            .expect("process face embeddings");

            let matches = merged
                .custom
                .get("face_embeddings")
                .and_then(|v| v.as_array())
                .expect("face_embeddings present");
            // The sole face matched nothing (cross-model exemplar skipped) → null.
            assert_eq!(matches.len(), 1);
            assert!(matches[0].is_null(), "cross-model exemplar must not match");
        });
    }

    #[test]
    fn process_face_embeddings_skips_empty_embedding() {
        runtime().block_on(async {
            let db = test_db().await;
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i32;
            let hash = format!("face-empty-test-{n}").into_bytes();
            let model_id = format!("empty-model-{n}");

            let mut merged = MergedClassification {
                custom: serde_json::json!({
                    "faces": { "rects": [{ "x": 0, "y": 0, "width": 10, "height": 10 }] },
                    "face_embeddings_raw": [
                        { "face_index": 0, "embedding": [],
                          "model_id": model_id, "model_version": "1" }
                    ],
                })
                .as_object()
                .unwrap()
                .clone(),
                ..Default::default()
            };

            // Must not panic and must produce the (null) match array for the face.
            face::process_face_embeddings(
                db,
                &hash,
                &mut merged,
                MatchParams {
                    threshold: 0.8,
                    margin: 0.0,
                },
            )
            .await
            .expect("process face embeddings");

            let matches = merged
                .custom
                .get("face_embeddings")
                .and_then(|v| v.as_array())
                .expect("face_embeddings present");
            assert_eq!(matches.len(), 1);
            assert!(matches[0].is_null(), "empty embedding yields no match");
        });
    }
}
