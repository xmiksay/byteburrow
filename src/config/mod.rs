use std::env;
use std::sync::{Arc, OnceLock};

static INSTANCE: OnceLock<Arc<Config>> = OnceLock::new();

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub server_addr: String,
    pub frontend_dist: String,
    pub salt: String,
    pub thumbnail_storage: String,
    pub token_expiration_days: i64,
    pub token_length: usize,
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
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL must be set in .env file or environment");
        let server_addr = env::var("SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let frontend_dist =
            env::var("FRONTEND_DIST").unwrap_or_else(|_| "frontend/dist".to_string());
        let salt = env::var("SALT").expect("SALT must be set in .env file or environment");
        let thumbnail_storage =
            env::var("THUMBNAIL_STORAGE").unwrap_or_else(|_| "/tmp/thumbnails".to_string());
        let token_expiration_days = env::var("TOKEN_EXPIRATION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let token_length = env::var("TOKEN_LENGTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);

        Self {
            database_url,
            server_addr,
            frontend_dist,
            salt,
            thumbnail_storage,
            token_expiration_days,
            token_length,
        }
    }
}
