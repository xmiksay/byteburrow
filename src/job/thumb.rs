use std::path::Path;

use image::imageops::FilterType;
use image::GenericImageView;
use tracing::{info, warn};

use crate::config::Config;
use crate::storage::thumbnail;

/// Generate the mini/small/large thumbnail set for a file if missing.
/// Called from `JobRunner::process_file` and `JobRunner::create_thumbnail`.
pub(super) async fn generate_thumbnails(full_path: &Path, hash_hex: &str) -> anyhow::Result<()> {
    let config = Config::get();
    let thumbnail_dir = std::path::PathBuf::from(&config.thumbnail_storage);

    for (size_name, max_dim) in [("mini", 64u32), ("small", 256u32), ("large", 1024u32)] {
        let thumb_path = thumbnail::get_thumbnail_path(&thumbnail_dir, hash_hex, size_name);

        if thumb_path.exists() {
            continue;
        }

        thumbnail::ensure_thumbnail_dir(&thumb_path).await?;

        let full_path = full_path.to_path_buf();
        let thumb_path_clone = thumb_path.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let img = image::open(&full_path)?;
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
