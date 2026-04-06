use sea_orm::{Database, DatabaseConnection};

pub mod auth;
pub mod config;
pub mod entity;
pub mod ignore;
pub mod inotify;
pub mod job;
pub mod migration;
pub mod plugin;
pub mod storage;
pub mod web;

// Plugins
#[cfg(feature = "plugin-contactlist")]
pub mod plugins {
    pub mod contactlist;
}

pub async fn db_connect(config: &config::Config) -> Result<DatabaseConnection, sea_orm::DbErr> {
    Database::connect(&config.database_url).await
}
