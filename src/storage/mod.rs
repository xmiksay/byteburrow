use crate::entity::entry::{self, EntryType};
use crate::entity::storage;
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::instrument;

#[derive(Debug)]
pub struct Storage {
    pub model: storage::Model,
}

#[derive(Debug, Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct DirectoryEntry {
    pub id: Option<i32>,
    pub storage_id: i32,
    pub user_id: i32,
    pub group_id: i32,
    pub parent_id: Option<i32>,
    pub path: String,
    pub hash: Option<Vec<u8>>,
    pub entry_type: EntryType,
    pub notify: bool,
    pub size: i64,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<entry::Model> for DirectoryEntry {
    fn from(model: entry::Model) -> Self {
        Self {
            id: Some(model.id),
            storage_id: model.storage_id,
            user_id: model.user_id,
            group_id: model.group_id,
            parent_id: model.parent_id,
            path: model.path,
            hash: model.hash,
            entry_type: model.entry_type,
            notify: model.notify,
            size: model.size,
            modified_at: Utc.from_utc_datetime(&model.modified_at),
            created_at: Utc.from_utc_datetime(&model.created_at),
        }
    }
}

impl Storage {
    pub fn new(model: storage::Model) -> Self {
        Self { model }
    }

    /// Find a storage by ID in the database
    #[instrument(skip(db))]
    pub async fn find_by_id(
        db: &sea_orm::DatabaseConnection,
        id: i32,
    ) -> Result<Self, sea_orm::DbErr> {
        let model = storage::Entity::find_by_id(id).one(db).await?;
        model.map(Self::new).ok_or_else(|| {
            sea_orm::DbErr::RecordNotFound(format!("Storage with id {} not found", id))
        })
    }

    /// List directory contents from the filesystem for a given subpath within the storage
    #[instrument]
    pub async fn list_directory_fs(&self, sub_path: &str) -> io::Result<Vec<DirectoryEntry>> {
        let base_path = PathBuf::from(&self.model.path);
        // Canonicalize the root once for safe relative-path computation below.
        let canon_root = tokio::fs::canonicalize(&base_path)
            .await
            .unwrap_or_else(|_| base_path.clone());
        // Resolve the requested directory safely (rejects traversal).
        let full_path = self.resolve_safe_path(sub_path).await?;

        let mut read_dir = fs::read_dir(&full_path).await?;
        let mut entries = Vec::new();

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            // Get relative path as string
            let relative_path = path
                .strip_prefix(&canon_root)
                .or_else(|_| path.strip_prefix(&base_path))
                .map_err(io::Error::other)?
                .to_string_lossy()
                .into_owned();

            let modified_at = metadata
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from)
                .unwrap();

            let entry_type = if metadata.is_dir() {
                EntryType::Directory
            } else if metadata.is_file() {
                EntryType::File
            } else {
                EntryType::Symlink
            };

            entries.push(DirectoryEntry {
                id: None,
                storage_id: self.model.id,
                user_id: self.model.default_user,
                group_id: self.model.default_group,
                parent_id: None, // Will be resolved during discovery/sync
                path: relative_path,
                hash: None, // Will be calculated if needed
                entry_type,
                notify: false,
                size: metadata.len().try_into().unwrap(),
                created_at: modified_at,
                modified_at,
            });
        }

        Ok(entries)
    }

    /// List directory contents from the database for a given subpath within the storage
    pub async fn list_directory_db(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<Vec<DirectoryEntry>, sea_orm::DbErr> {
        let normalized_path = sub_path.trim_matches('/');

        let entries = if normalized_path.is_empty() {
            // Root directory
            entry::Entity::find()
                .filter(entry::Column::StorageId.eq(self.model.id))
                .filter(entry::Column::ParentId.is_null())
                .all(db)
                .await?
        } else {
            // Find directory entry first
            let dir_path = normalized_path.to_string();

            let directory = entry::Entity::find()
                .filter(entry::Column::StorageId.eq(self.model.id))
                .filter(entry::Column::Path.eq(&dir_path))
                .one(db)
                .await?;

            match directory {
                Some(dir) => {
                    // Get children
                    entry::Entity::find()
                        .filter(entry::Column::StorageId.eq(self.model.id))
                        .filter(entry::Column::ParentId.eq(dir.id))
                        .all(db)
                        .await?
                }
                None => {
                    // directory not in DB, return empty list (FS will provide entries)
                    Vec::new()
                }
            }
        };

        Ok(entries.into_iter().map(DirectoryEntry::from).collect())
    }

    /// List directory contents by merging filesystem and database state
    pub async fn list_directory(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<Vec<DirectoryEntry>, sea_orm::DbErr> {
        let fs_entries = self
            .list_directory_fs(sub_path)
            .await
            .map_err(|e| sea_orm::DbErr::Custom(format!("Filesystem error: {}", e)))?;

        let db_entries = self.list_directory_db(db, sub_path).await?;

        let mut db_map: std::collections::HashMap<String, DirectoryEntry> = db_entries
            .into_iter()
            .map(|e| (e.path.clone(), e))
            .collect();

        let mut result = Vec::new();

        for mut fs_entry in fs_entries {
            if let Some(db_entry) = db_map.remove(&fs_entry.path) {
                // Entry exists in both FS and DB
                fs_entry.id = db_entry.id;
                fs_entry.hash = db_entry.hash;
                fs_entry.notify = db_entry.notify;
                fs_entry.user_id = db_entry.user_id;
                fs_entry.group_id = db_entry.group_id;

                result.push(fs_entry);
            } else {
                // Entry exists only in FS
                tracing::info!("Extra file in FS (not in DB): {}", fs_entry.path);
                result.push(fs_entry);
            }
        }

        // Any remaining entries in db_map exist only in DB
        for (path, _) in db_map {
            tracing::info!("Extra file in DB (not in FS): {}", path);
        }

        Ok(result)
    }

    /// Get the full filesystem path for a subpath within this storage.
    ///
    /// **Security:** This only performs lexical joining; it does NOT verify the
    /// resolved path stays inside the storage root. Prefer [`Self::resolve_safe_path`]
    /// for any operation driven by user input, which canonicalizes the path and
    /// rejects traversal (`..`) escapes. Use `get_full_path` only when the sub_path
    /// is trusted (e.g. constructed internally) or for paths that may not yet exist
    /// (canonicalize requires the file to be present).
    #[instrument]
    pub fn get_full_path(&self, sub_path: &str) -> PathBuf {
        let mut full_path = PathBuf::from(&self.model.path);
        let sanitized_path = sub_path.trim_start_matches('/');
        if !sanitized_path.is_empty() {
            full_path.push(sanitized_path);
        }
        full_path
    }

    /// Resolve a user-supplied sub-path to an absolute filesystem path,
    /// verifying it stays inside this storage's root directory.
    ///
    /// This defends against path-traversal attacks (e.g. `../../etc/passwd`) by
    /// canonicalizing the resulting path and checking it starts with the
    /// canonicalized storage root. Use this for any read/write/delete operation
    /// driven by request input.
    ///
    /// Returns [`io::ErrorKind::NotFound`] if the path does not exist yet
    /// (canonicalize requires existence); for not-yet-created paths use
    /// [`Self::resolve_safe_path_lexical`].
    #[instrument]
    pub async fn resolve_safe_path(&self, sub_path: &str) -> io::Result<PathBuf> {
        let root = tokio::fs::canonicalize(&self.model.path)
            .await
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("storage root '{}' inaccessible: {}", self.model.path, e),
                )
            })?;

        let candidate = self.join_sub_path(&root, sub_path);
        let canonical = tokio::fs::canonicalize(&candidate).await.map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("path '{}' inaccessible: {}", candidate.display(), e),
            )
        })?;

        if !canonical.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "resolved path escapes storage root",
            ));
        }
        Ok(canonical)
    }

    /// Lexically resolve a user-supplied sub-path against the storage root,
    /// rejecting any `..` component that would escape the root.
    ///
    /// Use this for paths that do not yet exist on disk (create/rename) where
    /// `canonicalize` would fail. The parent directory, if it exists, is
    /// canonicalized to confirm containment.
    #[instrument]
    pub async fn resolve_safe_path_lexical(&self, sub_path: &str) -> io::Result<PathBuf> {
        let root = tokio::fs::canonicalize(&self.model.path)
            .await
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("storage root '{}' inaccessible: {}", self.model.path, e),
                )
            })?;

        let candidate = self.join_sub_path(&root, sub_path);

        // Walk components manually to reject traversal without requiring existence.
        let mut depth: i32 = 0;
        for comp in candidate
            .strip_prefix(&root)
            .unwrap_or(candidate.as_path())
            .components()
        {
            match comp {
                std::path::Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "path escapes storage root",
                        ));
                    }
                }
                std::path::Component::Normal(_) => depth += 1,
                std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "absolute path not allowed within storage",
                    ));
                }
                std::path::Component::CurDir => {}
            }
        }

        Ok(candidate)
    }

    /// Join a sub-path onto a given base, stripping a leading slash.
    fn join_sub_path(&self, base: &Path, sub_path: &str) -> PathBuf {
        let sanitized = sub_path.trim_start_matches('/');
        let mut full = base.to_path_buf();
        if !sanitized.is_empty() {
            full.push(sanitized);
        }
        full
    }

    /// Open a file and return its handle and metadata.
    ///
    /// The sub-path is resolved safely and verified to stay within the storage
    /// root (rejects `..` traversal).
    #[instrument]
    pub async fn open_file(
        &self,
        sub_path: &str,
    ) -> io::Result<(tokio::fs::File, std::fs::Metadata)> {
        let full_path = self.resolve_safe_path(sub_path).await?;
        let file = tokio::fs::File::open(&full_path).await?;
        let metadata = file.metadata().await?;

        if metadata.is_dir() {
            return Err(io::Error::other("Path is a directory"));
        }

        Ok((file, metadata))
    }

    /// Save file content to the filesystem.
    ///
    /// The sub-path is resolved safely (lexical, since the file may not exist
    /// yet) and verified to stay within the storage root.
    #[instrument(skip(data))]
    pub async fn save_file(&self, sub_path: &str, data: &[u8]) -> io::Result<()> {
        let full_path = self.resolve_safe_path_lexical(sub_path).await?;

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(full_path, data).await
    }

    /// Create a new directory.
    ///
    /// The sub-path is resolved safely (lexical) and verified to stay within
    /// the storage root.
    #[instrument]
    pub async fn create_directory(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.resolve_safe_path_lexical(sub_path).await?;
        tokio::fs::create_dir_all(full_path).await
    }

    /// Create an empty file.
    ///
    /// The sub-path is resolved safely (lexical) and verified to stay within
    /// the storage root.
    #[instrument]
    pub async fn create_file(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.resolve_safe_path_lexical(sub_path).await?;

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::File::create(full_path).await?;
        Ok(())
    }

    /// Rename or move an entry.
    ///
    /// Both source and destination are resolved safely (lexical) and verified
    /// to stay within the storage root.
    #[instrument]
    pub async fn rename_entry(&self, old_path: &str, new_path: &str) -> io::Result<()> {
        let old_full_path = self.resolve_safe_path(old_path).await?;
        let new_full_path = self.resolve_safe_path_lexical(new_path).await?;

        // Ensure parent directory for the new path exists
        if let Some(parent) = new_full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::rename(old_full_path, new_full_path).await
    }

    /// Remove an entry (file or directory).
    ///
    /// The sub-path is resolved safely and verified to stay within the storage
    /// root.
    #[instrument]
    pub async fn remove_entry(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.resolve_safe_path(sub_path).await?;
        let metadata = tokio::fs::metadata(&full_path).await?;

        if metadata.is_dir() {
            tokio::fs::remove_dir_all(full_path).await
        } else {
            tokio::fs::remove_file(full_path).await
        }
    }

    /// Get or create the parent directory entry for a given sub-path.
    /// Returns the parent's ID. For paths without a `/`, returns the ID of
    /// the entry matching `sub_path` itself (treated as root-level directory).
    /// Recursively creates ancestor entries as needed.
    pub async fn get_parent_id(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<i32> {
        let target_path = sub_path.trim_matches('/');

        // Determine the directory path we need to resolve
        let dir_path = match target_path.rfind('/') {
            Some(pos) => &target_path[..pos],
            None => target_path,
        };

        // Check if the directory entry already exists in DB
        let existing = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(self.model.id))
            .filter(entry::Column::Path.eq(dir_path))
            .one(db)
            .await?;

        if let Some(m) = existing {
            return Ok(m.id);
        }

        // Recursively ensure the parent of this directory exists
        let parent_id = if let Some(pos) = dir_path.rfind('/') {
            let ancestor_path = &dir_path[..pos];
            Some(Box::pin(self.get_parent_id(db, ancestor_path)).await?)
        } else {
            None
        };

        // Gather metadata from the filesystem if available
        let full_path = self.get_full_path(dir_path);
        let modified_at = tokio::fs::metadata(&full_path)
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).naive_utc())
            .unwrap_or_else(|| Utc::now().naive_utc());

        let active = entry::ActiveModel {
            storage_id: Set(self.model.id),
            user_id: Set(self.model.default_user),
            group_id: Set(self.model.default_group),
            parent_id: Set(parent_id),
            path: Set(dir_path.to_string()),
            entry_type: Set(EntryType::Directory),
            notify: Set(false),
            size: Set(0),
            modified_at: Set(modified_at),
            created_at: Set(Utc::now().naive_utc()),
            ..Default::default()
        };

        let inserted = active.insert(db).await?;
        Ok(inserted.id)
    }

    /// Find or create a database entry for a given sub-path.
    ///
    /// The sub-path is resolved safely (rejecting traversal escapes) and
    /// metadata is read asynchronously (no blocking `std::fs` call).
    #[instrument(skip(self, db))]
    pub async fn ensure_entry(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<entry::Model> {
        let normalized_path = sub_path.trim_matches('/').to_string();
        let full_path = self.resolve_safe_path(&normalized_path).await?;

        anyhow::ensure!(full_path.exists(), "Path {} not found on disk", sub_path);

        let existing = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(self.model.id))
            .filter(entry::Column::Path.eq(&normalized_path))
            .one(db)
            .await?;

        if let Some(model) = existing {
            return Ok(model);
        }

        let parent_id = if normalized_path.contains('/') {
            Some(self.get_parent_id(db, &normalized_path).await?)
        } else {
            None
        };
        let metadata = tokio::fs::metadata(&full_path).await?;

        let entry_type = if metadata.is_dir() {
            EntryType::Directory
        } else if metadata.is_file() {
            EntryType::File
        } else {
            EntryType::Symlink
        };

        let active = entry::ActiveModel {
            storage_id: Set(self.model.id),
            user_id: Set(self.model.default_user),
            group_id: Set(self.model.default_group),
            parent_id: Set(parent_id),
            path: Set(normalized_path),
            entry_type: Set(entry_type),
            notify: Set(false),
            size: Set(metadata.len().try_into()?),
            modified_at: Set(metadata
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).naive_utc())
                .unwrap_or_else(|| Utc::now().naive_utc())),
            created_at: Set(Utc::now().naive_utc()),
            ..Default::default()
        };

        Ok(active.insert(db).await?)
    }

    /// Set the notify flag on a directory entry, creating it if it doesn't exist
    #[instrument(skip(db))]
    pub async fn set_notify(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
        notify: bool,
    ) -> Result<i32> {
        let model = self.ensure_entry(db, sub_path).await?;
        let mut active: entry::ActiveModel = model.into();
        active.notify = Set(notify);
        Ok(active.update(db).await?.id)
    }
}

/// Check if a path exists on the filesystem
pub fn path_exists(path: &str) -> bool {
    PathBuf::from(path).exists()
}

/// Validate that a path exists and is a readable directory
pub async fn validate_storage_path(path: &str) -> Result<()> {
    let path_buf = PathBuf::from(path);

    anyhow::ensure!(path_buf.exists(), "Path does not exist: {}", path);
    anyhow::ensure!(path_buf.is_dir(), "Path is not a directory: {}", path);

    let _ = fs::read_dir(&path_buf).await.context(format!(
        "Cannot read directory (permission denied): {}",
        path
    ))?;

    Ok(())
}

mod content_type;
mod hash;
pub mod thumbnail;
pub use content_type::determine_content_type;

/// Format file size in human-readable format
pub fn format_size(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Build a `Storage` rooted at a fresh temp directory holding one file.
    async fn temp_storage() -> Storage {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "byteburrow_storage_test_{}_{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(&root).await.unwrap();
        fs::write(root.join("inside.txt"), b"inside").await.unwrap();

        Storage::new(storage::Model {
            id: 1,
            name: "test".to_string(),
            description: None,
            path: root.to_string_lossy().into_owned(),
            default_user: 1,
            default_group: 1,
            ignore_patterns: String::new(),
        })
    }

    #[tokio::test]
    async fn resolve_safe_path_allows_files_inside_root() {
        let storage = temp_storage().await;
        let resolved = storage.resolve_safe_path("inside.txt").await.unwrap();
        assert!(resolved.ends_with("inside.txt"));
    }

    #[tokio::test]
    async fn resolve_safe_path_rejects_parent_traversal() {
        let storage = temp_storage().await;
        // A real target outside the root (/etc/passwd exists) must still be
        // rejected because it escapes the canonicalized storage root.
        let err = storage
            .resolve_safe_path("../../../../../../etc/passwd")
            .await
            .expect_err("traversal must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn resolve_safe_path_lexical_rejects_parent_traversal() {
        let storage = temp_storage().await;
        let err = storage
            .resolve_safe_path_lexical("../escape.txt")
            .await
            .expect_err("traversal must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn resolve_safe_path_lexical_treats_leading_slash_as_relative() {
        let storage = temp_storage().await;
        // A leading `/` is stripped before joining, so this must resolve
        // *inside* the storage root rather than escaping to the real /etc.
        let resolved = storage
            .resolve_safe_path_lexical("/etc/passwd")
            .await
            .expect("leading slash must not error");
        let root = tokio::fs::canonicalize(&storage.model.path).await.unwrap();
        assert!(resolved.starts_with(&root));
    }

    #[tokio::test]
    async fn save_file_rejects_traversal_escape() {
        let storage = temp_storage().await;
        let err = storage
            .save_file("../pwned.txt", b"data")
            .await
            .expect_err("write outside root must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
