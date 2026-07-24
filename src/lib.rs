use sea_orm::{Database, DatabaseConnection};

pub mod auth;
pub mod config;
pub mod entity;
pub mod face_match;
pub mod ignore;
pub mod inotify;
pub mod job;
pub mod migration;
pub mod plugin;
pub mod storage;
pub mod web;

#[cfg(test)]
pub mod test_support;

pub async fn db_connect(config: &config::Config) -> Result<DatabaseConnection, sea_orm::DbErr> {
    Database::connect(&config.database_url).await
}
