//! Regression test for issue #5 (A2): unauthenticated path traversal via the
//! public share index endpoint (`share_index_impl` /
//! `GET /share/:share_id/index/*path`).
//!
//! Unlike `share_show_handler`, `share_index_impl` used to build the full
//! filesystem path with the unsafe, non-canonicalizing `get_full_path` and
//! never rejected `..` segments, so anyone holding a public share token could
//! read arbitrary files outside the shared directory without authenticating.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `storage_access_integration.rs`.

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
    let name = format!("st_{tag}_{}", uniq());
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
        name: Set(format!("st_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

/// Build an `AppState` sufficient to exercise the share router. The job
/// runner is constructed but never started; the traversal request under test
/// never reaches the hashing/classification path. Its inner runtime can't be
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

/// Sets up: a storage rooted at a fresh temp directory containing a
/// `shared/` subdirectory (the share's base entry) with a public file inside
/// it, plus a `secret.txt` file one level *above* the storage root — i.e.
/// outside the storage entirely. Returns `(share_token, storage_root_dir)`.
async fn setup_share(db: &DatabaseConnection) -> (String, std::path::PathBuf) {
    let owner = make_user(db, "owner").await;
    let grp = make_group(db).await;

    let test_dir = std::env::temp_dir().join(format!("byteburrow_share_traversal_{}", uniq()));
    let storage_root = test_dir.join("root");
    let shared_dir = storage_root.join("shared");
    std::fs::create_dir_all(&shared_dir).expect("create shared dir");
    std::fs::write(shared_dir.join("public.txt"), b"public contents").expect("write public file");
    std::fs::write(test_dir.join("secret.txt"), b"TOP SECRET").expect("write secret file");

    let stor = storage::ActiveModel {
        name: Set(format!("st_storage_{}", uniq())),
        description: Set(None),
        path: Set(storage_root.to_string_lossy().into_owned()),
        default_user: Set(owner.id),
        default_group: Set(grp.id),
        ignore_patterns: Set(String::new()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert storage");

    let now = Utc::now().naive_utc();
    let entry_model = entry::ActiveModel {
        storage_id: Set(stor.id),
        user_id: Set(owner.id),
        group_id: Set(grp.id),
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
    .expect("insert entry");

    let token = format!("share-token-{}", uniq());
    shared::ActiveModel {
        path_id: Set(entry_model.id),
        owner_id: Set(owner.id),
        // `shared.token` stores a hash of the plaintext (issue #10) — insert
        // it the same way `share_entry_handler` does, so a request using the
        // plaintext `token` returned from this fixture resolves correctly.
        token: Set(Some(Auth::hash_string(&token))),
        can_write: Set(false),
        user_ids: Set(vec![]),
        group_ids: Set(vec![]),
        expires_at: Set(None),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert share");

    (token, test_dir)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn share_index_serves_legitimate_file_within_share() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (token, _test_dir) = setup_share(&db).await;
        let app = storage_web::router().with_state(make_app_state(db));

        let req = Request::builder()
            .uri(format!("/share/{token}/index/public.txt"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_string(res).await, "public contents");
    });
}

#[test]
fn share_index_rejects_unauthenticated_path_traversal() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (token, _test_dir) = setup_share(&db).await;
        let app = storage_web::router().with_state(make_app_state(db));

        // No Authorization header at all: the share is reached purely via
        // its public token, matching the unauthenticated attacker scenario.
        let req = Request::builder()
            .uri(format!("/share/{token}/index/../../secret.txt"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();

        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_string(res).await;
        assert!(
            !body.contains("TOP SECRET"),
            "traversal must not leak file contents outside the share root, got: {body}"
        );
    });
}
