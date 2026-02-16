use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use image::imageops::FilterType;
use image::GenericImageView;
use nom_exif::{ExifIter, ExifTag};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};

use crate::config::Config;
use crate::entity::{entry, photo};
use crate::storage::{thumbnail, Storage};

#[derive(Debug)]
pub enum Job {
    CheckFile { storage_id: i32, path: String },
    ChangedHash(Vec<u8>),
}

pub type JobSender = mpsc::UnboundedSender<Job>;

pub struct JobRunner {
    rx: mpsc::UnboundedReceiver<Job>,
    db: Arc<DatabaseConnection>,
    tx: JobSender,
}

impl JobRunner {
    pub fn new(db: DatabaseConnection) -> (Self, JobSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                rx,
                db: Arc::new(db),
                tx: tx.clone(),
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
                    let (updated, hash) = storage.calculate_hash(self.db.as_ref(), &path).await?;
                    if updated {
                        info!(path = &path, hash = hex::encode(&hash), "Hash updated");
                        let _ = self.tx.send(Job::ChangedHash(hash));
                    }
                }
            }

            Job::ChangedHash(ref hash_bytes) => {
                self.process_changed_hash(hash_bytes).await?;
            }
        }

        Ok(())
    }

    async fn process_changed_hash(&self, hash_bytes: &[u8]) -> anyhow::Result<()> {
        let hash_hex = hex::encode(hash_bytes);

        // Find entry with this hash
        let entry = entry::Entity::find()
            .filter(entry::Column::Hash.eq(hash_bytes.to_vec()))
            .one(self.db.as_ref())
            .await?;

        let entry = match entry {
            Some(e) => e,
            None => {
                warn!(hash = %hash_hex, "No entry found for hash");
                return Ok(());
            }
        };

        // Check if it's an image by extension
        if !is_image_file(&entry.path) {
            return Ok(());
        }

        // Resolve full file path
        let storage = Storage::find_by_id(&self.db, entry.storage_id).await?;
        let full_path = storage.get_full_path(&entry.path);

        info!(path = &entry.path, "Processing image for photo library");

        // Extract EXIF metadata
        let (latitude, longitude, date) = extract_exif(&full_path);

        // Upsert photo record
        let existing = photo::Entity::find_by_id(hash_bytes.to_vec())
            .one(self.db.as_ref())
            .await?;

        if existing.is_some() {
            let active = photo::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                latitude: Set(latitude),
                longitude: Set(longitude),
                date: Set(date),
                ..Default::default()
            };
            active.update(self.db.as_ref()).await?;
        } else {
            let active = photo::ActiveModel {
                hash: Set(hash_bytes.to_vec()),
                latitude: Set(latitude),
                longitude: Set(longitude),
                date: Set(date),
                keywords: Set(vec![]),
            };
            active.insert(self.db.as_ref()).await?;
        }

        info!(
            path = &entry.path,
            lat = ?latitude,
            lon = ?longitude,
            date = ?date,
            "Photo record saved"
        );

        // Generate thumbnails
        let config = Config::get();
        let thumbnail_dir = std::path::PathBuf::from(&config.thumbnail_storage);

        for (size_name, max_dim) in [("mini", 64u32), ("small", 256u32), ("large", 1024u32)] {
            let thumb_path = thumbnail::get_thumbnail_path(&thumbnail_dir, &hash_hex, size_name);

            if thumb_path.exists() {
                continue;
            }

            thumbnail::ensure_thumbnail_dir(&thumb_path).await?;

            let full_path = full_path.clone();
            let thumb_path_clone = thumb_path.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let img = image::open(&full_path)?;
                let (w, h) = img.dimensions();
                if w <= max_dim && h <= max_dim {
                    img.save(&thumb_path_clone)?;
                } else {
                    let thumb = img.resize(max_dim, max_dim, FilterType::Lanczos3);
                    thumb.save(&thumb_path_clone)?;
                }
                Ok(())
            })
            .await?;

            match result {
                Ok(()) => {
                    info!(size = size_name, "Thumbnail generated");
                }
                Err(e) => {
                    warn!(size = size_name, error = %e, "Failed to generate thumbnail");
                }
            }
        }

        Ok(())
    }
}

fn is_image_file(path: &str) -> bool {
    let path = Path::new(path);
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "heif" | "avif"
        ),
        None => false,
    }
}

fn latlng_to_decimal(latlng: &nom_exif::LatLng, reference: char) -> f64 {
    let deg = latlng.0.as_float();
    let min = latlng.1.as_float();
    let sec = latlng.2.as_float();
    let decimal = deg + min / 60.0 + sec / 3600.0;
    if reference == 'S' || reference == 'W' {
        -decimal
    } else {
        decimal
    }
}

fn extract_exif(full_path: &Path) -> (Option<f64>, Option<f64>, Option<chrono::NaiveDateTime>) {
    let mut latitude = None;
    let mut longitude = None;
    let mut date = None;

    let mut file = match std::fs::File::open(full_path) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Failed to open file for EXIF");
            return (latitude, longitude, date);
        }
    };

    let mut buf = Vec::new();
    if let Err(e) = file.read_to_end(&mut buf) {
        warn!(error = %e, "Failed to read file for EXIF");
        return (latitude, longitude, date);
    }

    let iter: Option<ExifIter> = match nom_exif::parse_exif(&buf[..], None) {
        Ok(iter) => iter,
        Err(e) => {
            warn!(error = %e, "Failed to parse EXIF data");
            return (latitude, longitude, date);
        }
    };

    let iter = match iter {
        Some(iter) => iter,
        None => return (latitude, longitude, date),
    };

    // Extract GPS
    if let Ok(Some(gps)) = iter.parse_gps_info() {
        latitude = Some(latlng_to_decimal(&gps.latitude, gps.latitude_ref));
        longitude = Some(latlng_to_decimal(&gps.longitude, gps.longitude_ref));
    }

    // Extract date
    let exif: nom_exif::Exif = iter.into();
    if let Some(val) = exif.get(ExifTag::DateTimeOriginal) {
        if let Some(dt) = val.as_time() {
            date = Some(dt.naive_utc());
        }
    }

    (latitude, longitude, date)
}
