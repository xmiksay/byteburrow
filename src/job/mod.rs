use std::sync::Arc;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tracing::{info, error, instrument};

use crate::storage::Storage;

#[derive(Debug)]
pub enum Job {
    CalculateHash { storage_id: i32, path: String },
}

pub type JobSender = mpsc::UnboundedSender<Job>;

pub struct JobRunner {
    rx: mpsc::UnboundedReceiver<Job>,
    db: Arc<DatabaseConnection>,
}

impl JobRunner {
    pub fn new(db: DatabaseConnection) -> (Self, JobSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { rx, db: Arc::new(db) }, tx)
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
            Job::CalculateHash { storage_id, path } => {
                let storage = Storage::find_by_id(&self.db, storage_id).await?;
                let (updated, hash) = storage.calculate_hash(self.db.as_ref(), &path).await?;
                if updated {
                    info!(path = &path, hash = hex::encode(&hash), "Hash updated");
                }
            }
        }

        Ok(())
    }
}
