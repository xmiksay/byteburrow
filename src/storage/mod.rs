use crate::entity::entry::{self, EntryType};
use crate::entity::storage;
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use tokio::fs;
use tracing::instrument;

#[derive(Debug)]
pub struct Storage {
    pub model: storage::Model,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DirectoryEntry {
    pub id: Option<i32>,
    pub storage_id: i32,
    pub user_id: i32,
    pub group_id: i32,
    pub parent_id: Option<i32>,
    pub path: String,
    pub hash: Option<Vec<u8>>,
    pub entry_type: EntryType,
    pub size: i64,
    pub tags: Vec<i32>,
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
            size: model.size,
            tags: model.tags,
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
        let mut full_path = base_path.clone();

        // Sanitize sub_path to prevent path traversal
        let sanitized_path = sub_path.trim_start_matches('/');
        if !sanitized_path.is_empty() {
            full_path.push(sanitized_path);
        }

        let mut read_dir = fs::read_dir(&full_path).await?;
        let mut entries = Vec::new();

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            // Get relative path as string
            let relative_path = path
                .strip_prefix(&base_path)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
                .to_string_lossy()
                .into_owned();

            let modified_at = metadata
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t))
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
                size: metadata.len().try_into().unwrap(),
                tags: Vec::new(),
                created_at: modified_at.clone(),
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
                fs_entry.tags = db_entry.tags;
                fs_entry.hash = db_entry.hash;
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

    /// Get the full filesystem path for a subpath within this storage
    #[instrument]
    pub fn get_full_path(&self, sub_path: &str) -> PathBuf {
        let mut full_path = PathBuf::from(&self.model.path);
        let sanitized_path = sub_path.trim_start_matches('/');
        if !sanitized_path.is_empty() {
            full_path.push(sanitized_path);
        }
        full_path
    }

    /// Open a file and return its handle and metadata
    #[instrument]
    pub async fn open_file(
        &self,
        sub_path: &str,
    ) -> io::Result<(tokio::fs::File, std::fs::Metadata)> {
        let full_path = self.get_full_path(sub_path);
        let file = tokio::fs::File::open(&full_path).await?;
        let metadata = file.metadata().await?;

        if metadata.is_dir() {
            return Err(io::Error::new(io::ErrorKind::Other, "Path is a directory"));
        }

        Ok((file, metadata))
    }

    /// Save file content to the filesystem
    #[instrument(skip(data))]
    pub async fn save_file(&self, sub_path: &str, data: &[u8]) -> io::Result<()> {
        let full_path = self.get_full_path(sub_path);

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(full_path, data).await
    }

    /// Create a new directory
    #[instrument]
    pub async fn create_directory(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.get_full_path(sub_path);
        tokio::fs::create_dir_all(full_path).await
    }

    /// Create an empty file
    #[instrument]
    pub async fn create_file(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.get_full_path(sub_path);

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::File::create(full_path).await?;
        Ok(())
    }

    /// Rename or move an entry
    #[instrument]
    pub async fn rename_entry(&self, old_path: &str, new_path: &str) -> io::Result<()> {
        let old_full_path = self.get_full_path(old_path);
        let new_full_path = self.get_full_path(new_path);

        // Ensure parent directory for the new path exists
        if let Some(parent) = new_full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::rename(old_full_path, new_full_path).await
    }

    /// Remove an entry (file or directory)
    #[instrument]
    pub async fn remove_entry(&self, sub_path: &str) -> io::Result<()> {
        let full_path = self.get_full_path(sub_path);
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
            .unwrap();

        let active = entry::ActiveModel {
            storage_id: Set(self.model.id),
            user_id: Set(self.model.default_user),
            group_id: Set(self.model.default_group),
            parent_id: Set(parent_id),
            path: Set(dir_path.to_string()),
            entry_type: Set(EntryType::Directory),
            size: Set(0),
            tags: Set(Vec::new()),
            modified_at: Set(modified_at),
            created_at: Set(Utc::now().naive_utc()),
            ..Default::default()
        };

        let inserted = active.insert(db).await?;
        Ok(inserted.id)
    }

    /// Find or create a database entry for a given sub-path
    #[instrument(skip(self, db))]
    pub async fn ensure_entry(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
    ) -> Result<entry::Model> {
        let normalized_path = sub_path.trim_matches('/').to_string();
        let full_path = self.get_full_path(&normalized_path);

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
        let metadata = std::fs::metadata(&full_path)?;

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
            size: Set(metadata.len().try_into()?),
            tags: Set(Vec::new()),
            modified_at: Set(metadata
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).naive_utc())
                .unwrap()),
            created_at: Set(Utc::now().naive_utc()),
            ..Default::default()
        };

        Ok(active.insert(db).await?)
    }

    /// Set tags for an entry, creating it if it doesn't exist in the database
    #[instrument(skip(db, tags))]
    pub async fn set_tags(
        &self,
        db: &sea_orm::DatabaseConnection,
        sub_path: &str,
        tags: Vec<i32>,
    ) -> Result<i32> {
        let model = self.ensure_entry(db, sub_path).await?;
        let mut active: entry::ActiveModel = model.into();
        active.tags = Set(tags);
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

    let _ = fs::read_dir(&path_buf)
        .await
        .context(format!("Cannot read directory (permission denied): {}", path))?;

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
