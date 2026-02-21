use crate::entity::storage;
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

pub struct InotifyHandler {
    db: Arc<DatabaseConnection>,
    job_sender: JobSender,
    watcher: Option<RecommendedWatcher>,
    watched_storages: HashMap<i32, PathBuf>,
}

impl InotifyHandler {
    pub fn new(db: DatabaseConnection, job_sender: JobSender) -> Self {
        Self {
            db: Arc::new(db),
            job_sender,
            watcher: None,
            watched_storages: HashMap::new(),
        }
    }

    /// Start the inotify handler
    pub async fn run(mut self) {
        info!("Inotify handler started");

        // Create a channel for receiving filesystem events
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Create the filesystem watcher
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

        // Initial scan: load all storages with inotify enabled
        if let Err(e) = self.reload_watched_storages().await {
            error!("Failed to load initial watched storages: {}", e);
        }

        // Periodic reload interval (in seconds)
        let reload_interval = tokio::time::Duration::from_secs(60);
        let mut reload_timer = tokio::time::interval(reload_interval);

        loop {
            tokio::select! {
                // Handle filesystem events
                Some(event) = rx.recv() => {
                    self.handle_event(event).await;
                }

                // Periodically reload watched storages
                _ = reload_timer.tick() => {
                    if let Err(e) = self.reload_watched_storages().await {
                        error!("Failed to reload watched storages: {}", e);
                    }
                }
            }
        }
    }

    /// Reload the list of watched storages from the database
    async fn reload_watched_storages(&mut self) -> Result<(), sea_orm::DbErr> {
        debug!("Reloading watched storages");

        let storages = storage::Entity::find()
            .filter(storage::Column::Inotify.eq(true))
            .all(self.db.as_ref())
            .await?;

        let mut new_watched: HashMap<i32, PathBuf> = HashMap::new();

        for storage_model in storages {
            let path = PathBuf::from(&storage_model.path);

            // Check if already watching
            if let Some(existing_path) = self.watched_storages.get(&storage_model.id) {
                if existing_path == &path {
                    new_watched.insert(storage_model.id, path);
                    continue;
                }
            }

            // Start watching new storage
            if let Some(watcher) = &mut self.watcher {
                match watcher.watch(&path, RecursiveMode::Recursive) {
                    Ok(_) => {
                        info!("Watching storage {} at {}", storage_model.id, path.display());
                        new_watched.insert(storage_model.id, path);
                    }
                    Err(e) => {
                        error!("Failed to watch storage {}: {}", storage_model.id, e);
                    }
                }
            }
        }

        // Unwatch removed storages
        if let Some(watcher) = &mut self.watcher {
            for (storage_id, path) in &self.watched_storages {
                if !new_watched.contains_key(storage_id) {
                    if let Err(e) = watcher.unwatch(path) {
                        warn!("Failed to unwatch storage {}: {}", storage_id, e);
                    } else {
                        info!("Stopped watching storage {} at {}", storage_id, path.display());
                    }
                }
            }
        }

        self.watched_storages = new_watched;
        Ok(())
    }

    /// Handle a filesystem event
    async fn handle_event(&self, event: Event) {
        debug!("Filesystem event: {:?}", event);

        let storage_id = match self.find_storage_for_path(&event.paths) {
            Some(id) => id,
            None => return,
        };

        match event.kind {
            EventKind::Create(CreateKind::File) => {
                for path in &event.paths {
                    self.handle_file_created(storage_id, path).await;
                }
            }
            EventKind::Modify(ModifyKind::Data(_)) => {
                for path in &event.paths {
                    self.handle_file_modified(storage_id, path).await;
                }
            }
            EventKind::Remove(RemoveKind::File) => {
                for path in &event.paths {
                    self.handle_file_removed(storage_id, path).await;
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if event.paths.len() >= 2 {
                    self.handle_file_renamed(storage_id, &event.paths[0], &event.paths[1])
                        .await;
                }
            }
            EventKind::Create(CreateKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_created(storage_id, path).await;
                }
            }
            EventKind::Remove(RemoveKind::Folder) => {
                for path in &event.paths {
                    self.handle_folder_removed(storage_id, path).await;
                }
            }
            _ => {}
        }
    }

    fn find_storage_for_path(&self, paths: &[PathBuf]) -> Option<i32> {
        for path in paths {
            for (storage_id, storage_path) in &self.watched_storages {
                if path.starts_with(storage_path) {
                    return Some(*storage_id);
                }
            }
        }
        None
    }

    fn get_relative_path(&self, storage_id: i32, full_path: &PathBuf) -> Option<String> {
        if let Some(storage_path) = self.watched_storages.get(&storage_id) {
            if let Ok(relative) = full_path.strip_prefix(storage_path) {
                return Some(relative.to_string_lossy().to_string());
            }
        }
        None
    }

    async fn handle_file_created(&self, storage_id: i32, path: &PathBuf) {
        if let Some(relative_path) = self.get_relative_path(storage_id, path) {
            info!("File created in storage {}: {}", storage_id, relative_path);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: relative_path,
                })
                .ok();
        }
    }

    async fn handle_file_modified(&self, storage_id: i32, path: &PathBuf) {
        if let Some(relative_path) = self.get_relative_path(storage_id, path) {
            debug!("File modified in storage {}: {}", storage_id, relative_path);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: relative_path,
                })
                .ok();
        }
    }

    async fn handle_file_removed(&self, storage_id: i32, path: &PathBuf) {
        if let Some(relative_path) = self.get_relative_path(storage_id, path) {
            info!("File removed in storage {}: {}", storage_id, relative_path);
            // TODO: Handle file deletion in database
        }
    }

    async fn handle_file_renamed(
        &self,
        storage_id: i32,
        _old_path: &PathBuf,
        new_path: &PathBuf,
    ) {
        if let Some(new_relative) = self.get_relative_path(storage_id, new_path) {
            info!("File renamed in storage {}: {}", storage_id, new_relative);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: new_relative,
                })
                .ok();
        }
    }

    async fn handle_folder_created(&self, storage_id: i32, path: &PathBuf) {
        if let Some(relative_path) = self.get_relative_path(storage_id, path) {
            info!("Folder created in storage {}: {}", storage_id, relative_path);
            self.job_sender
                .send(Job::CheckFile {
                    storage_id,
                    path: relative_path,
                })
                .ok();
        }
    }

    async fn handle_folder_removed(&self, storage_id: i32, path: &PathBuf) {
        if let Some(relative_path) = self.get_relative_path(storage_id, path) {
            info!("Folder removed in storage {}: {}", storage_id, relative_path);
            // TODO: Handle folder deletion in database
        }
    }
}
