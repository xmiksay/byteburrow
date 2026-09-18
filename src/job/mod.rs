mod classify;
mod exif;
mod face;
mod thumb;

// Re-exported for the face-management API (`src/web/face.rs`) and the CLI
// (`src/bin/byteburrow_cli.rs`): the confirm/assign endpoints sync per-file
// meta after every label change, the explicit rematch endpoint runs the
// backfill synchronously (#26/#27), and the CLI `face_match`/`face_rematch`
// tools share the exact same code paths.
pub use face::{rematch_unconfirmed_faces, sync_face_meta, RematchOutcome, RematchScope};

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
    /// Re-run face matching over stored embeddings (issue #27). `scope`
    /// restricts the pass to one embedding model — the exact slice a single
    /// confirmation can change; `None` re-decides every machine-suggested
    /// face (manual backfill, contact deletion).
    RematchFaces { scope: Option<(String, String)> },
}

/// Bounded channel capacity — prevents OOM under bulk file copy (M8). Jobs
/// that can't be enqueued immediately are rejected (the sender logs and drops
/// them); this is the safe backpressure behavior for a background classifier.
const JOB_CHANNEL_CAPACITY: usize = 1024;

// ===== Retry policy + dead-letter (issue #19) =====

/// Total attempts a job gets before it is dead-lettered: the initial run
/// plus up to two retries.
const MAX_JOB_ATTEMPTS: u32 = 3;

/// Seconds to sleep after the n-th failed attempt (`n` is 1-based, so the
/// initial attempt needs no slot). Kept as a table so the schedule is
/// greppable and adjustable in one place.
const RETRY_BACKOFF_SECS: [u64; 2] = [5, 30];

// One backoff slot per retry — keeps the table honest if either const moves.
const _: () = assert!(
    RETRY_BACKOFF_SECS.len() == MAX_JOB_ATTEMPTS as usize - 1,
    "one backoff slot per retry (the initial attempt needs no backoff)"
);

/// Backoff in seconds before the attempt after the n-th failure, where `n`
/// is 1-based (n = 1 → first retry). Valid for `n < MAX_JOB_ATTEMPTS`;
/// out-of-range values return 0 as a safe fallback (callers never produce
/// them — see the const assert below).
///
/// Pure function on purpose: tests exercise the whole schedule without
/// sleeping, and `run_with_retries` takes the schedule as a parameter.
fn retry_backoff_secs(failed_attempt: u32) -> u64 {
    // Attempt numbers are 1-based; 0 (and any past-the-table value) has no
    // slot. The retry loop caps at MAX_JOB_ATTEMPTS so out-of-range inputs
    // never occur in production — 0 keeps malformed calls honest anyway.
    let Some(idx) = failed_attempt.checked_sub(1) else {
        return 0;
    };
    RETRY_BACKOFF_SECS.get(idx as usize).copied().unwrap_or(0)
}

/// Marker for expected, non-retryable outcomes — e.g. a path that matches
/// the storage's ignore patterns. Replaces the bare `bail!("ignored")` in
/// `hash_and_diff` so `is_transient` can recognize it structurally instead
/// of string-matching.
#[derive(Debug)]
struct IgnoredPath;

impl std::fmt::Display for IgnoredPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("path matches storage ignore patterns")
    }
}
impl std::error::Error for IgnoredPath {}

/// Decide whether a failed job attempt is worth retrying.
///
/// Walks the whole `anyhow` error *chain* (context layers wrap the real
/// cause) and defaults to **permanent** — retrying a logic bug just burns a
/// worker for 35s and re-logs the same failure:
///
/// - **Transient**: `sea_orm::DbErr` (pool timeouts, dropped connections)
///   except `RecordNotFound` (a missing storage/entry row will not heal),
///   and `std::io::Error` except `NotFound`/`IsADirectory` (file vanished
///   or was replaced by a directory between the inotify event and
///   processing — expected churn, not a fault).
/// - **Permanent**: the [`IgnoredPath`] marker, `RecordNotFound`,
///   vanished-file io errors, and anything unrecognized (plugin/FFI
///   failures, image decode errors, …).
///
/// The first recognizable type in the chain wins; mixed chains do not
/// occur in practice because each failure site produces one root cause.
fn is_transient(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        // `chain()` includes the outermost context; `source()` would skip it.
        if let Some(db_err) = cause.downcast_ref::<sea_orm::DbErr>() {
            return !matches!(db_err, sea_orm::DbErr::RecordNotFound(_));
        }
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            return !matches!(
                io_err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::IsADirectory
            );
        }
        if cause.downcast_ref::<IgnoredPath>().is_some() {
            return false;
        }
    }
    false
}

/// Outcome of [`run_with_retries`]: what actually happened, so the caller
/// can tell "succeeded (possibly after retries)" from "dead-letter this".
#[derive(Debug)]
enum RetryOutcome {
    /// Gave up. Carries the attempts made and the final error.
    Exhausted {
        attempts: u32,
        last_error: anyhow::Error,
    },
    /// `op` succeeded — on the first try if `attempts == 1`, else after
    /// transient failures that later recovered.
    Succeeded { attempts: u32 },
}

/// Run `op` up to [`MAX_JOB_ATTEMPTS`] times, sleeping `backoff(failed_n)`
/// between attempts — but only when the failure is [`is_transient`].
/// Permanent failures return immediately; transient failures that run out
/// of attempts return [`RetryOutcome::Exhausted`] for the caller to
/// dead-letter.
///
/// `op` is a closure returning a fresh future per attempt (jobs are
/// re-executed from scratch, not resumed); `backoff` is injected so tests
/// pass `|_| 0` and never sleep. Attempt-count termination lives *here*,
/// not in the backoff schedule, so no backoff function can loop forever.
///
/// This is the retry half of issue #19; the dead-letter log lives at the
/// call site, where the `Job` (for structured fields) is still in scope.
async fn run_with_retries<F, Fut>(mut op: F, backoff: impl Fn(u32) -> u64) -> RetryOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut attempt: u32 = 1;
    loop {
        let err = match op().await {
            Ok(()) => return RetryOutcome::Succeeded { attempts: attempt },
            Err(e) => e,
        };

        if attempt >= MAX_JOB_ATTEMPTS || !is_transient(&err) {
            return RetryOutcome::Exhausted {
                attempts: attempt,
                last_error: err,
            };
        }

        let secs = backoff(attempt);
        warn!(
            attempt,
            total = MAX_JOB_ATTEMPTS,
            backoff_secs = secs,
            error = %err,
            "Job attempt failed (transient); retrying"
        );
        tokio::time::sleep(Duration::from_secs(secs)).await;
        attempt += 1;
    }
}

/// Emit the single structured dead-letter line after a job exhausted its
/// retries (issue #19). Exactly one `error!` per dead-lettered job;
/// per-attempt detail was already logged as `warn!` by `run_with_retries`,
/// so this line carries only identity + final state.
fn dead_letter(job: &Job, attempts: u32, err: &anyhow::Error) {
    match job {
        Job::ProcessFile {
            storage_id, path, ..
        } => error!(
            job = ?job,
            storage_id,
            path,
            attempts,
            error = %err,
            "Dead-lettering job: retries exhausted"
        ),
        Job::CreateThumbnail { hash, .. } => error!(
            job = ?job,
            hash = %hex::encode(hash),
            attempts,
            error = %err,
            "Dead-lettering job: retries exhausted"
        ),
        Job::RematchFaces { scope } => error!(
            job = ?job,
            scope = ?scope,
            attempts,
            error = %err,
            "Dead-lettering job: retries exhausted"
        ),
    }
}

pub type JobSender = mpsc::Sender<Job>;

pub struct JobRunner {
    rx: mpsc::Receiver<Job>,
    db: Arc<DatabaseConnection>,
    semaphore: Arc<Semaphore>,
    plugins: Arc<PluginRegistry>,
    runtime: tokio::runtime::Runtime,
}

impl JobRunner {
    pub fn new(db: DatabaseConnection, plugins: PluginRegistry) -> (Self, JobSender) {
        let (tx, rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
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
                let _ = unsafe { libc::nice(JOB_THREAD_NICE) }; // M14: don't ignore failure
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
    ///
    /// When the sender is dropped, the channel drains: in-flight jobs are
    /// awaited (M6 — graceful shutdown) before the runtime shuts down.
    ///
    /// Retries (issue #19) happen *inside* the spawned task, so a job holds
    /// its semaphore permit across backoff sleeps (up to 5s + 30s). Tradeoff:
    /// a worker slot sits idle during backoff, briefly lowering effective
    /// concurrency below `workers`. That is preferred over re-enqueuing
    /// through the channel because re-enqueue costs a `try_send` race — a
    /// full channel would dead-letter a job before its retries even ran —
    /// and keeps the M6 drain guarantee simple: draining waits out pending
    /// retries too.
    pub fn run(mut self) {
        self.runtime.block_on(async move {
            info!(
                "Job runner started (dedicated runtime, nice {}, channel cap {}, max attempts {})",
                JOB_THREAD_NICE, JOB_CHANNEL_CAPACITY, MAX_JOB_ATTEMPTS
            );
            // Track spawned jobs so we can await them on shutdown (M6).
            let mut tasks = tokio::task::JoinSet::new();
            while let Some(job) = self.rx.recv().await {
                let permit = self
                    .semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore is never closed");
                let db = self.db.clone();
                let plugins = self.plugins.clone();
                tasks.spawn(async move {
                    info!(?job, "Processing job");
                    match run_with_retries(
                        || Self::process_job(&db, &plugins, &job),
                        retry_backoff_secs,
                    )
                    .await
                    {
                        RetryOutcome::Succeeded { attempts } if attempts > 1 => {
                            info!(?job, attempts, "Job succeeded after retry");
                        }
                        RetryOutcome::Succeeded { .. } => {}
                        RetryOutcome::Exhausted {
                            attempts,
                            last_error,
                        } => dead_letter(&job, attempts, &last_error),
                    }
                    drop(permit);
                });
            }
            // M6: graceful drain — wait for all in-flight jobs to finish.
            info!(
                "Job channel closed; draining {} in-flight job(s)",
                tasks.len()
            );
            while tasks.join_next().await.is_some() {}
            info!("Job runner stopped");
        });
    }

    #[instrument(skip(db, plugins))]
    async fn process_job(
        db: &DatabaseConnection,
        plugins: &PluginRegistry,
        job: &Job,
    ) -> anyhow::Result<()> {
        match job {
            Job::ProcessFile {
                storage_id,
                path,
                mode,
            } => {
                Self::process_file(db, plugins, *storage_id, path, *mode).await?;
            }

            Job::CreateThumbnail {
                ref hash,
                regenerate,
            } => {
                Self::create_thumbnail(db, hash, *regenerate).await?;
            }

            Job::RematchFaces { scope } => {
                let config = Config::get();
                let outcome = face::rematch_unconfirmed_faces(
                    db,
                    crate::face_match::MatchParams {
                        threshold: config.face_match_threshold,
                        margin: config.face_match_margin,
                    },
                    scope.clone(),
                )
                .await?;
                info!(%outcome, "RematchFaces job complete");
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
            // Marker type (not a string bail) so `is_transient` recognizes
            // this expected outcome structurally and never retries it.
            return Err(IgnoredPath.into());
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

    // ===== issue #19: retry classification, backoff schedule, retry loop =====

    fn io_err(kind: std::io::ErrorKind) -> anyhow::Error {
        std::io::Error::from(kind).into()
    }

    #[test]
    fn backoff_table_has_one_slot_per_retry() {
        assert_eq!(RETRY_BACKOFF_SECS.len(), (MAX_JOB_ATTEMPTS - 1) as usize);
        assert_eq!(MAX_JOB_ATTEMPTS, 3);
    }

    #[test]
    fn retry_backoff_schedule_matches_consts() {
        assert_eq!(retry_backoff_secs(1), RETRY_BACKOFF_SECS[0]);
        assert_eq!(retry_backoff_secs(2), RETRY_BACKOFF_SECS[1]);
        assert_eq!(RETRY_BACKOFF_SECS, [5, 30]);
    }

    #[test]
    fn retry_backoff_out_of_range_falls_back_to_zero() {
        // No third retry exists; 0 also guards the saturating-sub path.
        assert_eq!(retry_backoff_secs(0), 0);
        assert_eq!(retry_backoff_secs(MAX_JOB_ATTEMPTS), 0);
        assert_eq!(retry_backoff_secs(u32::MAX), 0);
    }

    #[test]
    fn db_errors_are_transient() {
        let err: anyhow::Error =
            sea_orm::DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::Timeout).into();
        assert!(is_transient(&err));

        let err: anyhow::Error =
            sea_orm::DbErr::Conn(sea_orm::RuntimeErr::Internal("closed".into())).into();
        assert!(is_transient(&err));

        let err: anyhow::Error = sea_orm::DbErr::Custom("pool saturated".into()).into();
        assert!(is_transient(&err));
    }

    #[test]
    fn record_not_found_is_permanent_even_with_context() {
        let err: anyhow::Error = sea_orm::DbErr::RecordNotFound("storage 1".into()).into();
        assert!(!is_transient(&err));

        let wrapped = anyhow::Error::from(sea_orm::DbErr::RecordNotFound("storage 1".into()))
            .context("looking up storage");
        assert!(!is_transient(&wrapped));
    }

    #[test]
    fn vanished_file_is_permanent_but_other_io_errors_are_transient() {
        assert!(!is_transient(&io_err(std::io::ErrorKind::NotFound)));
        assert!(!is_transient(&io_err(std::io::ErrorKind::IsADirectory)));
        assert!(is_transient(&io_err(std::io::ErrorKind::PermissionDenied)));
        assert!(is_transient(&io_err(std::io::ErrorKind::TimedOut)));
    }

    #[test]
    fn ignored_path_marker_is_permanent_even_with_context() {
        assert!(!is_transient(&(IgnoredPath.into())));

        let wrapped = anyhow::Error::from(IgnoredPath).context("hash_and_diff");
        assert!(!is_transient(&wrapped));
    }

    #[test]
    fn unrecognized_errors_default_to_permanent() {
        // A bare bail! (string error) has no recognizable cause — retrying
        // a logic bug would only burn workers, so it must not retry.
        assert!(!is_transient(&anyhow::anyhow!("plugin panicked")));
        let err: anyhow::Error = std::fmt::Error.into();
        assert!(!is_transient(&err));
    }

    #[tokio::test]
    async fn flaky_job_recovers_on_third_attempt() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let outcome = run_with_retries(
            move || {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                async move {
                    if n < 3 {
                        Err(anyhow::Error::new(std::io::Error::other("pool flake")))
                    } else {
                        Ok(())
                    }
                }
            },
            |_| 0, // injected schedule: tests never sleep
        )
        .await;

        assert!(matches!(outcome, RetryOutcome::Succeeded { attempts: 3 }));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn permanent_failure_is_not_retried() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let outcome = run_with_retries(
            move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Err(anyhow::Error::new(IgnoredPath)) }
            },
            |_| 0,
        )
        .await;

        match outcome {
            RetryOutcome::Exhausted {
                attempts,
                last_error,
            } => {
                assert_eq!(attempts, 1, "permanent failures must not be retried");
                assert!(last_error.downcast_ref::<IgnoredPath>().is_some());
            }
            RetryOutcome::Succeeded { .. } => panic!("expected exhaustion"),
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn persistent_transient_failure_exhausts_after_max_attempts() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let outcome = run_with_retries(
            move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Err(anyhow::Error::new(std::io::Error::other("db down"))) }
            },
            |_| 0,
        )
        .await;

        match outcome {
            RetryOutcome::Exhausted {
                attempts,
                last_error,
            } => {
                assert_eq!(attempts, MAX_JOB_ATTEMPTS);
                assert!(is_transient(&last_error));
            }
            RetryOutcome::Succeeded { .. } => panic!("expected exhaustion"),
        }
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            MAX_JOB_ATTEMPTS as usize
        );
    }

    #[tokio::test]
    async fn backoff_receives_one_based_failed_attempt_numbers() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let outcome = run_with_retries(
            || async { Err::<(), anyhow::Error>(std::io::Error::other("x").into()) },
            move |failed_attempt| {
                recorder.lock().unwrap().push(failed_attempt);
                0
            },
        )
        .await;

        assert!(matches!(
            outcome,
            RetryOutcome::Exhausted {
                attempts: MAX_JOB_ATTEMPTS,
                ..
            }
        ));
        assert_eq!(*seen.lock().unwrap(), vec![1, 2]);
    }

    #[test]
    fn dead_letter_handles_both_job_variants() {
        // tracing macros are no-ops without a subscriber; this exercises
        // the field-extraction match arms end to end.
        dead_letter(
            &Job::ProcessFile {
                storage_id: 1,
                path: "photos/a.jpg".into(),
                mode: ProcessMode::Auto,
            },
            MAX_JOB_ATTEMPTS,
            &anyhow::anyhow!("boom"),
        );
        dead_letter(
            &Job::CreateThumbnail {
                hash: vec![0xde, 0xad],
                regenerate: false,
            },
            MAX_JOB_ATTEMPTS,
            &anyhow::anyhow!("boom"),
        );
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
