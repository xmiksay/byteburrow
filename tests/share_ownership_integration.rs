//! Integration tests for `byteburrow::web::require_entry_owner` — the
//! authorization gate that guards the share-management endpoints (list,
//! create, update, delete) against IDOR (issue #6). Verifies a non-owner is
//! denied while the entry owner, a member of the entry's owning group, and
//! admins are granted.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `storage_access_integration.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::entry::EntryType;
use byteburrow::entity::{entry, group, group_user, shared, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{require_entry_owner, storage as storage_web, ApiError, AppState};
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

async fn make_user(db: &DatabaseConnection, tag: &str, admin: bool) -> user::Model {
    let name = format!("so_{tag}_{}", uniq());
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
        name: Set(format!("so_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("so_storage_{}", uniq())),
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

async fn make_entry(
    db: &DatabaseConnection,
    storage_id: i32,
    owner: i32,
    owning_group: i32,
) -> entry::Model {
    let now = chrono::Utc::now().naive_utc();
    entry::ActiveModel {
        storage_id: Set(storage_id),
        user_id: Set(owner),
        group_id: Set(owning_group),
        parent_id: Set(None),
        path: Set(format!("so_entry_{}", uniq())),
        entry_type: Set(EntryType::File),
        notify: Set(false),
        skip_plugins: Set(false),
        size: Set(0),
        modified_at: Set(now),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert entry")
}

#[test]
fn owner_is_granted_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;
        let ent = make_entry(db, stor.id, owner.id, grp.id).await;

        require_entry_owner(&Auth::new(owner), &ent, db)
            .await
            .expect("entry owner must be allowed");
    });
}

#[test]
fn non_owner_is_denied_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let stranger = make_user(db, "stranger", false).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;
        let ent = make_entry(db, stor.id, owner.id, grp.id).await;

        let err = require_entry_owner(&Auth::new(stranger), &ent, db)
            .await
            .expect_err("non-owner must be denied");
        assert!(matches!(err, ApiError::Forbidden { .. }), "got {err:?}");
    });
}

#[test]
fn recipient_of_a_share_is_not_granted_management_access() {
    // Having a share of the entry (i.e. being a recipient) must not be
    // conflated with owning it: only the owner/group/admin may manage shares.
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let recipient = make_user(db, "recipient", false).await;
        let grp = make_group(db).await;
        let other_grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;
        let ent = make_entry(db, stor.id, owner.id, grp.id).await;

        // recipient belongs to an unrelated group, not the entry's group.
        group_user::ActiveModel {
            user_id: Set(recipient.id),
            group_id: Set(other_grp.id),
            admin: Set(false),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert group membership");

        let err = require_entry_owner(&Auth::new(recipient), &ent, db)
            .await
            .expect_err("share recipient must be denied management access");
        assert!(matches!(err, ApiError::Forbidden { .. }), "got {err:?}");
    });
}

#[test]
fn group_member_is_granted_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let member = make_user(db, "member", false).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;
        let ent = make_entry(db, stor.id, owner.id, grp.id).await;

        group_user::ActiveModel {
            user_id: Set(member.id),
            group_id: Set(grp.id),
            admin: Set(false),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert group membership");

        require_entry_owner(&Auth::new(member), &ent, db)
            .await
            .expect("group member must be allowed");
    });
}

#[test]
fn admin_is_granted_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let admin = make_user(db, "admin", true).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;
        let ent = make_entry(db, stor.id, owner.id, grp.id).await;

        require_entry_owner(&Auth::new(admin), &ent, db)
            .await
            .expect("admin must be allowed");
    });
}

// ---------------------------------------------------------------------------
// HTTP-level tests: exercise the actual handlers end-to-end, not just the
// `require_entry_owner` helper in isolation.
// ---------------------------------------------------------------------------

/// Build an `AppState` sufficient to exercise the share router. The job
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
    })
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn bearer_token(db: &DatabaseConnection, user: user::Model) -> String {
    Auth::new(user)
        .create_token(db, None, None)
        .await
        .expect("create token")
}

#[test]
fn list_and_create_share_reject_non_owner_and_allow_owner() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id).await;
        let entry_path = ent.path.clone();

        let owner_token = bearer_token(&db, owner).await;
        let stranger_token = bearer_token(&db, stranger).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // Stranger cannot list shares on an entry they don't own.
        let req = Request::builder()
            .uri(format!("/{}/share/{entry_path}", stor.id))
            .header("Authorization", format!("Bearer {stranger_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Stranger cannot create a share on an entry they don't own.
        let req = Request::builder()
            .uri(format!("/{}/share/{entry_path}", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {stranger_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "can_write": false,
                    "expires_in_days": null,
                    "public_link": true,
                    "user_ids": [],
                    "group_ids": [],
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // The owner can list (empty) and create a share.
        let req = Request::builder()
            .uri(format!("/{}/share/{entry_path}", stor.id))
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let req = Request::builder()
            .uri(format!("/{}/share/{entry_path}", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "can_write": false,
                    "expires_in_days": null,
                    "public_link": true,
                    "user_ids": [],
                    "group_ids": [],
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);
    });
}

#[test]
fn my_shares_are_keyed_on_creator_not_entry_ownership() {
    // Issue #32 / G1: `GET /share` ("my shares") lists shares by their explicit
    // `owner_id` (creator). A share created by the entry owner shows up in the
    // owner's list; a group member who *can manage* the entry but did not create
    // the share must not see it in their own "my shares".
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let member = make_user(&db, "member", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id).await;
        let entry_path = ent.path.clone();

        // `member` is in the entry's owning group, so require_entry_owner lets
        // them manage its shares — but they still aren't the creator.
        group_user::ActiveModel {
            user_id: Set(member.id),
            group_id: Set(grp.id),
            admin: Set(false),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert group membership");

        let owner_id = owner.id;
        let owner_token = bearer_token(&db, owner).await;
        let member_token = bearer_token(&db, member).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // Owner creates the share; the response records them as the owner.
        let req = Request::builder()
            .uri(format!("/{}/share/{entry_path}", stor.id))
            .method("POST")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "can_write": false,
                    "expires_in_days": null,
                    "public_link": false,
                    "user_ids": [],
                    "group_ids": [],
                })
                .to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let created: serde_json::Value =
            serde_json::from_str(&body_string(res).await).expect("parse share response");
        assert_eq!(created["owner_id"], owner_id);
        let share_id = created["id"].as_i64().expect("share id");

        // Owner's "my shares" includes it.
        let req = Request::builder()
            .uri("/share")
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let owner_shares: Vec<serde_json::Value> =
            serde_json::from_str(&body_string(res).await).expect("parse owner shares");
        assert!(
            owner_shares
                .iter()
                .any(|s| s["id"].as_i64() == Some(share_id)),
            "owner's my-shares must contain the share they created"
        );

        // The group member can manage the entry but did not create the share,
        // so it must not appear in *their* my-shares.
        let req = Request::builder()
            .uri("/share")
            .header("Authorization", format!("Bearer {member_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let member_shares: Vec<serde_json::Value> =
            serde_json::from_str(&body_string(res).await).expect("parse member shares");
        assert!(
            !member_shares
                .iter()
                .any(|s| s["id"].as_i64() == Some(share_id)),
            "a non-creator must not see the share in their my-shares"
        );
    });
}

#[test]
fn update_and_delete_share_reject_non_owner_and_allow_owner() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id).await;

        let now = chrono::Utc::now().naive_utc();
        let share = shared::ActiveModel {
            path_id: Set(ent.id),
            owner_id: Set(owner.id),
            token: Set(None),
            can_write: Set(false),
            user_ids: Set(vec![]),
            group_ids: Set(vec![]),
            expires_at: Set(None),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert share");

        let owner_token = bearer_token(&db, owner).await;
        let stranger_token = bearer_token(&db, stranger).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let update_body = || {
            Body::from(
                serde_json::json!({
                    "can_write": true,
                    "expires_in_days": null,
                    "public_link": false,
                    "user_ids": [],
                    "group_ids": [],
                })
                .to_string(),
            )
        };

        // Stranger cannot update or delete a share on an entry they don't own.
        let req = Request::builder()
            .uri(format!("/share/{}", share.id))
            .method("PUT")
            .header("Authorization", format!("Bearer {stranger_token}"))
            .header("Content-Type", "application/json")
            .body(update_body())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let req = Request::builder()
            .uri(format!("/share/{}", share.id))
            .method("DELETE")
            .header("Authorization", format!("Bearer {stranger_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // The owner can update and then delete the share.
        let req = Request::builder()
            .uri(format!("/share/{}", share.id))
            .method("PUT")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(update_body())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);

        let req = Request::builder()
            .uri(format!("/share/{}", share.id))
            .method("DELETE")
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);
    });
}
