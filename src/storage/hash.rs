use anyhow::Result;
use sea_orm::{ActiveModelTrait, Set};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tracing::{info, instrument};

use super::Storage;
use crate::entity::entry;

impl Storage {
    /// Calculate SHA256 hash for a file, skipping if DB record is up-to-date with FS.
    ///
    /// Returns `(updated, hash, entry)` where `updated` indicates whether the hash
    /// was recalculated (i.e. the file changed since last check).
    #[instrument(skip(self, db))]
    pub async fn calculate_hash(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<(bool, Vec<u8>, entry::Model)> {
        let normalized_path = sub_path.trim_matches('/').to_string();
        let full_path = self.get_full_path(&normalized_path);

        let fs_modified = chrono::DateTime::<chrono::Utc>::from(
            tokio::fs::metadata(&full_path).await?.modified()?,
        )
        .naive_utc();

        let model = self.ensure_entry(db, sub_path).await?;

        // Skip if DB record already has a hash and the file hasn't changed.
        // We compare both mtime (H13: sub-second race) AND size, so a file
        // rewritten within the same second with different content (different
        // size) is still re-hashed.
        if let Some(existing_hash) = model.hash.clone() {
            let fs_size = tokio::fs::metadata(&full_path).await?.len();
            let same_mtime = (fs_modified - model.modified_at).num_seconds().abs() < 1;
            let same_size = fs_size == model.size as u64;
            if same_mtime && same_size {
                info!(path = sub_path, "Hash up-to-date, skipping");
                return Ok((false, existing_hash, model));
            } else {
                info!(
                    "Hash is stale (mtime_match={}, size_match={}, db_size={}, fs_size={}), rehashing",
                    same_mtime, same_size, model.size, fs_size
                );
            }
        }

        let mut file = tokio::fs::File::open(&full_path).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = hasher.finalize().to_vec();

        let mut active: entry::ActiveModel = model.into();
        let fs_size_meta = tokio::fs::metadata(&full_path).await?;
        active.hash = Set(Some(hash.clone()));
        active.modified_at = Set(fs_modified);
        active.size = Set(fs_size_meta.len() as i64);
        let updated_model = active.update(db).await?;
        info!(path = sub_path, hash = hex::encode(&hash), "Hash updated");

        Ok((true, hash, updated_model))
    }
}
