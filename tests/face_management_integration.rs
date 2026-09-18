//! Integration tests for the contacts/faces management API (issues #26/#27):
//! contact CRUD, detected-face listing with per-user visibility, the human
//! assignment + confirm step, and the synchronous backfill re-match endpoint.
//!
//! Concurrency note: `make_app_state` wires a live (forgotten) `JobRunner`,
//! so the scoped re-match jobs queued by the confirm endpoints may execute
//! concurrently with the test body. Every assertion below is either on a
//! specific row's own fields or on a convergent end state (the queued job and
//! the explicit `POST /rematch` compute the same deterministic decision), so
//! the async jobs can't flip an assertion.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::entry::EntryType;
use byteburrow::entity::{contact, entry, face_reference, group, meta, storage, user};
use byteburrow::face_match::floats_to_bytes;
use byteburrow::job::JobRunner;
use byteburrow::migration::Migrator;
use byteburrow::plugin::PluginRegistry;
use byteburrow::web::{face as face_web, AppState};
use minijinja::Environment;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use serde_json::{json, Value};
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
/// Time-based (not a plain counter) so separate runs/processes against the
/// shared scratch database can't repeat a name — leftover rows from an
/// earlier run would otherwise collide on contact names and make re-match
/// scopes sweep another run's faces.
fn uniq() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    nanos.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed))
}

async fn make_user(db: &DatabaseConnection, tag: &str, admin: bool) -> user::Model {
    let name = format!("fm_{tag}_{}", uniq());
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
        name: Set(format!("fm_group_{}", uniq())),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group")
}

async fn make_storage(db: &DatabaseConnection, owner: i32, default_group: i32) -> storage::Model {
    storage::ActiveModel {
        name: Set(format!("fm_storage_{}", uniq())),
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
        path: Set(format!("fm_entry_{}", uniq())),
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

async fn make_contact_direct(db: &DatabaseConnection, name: &str) -> contact::Model {
    contact::ActiveModel {
        name: Set(name.to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert contact")
}

/// Insert an unconfirmed, unpinned face reference (a fresh machine suggestion).
async fn make_face(
    db: &DatabaseConnection,
    hash: &[u8],
    face_index: i16,
    embedding: &[f32],
    model_id: &str,
) -> face_reference::Model {
    face_reference::ActiveModel {
        hash: Set(hash.to_vec()),
        face_index: Set(face_index),
        contact_id: Set(None),
        bbox_x: Set(0),
        bbox_y: Set(0),
        bbox_w: Set(10),
        bbox_h: Set(10),
        embedding: Set(floats_to_bytes(embedding)),
        model_id: Set(model_id.to_string()),
        model_version: Set("1".to_string()),
        dim: Set(embedding.len() as i32),
        confirmed: Set(false),
        pinned: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert face_reference")
}

async fn make_meta(db: &DatabaseConnection, hash: &[u8]) -> meta::Model {
    meta::ActiveModel {
        hash: Set(hash.to_vec()),
        tags: Set(vec![]),
        keywords: Set(vec![]),
        custom: Set(json!({})),
    }
    .insert(db)
    .await
    .expect("insert meta")
}

async fn face_row(db: &DatabaseConnection, id: i32) -> face_reference::Model {
    face_reference::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query face")
        .expect("face row exists")
}

async fn meta_face_embeddings(db: &DatabaseConnection, hash: &[u8]) -> Value {
    meta::Entity::find_by_id(hash.to_vec())
        .one(db)
        .await
        .expect("query meta")
        .expect("meta row exists")
        .custom
        .get("face_embeddings")
        .cloned()
        .expect("face_embeddings in meta.custom")
}

async fn bearer_token(db: &DatabaseConnection, user: user::Model) -> String {
    Auth::new(user)
        .create_token(db, None, None)
        .await
        .expect("create token")
}

fn make_app_state(db: DatabaseConnection) -> Arc<AppState> {
    let plugin_dir = std::env::temp_dir().join(format!("byteburrow_fm_plugins_{}", uniq()));
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

/// One JSON API call through the mounted face router; returns status + body.
async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .uri(uri)
        .method(method)
        .header("Authorization", format!("Bearer {token}"));

    let req = match body {
        Some(v) => builder
            .header("Content-Type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };

    let res = app.clone().oneshot(req).await.expect("call router");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Extract the listed face ids out of a paginated `/refs` response.
fn listed_ids(body: &Value) -> Vec<i64> {
    body["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter_map(|r| r["id"].as_i64())
        .collect()
}

/// Walk every page of `/refs` and collect all face ids. A shared scratch DB
/// accumulates face rows across runs, so freshly inserted fixtures (random
/// UUID hashes, ordered anywhere in the list) need not be on the default
/// first page.
async fn all_ref_ids(app: &axum::Router, token: &str, query: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    let mut page = 1u64;
    loop {
        let uri = if query.is_empty() {
            format!("/refs?page={page}")
        } else {
            format!("/refs?{query}&page={page}")
        };
        let (st, body) = call(app, "GET", &uri, token, None).await;
        assert_eq!(st, StatusCode::OK, "uri={uri} body: {body}");
        let items = body["items"].as_array().expect("items array").clone();
        let total_pages = body["total_pages"].as_u64().expect("total_pages field");
        ids.extend(items.iter().filter_map(|r| r["id"].as_i64()));
        if page >= total_pages || items.is_empty() {
            break;
        }
        page += 1;
    }
    ids
}

#[test]
fn contact_crud_lifecycle() {
    runtime().block_on(async {
        let db = test_db().await;
        let admin = make_user(db, "admin", true).await;
        let pleb = make_user(db, "pleb", false).await;
        let admin_token = bearer_token(db, admin).await;
        let pleb_token = bearer_token(db, pleb).await;
        let app = face_web::router().with_state(make_app_state(db.clone()));

        let name = format!("fm-contact-{}", uniq());

        // Mutations are admin-only.
        let (st, _) = call(
            &app,
            "POST",
            "/contacts",
            &pleb_token,
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // Admin creates.
        let (st, body) = call(
            &app,
            "POST",
            "/contacts",
            &admin_token,
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "body: {body}");
        let id = body["id"].as_i64().expect("contact id") as i32;
        assert_eq!(body["name"].as_str(), Some(name.as_str()));

        // Duplicate name conflicts.
        let (st, _) = call(
            &app,
            "POST",
            "/contacts",
            &admin_token,
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // Whitespace-only name rejected.
        let (st, _) = call(
            &app,
            "POST",
            "/contacts",
            &admin_token,
            Some(json!({ "name": "   " })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // Rename to a fresh name works.
        let new_name = format!("{name}-renamed");
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/contacts/{id}"),
            &admin_token,
            Some(json!({ "name": new_name })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        // Rename onto a taken name conflicts.
        let name2 = format!("fm-contact2-{}", uniq());
        let (_, body2) = call(
            &app,
            "POST",
            "/contacts",
            &admin_token,
            Some(json!({ "name": name2 })),
        )
        .await;
        let id2 = body2["id"].as_i64().expect("second contact id") as i32;
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/contacts/{id2}"),
            &admin_token,
            Some(json!({ "name": new_name })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // Delete + 404 afterwards.
        let (st, _) = call(
            &app,
            "DELETE",
            &format!("/contacts/{id2}"),
            &admin_token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/contacts/{id2}"),
            &admin_token,
            Some(json!({ "name": "gone" })),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Anyone authenticated can list; the renamed contact is there with counts.
        let (st, body) = call(&app, "GET", "/contacts", &pleb_token, None).await;
        assert_eq!(st, StatusCode::OK);
        let me = body
            .as_array()
            .expect("contacts array")
            .iter()
            .find(|c| c["id"].as_i64() == Some(id as i64))
            .expect("renamed contact in list");
        assert_eq!(me["name"].as_str(), Some(new_name.as_str()));
        assert_eq!(me["confirmed_faces"].as_i64(), Some(0));
        assert_eq!(me["total_faces"].as_i64(), Some(0));
    });
}

#[test]
fn face_refs_listing_respects_storage_access() {
    runtime().block_on(async {
        let db = test_db().await;
        let admin = make_user(db, "visadmin", true).await;
        let owner = make_user(db, "visowner", false).await;
        let outsider = make_user(db, "visout", false).await;
        let grp = make_group(db).await;
        let stor = make_storage(db, owner.id, grp.id).await;

        // hash_visible has an entry in the owner's storage; hash_hidden has none.
        let hash_visible = uuid::Uuid::new_v4().as_bytes().to_vec();
        let hash_hidden = uuid::Uuid::new_v4().as_bytes().to_vec();
        make_entry(db, stor.id, owner.id, grp.id, Some(hash_visible.clone())).await;

        let model = format!("fm-vis-{}", uniq());
        let mut e = vec![0.0f32; 64];
        e[0] = 1.0;
        let f_visible = make_face(db, &hash_visible, 0, &e, &model).await;
        let f_hidden = make_face(db, &hash_hidden, 0, &e, &model).await;

        let owner_token = bearer_token(db, owner).await;
        let outsider_token = bearer_token(db, outsider).await;
        let admin_token = bearer_token(db, admin).await;
        let app = face_web::router().with_state(make_app_state(db.clone()));

        // Owner sees the accessible-hash face only.
        let ids = all_ref_ids(&app, &owner_token, "").await;
        assert!(ids.contains(&(f_visible.id as i64)));
        assert!(
            !ids.contains(&(f_hidden.id as i64)),
            "faces of inaccessible files must not be listed"
        );

        // A user with access to nothing sees an empty list.
        let ids = all_ref_ids(&app, &outsider_token, "").await;
        assert!(ids.is_empty());

        // Admin sees both.
        let ids = all_ref_ids(&app, &admin_token, "").await;
        assert!(ids.contains(&(f_visible.id as i64)));
        assert!(ids.contains(&(f_hidden.id as i64)));

        // The unassigned filter keeps the review queue shape.
        let ids = all_ref_ids(&app, &admin_token, "unassigned=true").await;
        assert!(ids.contains(&(f_visible.id as i64)));

        // Filtering by contact narrows to labeled faces (none of ours).
        let (st, body) = call(
            &app,
            "GET",
            "/refs?contact_id=123456789",
            &admin_token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(!listed_ids(&body).contains(&(f_visible.id as i64)));
    });
}

/// The #26 + #27 end-to-end loop: confirm one face as an exemplar through the
/// API, then re-match — an already-processed face of the same model gets
/// retroactively assigned and the file's `meta.custom.face_embeddings` array
/// is rewritten.
#[test]
fn confirm_then_rematch_backfills_assignments() {
    runtime().block_on(async {
        let db = test_db().await;
        let admin = make_user(db, "cadmin", true).await;
        let token = bearer_token(db, admin).await;
        let app = face_web::router().with_state(make_app_state(db.clone()));

        let contact = make_contact_direct(db, &format!("fm-alice-{}", uniq())).await;
        let model = format!("fm-confirm-{}", uniq());
        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        make_meta(db, &hash).await;

        // Face 0 will become the exemplar; face 1 is an already-processed
        // face that used to have no comparable exemplar (unassigned).
        let mut e0 = vec![0.0f32; 64];
        e0[0] = 1.0;
        let mut e1 = e0.clone();
        e1[0] = 0.99;
        e1[1] = 0.01;
        let face0 = make_face(db, &hash, 0, &e0, &model).await;
        let face1 = make_face(db, &hash, 1, &e1, &model).await;

        // Confirm face 0 for the contact (queues a scoped re-match job).
        let (st, body) = call(
            &app,
            "POST",
            &format!("/refs/{}/confirm", face0.id),
            &token,
            Some(json!({ "contact_id": contact.id })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "body: {body}");

        let f0 = face_row(db, face0.id).await;
        assert!(f0.confirmed && f0.pinned);
        assert_eq!(f0.contact_id, Some(contact.id));

        // Explicit synchronous re-match. It must be idempotent with the
        // scoped re-match job the confirm endpoint queued — that job may
        // already have run by now, in which case `assigned` is legitimately 0;
        // the row/meta assertions below prove the backfill either way.
        let (st, body) = call(&app, "POST", "/rematch", &token, None).await;
        assert_eq!(st, StatusCode::OK, "body: {body}");

        // The already-processed face is retroactively Alice's — as a
        // suggestion, not an exemplar.
        let f1 = face_row(db, face1.id).await;
        assert_eq!(f1.contact_id, Some(contact.id), "backfill must assign");
        assert!(!f1.confirmed);
        assert!(!f1.pinned);

        // And the file's meta array reflects both faces.
        assert_eq!(
            meta_face_embeddings(db, &hash).await,
            json!([contact.id, contact.id])
        );
    });
}

#[test]
fn confirm_withdrawal_and_assignment_clear_semantics() {
    runtime().block_on(async {
        let db = test_db().await;
        let admin = make_user(db, "wadmin", true).await;
        let pleb = make_user(db, "wpleb", false).await;
        let token = bearer_token(db, admin).await;
        let pleb_token = bearer_token(db, pleb).await;
        let app = face_web::router().with_state(make_app_state(db.clone()));

        let contact = make_contact_direct(db, &format!("fm-bob-{}", uniq())).await;
        let model = format!("fm-withdraw-{}", uniq());
        let hash = uuid::Uuid::new_v4().as_bytes().to_vec();
        make_meta(db, &hash).await;

        let mut e0 = vec![0.0f32; 64];
        e0[0] = 1.0;
        let mut e1 = e0.clone();
        e1[2] = 1.0; // orthogonal — never matches e0
        let face0 = make_face(db, &hash, 0, &e0, &model).await;
        let face1 = make_face(db, &hash, 1, &e1, &model).await;

        // Confirming requires a contact one way or another.
        let (st, _) = call(
            &app,
            "POST",
            &format!("/refs/{}/confirm", face0.id),
            &token,
            Some(json!({})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // Unknown face / unknown contact.
        let (st, _) = call(
            &app,
            "POST",
            "/refs/999999999/confirm",
            &token,
            Some(json!({ "contact_id": contact.id })),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = call(
            &app,
            "POST",
            &format!("/refs/{}/confirm", face0.id),
            &token,
            Some(json!({ "contact_id": 999999999 })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // Confirm both faces, then withdraw the first: the label is kept as a
        // pinned human label, only exemplar status is dropped.
        for f in [&face0, &face1] {
            let (st, _) = call(
                &app,
                "POST",
                &format!("/refs/{}/confirm", f.id),
                &token,
                Some(json!({ "contact_id": contact.id })),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
        }
        let (st, _) = call(
            &app,
            "DELETE",
            &format!("/refs/{}/confirm", face0.id),
            &token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let f0 = face_row(db, face0.id).await;
        assert!(!f0.confirmed, "exemplar status withdrawn");
        assert!(f0.pinned, "human label stays pinned");
        assert_eq!(f0.contact_id, Some(contact.id));

        // Clearing the label of a confirmed face unconfirms it (an exemplar
        // without a contact is meaningless) and unpins it.
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/refs/{}/assignment", face1.id),
            &token,
            Some(json!({ "contact_id": null })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let f1 = face_row(db, face1.id).await;
        assert_eq!(f1.contact_id, None);
        assert!(!f1.confirmed, "clearing the label must unconfirm");
        assert!(!f1.pinned);

        // A plain human assignment pins the label and syncs meta.
        let (st, body) = call(
            &app,
            "PUT",
            &format!("/refs/{}/assignment", face1.id),
            &token,
            Some(json!({ "contact_id": contact.id })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "body: {body}");
        let f1 = face_row(db, face1.id).await;
        assert_eq!(f1.contact_id, Some(contact.id));
        assert!(f1.pinned);
        assert!(!f1.confirmed, "assignment alone does not confirm");
        assert_eq!(
            meta_face_embeddings(db, &hash).await,
            json!([contact.id, contact.id])
        );

        // Non-admins cannot mutate faces.
        let (st, _) = call(
            &app,
            "PUT",
            &format!("/refs/{}/assignment", face1.id),
            &pleb_token,
            Some(json!({ "contact_id": null })),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = call(&app, "POST", "/rematch", &pleb_token, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    });
}
