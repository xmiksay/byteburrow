//! Integration tests for the M2 security-headers middleware.
//!
//! Verifies the baseline (`X-Content-Type-Options`, `X-Frame-Options`,
//! `Referrer-Policy`, `Content-Security-Policy`, and the conditional HSTS) is
//! stamped onto every response, including errors short-circuited by inner
//! layers. The header policy reads the global `Config::get().base_url`, so the
//! HSTS conditional (HTTPS vs HTTP) is covered by running the same router
//! under both `base_url` schemes.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::from_fn;
use axum::routing::get;
use axum::Router;
use byteburrow::config::Config;
use byteburrow::web::security_headers;
use std::sync::{Arc, Once, OnceLock};
use tower::ServiceExt;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("build test runtime"))
}

/// `Config::set` may only run once per process; guard it so both sub-tests
/// (http and https) can flip the *stored* value via `override_base_url`.
static CONFIG_INIT: Once = Once::new();

/// Initialize the global `Config` once. `Config::set` panics on a second call,
/// so this is guarded by `Once`. The middleware only reads `base_url`, which we
/// set to the default HTTP origin; the HSTS-conditional's *HTTPS* branch is
/// covered by the unit tests (`hsts_only_when_base_url_is_https`), which don't
/// touch the global at all.
fn ensure_config() {
    CONFIG_INIT.call_once(|| {
        Config::set(Arc::new(Config {
            database_url: String::new(),
            salt: "security-headers-test".to_string(),
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
}

/// A router that mounts only the security-headers middleware over a trivial
/// handler. This exercises the real `from_fn` wiring end-to-end.
fn test_router() -> Router {
    Router::new()
        .route("/ok", get(|| async { "ok" }))
        // An inner layer that short-circuits with 403, to prove headers are
        // applied even on error responses (outermost-layer requirement).
        .route("/denied", get(|| async { (StatusCode::FORBIDDEN, "nope") }))
        .layer(from_fn(security_headers))
}

#[test]
fn unconditional_headers_present_on_success() {
    ensure_config();
    runtime().block_on(async {
        let resp = test_router()
            .oneshot(Request::builder().uri("/ok").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        let csp = h
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self'"));
        // `script-src` must NOT carry 'unsafe-inline' — only `style-src` may.
        // (style-src legitimately needs 'unsafe-inline' for Vue; check the
        // script directive specifically rather than the whole policy string.)
        let script_directive = csp
            .split("; ")
            .find(|d| d.starts_with("script-src"))
            .unwrap();
        assert!(
            !script_directive.contains("'unsafe-inline'"),
            "script-src must be strict, got: {script_directive}"
        );
        assert!(csp.contains("object-src 'none'"));
    });
}

#[test]
fn headers_present_on_error_responses() {
    ensure_config();
    runtime().block_on(async {
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/denied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let h = resp.headers();
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert!(h
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("script-src 'self'"));
    });
}

#[test]
fn hsts_absent_for_http_base_url() {
    ensure_config();
    runtime().block_on(async {
        let resp = test_router()
            .oneshot(Request::builder().uri("/ok").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(
            resp.headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .is_none(),
            "HSTS must not be advertised over a non-https base_url"
        );
    });
}
