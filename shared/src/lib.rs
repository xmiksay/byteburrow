use sea_orm::{Database, DatabaseConnection};
use std::env;

pub mod entities;
pub mod migrations;

pub use entities::dummy;

pub struct Config {
    pub database_url: String,
    pub server_addr: String,
    pub frontend_dist: String,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        let database_url = env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set in .env file or environment");
        let server_addr = env::var("SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let frontend_dist = env::var("FRONTEND_DIST").unwrap_or_else(|_| "frontend/dist".to_string());

        Self {
            database_url,
            server_addr,
            frontend_dist,
        }
    }
}

pub async fn db_connect(config: &Config) -> Result<DatabaseConnection, sea_orm::DbErr> {
    Database::connect(&config.database_url).await
}
