//! Regression test for issue #9 (A6): a share on a single nested entry used
//! to grant access to the *whole* storage via the generic
//! `/api/storage/:id/...` endpoints (`require_storage_access` only checked
//! that *some* share existed anywhere in the storage). Access derived from a
//! share must be scoped to that entry's own subtree, and read-only shares
//! must not grant write access via these endpoints either.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `storage_access_integration.rs` /
//! `share_traversal_integration.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::entry::EntryType;
use byteburrow::entity::{entry, group, shared, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{storage as storage_web, AppState};
use chrono::Utc;
use minijinja::Environment;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
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

/// Unique suffix so parallel tests don't collide on unique columns / paths.
fn uniq() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn make_user(db: &DatabaseConnection, tag: &str) -> user::Model {
    let name = format!("ss_{tag}_{}", uniq());
    user::ActiveModel {
        name: Set(name.clone()),
        description: Set(None),
        username: Set(name),
        password: Set(Auth::hash_string("pw")),
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
        name: Set(format!("ss_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

/// Build an `AppState` sufficient to exercise the storage router. The job
/// runner is constructed but never started. Its inner runtime can't be
/// dropped from within the test's own async context (tokio forbids dropping
/// a multi-thread runtime from async code), so it's leaked rather than
/// dropped — acceptable for a short-lived test process.
fn make_app_state(db: DatabaseConnection) -> Arc<AppState> {
    let plugin_dir = std::env::temp_dir().join(format!("byteburrow_test_plugins_{}", uniq()));
    std::fs::create_dir_all(&plugin_dir).expect("create empty plugin dir");
    let registry = PluginRegistry::load_from_directory(&plugin_dir, &Default::default());
    let (job_runner, job_sender) = JobRunner::new(db.clone(), registry);
    std::mem::forget(job_runner);

    Arc::new(AppState {
        db,
        config: (*Config::get()).clone(),
        jinja: Environment::new(),
        job_sender,
    })
}

async fn bearer_token(db: &DatabaseConnection, user: user::Model) -> String {
    Auth::new(user)
        .create_token(db, None, None)
        .await
        .expect("create token")
}

/// Sets up a storage owned by `owner` (unrelated to the recipient) with two
/// sibling subtrees on disk — `shared/inside.txt` and `other/outside.txt` —
/// and a share on the `shared` entry only, granted to `recipient`.
/// Returns `(storage, recipient_token, storage_root_dir)`.
async fn setup_storage_with_scoped_share(
    db: &DatabaseConnection,
    can_write: bool,
) -> (storage::Model, String, std::path::PathBuf) {
    let owner = make_user(db, "owner").await;
    let recipient = make_user(db, "recipient").await;
    let owner_grp = make_group(db).await;

    let storage_root = std::env::temp_dir().join(format!("byteburrow_share_scope_{}", uniq()));
    let shared_dir = storage_root.join("shared");
    let other_dir = storage_root.join("other");
    std::fs::create_dir_all(&shared_dir).expect("create shared dir");
    std::fs::create_dir_all(&other_dir).expect("create other dir");
    std::fs::write(shared_dir.join("inside.txt"), b"inside contents").expect("write inside file");
    std::fs::write(other_dir.join("outside.txt"), b"outside contents").expect("write outside file");

    let stor = storage::ActiveModel {
        name: Set(format!("ss_storage_{}", uniq())),
        description: Set(None),
        path: Set(storage_root.to_string_lossy().into_owned()),
        default_user: Set(owner.id),
        default_group: Set(owner_grp.id),
        ignore_patterns: Set(String::new()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert storage");

    let now = Utc::now().naive_utc();
    let shared_entry = entry::ActiveModel {
        storage_id: Set(stor.id),
        user_id: Set(owner.id),
        group_id: Set(owner_grp.id),
        parent_id: Set(None),
        path: Set("shared".to_string()),
        entry_type: Set(EntryType::Directory),
        notify: Set(false),
        skip_plugins: Set(false),
        size: Set(0),
        modified_at: Set(now),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert shared entry");

    shared::ActiveModel {
        path_id: Set(shared_entry.id),
        token: Set(None),
        can_write: Set(can_write),
        user_ids: Set(vec![recipient.id]),
        group_ids: Set(vec![]),
        expires_at: Set(None),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert share");

    let recipient_token = bearer_token(db, recipient).await;

    (stor, recipient_token, storage_root)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn share_recipient_can_read_within_shared_subtree() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, false).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .uri(format!("/{}/show/shared/inside.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_string(res).await, "inside contents");
    });
}

#[test]
fn share_recipient_cannot_read_outside_shared_subtree() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, false).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .uri(format!("/{}/show/other/outside.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn share_recipient_cannot_list_storage_root() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, false).await;
        let app = storage_web::router().with_state(make_app_state(db));

        // Root listing is not within the shared entry's subtree.
        let req = Request::builder()
            .uri(format!("/{}/list", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn share_recipient_can_list_within_shared_subtree() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, false).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .uri(format!("/{}/list/shared", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::OK);
    });
}

#[test]
fn read_only_share_cannot_write_within_its_own_subtree() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, false).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .method("POST")
            .uri(format!("/{}/create/shared/new.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"entry_type": "File"}).to_string(),
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn writable_share_can_write_within_subtree_but_not_outside() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, true).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .method("POST")
            .uri(format!("/{}/create/shared/new.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"entry_type": "File"}).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let req = Request::builder()
            .method("POST")
            .uri(format!("/{}/create/other/new.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"entry_type": "File"}).to_string(),
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn writable_share_cannot_rename_into_a_path_outside_its_subtree() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (stor, token, _root) = setup_storage_with_scoped_share(&db, true).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .method("POST")
            .uri(format!("/{}/rename/shared/inside.txt", stor.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"new_path": "other/escaped.txt"}).to_string(),
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}
