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
    #[serde(default = "defaults::frontend_dist")]
    pub frontend_dist: String,
    #[serde(default = "defaults::thumbnail_storage")]
    pub thumbnail_storage: String,
    #[serde(default = "defaults::base_url")]
    pub base_url: String,
    #[serde(default = "defaults::token_expiration_days")]
    pub token_expiration_days: i64,
    #[serde(default = "defaults::token_length")]
    pub token_length: usize,
}

mod defaults {
    pub fn server_addr() -> String {
        "0.0.0.0:3000".to_string()
    }
    pub fn frontend_dist() -> String {
        "frontend/dist".to_string()
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
