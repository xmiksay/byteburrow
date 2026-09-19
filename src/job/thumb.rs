use image::imageops::FilterType;
use image::GenericImageView;
use tracing::{info, warn};

use crate::config::Config;
use crate::storage::thumbnail::{self, StorageSource};
use crate::storage::Storage;

/// Generate the mini/small/large thumbnail set for a file if missing.
/// Called from `JobRunner::process_file` and `JobRunner::create_thumbnail`.
///
/// Backend-neutral since ADR 0008: local storages decode via
/// `image::open(full_path)`; remote (nextcloud) storages GET the bytes and
/// decode with `image::load_from_memory`.
pub(super) async fn generate_thumbnails(
    storage: &Storage,
    path: &str,
    hash_hex: &str,
) -> anyhow::Result<()> {
    let config = Config::get();
    let thumbnail_dir = std::path::PathBuf::from(&config.thumbnail_storage);

    for (size_name, max_dim) in [("mini", 64u32), ("small", 256u32), ("large", 1024u32)] {
        let thumb_path = thumbnail::get_thumbnail_path(&thumbnail_dir, hash_hex, size_name);

        if thumb_path.exists() {
            continue;
        }

        thumbnail::ensure_thumbnail_dir(&thumb_path).await?;

        // Local: hand the decoder the on-disk path (streamed decode, no
        // buffering). Remote: fetch the bytes once per missing size.
        let source = if storage.is_local() {
            let full_path = storage.get_full_path(path)?;
            StorageSource::Path(full_path)
        } else {
            let bytes = storage.read_file(path).await?;
            StorageSource::Bytes(bytes)
        };

        let thumb_path_clone = thumb_path.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let img = match &source {
                StorageSource::Path(p) => image::open(p)?,
                StorageSource::Bytes(b) => image::load_from_memory(b)?,
            };
            let (w, h) = img.dimensions();
            if w <= max_dim && h <= max_dim {
                img.save(&thumb_path_clone)?;
            } else {
                let thumb = img.resize(max_dim, max_dim, FilterType::Lanczos3);
                thumb.save(&thumb_path_clone)?;
            }
            Ok(())
        })
        .await?;

        match result {
            Ok(()) => info!(size = size_name, "Thumbnail generated"),
            Err(e) => warn!(size = size_name, error = %e, "Failed to generate thumbnail"),
        }
    }

    Ok(())
}
