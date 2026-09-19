use anyhow::Result;
use sea_orm::{ActiveModelTrait, Set};
use sha2::{Digest, Sha256};
use tracing::{info, instrument};

use super::Storage;
use crate::entity::entry;

impl Storage {
    /// Calculate SHA256 hash for a file, skipping if DB record is up-to-date
    /// with the storage backend (local FS mtime/size or remote WebDAV
    /// PROPFIND mtime/size).
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
        let stat = self.stat_entry(&normalized_path).await?;

        let model = self.ensure_entry(db, sub_path).await?;

        // Skip if DB record already has a hash and the file hasn't changed.
        // We compare both mtime (H13: sub-second race) AND size, so a file
        // rewritten within the same second with different content (different
        // size) is still re-hashed.
        if let Some(existing_hash) = model.hash.clone() {
            let same_mtime = (stat.modified_at.naive_utc() - model.modified_at)
                .num_seconds()
                .abs()
                < 1;
            let same_size = stat.size == model.size as u64;
            if same_mtime && same_size {
                info!(path = sub_path, "Hash up-to-date, skipping");
                return Ok((false, existing_hash, model));
            } else {
                info!(
                    "Hash is stale (mtime_match={}, size_match={}, db_size={}, fs_size={}), rehashing",
                    same_mtime, same_size, model.size, stat.size
                );
            }
        }

        let hash = self.hash_file(&normalized_path).await?;

        let mut active: entry::ActiveModel = model.into();
        active.hash = Set(Some(hash.clone()));
        active.modified_at = Set(stat.modified_at.naive_utc());
        active.size = Set(stat.size as i64);
        let updated_model = active.update(db).await?;
        info!(path = sub_path, hash = hex::encode(&hash), "Hash updated");

        Ok((true, hash, updated_model))
    }

    /// SHA256 over a file's bytes. Local storages stream from disk (constant
    /// memory); remote storages GET the bytes (the remote backend buffers —
    /// see ADR 0008).
    async fn hash_file(&self, normalized_path: &str) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;

        if !self.is_local() {
            let data = self.read_file(normalized_path).await?;
            return Ok(Sha256::digest(&data).to_vec());
        }

        let full_path = self.get_full_path(normalized_path)?;
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
        Ok(hasher.finalize().to_vec())
    }
}
