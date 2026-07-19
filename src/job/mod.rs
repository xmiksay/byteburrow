mod classify;
mod exif;
mod face;
mod thumb;

use std::path::Path;
use std::sync::Arc;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, instrument, warn};

use crate::config::Config;
use crate::entity::entry;
use crate::plugin::PluginRegistry;
use crate::storage::{thumbnail, Storage};

/// Default nice value for background job threads (lower priority than web server).
/// Range: 0 (normal) to 19 (lowest priority). 10 is a reasonable background level.
const JOB_THREAD_NICE: i32 = 10;

/// Controls what processing to perform on a file.
#[derive(Debug, Clone, Copy)]
pub enum ProcessMode {
    /// Check if file changed (hash differs). If yes, rehash AND run plugins.
    /// Respects the entry's `skip_plugins` flag.
    Auto,
    /// Force re-run the plugin classification cycle regardless of whether
    /// the file hash changed. Ignores the `skip_plugins` flag.
    ForceClassify,
    /// Only recalculate hash, never run plugins.
    HashOnly,
}

#[derive(Debug)]
pub enum Job {
    /// Unified file processing: check hash, optionally run plugin classification.
    ProcessFile {
        storage_id: i32,
        path: String,
        mode: ProcessMode,
    },
    /// Generate thumbnails for an entry identified by hash.
    CreateThumbnail { hash: Vec<u8>, regenerate: bool },
}

pub type JobSender = mpsc::UnboundedSender<Job>;

pub struct JobRunner {
    rx: mpsc::UnboundedReceiver<Job>,
    db: Arc<DatabaseConnection>,
    semaphore: Arc<Semaphore>,
    plugins: Arc<PluginRegistry>,
    runtime: tokio::runtime::Runtime,
}

impl JobRunner {
    pub fn new(db: DatabaseConnection, plugins: PluginRegistry) -> (Self, JobSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        info!(workers, nice = JOB_THREAD_NICE, "Job runner concurrency");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name("byteburrow-job")
            .on_thread_start(|| {
                // Set lower scheduling priority for job threads so the web
                // server (running on the main runtime) is always preferred
                // by the OS scheduler.
                unsafe {
                    libc::nice(JOB_THREAD_NICE);
                }
            })
            .enable_all()
            .build()
            .expect("Failed to create job runtime");

        (
            Self {
                rx,
                db: Arc::new(db),
                semaphore: Arc::new(Semaphore::new(workers)),
                plugins: Arc::new(plugins),
                runtime,
            },
            tx,
        )
    }

    /// Run the job processing loop. This blocks the calling thread and
    /// executes all jobs on the dedicated low-priority runtime.
    pub fn run(mut self) {
        self.runtime.block_on(async move {
            info!(
                "Job runner started (dedicated runtime, nice {})",
                JOB_THREAD_NICE
            );
            while let Some(job) = self.rx.recv().await {
                let permit = self
                    .semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore is never closed");
                let db = self.db.clone();
                let plugins = self.plugins.clone();
                tokio::spawn(async move {
                    info!(?job, "Processing job");
                    if let Err(e) = Self::process_job(&db, &plugins, job).await {
                        error!("Job failed: {e}");
                    }
                    drop(permit);
                });
            }
            info!("Job runner stopped");
        });
    }

    #[instrument(skip(db, plugins))]
    async fn process_job(
        db: &DatabaseConnection,
        plugins: &PluginRegistry,
        job: Job,
    ) -> anyhow::Result<()> {
        match job {
            Job::ProcessFile {
                storage_id,
                path,
                mode,
            } => {
                Self::process_file(db, plugins, storage_id, &path, mode).await?;
            }

            Job::CreateThumbnail {
                ref hash,
                regenerate,
            } => {
                Self::create_thumbnail(db, hash, regenerate).await?;
            }
        }

        Ok(())
    }

    async fn process_file(
        db: &DatabaseConnection,
        plugins: &PluginRegistry,
        storage_id: i32,
        path: &str,
        mode: ProcessMode,
    ) -> anyhow::Result<()> {
        let (changed, hash, entry, full_path) = Self::hash_and_diff(db, storage_id, path).await?;

        // In Auto mode, skip if nothing changed.
        if !changed && matches!(mode, ProcessMode::Auto) {
            return Ok(());
        }

        if Self::should_classify(&entry, mode) {
            classify::run_classification(db, plugins, &entry, &hash, &full_path).await?;
        }

        if is_image_file(&entry.path) {
            let hash_hex = hex::encode(&hash);
            thumb::generate_thumbnails(&full_path, &hash_hex).await?;
        }

        Ok(())
    }

    /// Compute the file hash, compare it to the stored entry, and return
    /// `(changed, hash, entry, full_path)`. Also short-circuits ignored paths.
    async fn hash_and_diff(
        db: &DatabaseConnection,
        storage_id: i32,
        path: &str,
    ) -> anyhow::Result<(bool, Vec<u8>, entry::Model, std::path::PathBuf)> {
        let storage = Storage::find_by_id(db, storage_id).await?;

        // Filter excluded paths using per-storage ignore patterns.
        let patterns = crate::ignore::parse_patterns(&storage.model.ignore_patterns);
        if crate::ignore::is_ignored(path, &patterns) {
            anyhow::bail!("ignored");
        }

        let (updated, hash, entry) = storage.calculate_hash(db, path).await?;
        let full_path = storage.get_full_path(&entry.path);

        Ok((updated, hash, entry, full_path))
    }

    /// Decide whether the classification pipeline should run for this entry
    /// under the given [`ProcessMode`].
    fn should_classify(entry: &entry::Model, mode: ProcessMode) -> bool {
        match mode {
            ProcessMode::HashOnly => false,
            ProcessMode::Auto => !entry.skip_plugins,
            ProcessMode::ForceClassify => true,
        }
    }

    async fn create_thumbnail(
        db: &DatabaseConnection,
        hash_bytes: &[u8],
        regenerate: bool,
    ) -> anyhow::Result<()> {
        let hash_hex = hex::encode(hash_bytes);

        let entry = entry::Entity::find()
            .filter(entry::Column::Hash.eq(hash_bytes.to_vec()))
            .one(db)
            .await?;

        let entry = match entry {
            Some(e) => e,
            None => {
                warn!(hash = %hash_hex, "No entry found for hash");
                return Ok(());
            }
        };

        if !is_image_file(&entry.path) {
            return Ok(());
        }

        let storage = Storage::find_by_id(db, entry.storage_id).await?;
        let full_path = storage.get_full_path(&entry.path);

        if regenerate {
            let config = Config::get();
            let thumbnail_dir = std::path::PathBuf::from(&config.thumbnail_storage);
            for size in ["mini", "small", "large"] {
                let path = thumbnail::get_thumbnail_path(&thumbnail_dir, &hash_hex, size);
                let _ = tokio::fs::remove_file(&path).await;
            }
        }

        thumb::generate_thumbnails(&full_path, &hash_hex).await
    }
}

fn is_image_file(path: &str) -> bool {
    let path = Path::new(path);
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "jpg"
                | "jpeg"
                | "png"
                | "gif"
                | "webp"
                | "bmp"
                | "tiff"
                | "tif"
                | "heic"
                | "heif"
                | "avif"
        ),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_image_file_recognizes_known_extensions() {
        for ext in [
            "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "heic", "heif", "avif",
        ] {
            assert!(is_image_file(&format!("photo.{ext}")), "ext {ext}");
            assert!(
                is_image_file(&format!("photo.{}", ext.to_uppercase())),
                "uppercase ext {ext}"
            );
        }
    }

    #[test]
    fn is_image_file_rejects_non_image_extensions() {
        assert!(!is_image_file("document.pdf"));
        assert!(!is_image_file("archive.zip"));
        assert!(!is_image_file("no_extension"));
        assert!(!is_image_file(""));
    }

    #[test]
    fn is_image_file_handles_nested_paths() {
        assert!(is_image_file("a/b/c/photo.PNG"));
        assert!(!is_image_file("a/b/c/readme"));
    }

    #[test]
    fn should_classify_hash_only_never_classifies() {
        let entry = make_entry(false);
        assert!(!JobRunner::should_classify(&entry, ProcessMode::HashOnly));

        let entry = make_entry(true);
        assert!(!JobRunner::should_classify(&entry, ProcessMode::HashOnly));
    }

    #[test]
    fn should_classify_auto_respects_skip_plugins_flag() {
        let entry = make_entry(false);
        assert!(JobRunner::should_classify(&entry, ProcessMode::Auto));

        let entry = make_entry(true);
        assert!(!JobRunner::should_classify(&entry, ProcessMode::Auto));
    }

    #[test]
    fn should_classify_force_classify_always_classifies() {
        let entry = make_entry(true);
        assert!(JobRunner::should_classify(
            &entry,
            ProcessMode::ForceClassify
        ));

        let entry = make_entry(false);
        assert!(JobRunner::should_classify(
            &entry,
            ProcessMode::ForceClassify
        ));
    }

    fn make_entry(skip_plugins: bool) -> entry::Model {
        let now = chrono::Utc::now().naive_utc();
        entry::Model {
            id: 1,
            storage_id: 1,
            user_id: 1,
            group_id: 1,
            parent_id: None,
            path: "photo.jpg".to_string(),
            hash: None,
            entry_type: entry::EntryType::File,
            notify: false,
            skip_plugins,
            size: 0,
            modified_at: now,
            created_at: now,
        }
    }
}
