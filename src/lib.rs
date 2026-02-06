use sea_orm::{Database, DatabaseConnection};
use std::env;

pub mod entity;
pub mod migration;
pub mod web;

// Plugins
#[cfg(feature = "plugin-contactlist")]
pub mod plugins {
    pub mod contactlist;
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub server_addr: String,
    pub frontend_dist: String,
    pub salt: String,
    pub thumbnail_storage: String,
}

impl Config {
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

        Self {
            database_url,
            server_addr,
            frontend_dist,
            salt,
            thumbnail_storage,
        }
    }
}

pub async fn db_connect(config: &Config) -> Result<DatabaseConnection, sea_orm::DbErr> {
    Database::connect(&config.database_url).await
}
