use crate::entity::{entry, face_reference, meta, photo, storage};
use crate::job::{Job, JobSender, ProcessMode};
use notify::{
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Result as NotifyResult, Watcher,
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Metadata for a watched directory entry.
struct WatchedEntry {
    storage_id: i32,
    /// Absolute path of the storage root (used for computing job-relative paths).
    storage_base: PathBuf,
    /// Absolute path being watched (storage_base + entry.path).
    abs_path: PathBuf,
    /// Parsed ignore patterns from the storage.
    ignore_patterns: Vec<String>,
}

pub struct InotifyHandler {
    db: Arc<DatabaseConnection>,
    job_sender: JobSender,
    watcher: Option<RecommendedWatcher>,
    /// entry_id -> watched entry metadata
    watched_entries: HashMap<i32, WatchedEntry>,
    /// Signal from the web layer that watched entries changed (H12).
    notify_reload: Arc<tokio::sync::Notify>,
}

impl InotifyHandler {
    pub fn new(
        db: DatabaseConnection,
        job_sender: JobSender,
        notify_reload: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            db: Arc::new(db),
            job_sender,
            watcher: None,
            watched_entries: HashMap::new(),
            notify_reload,
        }
    }

    /// Start the inotify handler
    pub async fn run(mut self) {
        info!("Inotify handler started");

        let (tx, mut rx) = mpsc::unbounded_channel();

        let watcher = match notify::recommended_watcher(move |res: NotifyResult<Event>| match res {
            Ok(event) => {
                if let Err(e) = tx.send(event) {
                    error!("Failed to send filesystem event: {}", e);
                }
            }
            Err(e) => error!("Filesystem watch error: {:?}", e),
        }) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create filesystem watcher: {}", e);
                return;
            }
        };

        self.watcher = Some(watcher);

        // Initial scan: load all directory entries with notify=true
        if let Err(e) = self.reload_watched_entries().await {
            error!("Failed to load initial watched entries: {}", e);
        }

        // H12: shrink the reload interval from 60s to 15s to reduce the
        // window where new watch dirs aren't watched.
        let reload_interval = tokio::time::Duration::from_secs(15);
        let mut reload_timer = tokio::time::interval(reload_interval);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    self.handle_event(event).await;
                }
                _ = reload_timer.tick() => {
                    if let Err(e) = self.reload_watched_entries().await {
                        error!("Failed to reload watched entries: {}", e);
                    }
                }
                // H12: immediate reload when the web layer signals a change
                // (e.g. after `set_notify`).
                _ = self.notify_reload.notified() => {
                    info!("Notify-reload signal received; reloading watched entries");
                    if let Err(e) = self.reload_watched_entries().await {
                        error!("Failed to reload watched entries: {}", e);
                    }
                }
            }
        }
    }

    /// Reload the list of watched directory entries from the database.
    /// Queries entries with notify=true (Directory type) and joins with their storage
    /// to obtain the absolute watched path.
    async fn reload_watched_entries(&mut self) -> Result<(), sea_orm::DbErr> {
        debug!("Reloading watched entries");

        let notify_entries = entry::Entity::find()
            .filter(entry::Column::Notify.eq(true))
            .filter(entry::Column::EntryType.eq(entry::EntryType::Directory))
            .all(self.db.as_ref())
            .await?;

        let mut new_watched: HashMap<i32, WatchedEntry> = HashMap::new();

        for entry_model in notify_entries {
            // Fetch the parent storage to get its base path
            let storage_model = match storage::Entity::find_by_id(entry_model.storage_id)
                .one(self.db.as_ref())
                .await?
            {
                Some(s) => s,
                None => {
                    warn!(
                        "Storage {} not found for entry {}",
                        entry_model.storage_id, entry_model.id
                    );
                    continue;
                }
            };

            let storage_base = PathBuf::from(&storage_model.path);
            let abs_path = if entry_model.path.is_empty() {
                storage_base.clone()
            } else {
                storage_base.join(&entry_model.path)
            };
            let ignore_patterns = crate::ignore::parse_patterns(&storage_model.ignore_patterns);

            // Check if already watching the same path
            if let Some(existing) = self.watched_entries.get(&entry_model.id) {
                if existing.abs_path == abs_path {
                    new_watched.insert(
                        entry_model.id,
                        WatchedEntry {
                            storage_id: entry_model.storage_id,
                            storage_base,
                            abs_path,
                            ignore_patterns,
                        },
                    );
                    continue;
                }
            }

            if let Some(watcher) = &mut self.watcher {
                match watcher.watch(&abs_path, RecursiveMode::Recursive) {
                    Ok(_) => {
                        info!(
                            "Watching entry {} at {}",
                            entry_model.id,
                            abs_path.display()
                        );
                        new_watched.insert(
                            entry_model.id,
                            WatchedEntry {
                                storage_id: entry_model.storage_id,
                                storage_base,
                                abs_path,
                                ignore_patterns,
                            },
                        );
                    }
                    Err(e) => {
                        error!("Failed to watch entry {}: {}", entry_model.id, e);
                    }
                }
            }
        }

        // Unwatch removed entries
        if let Some(watcher) = &mut self.watcher {
            for (entry_id, watched) in &self.watched_entries {
                if !new_watched.contains_key(entry_id) {
                    if let Err(e) = watcher.unwatch(&watched.abs_path) {
                        warn!("Failed to unwatch entry {}: {}", entry_id, e);
                    } else {
                        info!(
                            "Stopped watching entry {} at {}",
                            entry_id,
                            watched.abs_path.display()
                        );
                    }
                }
            }
        }

        self.watched_entries = new_watched;
        Ok(())
    }

    /// Handle a filesystem event
    async fn handle_event(&self, event: Event) {
        debug!("Filesystem event: {:?}", event);

        let (storage_id, storage_base, ignore_patterns) =
            match self.find_storage_for_path(&event.paths) {
                Some(tuple) => tuple,
                None => return,
            };

        match event.kind {
            EventKind::Create(CreateKind::File) => {
                for path in &event.paths {
                    self.handle_file_created(storage_id, &storage_base, &ignore_patterns, path)
                        .await;
                }
            }
            EventKind::Modify(ModifyKind::Data(_)) => {
                for path in &event.paths {
                    self.handle_file_modified(storage_id, &storage_base, &ignore_patterns, path)
                        .await;
                }
            }
            EventKind::Remove(RemoveKind::File) => {
                for path in &event.paths {
                    self.handle_file_removed(storage_id, &storage_base, &ignore_patterns, path)
                        .await;
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if event.paths.len() >= 2 {
                    self.handle_file_renamed(
                        storage_id,
                        &storage_base,
                        &ignore_patterns,
                        &event.paths[0],
                        &event.paths[1],
                    )
                    .await;
                }
            }
            EventKind::Create(CreateKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_created(storage_id, &storage_base, &ignore_patterns, path)
                        .await;
                }
            }
            EventKind::Remove(RemoveKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_removed(storage_id, &storage_base, &ignore_patterns, path)
                        .await;
                }
            }
            _ => {}
        }
    }

    /// Find the storage_id, storage base path, and ignore patterns for the first matching event path.
    fn find_storage_for_path(&self, paths: &[PathBuf]) -> Option<(i32, PathBuf, Vec<String>)> {
        for path in paths {
            for watched in self.watched_entries.values() {
                if path.starts_with(&watched.abs_path) {
                    return Some((
                        watched.storage_id,
                        watched.storage_base.clone(),
                        watched.ignore_patterns.clone(),
                    ));
                }
            }
        }
        None
    }

    /// Compute the path relative to the storage root.
    fn relative_path(storage_base: &Path, full_path: &Path) -> Option<String> {
        full_path
            .strip_prefix(storage_base)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    async fn handle_file_created(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            info!("File created in storage {}: {}", storage_id, rel);
            self.job_sender
                .try_send(Job::ProcessFile {
                    storage_id,
                    path: rel,
                    mode: ProcessMode::Auto,
                })
                .ok();
        }
    }

    async fn handle_file_modified(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            debug!("File modified in storage {}: {}", storage_id, rel);
            self.job_sender
                .try_send(Job::ProcessFile {
                    storage_id,
                    path: rel,
                    mode: ProcessMode::Auto,
                })
                .ok();
        }
    }

    /// Purge a single entry row and, if it was the last reference to its hash,
    /// also remove the hash-keyed `meta`/`photo`/`face_reference` rows and the
    /// on-disk thumbnails. Shares cascade automatically via the `shared` FK.
    async fn purge_entry_cascade(&self, entry: &entry::Model) {
        let db = self.db.as_ref();

        // 1. Delete the entry row by id. Shares cascade automatically.
        if let Err(e) = entry::Entity::delete_by_id(entry.id).exec(db).await {
            error!(entry_id = entry.id, error = %e, "Failed to delete entry row");
            return;
        }

        // 2. If the entry had a hash, clean up hash-keyed rows only when no other
        //    entry references the same hash (same content at another path/storage).
        if let Some(hash) = &entry.hash {
            let remaining = entry::Entity::find()
                .filter(entry::Column::Hash.eq(hash.clone()))
                .count(db)
                .await;

            let should_purge_hash = match remaining {
                Ok(0) => true,
                Ok(n) => {
                    debug!(hash = %hex::encode(hash), remaining = n, "Hash still referenced; keeping hash-keyed rows");
                    false
                }
                Err(e) => {
                    // Couldn't verify references — leave the hash-keyed rows in
                    // place rather than risk deleting shared data.
                    error!(hash = %hex::encode(hash), error = %e, "Failed to count hash references; skipping hash cleanup");
                    false
                }
            };

            if should_purge_hash {
                let hash_hex = hex::encode(hash);

                if let Err(e) = meta::Entity::delete_many()
                    .filter(meta::Column::Hash.eq(hash.clone()))
                    .exec(db)
                    .await
                {
                    error!(hash = %hash_hex, error = %e, "Failed to delete meta row");
                }
                if let Err(e) = photo::Entity::delete_many()
                    .filter(photo::Column::Hash.eq(hash.clone()))
                    .exec(db)
                    .await
                {
                    error!(hash = %hash_hex, error = %e, "Failed to delete photo row");
                }
                if let Err(e) = face_reference::Entity::delete_many()
                    .filter(face_reference::Column::Hash.eq(hash.clone()))
                    .exec(db)
                    .await
                {
                    error!(hash = %hash_hex, error = %e, "Failed to delete face_reference rows");
                }

                // Best-effort: remove the on-disk thumbnails. Missing files or
                // I/O errors are logged but not fatal.
                let thumbnail_dir =
                    std::path::PathBuf::from(&crate::config::Config::get().thumbnail_storage);
                for size in ["mini", "small", "large"] {
                    let thumb_path = crate::storage::thumbnail::get_thumbnail_path(
                        &thumbnail_dir,
                        &hash_hex,
                        size,
                    );
                    if let Err(e) = tokio::fs::remove_file(&thumb_path).await {
                        // NotFound is expected when no thumbnail existed.
                        warn!(hash = %hash_hex, size, error = %e, "Failed to remove thumbnail");
                    }
                }

                debug!(hash = %hash_hex, "Purged hash-keyed rows and thumbnails");
            }
        }
    }

    async fn handle_file_removed(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            info!(storage_id, path = %rel, "File removed");

            let entry = match entry::Entity::find()
                .filter(entry::Column::StorageId.eq(storage_id))
                .filter(entry::Column::Path.eq(&rel))
                .one(self.db.as_ref())
                .await
            {
                Ok(Some(m)) => m,
                Ok(None) => {
                    debug!(storage_id, path = %rel, "Removed file not tracked in DB");
                    return;
                }
                Err(e) => {
                    error!(storage_id, path = %rel, error = %e, "Failed to look up removed file");
                    return;
                }
            };

            self.purge_entry_cascade(&entry).await;
        }
    }

    async fn handle_file_renamed(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        _old_path: &Path,
        new_path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, new_path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            info!("File renamed in storage {}: {}", storage_id, rel);
            self.job_sender
                .try_send(Job::ProcessFile {
                    storage_id,
                    path: rel,
                    mode: ProcessMode::Auto,
                })
                .ok();
        }
    }

    async fn handle_folder_created(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            info!("Folder created in storage {}: {}", storage_id, rel);
            self.job_sender
                .try_send(Job::ProcessFile {
                    storage_id,
                    path: rel,
                    mode: ProcessMode::Auto,
                })
                .ok();
        }
    }

    async fn handle_folder_removed(
        &self,
        storage_id: i32,
        storage_base: &Path,
        ignore_patterns: &[String],
        path: &Path,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            if crate::ignore::is_ignored(&rel, ignore_patterns) {
                return;
            }
            info!(storage_id, path = %rel, "Folder removed");

            // Match the folder itself plus everything nested under it.
            let prefix = format!("{rel}/");
            let cond = Condition::any()
                .add(entry::Column::Path.eq(&rel))
                .add(entry::Column::Path.like(format!("{}%", prefix)));

            let entries = match entry::Entity::find()
                .filter(entry::Column::StorageId.eq(storage_id))
                .filter(cond)
                .all(self.db.as_ref())
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    error!(storage_id, path = %rel, error = %e, "Failed to look up removed folder contents");
                    return;
                }
            };

            let count = entries.len();
            for entry in &entries {
                self.purge_entry_cascade(entry).await;
            }
            info!(storage_id, path = %rel, removed = count, "Purged folder entries");
        }
    }
}
