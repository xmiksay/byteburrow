use std::collections::HashMap;
use std::path::Path;

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
    // Read file data and determine MIME type.
    let data = std::fs::read(full_path).unwrap_or_default();
    let mime_type = determine_content_type(full_path, &data);
    info!(path = &entry.path, mime = mime_type, "Classifying file");

    // Run classification (plugin path, or the inline-EXIF fallback).
    let result = classify_or_exif(plugins, entry, full_path, &data, mime_type).await?;

    if let Some(merged) = result {
        // Persist face embeddings (also mutates merged.custom for downstream
        // meta persistence).
        face::process_face_embeddings(db, hash_bytes, &mut merged.clone()).await?;

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
