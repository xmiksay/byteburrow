//! Integration tests for `byteburrow::web::require_storage_access` — the
//! authorization gate that guards the storage file-content endpoints against
//! IDOR (issue #4). Verifies a non-owner is denied while the owner, a
//! group member, and admins are granted.
//!
//! Shares the process-lifetime runtime/DB setup pattern with
//! `auth_integration.rs` (see that file for why a single runtime is reused).

use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::{group, group_user, storage, user};
use byteburrow::migration::Migrator;
use byteburrow::web::{require_storage_access, ApiError};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;
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
    let name = format!("sa_{tag}_{}", uniq());
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
        name: Set(format!("sa_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("sa_storage_{}", uniq())),
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

#[test]
fn owner_is_granted_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let owner = make_user(db, "owner", false).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;

        require_storage_access(&Auth::new(owner), &stor, db)
            .await
            .expect("owner must be allowed");
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

        let err = require_storage_access(&Auth::new(stranger), &stor, db)
            .await
            .expect_err("non-owner must be denied");
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

        group_user::ActiveModel {
            user_id: Set(member.id),
            group_id: Set(grp.id),
            admin: Set(false),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert group membership");

        require_storage_access(&Auth::new(member), &stor, db)
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

        require_storage_access(&Auth::new(admin), &stor, db)
            .await
            .expect("admin must be allowed");
    });
}
