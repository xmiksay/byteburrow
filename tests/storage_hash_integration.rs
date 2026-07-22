//! Integration tests for `Storage::calculate_hash` — specifically that a
//! not-yet-hashed entry (nullable `hash` column is `NULL`) is handled without
//! panicking (regression for issue #4: `unwrap` on a nullable hash column).
//!
//! Shares the process-lifetime runtime/DB setup pattern with the other
//! `*_integration.rs` tests (see `storage_access_integration.rs`).

use byteburrow::config::Config;
use byteburrow::entity::{entry, group, storage, user};
use byteburrow::migration::Migrator;
use byteburrow::storage::Storage;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sea_orm_migration::MigratorTrait;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::OnceCell;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();
static COUNTER: AtomicU32 = AtomicU32::new(0);

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build shared test runtime"))
}

async fn test_db() -> &'static DatabaseConnection {
    DB.get_or_init(|| async {
        static CONFIG_INIT: Once = Once::new();
        CONFIG_INIT.call_once(|| {
            let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://user:password@localhost:15432/byteburrow_test".to_string()
            });
            Config::set(Arc::new(Config {
                database_url,
                salt: "integration-test-salt".to_string(),
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
            }));
        });

        let db = byteburrow::db_connect(&Config::get())
            .await
            .expect("connect to test database");
        Migrator::up(&db, None).await.expect("run migrations");
        db
    })
    .await
}

/// Unique suffix so parallel tests don't collide on unique columns.
fn uniq() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn make_user(db: &DatabaseConnection) -> user::Model {
    let name = format!("hash_user_{}", uniq());
    user::ActiveModel {
        name: Set(name.clone()),
        description: Set(None),
        username: Set(name),
        password: Set("x".to_string()),
        enabled: Set(true),
        admin: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert user")
}

async fn make_group(db: &DatabaseConnection) -> group::Model {
    group::ActiveModel {
        name: Set(format!("hash_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

/// A `Storage` rooted at a fresh temp directory holding one file `data.txt`.
async fn make_storage(db: &DatabaseConnection) -> (Storage, std::path::PathBuf) {
    let owner = make_user(db).await;
    let grp = make_group(db).await;

    let root = std::env::temp_dir().join(format!(
        "byteburrow_hash_test_{}_{}",
        std::process::id(),
        uniq()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("data.txt"), b"hello byteburrow")
        .await
        .unwrap();

    let model = storage::ActiveModel {
        name: Set(format!("hash_storage_{}", uniq())),
        description: Set(None),
        path: Set(root.to_string_lossy().into_owned()),
        default_user: Set(owner.id),
        default_group: Set(grp.id),
        ignore_patterns: Set(String::new()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert storage");

    (Storage::new(model), root)
}

/// First call computes the hash for a fresh (NULL-hash) entry; the second call
/// hits the "hash already present, file unchanged" branch and returns the
/// stored hash — the path that previously did `model.hash.clone().unwrap()`.
#[test]
fn calculate_hash_handles_null_then_cached() {
    runtime().block_on(async {
        let db = test_db().await;
        let (storage, _root) = make_storage(db).await;

        let expected = Sha256::digest(b"hello byteburrow").to_vec();

        // Not-yet-hashed row: hash column is NULL.
        let (updated, hash, entry) = storage
            .calculate_hash(db, "data.txt")
            .await
            .expect("first hash calculation must not panic");
        assert!(updated, "first pass should compute and persist the hash");
        assert_eq!(hash, expected);
        assert_eq!(entry.hash.as_deref(), Some(expected.as_slice()));

        // Second pass: DB row now has a hash and the file is unchanged, so it
        // returns the cached value instead of recomputing.
        let (updated, hash, _) = storage
            .calculate_hash(db, "data.txt")
            .await
            .expect("cached hash lookup must not panic");
        assert!(!updated, "second pass should reuse the stored hash");
        assert_eq!(hash, expected);
    });
}

/// A row that already exists with a NULL hash (e.g. inserted by another code
/// path before hashing) must be hashed on the next call rather than panicking.
#[test]
fn calculate_hash_hashes_preexisting_null_row() {
    runtime().block_on(async {
        let db = test_db().await;
        let (storage, _root) = make_storage(db).await;

        // Materialize the entry with an explicit NULL hash.
        let created = storage
            .ensure_entry(db, "data.txt")
            .await
            .expect("ensure entry");
        assert!(created.hash.is_none(), "precondition: hash starts NULL");

        let (updated, hash, _) = storage
            .calculate_hash(db, "data.txt")
            .await
            .expect("hashing a NULL-hash row must not panic");
        assert!(updated);
        assert_eq!(hash, Sha256::digest(b"hello byteburrow").to_vec());

        let reloaded = entry::Entity::find()
            .filter(entry::Column::Id.eq(created.id))
            .one(db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(reloaded.hash.as_deref(), Some(hash.as_slice()));
    });
}
