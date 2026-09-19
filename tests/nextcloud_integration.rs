//! Integration tests for the Nextcloud (WebDAV) storage backend — issue #1,
//! ADR 0008.
//!
//! A mock WebDAV server (axum, in-process) answers `PROPFIND`/`GET`/`PUT`/
//! `MKCOL`/`DELETE`/`MOVE`/`COPY` for `/remote.php/dav/files/<user>/...` and
//! backs an in-memory tree shared with the assertions. The `Storage` wrapper
//! is then driven through its public seam methods — the same calls the web
//! handlers and job runner make — against a real DB row.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, Once};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use sea_orm::{DatabaseConnection, Set};
use tokio::sync::OnceCell;

use byteburrow::auth::Auth;
use byteburrow::config::Config;
use byteburrow::entity::{group, storage, user};
use byteburrow::storage::{DirectoryEntry, Storage as StorageWrapper, BACKEND_NEXTCLOUD};
use sea_orm_migration::MigratorTrait;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();
static COUNTER: AtomicU32 = AtomicU32::new(0);

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build shared test runtime"))
}

fn uniq() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
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
        byteburrow::migration::Migrator::up(&db, None)
            .await
            .expect("run migrations");
        db
    })
    .await
}

// ---------------------------------------------------------------------------
// Mock WebDAV server
// ---------------------------------------------------------------------------

/// An in-memory DAV tree: `path (no leading slash) -> (is_dir, bytes)`.
type Tree = Arc<Mutex<HashMap<String, (bool, Vec<u8>)>>>;

#[derive(Clone)]
struct MockDav {
    tree: Tree,
    /// Calls recorded as `(METHOD, path)`.
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl MockDav {
    /// Relative path of a request URI within the DAV files namespace, with
    /// the username segment stripped — the tree is storage-root-relative
    /// (exactly what `NextcloudClient` sees after relativizing hrefs).
    fn rel_of(uri: &Uri) -> String {
        let Some(rest) = uri
            .path()
            .strip_prefix("/remote.php/dav/files/")
            .or_else(|| uri.path().strip_prefix("/remote.php/dav/files"))
        else {
            return String::new();
        };
        // First segment is the username; everything after is the sub-path.
        match rest.split_once('/') {
            Some((_user, sub)) => sub.trim_matches('/').to_string(),
            // The DAV base itself (or a bare username collection) → root.
            None => String::new(),
        }
    }

    async fn handle(self, req: Request<Body>) -> Response {
        let method = req.method().clone();
        let uri = req.uri().clone();
        let headers = req.headers().clone();
        let body = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap_or_default()
            .to_vec();

        let rel = Self::rel_of(&uri);
        self.seen
            .lock()
            .unwrap()
            .push((method.to_string(), rel.clone()));

        // Basic auth must be present (the client always sends it).
        let authed = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("Basic "))
            .unwrap_or(false);
        if !authed {
            return StatusCode::UNAUTHORIZED.into_response();
        }

        match method.as_str() {
            "GET" => {
                let tree = self.tree.lock().unwrap();
                match tree.get(&rel) {
                    Some((false, data)) => (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/octet-stream")],
                        data.clone(),
                    )
                        .into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
            "PUT" => {
                self.tree.lock().unwrap().insert(rel, (false, body));
                StatusCode::CREATED.into_response()
            }
            "MKCOL" => {
                let mut tree = self.tree.lock().unwrap();
                match tree.get_mut(&rel) {
                    Some(_) => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                    None => {
                        tree.insert(rel, (true, Vec::new()));
                        StatusCode::CREATED.into_response()
                    }
                }
            }
            "DELETE" => {
                let mut tree = self.tree.lock().unwrap();
                let before = tree.len();
                tree.retain(|k, _| !k.starts_with(&format!("{rel}/")) && *k != rel);
                if tree.len() == before {
                    StatusCode::NOT_FOUND.into_response()
                } else {
                    StatusCode::NO_CONTENT.into_response()
                }
            }
            "MOVE" | "COPY" => {
                // Destination header: absolute URL; take its path.
                let dest = headers
                    .get("destination")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|raw| raw.parse::<Uri>().ok())
                    .map(|u| Self::rel_of(&u))
                    .unwrap_or_default();

                let mut tree = self.tree.lock().unwrap();
                let Some((is_dir, data)) = tree.get(&rel).cloned() else {
                    return StatusCode::NOT_FOUND.into_response();
                };

                // Copy the whole subtree for directories.
                let prefix = format!("{rel}/");
                let children: Vec<(String, (bool, Vec<u8>))> = tree
                    .iter()
                    .filter(|(k, _)| is_dir && k.starts_with(&prefix))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                tree.insert(dest.clone(), (is_dir, data));
                for (k, v) in children {
                    let child_dest = format!("{}/{}", dest, k.strip_prefix(&prefix).unwrap());
                    tree.insert(child_dest, v);
                }

                if method.as_str() == "MOVE" {
                    tree.retain(|k, _| !k.starts_with(&prefix) && *k != rel);
                }
                StatusCode::CREATED.into_response()
            }
            "PROPFIND" => {
                // Depth 0 → self only; Depth 1 → self + children.
                let depth = headers
                    .get("depth")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("1")
                    .to_string();

                let tree = self.tree.lock().unwrap();
                // The DAV base itself always "exists".
                let self_exists = rel.is_empty() || tree.contains_key(&rel);
                if !self_exists {
                    return StatusCode::NOT_FOUND.into_response();
                }

                let mut xml =
                    String::from(r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">"#);

                let self_is_dir =
                    rel.is_empty() || tree.get(&rel).map(|(d, _)| *d).unwrap_or(false);
                let href = if rel.is_empty() {
                    "/remote.php/dav/files/owner/".to_string()
                } else if self_is_dir {
                    format!("/remote.php/dav/files/owner/{rel}/")
                } else {
                    format!("/remote.php/dav/files/owner/{rel}")
                };
                let len = tree.get(&rel).map(|(_, b)| b.len()).unwrap_or(0);
                xml.push_str(&response_xml(&href, self_is_dir, len as u64));

                if depth != "0" {
                    let prefix = if rel.is_empty() {
                        String::new()
                    } else {
                        format!("{rel}/")
                    };
                    for (k, (is_dir, data)) in tree.iter() {
                        let direct_child = if prefix.is_empty() {
                            !k.contains('/')
                        } else {
                            k.starts_with(&prefix) && !k[prefix.len()..].contains('/')
                        };
                        if !direct_child {
                            continue;
                        }
                        let child_href = if *is_dir {
                            format!("/remote.php/dav/files/owner/{k}/")
                        } else {
                            format!("/remote.php/dav/files/owner/{k}")
                        };
                        xml.push_str(&response_xml(&child_href, *is_dir, data.len() as u64));
                    }
                }

                xml.push_str("</d:multistatus>");
                (
                    StatusCode::MULTI_STATUS,
                    [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                    xml,
                )
                    .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }
}

fn response_xml(href: &str, is_dir: bool, len: u64) -> String {
    let rt = if is_dir {
        "<d:resourcetype><d:collection/></d:resourcetype>"
    } else {
        "<d:resourcetype/>"
    };
    let len_prop = if is_dir {
        ""
    } else {
        &format!("<d:getcontentlength>{len}</d:getcontentlength>")
    };
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{rt}\
<d:getlastmodified>Mon, 01 Jan 2024 12:00:00 GMT</d:getlastmodified>{len_prop}\
</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn make_owner(db: &DatabaseConnection, tag: &str) -> (user::Model, group::Model) {
    let n = uniq();
    let u = user::ActiveModel {
        name: Set(format!("nc_{tag}_{n}")),
        description: Set(None),
        username: Set(format!("nc_{tag}_{n}")),
        password: Set(Auth::hash_string("pw")),
        enabled: Set(true),
        admin: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert user");

    let g = group::ActiveModel {
        name: Set(format!("nc_group_{tag}_{n}")),
        description: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert group");
    (u, g)
}

async fn make_remote_storage(
    db: &DatabaseConnection,
    base_url: &str,
    user_id: i32,
    group_id: i32,
) -> StorageWrapper {
    let s = storage::ActiveModel {
        name: Set(format!("nc_storage_{}", uniq())),
        description: Set(None),
        // `path` is derived and stored by the create handler; tests write it
        // directly to exercise the same shape.
        path: Set(format!("{base_url}/remote.php/dav/files/owner")),
        default_user: Set(user_id),
        default_group: Set(group_id),
        ignore_patterns: Set(String::new()),
        backend: Set(BACKEND_NEXTCLOUD.to_string()),
        remote_url: Set(Some(base_url.to_string())),
        remote_username: Set(Some("owner".to_string())),
        remote_password: Set(Some("app-password".to_string())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert nextcloud storage");
    StorageWrapper::new(s)
}

/// Start the mock DAV server on an ephemeral port; returns its base URL.
/// The caller's `Tree` Arc is shared with the server, so assertions can
/// inspect/seed it directly.
async fn spawn_mock_dav(tree: Tree) -> (String, MockDav) {
    let dav = MockDav {
        tree,
        seen: Arc::new(Mutex::new(Vec::new())),
    };

    let app = Router::new()
        // The collection root (empty path is not matched by /*path).
        .route(
            "/remote.php/dav/files",
            any(
                |State(state): State<MockDav>, req: Request<Body>| async move {
                    state.handle(req).await
                },
            ),
        )
        .route(
            "/remote.php/dav/files/*path",
            any(
                |State(state): State<MockDav>, req: Request<Body>| async move {
                    state.handle(req).await
                },
            ),
        )
        .with_state(dav.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), dav)
}

use sea_orm::ActiveModelTrait;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip every seam against the mock server: list, stat, read, write,
/// mkdir, rename, delete.
#[test]
fn nextcloud_backend_full_round_trip() {
    runtime().block_on(async {
        let db = test_db().await;
        let (u, g) = make_owner(db, "rt").await;

        let tree: Tree = Arc::new(Mutex::new(HashMap::new()));
        tree.lock()
            .unwrap()
            .insert("docs/readme.txt".to_string(), (false, b"hello".to_vec()));
        tree.lock()
            .unwrap()
            .insert("docs".to_string(), (true, Vec::new()));
        tree.lock()
            .unwrap()
            .insert("logo.png".to_string(), (false, vec![1u8, 2, 3, 4]));

        let (base_url, _dav) = spawn_mock_dav(tree.clone()).await;
        let storage = make_remote_storage(db, &base_url, u.id, g.id).await;

        // ── list (props: path, collection flag, size) ──────────────────
        let entries = storage.list_directory_fs("docs").await.expect("list docs");
        let by_path: HashMap<String, DirectoryEntry> =
            entries.into_iter().map(|e| (e.path.clone(), e)).collect();
        let readme = by_path.get("docs/readme.txt").expect("readme listed");
        assert_eq!(readme.size, 5, "getcontentlength must surface as size");
        assert!(!matches!(
            readme.entry_type,
            byteburrow::entity::entry::EntryType::Directory
        ));

        let root = storage.list_directory_fs("").await.expect("list root");
        assert!(
            root.len() == 2,
            "root lists docs/ and logo.png, got {root:?}"
        );

        // ── stat ───────────────────────────────────────────────────────
        let stat = storage.stat_entry("logo.png").await.expect("stat logo");
        assert!(!stat.is_dir());
        assert_eq!(stat.size, 4);

        // ── read round-trip ────────────────────────────────────────────
        let data = storage.read_file("docs/readme.txt").await.expect("read");
        assert_eq!(data, b"hello");

        // ── write (creates missing parents) ────────────────────────────
        storage
            .save_file("newdir/nested.txt", b"written over dav")
            .await
            .expect("write via PUT");
        assert_eq!(
            tree.lock().unwrap().get("newdir/nested.txt").unwrap().1,
            b"written over dav"
        );
        assert!(
            tree.lock().unwrap().contains_key("newdir"),
            "parent MKCOL'd"
        );

        // ── mkdir ──────────────────────────────────────────────────────
        storage.create_directory("albums").await.expect("MKCOL");
        assert!(
            tree.lock().unwrap().get("albums").unwrap().0,
            "is a collection"
        );

        // ── rename (MOVE) ──────────────────────────────────────────────
        storage
            .rename_entry("docs/readme.txt", "docs/readme-v2.txt")
            .await
            .expect("MOVE");
        assert!(!tree.lock().unwrap().contains_key("docs/readme.txt"));
        assert!(tree.lock().unwrap().contains_key("docs/readme-v2.txt"));

        // ── delete ─────────────────────────────────────────────────────
        storage.remove_entry("logo.png").await.expect("DELETE");
        assert!(!tree.lock().unwrap().contains_key("logo.png"));
    });
}

/// `..` traversal must be rejected client-side, before any URL is built.
#[test]
fn nextcloud_backend_rejects_traversal() {
    runtime().block_on(async {
        let db = test_db().await;
        let (u, g) = make_owner(db, "trav").await;

        let tree: Tree = Arc::new(Mutex::new(HashMap::new()));
        let (base_url, _dav) = spawn_mock_dav(tree.clone()).await;
        let storage = make_remote_storage(db, &base_url, u.id, g.id).await;

        for op_path in ["../pwned.txt", "docs/../../pwned.txt", ".."] {
            let err = storage.read_file(op_path).await.unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::PermissionDenied,
                "read {op_path}"
            );
            let err = storage.save_file(op_path, b"x").await.unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::PermissionDenied,
                "write {op_path}"
            );
            let err = storage.remove_entry(op_path).await.unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::PermissionDenied,
                "delete {op_path}"
            );
        }

        // And nothing ever reached the server.
        assert!(
            _dav.seen.lock().unwrap().is_empty(),
            "traversal must be rejected before any HTTP call"
        );
    });
}

/// A missing remote path surfaces as `NotFound` — the same io kind the web
/// layer maps to 404 for local storages.
#[test]
fn nextcloud_backend_missing_path_is_not_found() {
    runtime().block_on(async {
        let db = test_db().await;
        let (u, g) = make_owner(db, "404").await;

        let tree: Tree = Arc::new(Mutex::new(HashMap::new()));
        let (base_url, _dav) = spawn_mock_dav(tree.clone()).await;
        let storage = make_remote_storage(db, &base_url, u.id, g.id).await;

        let err = storage.read_file("does-not-exist.txt").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        let err = storage.list_directory_fs("no-such-dir").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        assert!(!storage.entry_exists("does-not-exist.txt").await);
    });
}

/// `validate_remote_storage` (the storage create/update gate) succeeds
/// against a reachable server and fails clearly against a dead one.
#[test]
fn nextcloud_validation_probes_the_dav_base() {
    runtime().block_on(async {
        let tree: Tree = Arc::new(Mutex::new(HashMap::new()));
        let (base_url, _dav) = spawn_mock_dav(tree.clone()).await;

        byteburrow::storage::validate_remote_storage(&base_url, "owner", "app-password")
            .await
            .expect("live server validates");

        // A port nothing listens on.
        let err = byteburrow::storage::validate_remote_storage(
            "http://127.0.0.1:9",
            "owner",
            "app-password",
        )
        .await
        .expect_err("dead server must fail");
        assert!(err.to_string().contains("Cannot reach Nextcloud"));
    });
}

/// Local-backend behavior must be untouched: the same seam methods on a
/// `local` storage still hit the filesystem.
#[test]
fn local_backend_seams_still_use_filesystem() {
    runtime().block_on(async {
        let db = test_db().await;
        let (u, g) = make_owner(db, "local").await;

        let root =
            std::env::temp_dir().join(format!("bb_nc_local_{}_{}", std::process::id(), uniq()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("a.txt"), b"local-bytes")
            .await
            .unwrap();

        let s = storage::ActiveModel {
            name: Set(format!("local_storage_{}", uniq())),
            description: Set(None),
            path: Set(root.to_string_lossy().into_owned()),
            default_user: Set(u.id),
            default_group: Set(g.id),
            ignore_patterns: Set(String::new()),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("insert local storage");
        let storage = StorageWrapper::new(s);

        assert!(storage.is_local());
        // `get_full_path` stays infallible for local storages' existing
        // callers (Result-wrapped now; must resolve).
        assert!(storage.get_full_path("a.txt").unwrap().ends_with("a.txt"));

        let data = storage.read_file("a.txt").await.expect("local read");
        assert_eq!(data, b"local-bytes");

        let stat = storage.stat_entry("a.txt").await.expect("local stat");
        assert_eq!(stat.size, b"local-bytes".len() as u64);

        // Local traversal rejection still applies: an existing target outside
        // the root must be rejected (canonicalize succeeds, containment fails).
        let outside = std::env::temp_dir().join(format!(
            "bb_nc_escape_{}_{}.txt",
            std::process::id(),
            uniq()
        ));
        tokio::fs::write(&outside, b"outside").await.unwrap();
        let rel_escape = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        let err = storage.read_file(&rel_escape).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        tokio::fs::remove_file(&outside).await.ok();

        tokio::fs::remove_dir_all(&root).await.ok();
    });
}
