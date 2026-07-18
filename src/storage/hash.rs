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

        let fs_modified = tokio::fs::metadata(&full_path)
            .await?
            .modified()
            .map(chrono::DateTime::<chrono::Utc>::from)
            .unwrap()
            .naive_utc();

        let model = self.ensure_entry(db, sub_path).await?;

        // Skip if DB record already has a hash and is not older than FS
        if model.hash.is_some() {
            if (fs_modified - model.modified_at).num_seconds() < 1 {
                info!(path = sub_path, "Hash up-to-date, skipping");
                return Ok((false, model.hash.clone().unwrap(), model));
            } else {
                info!(
                    "Hash is calcuated, file is newer: {:?} {:?} {}",
                    model.modified_at,
                    fs_modified,
                    fs_modified - model.modified_at
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
        active.hash = Set(Some(hash.clone()));
        active.modified_at = Set(fs_modified);
        let updated_model = active.update(db).await?;
        info!(path = sub_path, hash = hex::encode(&hash), "Hash updated");

        Ok((true, hash, updated_model))
    }
}
