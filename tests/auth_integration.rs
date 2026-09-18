//! Integration tests for `byteburrow::auth` against a real Postgres database.
//!
//! Requires `DATABASE_URL` to point at a scratch database (migrations run
//! automatically on first use). Defaults to the local docker-compose DB.
//!
//! All tests share one process-lifetime Tokio runtime (`runtime()`) rather
//! than `#[tokio::test]`'s per-test runtime: the DB connection pool spawns
//! background tasks on the runtime that created it, so reusing the pool
//! from a different (and by-then-dropped) per-test runtime hangs.

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use byteburrow::auth::{Auth, AuthError};
use byteburrow::config::Config;
use byteburrow::entity::user;
use byteburrow::migration::Migrator;
use byteburrow::web::AppState;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use sea_orm_migration::MigratorTrait;
use std::sync::{Arc, Once, OnceLock};
use tokio::sync::OnceCell;
use tower::ServiceExt;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();

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

async fn create_test_user(
    db: &DatabaseConnection,
    username: &str,
    password: &str,
    enabled: bool,
) -> user::Model {
    user::ActiveModel {
        name: Set(username.to_string()),
        description: Set(None),
        username: Set(username.to_string()),
        password: Set(Auth::hash_string(password)),
        enabled: Set(enabled),
        admin: Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert test user")
}

#[test]
fn login_with_correct_password_succeeds() {
    runtime().block_on(async {
        let db = test_db().await;
        let user = create_test_user(db, "it_login_ok", "correct-password", true).await;

        let auth = Auth::from_user_password("it_login_ok", "correct-password", db)
            .await
            .expect("login should succeed");
        assert_eq!(auth.user.id, user.id);
    });
}

#[test]
fn login_with_wrong_password_fails() {
    runtime().block_on(async {
        let db = test_db().await;
        create_test_user(db, "it_login_bad_password", "correct-password", true).await;

        let err = Auth::from_user_password("it_login_bad_password", "wrong-password", db)
            .await
            .err()
            .expect("login should fail");
        assert!(matches!(err, AuthError::InvalidCredentials));
    });
}

#[test]
fn login_with_unknown_username_fails() {
    runtime().block_on(async {
        let db = test_db().await;

        let err = Auth::from_user_password("it_login_does_not_exist", "irrelevant", db)
            .await
            .err()
            .expect("login should fail");
        assert!(matches!(err, AuthError::InvalidCredentials));
    });
}

#[test]
fn login_with_legacy_sha256_hash_rehashes_to_argon2id() {
    runtime().block_on(async {
        let db = test_db().await;
        let user = create_test_user(db, "it_login_legacy_rehash", "correct-password", true).await;
        assert!(
            !user.password.starts_with("$argon2"),
            "test fixture should seed a legacy SHA-256 hash"
        );

        Auth::from_user_password("it_login_legacy_rehash", "correct-password", db)
            .await
            .expect("login should succeed against legacy hash");

        let reloaded = user::Entity::find_by_id(user.id)
            .one(db)
            .await
            .expect("query should succeed")
            .expect("user should still exist");
        assert!(
            reloaded.password.starts_with("$argon2id$"),
            "password should have been rehashed to Argon2id after login"
        );

        // The rehashed password must still authenticate.
        Auth::from_user_password("it_login_legacy_rehash", "correct-password", db)
            .await
            .expect("login should succeed against rehashed password");
    });
}

#[test]
fn login_for_disabled_user_fails() {
    runtime().block_on(async {
        let db = test_db().await;
        create_test_user(db, "it_login_disabled", "correct-password", false).await;

        let err = Auth::from_user_password("it_login_disabled", "correct-password", db)
            .await
            .err()
            .expect("login should fail");
        assert!(matches!(err, AuthError::UserDisabled));
    });
}

#[test]
fn token_roundtrip_authenticates_and_revoke_invalidates() {
    runtime().block_on(async {
        let db = test_db().await;
        let user = create_test_user(db, "it_token_roundtrip", "correct-password", true).await;
        let auth = Auth::new(user.clone());

        let raw_token = auth
            .create_token(
                db,
                Some("test-agent".to_string()),
                Some("127.0.0.1".to_string()),
            )
            .await
            .expect("token creation should succeed");

        let reauth = Auth::from_token(&raw_token, db, None, None)
            .await
            .expect("token should authenticate");
        assert_eq!(reauth.user.id, user.id);

        Auth::revoke_token(&raw_token, db)
            .await
            .expect("revoke should succeed");

        let err = Auth::from_token(&raw_token, db, None, None)
            .await
            .err()
            .expect("revoked token should no longer authenticate");
        assert!(matches!(err, AuthError::InvalidToken));
    });
}

#[test]
fn revoke_all_tokens_invalidates_every_token_for_user() {
    runtime().block_on(async {
        let db = test_db().await;
        let user = create_test_user(db, "it_revoke_all", "correct-password", true).await;
        let auth = Auth::new(user.clone());

        let token_a = auth.create_token(db, None, None).await.unwrap();
        let token_b = auth.create_token(db, None, None).await.unwrap();

        auth.revoke_all_tokens(db)
            .await
            .expect("revoke_all should succeed");

        assert!(matches!(
            Auth::from_token(&token_a, db, None, None)
                .await
                .err()
                .unwrap(),
            AuthError::InvalidToken
        ));
        assert!(matches!(
            Auth::from_token(&token_b, db, None, None)
                .await
                .err()
                .unwrap(),
            AuthError::InvalidToken
        ));
    });
}

#[test]
fn unknown_token_fails_with_invalid_token() {
    runtime().block_on(async {
        let db = test_db().await;

        let err = Auth::from_token("not-a-real-token", db, None, None)
            .await
            .err()
            .expect("unknown token should fail");
        assert!(matches!(err, AuthError::InvalidToken));
    });
}

// ----------------------------------------------------------------------------
// HTTP-level transport tests (issue #36)
// ----------------------------------------------------------------------------

/// A trivial handler behind the `Auth` extractor. The body never matters —
/// the point is to exercise the real HTTP rejection path (status, headers,
/// JSON body) that the router produces when auth fails.
async fn protected(_auth: Auth) -> &'static str {
    "ok"
}

fn auth_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/protected", get(protected))
        .with_state(state)
}

/// Build a minimal `AppState` with a no-op job sender (the protected handler
/// never enqueues jobs).
fn make_state(db: DatabaseConnection) -> Arc<AppState> {
    let (job_sender, _rx) = tokio::sync::mpsc::channel(16);
    Arc::new(AppState {
        db,
        config: Config::get().as_ref().clone(),
        jinja: minijinja::Environment::new(),
        job_sender,
        notify_reload: Arc::new(tokio::sync::Notify::new()),
    })
}

#[test]
fn query_param_token_alone_is_rejected() {
    runtime().block_on(async {
        let db = test_db().await;
        let user = create_test_user(db, "it_query_token_rejected", "correct-password", true).await;
        let auth = Auth::new(user);

        let raw_token = auth
            .create_token(db, None, None)
            .await
            .expect("token creation should succeed");

        let app = auth_router(make_state(db.clone()));

        // Only credential is a `?token=` query parameter — a valid token in a
        // leaky transport must still be rejected (issue #36).
        let req = Request::builder()
            .uri(format!("/protected?token={raw_token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "query-param tokens must not authenticate"
        );

        // Positive control: the very same token over the accepted Bearer
        // transport authenticates, proving the 401 above is about the
        // transport, not the token itself.
        let req = Request::builder()
            .uri("/protected")
            .header(header::AUTHORIZATION, format!("Bearer {raw_token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    });
}

#[test]
fn unauthorized_rejection_body_is_json_error_envelope() {
    runtime().block_on(async {
        let db = test_db().await;
        let app = auth_router(make_state(db.clone()));

        let req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // `WWW-Authenticate: Basic realm="Cloud"` stays consistent with
        // `ApiError`'s Unauthorized handling (native clients / DAV gateways
        // rely on it to prompt for Basic credentials).
        assert_eq!(
            resp.headers()
                .get(header::WWW_AUTHENTICATE)
                .expect("WWW-Authenticate header must be present"),
            "Basic realm=\"Cloud\""
        );

        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("Content-Type header must be present")
            .to_str()
            .unwrap();
        assert!(
            content_type.starts_with("application/json"),
            "auth rejections must use the JSON error envelope, got Content-Type {content_type}"
        );

        let body = to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).expect("body must be valid JSON");
        assert_eq!(
            parsed["error"], "Missing authentication credentials",
            "body must match the ErrorResponse envelope"
        );
    });
}
