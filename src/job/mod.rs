use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, instrument};

use crate::storage::Storage;

#[derive(Debug)]
pub enum Job {
    CheckFile { storage_id: i32, path: String },
    ChangedHash(String),
}

pub type JobSender = mpsc::UnboundedSender<Job>;

pub struct JobRunner {
    rx: mpsc::UnboundedReceiver<Job>,
    db: Arc<DatabaseConnection>,
}

impl JobRunner {
    pub fn new(db: DatabaseConnection) -> (Self, JobSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                rx,
                db: Arc::new(db),
            },
            tx,
        )
    }

    pub async fn run(mut self) {
        info!("Job runner started");
        while let Some(job) = self.rx.recv().await {
            info!(?job, "Processing job");
            if let Err(e) = self.process(job).await {
                error!("Job failed: {e}");
            }
        }
        info!("Job runner stopped");
    }

    #[instrument(skip(self))]
    async fn process(&self, job: Job) -> anyhow::Result<()> {
        match job {
            Job::CheckFile { storage_id, path } => {
                // TODO: make this configurable
                if !path.contains(".git")
                    && !path.contains(".cache")
                    && !path.contains("node_nodules")
                {
                    let storage = Storage::find_by_id(&self.db, storage_id).await?;
                    // Path must be file
                    let (updated, hash) = storage.calculate_hash(self.db.as_ref(), &path).await?;
                    if updated {
                        info!(path = &path, hash = hex::encode(&hash), "Hash updated");
                    }
                }
            }

            Job::ChangedHash(hash) => {
                // Create thumbnail if image

                // Detect faces if images
                // Recognise person if faces detected
            }
        }

        Ok(())
    }
}
