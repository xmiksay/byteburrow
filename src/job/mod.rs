use std::sync::Arc;
use sea_orm::{DatabaseConnection, EntityTrait, ColumnTrait, QueryFilter, ActiveModelTrait, Set};
use sha2::{Sha256, Digest};
use tokio::sync::mpsc;
use tracing::{info, error, instrument};

use crate::entity::entry;
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

    async fn process(&self, job: Job) -> anyhow::Result<()> {
        match job {
            Job::CalculateHash { storage_id, path } => {
                self.calculate_hash(storage_id, &path).await
            }
        }
    }

    #[instrument(skip(self))]
    async fn calculate_hash(&self, storage_id: i32, path: &str) -> anyhow::Result<()> {
        let storage = Storage::find_by_id(&self.db, storage_id).await?;
        let full_path = storage.get_full_path(path);

        let data = tokio::fs::read(&full_path).await?;
        let hash = Sha256::digest(&data).to_vec();

        let normalized_path = path.trim_matches('/').to_string();

        let record = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(storage_id))
            .filter(entry::Column::Path.eq(&normalized_path))
            .one(self.db.as_ref())
            .await?;

        match record {
            Some(model) => {
                let mut active: entry::ActiveModel = model.into();
                active.hash = Set(Some(hash.clone()));
                active.update(self.db.as_ref()).await?;
                info!(path, hash = hex::encode(&hash), "Hash updated");
            }
            None => {
                info!(path, "Entry not in DB, skipping hash update");
            }
        }

        Ok(())
    }
}
