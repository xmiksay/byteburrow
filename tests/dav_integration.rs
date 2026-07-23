//! Integration tests for the WebDAV gateway (`src/web/dav`).
//!
//! These stand up the real Axum DAV router against a real Postgres (for auth +
//! storage lookup) and a temp directory (for the storage root), then exercise
//! the protocol surface: OPTIONS, PROPFIND, GET/HEAD, PUT, MKCOL, DELETE,
//! MOVE, LOCK/UNLOCK. CalDAV/CardDAV REPORT coverage is included as a smoke
//! test.
//!
//! Requires `DATABASE_URL` to point at a scratch database (migrations run
//! automatically on first use). Defaults to the local docker-compose DB.

use std::sync::{Arc, Once, OnceLock};
use std::time::SystemTime;

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderName, Method, Request, StatusCode};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;
use tokio::sync::OnceCell;
use tower::ServiceExt;

use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::{storage, user};
use byteburrow::migration::Migrator;
use byteburrow::web::dav;
use byteburrow::web::AppState;

// Shared process-lifetime runtime + DB, mirroring `auth_integration.rs`.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();
static CONFIG_INIT: Once = Once::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build shared test runtime"))
}

async fn test_db() -> &'static DatabaseConnection {
    DB.get_or_init(|| async {
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

async fn create_test_user(db: &DatabaseConnection, username: &str) -> (user::Model, String) {
    let password = format!("{username}-pw");
    let u = user::ActiveModel {
        name: Set(username.to_string()),
        description: Set(None),
        username: Set(username.to_string()),
        password: Set(Auth::hash_string(&password)),
        enabled: Set(true),
        admin: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert test user");
    (u, password)
}

/// Create a group and return its id. Storages reference a default_group via
/// FK, so we need a real row.
async fn create_test_group(db: &DatabaseConnection, name: &str) -> i32 {
    use byteburrow::entity::group;
    let g = group::ActiveModel {
        name: Set(name.to_string()),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert test group");
    g.id
}

/// A fresh temp-dir-backed storage owned by `user_id`.
async fn create_test_storage(
    db: &DatabaseConnection,
    user_id: i32,
    group_id: i32,
    label: &str,
) -> storage::Model {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("byteburrow_dav_it_{label}_{unique}"));
    tokio::fs::create_dir_all(&root).await.unwrap();
    storage::ActiveModel {
        name: Set(format!("dav_it_{label}")),
        description: Set(None),
        path: Set(root.to_string_lossy().into_owned()),
        default_user: Set(user_id),
        default_group: Set(group_id),
        ignore_patterns: Set(String::new()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert test storage")
}

/// Build a minimal `AppState` with a no-op job sender (the DAV handlers
/// don't enqueue jobs).
fn make_state(db: DatabaseConnection) -> Arc<AppState> {
    use minijinja::Environment;
    let jinja = Environment::new();
    let (_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    // JobSender is a type alias for mpsc::UnboundedSender<Job>; build one
    // directly. The receiver is intentionally dropped — no job runner spins
    // up in the test.
    let job_sender = _tx;
    Arc::new(AppState {
        db,
        config: Config::get().as_ref().clone(),
        jinja,
        job_sender,
    })
}

/// Build a Basic-auth `Authorization` header value for `user:pass`.
///
/// Hand-rolled base64 to avoid pulling the `base64` crate into dev-deps
/// (master slimmed it out of the main deps).
fn basic_auth(user: &str, pass: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = format!("{user}:{pass}").into_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(T[((n >> 18) & 0x3f) as usize] as char);
        out.push(T[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(T[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    format!("Basic {out}")
}

/// Issue a request to the DAV router and return `(status, body_bytes)`.
async fn dav_request(
    state: Arc<AppState>,
    method: &str,
    uri: &str,
    auth: Option<(&str, &str)>,
    extra_headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let app = dav::router().with_state(state);
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap())
        .uri(uri);
    if let Some((u, p)) = auth {
        req = req.header(header::AUTHORIZATION, basic_auth(u, p));
    }
    for (k, v) in extra_headers {
        req = req.header(HeaderName::from_bytes(k.as_bytes()).unwrap(), *v);
    }
    let req = req.body(Body::from(body)).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, body.to_vec())
}

#[test]
fn options_advertises_dav_versions() {
    runtime().block_on(async {
        let db = test_db().await;
        let state = make_state(db.clone());
        let (status, body) = dav_request(state, "OPTIONS", "/dav", None, &[], Vec::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        // DAV header is checked via the response headers, but oneshot consumed
        // them into the Response — re-issue to inspect headers.
        let app = dav::router().with_state(make_state(db.clone()));
        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/dav")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp
            .headers()
            .get("dav")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("calendar-access"));
    });
}

#[test]
fn webdav_put_get_roundtrip() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_put_get").await;
        let group_id = create_test_group(db, "dav_put_get_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "put_get").await;
        let state = make_state(db.clone());

        let uri = format!("/dav/storage/{}/hello.txt", storage.id);
        let auth = (user.username.as_str(), password.as_str());

        // PUT a file.
        let (status, _) = dav_request(
            state.clone(),
            "PUT",
            &uri,
            Some(auth),
            &[],
            b"hello webdav".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // GET it back.
        let (status, body) =
            dav_request(state.clone(), "GET", &uri, Some(auth), &[], Vec::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body, b"hello webdav");
    });
}

#[test]
fn webdav_propfind_depth1_lists_children() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_propfind").await;
        let group_id = create_test_group(db, "dav_propfind_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "propfind").await;
        let state = make_state(db.clone());
        let auth = (user.username.as_str(), password.as_str());

        // Seed two files.
        for name in ["a.txt", "b.txt"] {
            let uri = format!("/dav/storage/{}/{}", storage.id, name);
            dav_request(state.clone(), "PUT", &uri, Some(auth), &[], b"x".to_vec()).await;
        }

        // PROPFIND the storage root.
        let (status, body) = dav_request(
            state.clone(),
            "PROPFIND",
            &format!("/dav/storage/{}", storage.id),
            Some(auth),
            &[("depth", "1")],
            b"<?xml version=\"1.0\"?><D:propfind xmlns:D=\"DAV:\"><D:allprop/></D:propfind>"
                .to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let xml = String::from_utf8_lossy(&body);
        assert!(xml.contains("multistatus"));
        assert!(xml.contains("a.txt"));
        assert!(xml.contains("b.txt"));
        assert!(xml.contains("resourcetype"));
    });
}

#[test]
fn webdav_mkcol_and_delete() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_mkcol").await;
        let group_id = create_test_group(db, "dav_mkcol_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "mkcol").await;
        let state = make_state(db.clone());
        let auth = (user.username.as_str(), password.as_str());

        let dir_uri = format!("/dav/storage/{}/subdir", storage.id);

        // MKCOL a directory.
        let (status, _) = dav_request(
            state.clone(),
            "MKCOL",
            &dir_uri,
            Some(auth),
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // PUT a file into it.
        let file_uri = format!("{dir_uri}/inner.txt");
        let (status, _) = dav_request(
            state.clone(),
            "PUT",
            &file_uri,
            Some(auth),
            &[],
            b"inside".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // DELETE the directory (recursive).
        let (status, _) = dav_request(
            state.clone(),
            "DELETE",
            &dir_uri,
            Some(auth),
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // GET the deleted file → 404.
        let (status, _) =
            dav_request(state.clone(), "GET", &file_uri, Some(auth), &[], Vec::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    });
}

#[test]
fn webdav_lock_unlock_roundtrip() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_lock").await;
        let group_id = create_test_group(db, "dav_lock_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "lock").await;
        let state = make_state(db.clone());
        let auth = (user.username.as_str(), password.as_str());
        let uri = format!("/dav/storage/{}/locked.txt", storage.id);

        let lockbody = b"<?xml version=\"1.0\"?><D:lockinfo xmlns:D=\"DAV:\">\
                         <D:lockscope><D:exclusive/></D:lockscope>\
                         <D:locktype><D:write/></D:locktype>\
                         <D:owner>test</D:owner></D:lockinfo>";
        let (status, body) = dav_request(
            state.clone(),
            "LOCK",
            &uri,
            Some(auth),
            &[],
            lockbody.to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let xml = String::from_utf8_lossy(&body);
        // Extract the lock token from the body.
        let token_start = xml.find("<D:href>opaquelocktoken:").unwrap();
        let token_end = token_start + "<D:href>".len();
        let after = &xml[token_end..];
        let end = after.find("</D:href>").unwrap();
        let token = &after[..end];

        // UNLOCK with the token.
        let (status, _) = dav_request(
            state.clone(),
            "UNLOCK",
            &uri,
            Some(auth),
            &[("lock-token", &format!("<{token}>"))],
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    });
}

#[test]
fn caldav_mkcalendar_and_report() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_cal").await;
        let group_id = create_test_group(db, "dav_cal_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "cal").await;
        let state = make_state(db.clone());
        let auth = (user.username.as_str(), password.as_str());

        let cal_uri = format!("/dav/storage/{}/personal", storage.id);

        // MKCALENDAR.
        let (status, _) =
            dav_request(state.clone(), "MKCALENDAR", &cal_uri, Some(auth), &[], Vec::new()).await;
        assert_eq!(status, StatusCode::CREATED);

        // PUT an event.
        let event_uri = format!("{cal_uri}/abc.ics");
        let ics = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:abc\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let (status, _) = dav_request(
            state.clone(),
            "PUT",
            &event_uri,
            Some(auth),
            &[],
            ics.to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // REPORT calendar-query.
        let report = b"<?xml version=\"1.0\"?><C:calendar-query xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                       <D:prop><D:getetag/><C:calendar-data/></D:prop></C:calendar-query>";
        let (status, body) =
            dav_request(state.clone(), "REPORT", &cal_uri, Some(auth), &[], report.to_vec()).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let xml = String::from_utf8_lossy(&body);
        assert!(xml.contains("abc.ics"));
        assert!(xml.contains("BEGIN:VCALENDAR"));
    });
}

#[test]
fn carddav_mkcol_addressbook_and_report() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, password) = create_test_user(db, "dav_card").await;
        let group_id = create_test_group(db, "dav_card_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "card").await;
        let state = make_state(db.clone());
        let auth = (user.username.as_str(), password.as_str());

        let ab_uri = format!("/dav/storage/{}/contacts", storage.id);

        // Create the address book as a plain MKCOL first (directory), then
        // drop the marker file via PUT so REPORT recognizes it.
        let (status, _) =
            dav_request(state.clone(), "MKCOL", &ab_uri, Some(auth), &[], Vec::new()).await;
        assert_eq!(status, StatusCode::CREATED);
        let marker_uri = format!("{ab_uri}/.carddav-addressbook");
        dav_request(
            state.clone(),
            "PUT",
            &marker_uri,
            Some(auth),
            &[],
            Vec::new(),
        )
        .await;

        // PUT a vCard.
        let vcf_uri = format!("{ab_uri}/joe.vcf");
        let vcf = b"BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Joe\r\nEND:VCARD\r\n";
        let (status, _) = dav_request(
            state.clone(),
            "PUT",
            &vcf_uri,
            Some(auth),
            &[],
            vcf.to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // REPORT addressbook-query.
        let report = b"<?xml version=\"1.0\"?><C:addressbook-query xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:carddav\">\
                       <D:prop><C:address-data/></D:prop></C:addressbook-query>";
        let (status, body) =
            dav_request(state.clone(), "REPORT", &ab_uri, Some(auth), &[], report.to_vec()).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let xml = String::from_utf8_lossy(&body);
        assert!(xml.contains("joe.vcf"));
        assert!(xml.contains("BEGIN:VCARD"));
    });
}

#[test]
fn webdav_unauthenticated_rejected() {
    runtime().block_on(async {
        let db = test_db().await;
        let (user, _) = create_test_user(db, "dav_unauth").await;
        let group_id = create_test_group(db, "dav_unauth_grp").await;
        let storage = create_test_storage(db, user.id, group_id, "unauth").await;
        let state = make_state(db.clone());

        // No credentials → 401.
        let (status, _) = dav_request(
            state,
            "GET",
            &format!("/dav/storage/{}/x.txt", storage.id),
            None,
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    });
}
