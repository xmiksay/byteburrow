//! Integration tests for the A7 security-hardening bundle (issue #10):
//! - share tokens are hashed at rest, not stored/echoed as plaintext
//! - `GET /api/storage/thumbnail/:hash/:size` requires auth + hash access
//! - `GET /api/ws` requires auth
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` / `storage_access_integration.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::entry::EntryType;
use byteburrow::entity::{entry, group, shared, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{storage as storage_web, ws, AppState};
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
    let name = format!("sh_{tag}_{}", uniq());
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
        name: Set(format!("sh_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("sh_storage_{}", uniq())),
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
    hash: Option<Vec<u8>>,
) -> entry::Model {
    let now = chrono::Utc::now().naive_utc();
    entry::ActiveModel {
        storage_id: Set(storage_id),
        user_id: Set(owner),
        group_id: Set(owning_group),
        parent_id: Set(None),
        path: Set(format!("sh_entry_{}", uniq())),
        hash: Set(hash),
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

fn make_app_state(db: DatabaseConnection) -> Arc<AppState> {
    let plugin_dir = std::env::temp_dir().join(format!("byteburrow_test_plugins_{}", uniq()));
    std::fs::create_dir_all(&plugin_dir).expect("create empty plugin dir");
    let registry = PluginRegistry::load_from_directory(&plugin_dir, &Default::default());
    let (job_runner, job_sender) = JobRunner::new(db.clone(), registry);
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

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Creating a public-link share must (a) return the plaintext token exactly
/// once in the create response, and (b) never persist that plaintext to the
/// database — only its hash.
#[test]
fn creating_a_public_share_returns_plaintext_but_persists_only_a_hash() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id, None).await;
        let entry_path = ent.path.clone();
        let owner_token = bearer_token(&db, owner).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

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
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        assert_eq!(body["has_public_link"], serde_json::json!(true));
        let plaintext = body["token"]
            .as_str()
            .expect("create response must include the plaintext token")
            .to_string();

        // The DB row must not contain the plaintext — only its hash.
        let share_id = body["id"].as_i64().unwrap() as i32;
        let stored = shared::Entity::find_by_id(share_id)
            .one(&db)
            .await
            .unwrap()
            .expect("share row must exist");
        let stored_token = stored.token.expect("token column must be set");
        assert_ne!(
            stored_token, plaintext,
            "share token must not be stored in plaintext"
        );
        assert_eq!(stored_token, Auth::hash_string(&plaintext));
    });
}

/// Listing shares must never expose the plaintext or the stored hash —
/// callers only learn whether a public link exists via `has_public_link`.
#[test]
fn listing_shares_never_echoes_the_token() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id, None).await;

        shared::ActiveModel {
            path_id: Set(ent.id),
            owner_id: Set(owner.id),
            token: Set(Some(Auth::hash_string("some-plaintext-token"))),
            can_write: Set(false),
            user_ids: Set(vec![]),
            group_ids: Set(vec![]),
            expires_at: Set(None),
            created_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert share");

        let owner_token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/{}/share/{}", stor.id, ent.path))
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        let shares = body.as_array().expect("list response is an array");
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0]["has_public_link"], serde_json::json!(true));
        assert!(
            shares[0]["token"].is_null(),
            "list response must not include the token/hash, got {:?}",
            shares[0]["token"]
        );
    });
}

/// A public-link share created through the handler must remain reachable via
/// its plaintext token end-to-end (the hash-based lookup must round-trip).
#[test]
fn public_share_is_reachable_via_its_plaintext_token() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id, None).await;
        let entry_path = ent.path.clone();
        let owner_token = bearer_token(&db, owner).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

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
        let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
        let plaintext = body["token"].as_str().unwrap().to_string();

        // Anonymous request using only the plaintext token must succeed.
        let req = Request::builder()
            .uri(format!("/share/{plaintext}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // A wrong/guessed token must not resolve to the same share.
        let req = Request::builder()
            .uri("/share/not-the-real-token")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    });
}

/// `GET /api/storage/thumbnail/:hash/:size` used to be fully unauthenticated
/// and ungated; it must now require `Auth` and the same `require_hash_access`
/// check as `get_meta_handler`.
#[test]
fn thumbnail_endpoint_requires_auth_and_hash_access() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let hash = vec![0xabu8; 32];
        let _ent = make_entry(&db, stor.id, owner.id, grp.id, Some(hash.clone())).await;
        let hash_hex = hex::encode(&hash);

        let owner_token = bearer_token(&db, owner.clone()).await;
        let stranger_token = bearer_token(&db, stranger).await;

        let app = storage_web::router().with_state(make_app_state(db.clone()));

        // No credentials at all: must be rejected before any file lookup.
        let req = Request::builder()
            .uri(format!("/thumbnail/{hash_hex}/small"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Authenticated but with no access to any storage holding this hash.
        let req = Request::builder()
            .uri(format!("/thumbnail/{hash_hex}/small"))
            .header("Authorization", format!("Bearer {stranger_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // The owner passes the access gate; no thumbnail file exists on disk
        // for this test, so the handler reaches the (expected) 404 for the
        // *file*, proving the auth/access check itself let the request through.
        let req = Request::builder()
            .uri(format!("/thumbnail/{hash_hex}/small"))
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    });
}

/// `GET /api/ws` used to have no `Auth` extractor at all. Build a minimal
/// router mirroring the real mount point to verify the upgrade handler now
/// rejects unauthenticated requests before ever touching the socket.
#[test]
fn ws_endpoint_requires_auth() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let user = make_user(&db, "wsuser", false).await;
        let token = bearer_token(&db, user).await;

        let app: Router<Arc<AppState>> = Router::new().route("/ws", get(ws::ws_handler));
        let app = app.with_state(make_app_state(db));

        // No credentials: rejected by the `Auth` extractor before the
        // WebSocket upgrade is even attempted.
        let req = Request::builder().uri("/ws").body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Authenticated but not a real WebSocket handshake: must get past
        // `Auth` (i.e. not 401) — it fails afterward for an unrelated reason.
        let req = Request::builder()
            .uri("/ws")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
    });
}
