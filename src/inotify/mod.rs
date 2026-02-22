use crate::entity::{entry, storage};
use crate::job::{Job, JobSender};
use notify::{
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Result as NotifyResult, Watcher,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::HashMap;
use std::path::PathBuf;
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
}

pub struct InotifyHandler {
    db: Arc<DatabaseConnection>,
    job_sender: JobSender,
    watcher: Option<RecommendedWatcher>,
    /// entry_id -> watched entry metadata
    watched_entries: HashMap<i32, WatchedEntry>,
}

impl InotifyHandler {
    pub fn new(db: DatabaseConnection, job_sender: JobSender) -> Self {
        Self {
            db: Arc::new(db),
            job_sender,
            watcher: None,
            watched_entries: HashMap::new(),
        }
    }

    /// Start the inotify handler
    pub async fn run(mut self) {
        info!("Inotify handler started");

        let (tx, mut rx) = mpsc::unbounded_channel();

        let watcher = match notify::recommended_watcher(move |res: NotifyResult<Event>| {
            match res {
                Ok(event) => {
                    if let Err(e) = tx.send(event) {
                        error!("Failed to send filesystem event: {}", e);
                    }
                }
                Err(e) => error!("Filesystem watch error: {:?}", e),
            }
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

        let reload_interval = tokio::time::Duration::from_secs(60);
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

            // Check if already watching the same path
            if let Some(existing) = self.watched_entries.get(&entry_model.id) {
                if existing.abs_path == abs_path {
                    new_watched.insert(
                        entry_model.id,
                        WatchedEntry {
                            storage_id: entry_model.storage_id,
                            storage_base,
                            abs_path,
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

        let (storage_id, storage_base) = match self.find_storage_for_path(&event.paths) {
            Some(pair) => pair,
            None => return,
        };

        match event.kind {
            EventKind::Create(CreateKind::File) => {
                for path in &event.paths {
                    self.handle_file_created(storage_id, &storage_base, path)
                        .await;
                }
            }
            EventKind::Modify(ModifyKind::Data(_)) => {
                for path in &event.paths {
                    self.handle_file_modified(storage_id, &storage_base, path)
                        .await;
                }
            }
            EventKind::Remove(RemoveKind::File) => {
                for path in &event.paths {
                    self.handle_file_removed(storage_id, &storage_base, path)
                        .await;
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if event.paths.len() >= 2 {
                    self.handle_file_renamed(
                        storage_id,
                        &storage_base,
                        &event.paths[0],
                        &event.paths[1],
                    )
                    .await;
                }
            }
            EventKind::Create(CreateKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_created(storage_id, &storage_base, path)
                        .await;
                }
            }
            EventKind::Remove(RemoveKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_removed(storage_id, &storage_base, path)
                        .await;
                }
            }
            _ => {}
        }
    }

    /// Find the storage_id and storage base path for the first matching event path.
    fn find_storage_for_path(&self, paths: &[PathBuf]) -> Option<(i32, PathBuf)> {
        for path in paths {
            for watched in self.watched_entries.values() {
                if path.starts_with(&watched.abs_path) {
                    return Some((watched.storage_id, watched.storage_base.clone()));
                }
            }
        }
        None
    }

    /// Compute the path relative to the storage root.
    fn relative_path(storage_base: &PathBuf, full_path: &PathBuf) -> Option<String> {
        full_path
            .strip_prefix(storage_base)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    async fn handle_file_created(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            info!("File created in storage {}: {}", storage_id, rel);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: rel,
                })
                .ok();
        }
    }

    async fn handle_file_modified(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            debug!("File modified in storage {}: {}", storage_id, rel);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: rel,
                })
                .ok();
        }
    }

    async fn handle_file_removed(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            info!("File removed in storage {}: {}", storage_id, rel);
            // TODO: Handle file deletion in database
        }
    }

    async fn handle_file_renamed(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        _old_path: &PathBuf,
        new_path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, new_path) {
            info!("File renamed in storage {}: {}", storage_id, rel);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: rel,
                })
                .ok();
        }
    }

    async fn handle_folder_created(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            info!("Folder created in storage {}: {}", storage_id, rel);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: rel,
                })
                .ok();
        }
    }

    async fn handle_folder_removed(
        &self,
        storage_id: i32,
        storage_base: &PathBuf,
        path: &PathBuf,
    ) {
        if let Some(rel) = Self::relative_path(storage_base, path) {
            info!("Folder removed in storage {}: {}", storage_id, rel);
            // TODO: Handle folder deletion in database
        }
    }
}
