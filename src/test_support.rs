//! Test-only support shared by DB-backed unit tests across modules.
//!
//! `cargo test --lib` runs every unit test in one process. Other modules
//! (e.g. `auth::tests`) initialize the process-wide `Config` singleton with an
//! unusable `database_url`, so DB tests must build their own local `Config`,
//! share a single `tokio::Runtime`, and migrate once.
//!
//! Pattern (see `src/job/classify.rs`):
//!   ```ignore
//!   test_support::runtime().block_on(async {
//!       let db = test_support::test_db().await;
//!       // ...
//!   });
//!   ```

use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;
use std::sync::OnceLock;
use tokio::sync::OnceCell;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();

/// The shared process-lifetime test runtime. A per-test `#[tokio::test]`
/// runtime would be dropped mid-test and hang the connection pool's background
/// tasks.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build shared test runtime"))
}

/// A shared, migrated test database connection. Connects with a locally-built
/// `Config` (not the process-wide singleton, which other modules may have
/// poisoned with a dead `database_url`).
pub async fn test_db() -> &'static DatabaseConnection {
    DB.get_or_init(|| async {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://user:password@localhost:15432/byteburrow_test".to_string()
        });
        let config = crate::config::Config {
            database_url,
            salt: "test-support-salt".to_string(),
            server_addr: "0.0.0.0:3000".to_string(),
            thumbnail_storage: "/tmp/thumbnails".to_string(),
            base_url: "http://localhost:3000".to_string(),
            token_expiration_days: 30,
            token_length: 32,
            plugin_dir: "/tmp".to_string(),
            ignore_patterns: vec![],
            cors_allowed_origins: String::new(),
            trust_forwarded_headers: false,
            face_match_threshold: 0.8,
            face_match_margin: 0.05,
            plugin: std::collections::HashMap::new(),
        };

        let db = crate::db_connect(&config)
            .await
            .expect("connect to test database");
        crate::migration::Migrator::up(&db, None)
            .await
            .expect("run migrations");
        db
    })
    .await
}
