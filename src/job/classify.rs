use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
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
    // Read file data and determine MIME type. Propagate read errors instead
    // of defaulting to empty data, which would silently classify unreadable
    // files (permissions, race, I/O error) as empty. Async read keeps the job
    // worker off a blocking sync call.
    let data = tokio::fs::read(full_path)
        .await
        .with_context(|| format!("reading file for classification: {}", full_path.display()))?;
    let mime_type = determine_content_type(full_path, &data);
    info!(path = &entry.path, mime = mime_type, "Classifying file");

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
    use crate::migration::Migrator;
    use sea_orm_migration::MigratorTrait;
    use std::sync::OnceLock;
    use tokio::sync::OnceCell;

    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();

    fn runtime() -> &'static tokio::runtime::Runtime {
        RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build shared test runtime"))
    }

    /// Shares the process-lifetime runtime/DB setup pattern used by
    /// `tests/*_integration.rs` (a per-test `#[tokio::test]` runtime would be
    /// dropped mid-test and hang the connection pool's background tasks).
    ///
    /// Connects with a locally-built `Config` rather than the process-wide
    /// `Config` singleton: `cargo test --lib` runs every unit test in one
    /// process, and other modules (e.g. `auth::tests`) already initialize
    /// that singleton with an unusable `database_url`.
    async fn test_db() -> &'static DatabaseConnection {
        DB.get_or_init(|| async {
            let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://user:password@localhost:15432/byteburrow_test".to_string()
            });
            let config = crate::config::Config {
                database_url,
                salt: "classify-test-salt".to_string(),
                server_addr: "0.0.0.0:3000".to_string(),
                thumbnail_storage: "/tmp/thumbnails".to_string(),
                base_url: "http://localhost:3000".to_string(),
                token_expiration_days: 30,
                token_length: 32,
                plugin_dir: "/tmp".to_string(),
                ignore_patterns: vec![],
                cors_allowed_origins: String::new(),
                trust_forwarded_headers: false,
                face_match_threshold: 0.8,
                face_match_margin: 0.05,
            };

            let db = crate::db_connect(&config)
                .await
                .expect("connect to test database");
            Migrator::up(&db, None).await.expect("run migrations");
            db
        })
        .await
    }

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
}
