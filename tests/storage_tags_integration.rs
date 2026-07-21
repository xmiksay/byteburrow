//! Integration tests for the entry tag-update endpoints (issue #12):
//! `PUT /api/storage/:id/tags/*path` and `PUT /api/storage/share/:share_id/tags/*path`.
//! Before this change these routes didn't exist server-side, so the frontend
//! calls silently fell through to the SPA fallback.
//!
//! Shares the process-lifetime runtime/DB setup pattern with the other
//! `*_integration.rs` files.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::entry::EntryType;
use byteburrow::entity::{entry, meta, shared, storage, user};
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{storage as storage_web, AppState};
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
    let name = format!("st_{tag}_{}", uniq());
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

async fn make_group(db: &DatabaseConnection) -> byteburrow::entity::group::Model {
    byteburrow::entity::group::ActiveModel {
        name: Set(format!("st_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("st_storage_{}", uniq())),
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

/// Insert an entry, optionally already hashed (tags are content-addressed via
/// `meta`, keyed by the entry's hash).
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
        path: Set(format!("st_entry_{}", uniq())),
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
fn owner_can_update_tags_on_a_hashed_entry() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        let ent = make_entry(&db, stor.id, owner.id, grp.id, Some(hash.clone())).await;

        let owner_token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/{}/tags/{}", stor.id, ent.path))
            .method("PUT")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "tags": [1, 2] }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);

        let saved = meta::Entity::find_by_id(hash)
            .one(&db)
            .await
            .expect("query meta")
            .expect("meta row created");
        assert_eq!(saved.tags, vec![1, 2]);
    });
}

#[test]
fn updating_tags_preserves_existing_keywords_and_custom() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        let ent = make_entry(&db, stor.id, owner.id, grp.id, Some(hash.clone())).await;

        meta::ActiveModel {
            hash: Set(hash.clone()),
            tags: Set(vec![9]),
            keywords: Set(vec!["sunset".to_string()]),
            custom: Set(serde_json::json!({"faces": 2})),
        }
        .insert(&db)
        .await
        .expect("seed meta");

        let owner_token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/{}/tags/{}", stor.id, ent.path))
            .method("PUT")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({ "tags": [3] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);

        let saved = meta::Entity::find_by_id(hash)
            .one(&db)
            .await
            .expect("query meta")
            .expect("meta row still exists");
        assert_eq!(saved.tags, vec![3]);
        assert_eq!(saved.keywords, vec!["sunset".to_string()]);
        assert_eq!(saved.custom, serde_json::json!({"faces": 2}));
    });
}

#[test]
fn updating_tags_on_an_unhashed_entry_is_rejected() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let ent = make_entry(&db, stor.id, owner.id, grp.id, None).await;

        let owner_token = bearer_token(&db, owner).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/{}/tags/{}", stor.id, ent.path))
            .method("PUT")
            .header("Authorization", format!("Bearer {owner_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({ "tags": [1] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::CONFLICT,
            "{}",
            body_string(res).await
        );
    });
}

#[test]
fn non_owner_without_access_cannot_update_tags() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let stranger = make_user(&db, "stranger", false).await;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;
        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        let ent = make_entry(&db, stor.id, owner.id, grp.id, Some(hash)).await;

        let stranger_token = bearer_token(&db, stranger).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/{}/tags/{}", stor.id, ent.path))
            .method("PUT")
            .header("Authorization", format!("Bearer {stranger_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({ "tags": [1] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "{}",
            body_string(res).await
        );
    });
}

/// The shared entry is a directory; tags are set on a file within it (the
/// realistic case — `TagDialog.vue` edits tags on files, not share roots).
#[test]
fn writable_share_can_update_tags_but_read_only_share_cannot() {
    runtime().block_on(async {
        let db = test_db().await.clone();
        let owner = make_user(&db, "owner", false).await;
        let recipient = make_user(&db, "recipient", false).await;
        let recipient_id = recipient.id;
        let grp = make_group(&db).await;
        let stor = make_storage(&db, owner.id, grp.id).await;

        let now = chrono::Utc::now().naive_utc();
        let dir_path = format!("st_dir_{}", uniq());
        let dir = entry::ActiveModel {
            storage_id: Set(stor.id),
            user_id: Set(owner.id),
            group_id: Set(grp.id),
            parent_id: Set(None),
            path: Set(dir_path.clone()),
            hash: Set(None),
            entry_type: Set(EntryType::Directory),
            notify: Set(false),
            skip_plugins: Set(false),
            size: Set(0),
            modified_at: Set(now),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert directory");

        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        entry::ActiveModel {
            storage_id: Set(stor.id),
            user_id: Set(owner.id),
            group_id: Set(grp.id),
            parent_id: Set(Some(dir.id)),
            path: Set(format!("{dir_path}/file.txt")),
            hash: Set(Some(hash.clone())),
            entry_type: Set(EntryType::File),
            notify: Set(false),
            skip_plugins: Set(false),
            size: Set(0),
            modified_at: Set(now),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert file");

        let ro_share = shared::ActiveModel {
            path_id: Set(dir.id),
            token: Set(None),
            can_write: Set(false),
            user_ids: Set(vec![recipient_id]),
            group_ids: Set(vec![]),
            expires_at: Set(None),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert read-only share");

        let recipient_token = bearer_token(&db, recipient).await;
        let app = storage_web::router().with_state(make_app_state(db.clone()));

        let req = Request::builder()
            .uri(format!("/share/{}/tags/file.txt", ro_share.id))
            .method("PUT")
            .header("Authorization", format!("Bearer {recipient_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({ "tags": [1] }).to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "{}",
            body_string(res).await
        );

        let rw_share = shared::ActiveModel {
            path_id: Set(dir.id),
            token: Set(None),
            can_write: Set(true),
            user_ids: Set(vec![recipient_id]),
            group_ids: Set(vec![]),
            expires_at: Set(None),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("insert writable share");

        let req = Request::builder()
            .uri(format!("/share/{}/tags/file.txt", rw_share.id))
            .method("PUT")
            .header("Authorization", format!("Bearer {recipient_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "tags": [4, 5] }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{}", body_string(res).await);

        let saved = meta::Entity::find_by_id(hash)
            .one(&db)
            .await
            .expect("query meta")
            .expect("meta row created");
        assert_eq!(saved.tags, vec![4, 5]);
    });
}
