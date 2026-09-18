//! Integration tests for issue #37: directory-listing GETs must not have side
//! effects, and the new `POST /api/storage/:id/scan` is the explicit,
//! authorized way to discover files and queue hashing.
//!
//! - `GET /:id/list` must not create `entry` rows (the old
//!   `dispatch_hash_jobs` behavior leaked DB rows + jobs from a GET).
//! - `POST /:id/scan` creates missing `entry` rows, reports a summary, and
//!   enqueues nothing when every file is already hashed.
//! - `POST /:id/scan` requires auth and denies non-owners.
//!
//! The job channel is never drained in these tests (`JobRunner::run` is not
//! started), so the handler's `queued` count is deterministic; side effects on
//! the DB are asserted directly via `entry` rows.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `storage_access_integration.rs` / `security_hardening_integration.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::{entry, group, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{storage as storage_web, AppState};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use sea_orm_migration::MigratorTrait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::OnceCell;
use tower::ServiceExt;

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
                plugin: std::collections::HashMap::new(),
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

async fn make_user(db: &DatabaseConnection, tag: &str, admin: bool) -> user::Model {
    let name = format!("scan_{tag}_{}", uniq());
    user::ActiveModel {
        name: Set(name.clone()),
        description: Set(None),
        username: Set(name),
        password: Set(Auth::hash_string("pw")),
        enabled: Set(true),
        admin: Set(admin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert user")
}

async fn make_group(db: &DatabaseConnection) -> group::Model {
    group::ActiveModel {
        name: Set(format!("scan_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

/// A real on-disk storage root:
///
/// ```text
/// alpha.txt          (file)
/// sub/nested.txt     (file)
/// .git/ignored.txt   (file, matches the storage's ignore patterns)
/// ```
///
/// Returns the storage row and its tempdir root (removed by the caller).
async fn make_storage_with_tree(
    db: &DatabaseConnection,
    owner: i32,
    default_group: i32,
) -> (storage::Model, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "byteburrow_scan_test_{}_{}",
        std::process::id(),
        uniq()
    ));
    tokio::fs::create_dir_all(root.join("sub")).await.unwrap();
    tokio::fs::create_dir_all(root.join(".git")).await.unwrap();
    tokio::fs::write(root.join("alpha.txt"), b"alpha").await.unwrap();
    tokio::fs::write(root.join("sub/nested.txt"), b"nested")
        .await
        .unwrap();
    tokio::fs::write(root.join(".git/ignored.txt"), b"ignored")
        .await
        .unwrap();

    let model = storage::ActiveModel {
        name: Set(format!("scan_storage_{}", uniq())),
        description: Set(None),
        path: Set(root.to_string_lossy().into_owned()),
        default_user: Set(owner),
        default_group: Set(default_group),
        ignore_patterns: Set(".git".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert storage");

    (model, root)
}

fn make_app_state(db: DatabaseConnection) -> Arc<AppState> {
    let plugin_dir = std::env::temp_dir().join(format!("byteburrow_test_plugins_{}", uniq()));
    std::fs::create_dir_all(&plugin_dir).expect("create empty plugin dir");
    let registry = PluginRegistry::load_from_directory(&plugin_dir, &Default::default());
    let (job_runner, job_sender) = JobRunner::new(db.clone(), registry);
    // Never started → nothing drains the job channel, so `queued` counts in
    // scan responses are deterministic.
    std::mem::forget(job_runner);

    Arc::new(AppState {
        db,
        config: (*Config::get()).clone(),
        jinja: minijinja::Environment::new(),
        job_sender,
        notify_reload: std::sync::Arc::new(tokio::sync::Notify::new()),
    })
}

async fn bearer_token(db: &DatabaseConnection, user: user::Model) -> String {
    Auth::new(user)
        .create_token(db, None, None)
        .await
        .expect("create token")
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("parse JSON body")
}

async fn count_entries(db: &DatabaseConnection, storage_id: i32) -> u64 {
    entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .count(db)
        .await
        .expect("count entries")
}

/// A scan of a fresh tree must create `entry` rows for every non-ignored
/// path (files *and* directories), skip ignored subtrees, and report both
/// counts in the summary.
#[test]
fn scan_creates_entry_rows_and_reports_summary() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let (stor, root) = make_storage_with_tree(&db, owner.id, grp.id).await;
        let token = bearer_token(&db, owner).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));
        let req = Request::builder()
            .uri(format!("/{}/scan", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body = body_json(res).await;
        // alpha.txt + sub/nested.txt files, sub directory, .git ignored.
        assert_eq!(body["created"], serde_json::json!(3), "got {body}");
        assert_eq!(body["queued"], serde_json::json!(2), "got {body}");

        let rows = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(stor.id))
            .all(&db)
            .await
            .unwrap();
        let paths: Vec<&str> = rows.iter().map(|e| e.path.as_str()).collect();
        for expected in ["alpha.txt", "sub", "sub/nested.txt"] {
            assert!(paths.contains(&expected), "missing {expected}, got {paths:?}");
        }
        assert!(
            !paths.iter().any(|p| p.starts_with(".git")),
            "ignored subtree must not be tracked, got {paths:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    });
}

/// Scanning a storage whose files are all already hashed (and tracked) must
/// be a no-op: nothing created, nothing queued.
#[test]
fn scan_of_fully_hashed_storage_enqueues_nothing() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let (stor, root) = make_storage_with_tree(&db, owner.id, grp.id).await;
        let token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // First scan discovers the tree.
        let req = Request::builder()
            .uri(format!("/{}/scan", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let first = body_json(res).await;
        assert_eq!(first["created"], serde_json::json!(3));

        // Mark every file entry as hashed (content is irrelevant here).
        let rows = entry::Entity::find()
            .filter(entry::Column::StorageId.eq(stor.id))
            .filter(entry::Column::EntryType.eq(entry::EntryType::File))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        for m in rows {
            let mut active: entry::ActiveModel = m.into();
            active.hash = Set(Some(vec![0xaau8; 32]));
            active.update(&db).await.expect("set hash");
        }

        // Second scan: everything tracked and hashed → nothing to do.
        let req = Request::builder()
            .uri(format!("/{}/scan", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let second = body_json(res).await;
        assert_eq!(second["created"], serde_json::json!(0), "got {second}");
        assert_eq!(second["queued"], serde_json::json!(0), "got {second}");

        let _ = std::fs::remove_dir_all(&root);
    });
}

/// The scan endpoint must reject unauthenticated requests before touching
/// anything (401).
#[test]
fn scan_requires_authentication() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let (stor, root) = make_storage_with_tree(&db, owner.id, grp.id).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));
        let req = Request::builder()
            .uri(format!("/{}/scan", stor.id))
            .method("POST")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // And it must not have created anything on the rejected request.
        assert_eq!(count_entries(&db, stor.id).await, 0);

        let _ = std::fs::remove_dir_all(&root);
    });
}

/// A user with no relationship to the storage gets 403, and the scan leaves
/// no rows behind.
#[test]
fn scan_denies_non_owner() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let (stor, root) = make_storage_with_tree(&db, owner.id, grp.id).await;
        let token = bearer_token(&db, stranger).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));
        let req = Request::builder()
            .uri(format!("/{}/scan", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_eq!(count_entries(&db, stor.id).await, 0);

        let _ = std::fs::remove_dir_all(&root);
    });
}

/// Regression for issue #37: a directory-listing GET over a tree containing
/// untracked, unhashed files must not create `entry` rows (the old
/// `dispatch_hash_jobs` also enqueued jobs from these GETs; the DB row count
/// is the integration-visible part of that side effect).
#[test]
fn listing_get_does_not_create_entry_rows() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let (stor, root) = make_storage_with_tree(&db, owner.id, grp.id).await;
        let token = bearer_token(&db, owner).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // Root listing…
        let req = Request::builder()
            .uri(format!("/{}/list", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // …and a subdirectory listing (both used to dispatch hash jobs).
        let req = Request::builder()
            .uri(format!("/{}/list/sub", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // The GETs served the (unhashed) files but must not have tracked them.
        assert_eq!(
            count_entries(&db, stor.id).await,
            0,
            "GET listing must not create entry rows"
        );

        let _ = std::fs::remove_dir_all(&root);
    });
}
