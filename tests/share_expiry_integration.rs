//! Regression test for issue #42: shares created with `expires_in_days` must
//! actually stop working once `shared.expires_at` is in the past.
//!
//! The expiry check lives in `get_share_context` (`src/web/storage.rs`): a
//! share whose `expires_at` is strictly before `now` fails every share-access
//! route with `ApiError::Gone` → HTTP 410 and body
//! `{"error": "Share has expired"}`. These tests create a share through the
//! real handler (`POST /:id/share/*path` with `expires_in_days`), backdate
//! the row's `expires_at` directly via a SeaORM update, and verify the public
//! share routes react accordingly.
//!
//! Boundary note: the handler compares strictly (`expires_at < now`), so
//! "expires_at exactly now ⇒ still valid" cannot be observed through HTTP
//! without a frozen clock — real time always advances between the DB write
//! and the handler's comparison. The observable boundary covered here is
//! therefore "one second ago ⇒ expired" and "exactly the write-time `now`
//! ⇒ expired by the time the request is served", against "in the future ⇒
//! served" and "no expiry ⇒ served".
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `share_traversal_integration.rs`.

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
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
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
                reverse_geocode_url: String::new(),
                reverse_geocode_api_key: String::new(),
                reverse_geocode_timeout: 10,
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
    let name = format!("se_{tag}_{}", uniq());
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
        name: Set(format!("se_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

/// Build an `AppState` sufficient to exercise the share router. The job
/// runner is constructed but never started; the requests under test never
/// reach the hashing/classification path. Its inner runtime can't be dropped
/// from within the test's own async context (tokio forbids dropping a
/// multi-thread runtime from async code), so it's leaked rather than dropped
/// — acceptable for a short-lived test process.
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
        notify_reload: std::sync::Arc::new(tokio::sync::Notify::new()),
    })
}

async fn bearer_token(db: &DatabaseConnection, user: user::Model) -> String {
    Auth::new(user)
        .create_token(db, None, None)
        .await
        .expect("create token")
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Sets up: an owner user, a storage rooted at a fresh temp directory
/// containing a `shared/` subdirectory (the share target) with a public file
/// inside it, and the DB rows for storage + directory entry. Returns
/// `(storage_id, owner_bearer_token, test_dir)`; the temp dir is left in
/// place for the lifetime of the test, like the sibling fixtures.
async fn setup_storage(db: &DatabaseConnection) -> (i32, String, std::path::PathBuf) {
    let owner = make_user(db, "owner").await;
    let grp = make_group(db).await;

    let test_dir = std::env::temp_dir().join(format!("byteburrow_share_expiry_{}", uniq()));
    let storage_root = test_dir.join("root");
    let shared_dir = storage_root.join("shared");
    std::fs::create_dir_all(&shared_dir).expect("create shared dir");
    std::fs::write(shared_dir.join("public.txt"), b"public contents").expect("write public file");

    let stor = storage::ActiveModel {
        name: Set(format!("se_storage_{}", uniq())),
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
    entry::ActiveModel {
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

    let token = bearer_token(db, owner).await;
    (stor.id, token, test_dir)
}

/// Create a public-link share on the `shared` entry through the real handler
/// (`POST /:id/share/*path`). Returns `(share_id, plaintext_token)`.
async fn create_public_share(
    app: &axum::Router,
    bearer: &str,
    storage_id: i32,
    expires_in_days: serde_json::Value,
) -> (i32, String) {
    let req = Request::builder()
        .uri(format!("/{storage_id}/share/shared"))
        .method("POST")
        .header("Authorization", format!("Bearer {bearer}"))
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "can_write": false,
                "expires_in_days": expires_in_days,
                "public_link": true,
                "user_ids": [],
                "group_ids": [],
            })
            .to_string(),
        ))
        .unwrap();
    let res = app
        .clone()
        .oneshot(req)
        .await
        .expect("send create-share request");
    assert_eq!(res.status(), StatusCode::OK, "share creation must succeed");

    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    let share_id = body["id"].as_i64().expect("share id") as i32;
    let token = body["token"]
        .as_str()
        .expect("create response must include the plaintext token")
        .to_string();
    (share_id, token)
}

/// Overwrite a share row's `expires_at` directly in the DB (SeaORM update on
/// `shared::ActiveModel`) — the "time has passed" step of the fixture.
async fn set_expires_at(
    db: &DatabaseConnection,
    share_id: i32,
    expires_at: Option<chrono::NaiveDateTime>,
) {
    let mut active: shared::ActiveModel = shared::Entity::find_by_id(share_id)
        .one(db)
        .await
        .expect("load share")
        .expect("share row must exist")
        .into();
    active.expires_at = Set(expires_at);
    active.update(db).await.expect("update share expires_at");
}

/// A share whose `expires_at` was backdated into the past must fail every
/// share-access route with 410 Gone + the exact error body, and creating it
/// with `expires_in_days` must have persisted a future expiry in the first
/// place (the original #42 gap).
#[test]
fn expired_share_returns_410_gone_on_share_index() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (storage_id, owner_token, _test_dir) = setup_storage(&db).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let (share_id, token) =
            create_public_share(&app, &owner_token, storage_id, serde_json::json!(7)).await;

        // Sanity: the create path must have persisted a *future* expiry.
        let stored = shared::Entity::find_by_id(share_id)
            .one(&db)
            .await
            .unwrap()
            .expect("share row must exist");
        let expires_at = stored
            .expires_at
            .expect("expires_in_days=7 must set expires_at");
        assert!(
            expires_at > Utc::now().naive_utc(),
            "fresh share must not already be expired"
        );

        // Backdate the row directly, as if the days had elapsed.
        set_expires_at(
            &db,
            share_id,
            Some(Utc::now().naive_utc() - chrono::Duration::hours(1)),
        )
        .await;

        // Share index (file inside the shared directory): 410 + exact body.
        let req = Request::builder()
            .uri(format!("/share/{token}/index/public.txt"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::GONE);
        let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        assert_eq!(body, serde_json::json!({"error": "Share has expired"}));

        // Share info route resolves through the same `get_share_context`
        // expiry gate and must be gone too.
        let req = Request::builder()
            .uri(format!("/share/{token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::GONE);
        let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        assert_eq!(body, serde_json::json!({"error": "Share has expired"}));
    });
}

/// Control group: shares that have not expired keep serving — both one with a
/// future `expires_at` and one created without any expiry (`null`).
#[test]
fn unexpired_share_still_serves_200() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (storage_id, owner_token, _test_dir) = setup_storage(&db).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        for days in [serde_json::json!(7), serde_json::json!(null)] {
            let label = days.to_string();
            let (_share_id, token) =
                create_public_share(&app, &owner_token, storage_id, days).await;

            let req = Request::builder()
                .uri(format!("/share/{token}/index/public.txt"))
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK, "days = {label}");
            assert_eq!(body_string(res).await, "public contents");
        }
    });
}

/// Expiry boundary, as tight as is observable through HTTP without a frozen
/// clock: `expires_at` one second in the past, and `expires_at` set to the
/// exact `now` captured before the write — the handler's own `now` is always
/// strictly later by the time it compares, so both must be 410 (the check is
/// `expires_at < now`, strict, never `<=`).
#[test]
fn expiry_boundary_one_second_ago_is_expired() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let (storage_id, owner_token, _test_dir) = setup_storage(&db).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let (share_id, token) =
            create_public_share(&app, &owner_token, storage_id, serde_json::json!(7)).await;

        for expires_at in [
            Utc::now().naive_utc() - chrono::Duration::seconds(1),
            Utc::now().naive_utc(),
        ] {
            set_expires_at(&db, share_id, Some(expires_at)).await;

            let req = Request::builder()
                .uri(format!("/share/{token}/index/public.txt"))
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::GONE,
                "expires_at = {expires_at} must read as expired"
            );
            let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
            assert_eq!(body, serde_json::json!({"error": "Share has expired"}));
        }
    });
}
