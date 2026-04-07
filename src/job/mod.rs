use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use image::imageops::FilterType;
use image::GenericImageView;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, instrument, warn};

use crate::config::Config;
use crate::entity::{entry, face_reference, meta, photo};
use crate::plugin::PluginRegistry;
use crate::storage::{determine_content_type, thumbnail, Storage};

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
    CreateThumbnail {
        hash: Vec<u8>,
        regenerate: bool,
    },
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
            info!("Job runner started (dedicated runtime, nice {})", JOB_THREAD_NICE);
            while let Some(job) = self.rx.recv().await {
                let permit = self.semaphore.clone().acquire_owned().await.unwrap();
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
            Job::ProcessFile { storage_id, path, mode } => {
                Self::process_file(db, plugins, storage_id, &path, mode).await?;
            }

            Job::CreateThumbnail { ref hash, regenerate } => {
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
        let storage = Storage::find_by_id(db, storage_id).await?;

        // Filter excluded paths using per-storage ignore patterns
        let patterns = crate::ignore::parse_patterns(&storage.model.ignore_patterns);
        if crate::ignore::is_ignored(path, &patterns) {
            return Ok(());
        }
        let (updated, hash, entry) = storage.calculate_hash(db, path).await?;

        // In Auto mode, skip if nothing changed
        if !updated && matches!(mode, ProcessMode::Auto) {
            return Ok(());
        }

        let hash_hex = hex::encode(&hash);
        let full_path = storage.get_full_path(&entry.path);

        // Decide whether to run plugins
        let run_plugins = match mode {
            ProcessMode::HashOnly => false,
            ProcessMode::Auto => !entry.skip_plugins,
            ProcessMode::ForceClassify => true,
        };

        if run_plugins {
            Self::run_classification(db, plugins, &entry, &hash, &full_path).await?;
        }

        // Generate thumbnails for images (always, not gated by plugin skip)
        if is_image_file(&entry.path) {
            Self::generate_thumbnails(&full_path, &hash_hex).await?;
        }

        Ok(())
    }

    async fn run_classification(
        db: &DatabaseConnection,
        plugins: &PluginRegistry,
        entry: &entry::Model,
        hash_bytes: &[u8],
        full_path: &Path,
    ) -> anyhow::Result<()> {
        // Read file data and determine MIME type
        let data = std::fs::read(full_path).unwrap_or_default();
        let mime_type = determine_content_type(full_path, &data);

        info!(path = &entry.path, mime = mime_type, "Classifying file");

        if !plugins.is_empty() {
            let existing_custom: HashMap<String, serde_json::Value> = HashMap::new();
            let ctx = byteburrow_plugin_api::FileContext {
                path: &entry.path,
                full_path,
                data: &data,
                mime_type,
                size: entry.size as u64,
                custom: &existing_custom,
            };

            let mut merged = plugins.classify_file(&ctx);

            // Process face embeddings: store in face_reference, match against contacts
            Self::process_face_embeddings(db, hash_bytes, &mut merged).await?;

            // Upsert meta record with keywords + custom
            if !merged.keywords.is_empty() || !merged.custom.is_empty() {
                let existing_meta = meta::Entity::find_by_id(hash_bytes.to_vec())
                    .one(db)
                    .await?;

                match existing_meta {
                    Some(m) => {
                        let mut kw = m.keywords.clone();
                        kw.extend(merged.keywords);
                        kw.sort();
                        kw.dedup();

                        let custom = match m.custom {
                            serde_json::Value::Object(mut map) => {
                                map.extend(merged.custom);
                                serde_json::Value::Object(map)
                            }
                            _ => {
                                if merged.custom.is_empty() {
                                    m.custom
                                } else {
                                    serde_json::Value::Object(merged.custom)
                                }
                            }
                        };

                        let active = meta::ActiveModel {
                            hash: Set(hash_bytes.to_vec()),
                            keywords: Set(kw),
                            custom: Set(custom),
                            ..Default::default()
                        };
                        active.update(db).await?;
                    }
                    None => {
                        let active = meta::ActiveModel {
                            hash: Set(hash_bytes.to_vec()),
                            tags: Set(vec![]),
                            keywords: Set(merged.keywords),
                            custom: Set(if merged.custom.is_empty() {
                                serde_json::Value::Object(serde_json::Map::new())
                            } else {
                                serde_json::Value::Object(merged.custom)
                            }),
                        };
                        active.insert(db).await?;
                    }
                }
            }

            // Upsert photo record if geo/date data was produced
            if merged.latitude.is_some()
                || merged.longitude.is_some()
                || merged.date_unix.is_some()
            {
                let date = merged.date_unix.and_then(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.naive_utc())
                });

                let existing = photo::Entity::find_by_id(hash_bytes.to_vec())
                    .one(db)
                    .await?;

                if existing.is_some() {
                    let active = photo::ActiveModel {
                        hash: Set(hash_bytes.to_vec()),
                        latitude: Set(merged.latitude),
                        longitude: Set(merged.longitude),
                        date: Set(date),
                        ..Default::default()
                    };
                    active.update(db).await?;
                } else {
                    let active = photo::ActiveModel {
                        hash: Set(hash_bytes.to_vec()),
                        latitude: Set(merged.latitude),
                        longitude: Set(merged.longitude),
                        date: Set(date),
                        keywords: Set(vec![]),
                    };
                    active.insert(db).await?;
                }
            }
        } else {
            // Fallback: inline EXIF extraction when no plugins loaded
            if is_image_file(&entry.path) {
                info!(path = &entry.path, "Processing image (inline, no plugins)");
                let (latitude, longitude, date) = extract_exif(full_path);

                let existing = photo::Entity::find_by_id(hash_bytes.to_vec())
                    .one(db)
                    .await?;

                if existing.is_some() {
                    let active = photo::ActiveModel {
                        hash: Set(hash_bytes.to_vec()),
                        latitude: Set(latitude),
                        longitude: Set(longitude),
                        date: Set(date),
                        ..Default::default()
                    };
                    active.update(db).await?;
                } else {
                    let active = photo::ActiveModel {
                        hash: Set(hash_bytes.to_vec()),
                        latitude: Set(latitude),
                        longitude: Set(longitude),
                        date: Set(date),
                        keywords: Set(vec![]),
                    };
                    active.insert(db).await?;
                }
            }
        }

        Ok(())
    }

    async fn process_face_embeddings(
        db: &DatabaseConnection,
        hash_bytes: &[u8],
        merged: &mut crate::plugin::MergedClassification,
    ) -> anyhow::Result<()> {
        // Extract raw embeddings (temporary key, not persisted to meta)
        let raw_embeddings = match merged.custom.remove("face_embeddings_raw") {
            Some(v) => v,
            None => return Ok(()),
        };

        let raw_arr = match raw_embeddings.as_array() {
            Some(a) => a.clone(),
            None => return Ok(()),
        };

        // Get face bounding boxes from the faces plugin output
        let faces_data = merged.custom.get("faces").cloned();
        let rects = faces_data
            .as_ref()
            .and_then(|f| f.get("rects"))
            .and_then(|r| r.as_array());

        // Load all confirmed face references for matching
        let confirmed_refs = face_reference::Entity::find()
            .filter(face_reference::Column::Confirmed.eq(true))
            .all(db)
            .await?;

        let face_count = rects.map(|r| r.len()).unwrap_or(0);
        let mut contact_matches: Vec<serde_json::Value> =
            vec![serde_json::Value::Null; face_count];

        for entry in &raw_arr {
            let face_index = entry
                .get("face_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let embedding_floats: Vec<f32> = match entry.get("embedding").and_then(|v| v.as_array())
            {
                Some(arr) => arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect(),
                None => continue,
            };

            if embedding_floats.is_empty() {
                continue;
            }

            let embedding_bytes = floats_to_bytes(&embedding_floats);

            // Get bbox from faces data
            let (bbox_x, bbox_y, bbox_w, bbox_h) = if let Some(rect) =
                rects.and_then(|r| r.get(face_index))
            {
                (
                    rect.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    rect.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                )
            } else {
                (0, 0, 0, 0)
            };

            // Upsert face_reference row
            let existing = face_reference::Entity::find()
                .filter(face_reference::Column::Hash.eq(hash_bytes.to_vec()))
                .filter(face_reference::Column::FaceIndex.eq(face_index as i16))
                .one(db)
                .await?;

            match existing {
                Some(existing_ref) => {
                    let active = face_reference::ActiveModel {
                        id: Set(existing_ref.id),
                        embedding: Set(embedding_bytes),
                        bbox_x: Set(bbox_x),
                        bbox_y: Set(bbox_y),
                        bbox_w: Set(bbox_w),
                        bbox_h: Set(bbox_h),
                        ..Default::default()
                    };
                    active.update(db).await?;
                }
                None => {
                    let active = face_reference::ActiveModel {
                        hash: Set(hash_bytes.to_vec()),
                        face_index: Set(face_index as i16),
                        bbox_x: Set(bbox_x),
                        bbox_y: Set(bbox_y),
                        bbox_w: Set(bbox_w),
                        bbox_h: Set(bbox_h),
                        embedding: Set(embedding_bytes),
                        confirmed: Set(false),
                        ..Default::default()
                    };
                    active.insert(db).await?;
                }
            }

            // Match against confirmed references
            let mut best_contact_id: Option<i32> = None;
            let mut best_similarity: f32 = 0.0;

            for reference in &confirmed_refs {
                if let Some(contact_id) = reference.contact_id {
                    let ref_floats = bytes_to_floats(&reference.embedding);
                    let sim = cosine_similarity(&embedding_floats, &ref_floats);
                    if sim > best_similarity {
                        best_similarity = sim;
                        best_contact_id = Some(contact_id);
                    }
                }
            }

            if best_similarity > 0.8 {
                if let Some(contact_id) = best_contact_id {
                    if face_index < contact_matches.len() {
                        contact_matches[face_index] =
                            serde_json::Value::Number(contact_id.into());
                    }
                    info!(
                        face_index,
                        contact_id,
                        similarity = best_similarity,
                        "Face matched to contact"
                    );
                }
            }
        }

        // Store the contact match array in meta.custom
        merged.custom.insert(
            "face_embeddings".to_string(),
            serde_json::Value::Array(contact_matches),
        );

        Ok(())
    }

    async fn generate_thumbnails(full_path: &Path, hash_hex: &str) -> anyhow::Result<()> {
        let config = Config::get();
        let thumbnail_dir = std::path::PathBuf::from(&config.thumbnail_storage);

        for (size_name, max_dim) in [("mini", 64u32), ("small", 256u32), ("large", 1024u32)] {
            let thumb_path =
                thumbnail::get_thumbnail_path(&thumbnail_dir, hash_hex, size_name);

            if thumb_path.exists() {
                continue;
            }

            thumbnail::ensure_thumbnail_dir(&thumb_path).await?;

            let full_path = full_path.to_path_buf();
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
                Ok(()) => info!(size = size_name, "Thumbnail generated"),
                Err(e) => warn!(size = size_name, error = %e, "Failed to generate thumbnail"),
            }
        }

        Ok(())
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

        Self::generate_thumbnails(&full_path, &hash_hex).await
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
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

fn rational_to_f64(r: &exif::Rational) -> f64 {
    r.num as f64 / r.denom as f64
}

fn dms_to_decimal(dms: &[exif::Rational], reference: &str) -> Option<f64> {
    if dms.len() < 3 {
        return None;
    }
    let deg = rational_to_f64(&dms[0]);
    let min = rational_to_f64(&dms[1]);
    let sec = rational_to_f64(&dms[2]);
    let decimal = deg + min / 60.0 + sec / 3600.0;
    Some(if reference == "S" || reference == "W" {
        -decimal
    } else {
        decimal
    })
}

fn extract_exif(full_path: &Path) -> (Option<f64>, Option<f64>, Option<chrono::NaiveDateTime>) {
    let mut latitude = None;
    let mut longitude = None;
    let mut date = None;

    let file = match std::fs::File::open(full_path) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "Failed to open file for EXIF");
            return (latitude, longitude, date);
        }
    };

    let exif_data = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(data) => data,
        Err(e) => {
            warn!(error = %e, "Failed to parse EXIF data");
            return (latitude, longitude, date);
        }
    };

    // Extract GPS
    let lat_field = exif_data.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY);
    let lat_ref = exif_data.get_field(exif::Tag::GPSLatitudeRef, exif::In::PRIMARY);
    let lon_field = exif_data.get_field(exif::Tag::GPSLongitude, exif::In::PRIMARY);
    let lon_ref = exif_data.get_field(exif::Tag::GPSLongitudeRef, exif::In::PRIMARY);

    if let (Some(lat_f), Some(lat_r), Some(lon_f), Some(lon_r)) =
        (lat_field, lat_ref, lon_field, lon_ref)
    {
        if let (exif::Value::Rational(ref lat_dms), exif::Value::Rational(ref lon_dms)) =
            (&lat_f.value, &lon_f.value)
        {
            let lat_ref_str = lat_r.display_value().to_string();
            let lon_ref_str = lon_r.display_value().to_string();
            latitude = dms_to_decimal(lat_dms, &lat_ref_str);
            longitude = dms_to_decimal(lon_dms, &lon_ref_str);
        }
    }

    // Extract date
    if let Some(dt_field) = exif_data.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY) {
        if let exif::Value::Ascii(ref vec) = dt_field.value {
            if let Some(bytes) = vec.first() {
                if let Ok(dt) = exif::DateTime::from_ascii(bytes) {
                    date = chrono::NaiveDate::from_ymd_opt(
                        dt.year.into(),
                        dt.month.into(),
                        dt.day.into(),
                    )
                    .and_then(|d| {
                        d.and_hms_opt(dt.hour.into(), dt.minute.into(), dt.second.into())
                    });
                }
            }
        }
    }

    (latitude, longitude, date)
}
