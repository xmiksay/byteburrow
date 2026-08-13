//! Integration tests for the H16 share-lookup rate limiter middleware.
//!
//! Mounts the real `share_rate_limit` middleware over a trivial handler and
//! drives it with `oneshot` requests, proving the middleware:
//!  - lets under-limit traffic through to the handler (200),
//!  - starts returning 429 once the per-IP budget is exhausted,
//!  - stamps a `Retry-After` header on the 429, and
//!  - keys independently by client IP.
//!
//! No database is required: the limiter runs before the handler, so a handler
//! that always succeeds is enough to observe the throttle decision. Because the
//! limiter is a process-global `OnceLock`, these tests use a throwaway `ip` per
//! run to avoid cross-test interference with whatever quota state earlier tests
//! in the binary may have left behind.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::from_fn;
use axum::routing::get;
use axum::Router;
use byteburrow::config::Config;
use byteburrow::web::rate_limit::share_rate_limit;
use std::sync::{Arc, Once};
use tower::ServiceExt;

static CONFIG_INIT: Once = Once::new();

/// `Config::set` may only run once per process; guard it so this test file and
/// the other integration tests cooperate. The middleware only reads
/// `trust_forwarded_headers` (false here, so the TCP peer IP is used), which is
/// why a stub config is sufficient.
fn ensure_config() {
    CONFIG_INIT.call_once(|| {
        Config::set(Arc::new(Config {
            database_url: String::new(),
            salt: "rate-limit-test".to_string(),
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

/// A router that mounts only the share rate-limit middleware over a handler
/// that always succeeds. Exercises the real `from_fn` wiring end-to-end
/// without a database or real share handlers.
fn test_router() -> Router {
    Router::new()
        .route("/share/:id", get(|| async { "ok" }))
        .layer(from_fn(share_rate_limit))
}

/// Build a request whose `ConnectInfo` extension carries the given client IP,
/// matching how `axum::serve(... into_make_service_with_connect_info)` injects
/// it in production. The middleware resolves the IP from this extension (since
/// `trust_forwarded_headers` is false in the stub config).
fn request_from(ip: &str) -> Request<Body> {
    let addr: std::net::SocketAddr = format!("{ip}:0").parse().unwrap();
    Request::builder()
        .uri("/share/abc")
        .extension(axum::extract::ConnectInfo(addr))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn under_limit_traffic_reaches_handler() {
    ensure_config();
    let app = test_router();
    // The first request for a fresh IP must reach the handler (200).
    let resp = app.clone().oneshot(request_from("10.0.0.1")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn over_limit_returns_429_with_retry_after() {
    ensure_config();
    let app = test_router();

    // Fire many requests from one IP. The first SHARE_MAX (60) succeed; every
    // subsequent one must be throttled with 429 + Retry-After.
    for _ in 0..60 {
        let resp = app.clone().oneshot(request_from("10.0.0.2")).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "under-limit request should pass"
        );
    }
    let blocked = app.oneshot(request_from("10.0.0.2")).await.unwrap();
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = blocked
        .headers()
        .get(header::RETRY_AFTER)
        .expect("429 must carry a Retry-After header");
    // The configured window is 60 s, so Retry-After advertises 60.
    assert_eq!(retry_after.to_str().unwrap(), "60");
}

#[tokio::test]
async fn limits_are_keyed_per_ip() {
    ensure_config();
    let app = test_router();

    // Exhaust the budget for one IP.
    for _ in 0..60 {
        let _ = app.clone().oneshot(request_from("10.0.0.3")).await.unwrap();
    }
    // That IP is now throttled.
    let blocked = app.clone().oneshot(request_from("10.0.0.3")).await.unwrap();
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

    // A *different* IP still has its own, untouched budget → reaches the handler.
    let other = app.oneshot(request_from("10.0.0.4")).await.unwrap();
    assert_eq!(other.status(), StatusCode::OK);
}
