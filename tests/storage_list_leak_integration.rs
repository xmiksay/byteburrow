//! HTTP-level integration tests for issue #7: `GET /api/storage` must only
//! return storages the caller can access (mirrors `require_storage_access`),
//! and `GET /api/storage/:id` must use the same gate rather than
//! admin-only, so the two endpoints agree on who can see what.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `storage_access_integration.rs` /
//! `share_ownership_integration.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::{group, group_user, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{storage as storage_web, AppState};
use minijinja::Environment;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
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
    let name = format!("sl_{tag}_{}", uniq());
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
        name: Set(format!("sl_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("sl_storage_{}", uniq())),
        description: Set(None),
        path: Set("/tmp".to_string()),
        default_user: Set(owner),
        default_group: Set(default_group),
        ignore_patterns: Set(String::new()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert storage")
}

/// Build an `AppState` sufficient to exercise the storage router. The job
/// runner is constructed but never started; these requests never reach the
/// hashing/classification path. Its inner runtime can't be dropped from
/// within the test's own async context (tokio forbids dropping a multi-thread
/// runtime from async code), so it's leaked rather than dropped — acceptable
/// for a short-lived test process.
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

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("parse json body")
}

#[test]
fn list_storages_only_returns_storages_the_caller_can_access() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let other_grp = make_group(&db).await;

        // Owned by `owner`, not accessible to `stranger`.
        let owned = make_storage(&db, owner.id, grp.id).await;
        // Neither owned by nor group-shared with either user.
        let unrelated = make_storage(&db, owner.id, other_grp.id).await;

        let stranger_token = bearer_token(&db, stranger).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri("/")
            .header("Authorization", format!("Bearer {stranger_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body = body_json(res).await;
        let ids: Vec<i64> = body["items"]
            .as_array()
            .expect("paginated list body must have an items array")
            .iter()
            .map(|s| s["id"].as_i64().expect("id field"))
            .collect();

        assert!(
            !ids.contains(&(owned.id as i64)),
            "stranger must not see a storage they don't own/share: {ids:?}"
        );
        assert!(
            !ids.contains(&(unrelated.id as i64)),
            "stranger must not see an unrelated storage: {ids:?}"
        );
    });
}

#[test]
fn list_storages_includes_owned_and_group_shared_storages() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let member = make_user(&db, "member", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;

        group_user::ActiveModel {
            user_id: Set(member.id),
            group_id: Set(grp.id),
            admin: Set(false),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert group membership");

        let owner_token = bearer_token(&db, owner).await;
        let member_token = bearer_token(&db, member).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        for token in [owner_token, member_token] {
            let req = Request::builder()
                .uri("/")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);

            let body = body_json(res).await;
            let ids: Vec<i64> = body["items"]
                .as_array()
                .expect("paginated list body must have an items array")
                .iter()
                .map(|s| s["id"].as_i64().expect("id field"))
                .collect();
            assert!(
                ids.contains(&(stor.id as i64)),
                "owner/group member must see their storage: {ids:?}"
            );
        }
    });
}

#[test]
fn list_storages_admin_sees_everything() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let admin = make_user(&db, "admin", true).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;

        let admin_token = bearer_token(&db, admin).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri("/")
            .header("Authorization", format!("Bearer {admin_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body = body_json(res).await;
        let ids: Vec<i64> = body["items"]
            .as_array()
            .expect("paginated list body must have an items array")
            .iter()
            .map(|s| s["id"].as_i64().expect("id field"))
            .collect();
        assert!(
            ids.contains(&(stor.id as i64)),
            "admin must see all storages: {ids:?}"
        );
    });
}

#[test]
fn get_storage_by_id_matches_list_gate_not_admin_only() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;

        let owner_token = bearer_token(&db, owner).await;
        let stranger_token = bearer_token(&db, stranger).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // Owner (non-admin) can fetch their own storage by id — this used to
        // be admin-only, inconsistent with the list gate.
        let req = Request::builder()
            .uri(format!("/{}", stor.id))
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // A stranger with no relation to the storage is still denied.
        let req = Request::builder()
            .uri(format!("/{}", stor.id))
            .header("Authorization", format!("Bearer {stranger_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn list_storages_is_paginated_and_caps_per_page() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "pager", false).await;
        let grp = make_group(&db).await;
        // At least three storages this owner can see.
        for _ in 0..3 {
            make_storage(&db, owner.id, grp.id).await;
        }

        let token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // per_page=1 must return exactly one item and expose page metadata.
        let req = Request::builder()
            .uri("/?page=1&per_page=1")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res).await;
        assert_eq!(body["page"].as_u64(), Some(1));
        assert_eq!(body["per_page"].as_u64(), Some(1));
        assert_eq!(
            body["items"].as_array().expect("items array").len(),
            1,
            "per_page=1 must return a single item"
        );
        assert!(
            body["total"].as_u64().unwrap() >= 3,
            "owner must see at least the 3 storages created here"
        );
        assert!(body["total_pages"].as_u64().unwrap() >= 3);

        // An oversized per_page is clamped to the server's hard cap (200).
        let req = Request::builder()
            .uri("/?per_page=99999")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = body_json(res).await;
        assert_eq!(
            body["per_page"].as_u64(),
            Some(200),
            "per_page must be clamped to MAX_PER_PAGE"
        );
    });
}
