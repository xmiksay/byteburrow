use config::Environment;
use serde::Deserialize;
use std::sync::{Arc, OnceLock};

static INSTANCE: OnceLock<Arc<Config>> = OnceLock::new();

#[derive(Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    pub salt: String,
    #[serde(default = "defaults::server_addr")]
    pub server_addr: String,
    #[serde(default = "defaults::thumbnail_storage")]
    pub thumbnail_storage: String,
    #[serde(default = "defaults::base_url")]
    pub base_url: String,
    #[serde(default = "defaults::token_expiration_days")]
    pub token_expiration_days: i64,
    #[serde(default = "defaults::token_length")]
    pub token_length: usize,
    #[serde(default = "defaults::plugin_dir")]
    pub plugin_dir: String,
    #[serde(default = "defaults::ignore_patterns")]
    pub ignore_patterns: Vec<String>,
    /// Comma-separated list of origins allowed to make cross-origin requests
    /// (CORS). Empty by default — same-origin requests are never subject to
    /// CORS, so this only needs to be set when the frontend is served from a
    /// different origin than the API.
    #[serde(default)]
    pub cors_allowed_origins: String,
    /// Whether to trust `X-Forwarded-For` / `X-Real-IP` headers for the
    /// client IP recorded on session tokens. Only enable this when the
    /// server sits behind a reverse proxy that sets these headers itself —
    /// otherwise any client can spoof them. Defaults to `false`, falling
    /// back to the real TCP peer address.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
    /// Minimum cosine similarity for a face to be matched to a known contact.
    /// The single "is this a known person" threshold shared by the job pipeline
    /// and the CLI `face_match` tool (see `crate::face_match`).
    #[serde(default = "defaults::face_match_threshold")]
    pub face_match_threshold: f32,
    /// Minimum gap between the best contact's similarity and the best
    /// *different* contact's similarity. Rejects ambiguous matches where two
    /// people are almost equally close. Set to `0.0` to disable the guard.
    #[serde(default = "defaults::face_match_margin")]
    pub face_match_margin: f32,
}

mod defaults {
    pub fn server_addr() -> String {
        "0.0.0.0:3000".to_string()
    }
    pub fn thumbnail_storage() -> String {
        "/tmp/thumbnails".to_string()
    }
    pub fn base_url() -> String {
        "http://localhost:3000".to_string()
    }
    pub fn token_expiration_days() -> i64 {
        30
    }
    pub fn token_length() -> usize {
        32
    }
    pub fn plugin_dir() -> String {
        "/etc/byteburrow/plugins".to_string()
    }
    pub fn face_match_threshold() -> f32 {
        0.8
    }
    pub fn face_match_margin() -> f32 {
        0.05
    }
    pub fn ignore_patterns() -> Vec<String> {
        vec![
            ".git".to_string(),
            ".cache".to_string(),
            "node_modules".to_string(),
            ".DS_Store".to_string(),
            "__pycache__".to_string(),
            ".Trash".to_string(),
        ]
    }
}

impl Config {
    pub fn get() -> Arc<Config> {
        INSTANCE.get().expect("Config not initialized").clone()
    }

    pub fn set(config: Arc<Config>) {
        INSTANCE
            .set(config)
            .unwrap_or_else(|_| panic!("Config already initialized"));
    }

    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        config::Config::builder()
            .add_source(
                Environment::with_prefix("BYTEBURROW")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .expect("Failed to build configuration")
            .try_deserialize()
            .expect("Failed to deserialize configuration")
    }
}
