//! Lightweight, process-local rate limiting (M1 + H16).
//!
//! Two brute-force surfaces are throttled by client IP:
//!
//! - **Login** (`POST /api/user/login`): credential stuffing. Default 10
//!   attempts / 60 s / IP.
//! - **Share lookup** (`/api/storage/share/{id-or-token}/…`): public share
//!   tokens are unauthenticated, so they are guessable in principle. Default
//!   60 requests / 60 s / IP — generous enough for legitimate browsing of a
//!   shared album, tight enough to kill token enumeration.
//!
//! # Implementation
//!
//! A fixed-window counter per `(ip, window-start)` pair is the simplest scheme
//! that needs no clock arithmetic on each request beyond a floor division.
//! Each `check` records the window start and request count; when the window
//! advances the counter resets. Expired windows are pruned opportunistically on
//! every check so the table can never grow unbounded from idle clients.
//!
//! The state lives in a `std::sync::Mutex<HashMap>` rather than a tokio mutex:
//! the critical section is a HashMap lookup plus a couple of integer ops, never
//! awaiting, so a sync mutex avoids the async overhead and is held for
//! microseconds at most.
//!
//! # Scope / trade-offs
//!
//! This is process-local, exactly like the WebDAV [`LockManager`](crate::web::dav)
//! and the session store. ByteBurrow is a single-node personal-cloud app, so a
//! per-process limiter is sufficient; a multi-node deployment would need a
//! shared store (Redis et al.), which is explicitly out of scope here. The
//! limiter is also best-effort: it protects the common case of a single client
//! hammering the API, not a determined distributed attacker.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The per-key state of one fixed window.
#[derive(Debug, Clone, Copy)]
struct WindowState {
    /// Start instant of the current window.
    window_start: Instant,
    /// Requests counted so far within that window.
    count: u32,
}

/// A fixed-window rate limiter keyed by an arbitrary string (typically a
/// client IP).
///
/// See the [module docs](self) for the design rationale and scope limits.
pub struct RateLimiter {
    inner: Mutex<HashMap<String, WindowState>>,
    max_requests: u32,
    window: Duration,
}

impl RateLimiter {
    /// Construct a limiter that allows `max_requests` per rolling `window`.
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_requests,
            window,
        }
    }

    /// Record an attempt for `key` and return `true` if it is allowed (i.e. the
    /// key is under its limit for the current window) or `false` if it must be
    /// rejected with 429.
    ///
    /// The attempt is counted even when it tips the key over the limit, so a
    /// flood of requests keeps the window "hot" and stays blocked until it
    /// elapses — an attacker cannot burn their quota and then immediately retry.
    ///
    /// Expired entries (windows older than `window`) are pruned on every call so
    /// the map cannot grow without bound from one-off clients.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let max = self.max_requests;
        let window = self.window;

        let mut map = match self.inner.lock() {
            Ok(guard) => guard,
            // A poisoned mutex means a previous caller panicked mid-check; we'd
            // rather fail open (allow the request) than wedge the endpoint.
            Err(guard) => {
                let mut map = guard.into_inner();
                prune(&mut map, now, window);
                return true;
            }
        };

        // Opportunistic GC: drop windows that have fully elapsed so idle keys
        // don't accumulate. Runs on every check, so the cost stays amortized.
        prune(&mut map, now, window);

        let state = map.entry(key.to_owned()).or_insert(WindowState {
            window_start: now,
            count: 0,
        });

        // If the previous window has rolled over, start a fresh count.
        if now.duration_since(state.window_start) >= window {
            state.window_start = now;
            state.count = 0;
        }

        state.count += 1;
        state.count <= max
    }

    /// The window length this limiter was configured with. Used to populate the
    /// `Retry-After` header on 429 responses.
    pub fn window(&self) -> Duration {
        self.window
    }
}

/// Drop every entry whose window has fully elapsed relative to `now`.
fn prune(map: &mut HashMap<String, WindowState>, now: Instant, window: Duration) {
    map.retain(|_, state| now.duration_since(state.window_start) < window);
}

// ---------------------------------------------------------------------------
// Global instances — one limiter per protected surface.
// ---------------------------------------------------------------------------

/// 10 login attempts per 60 s per IP.
const LOGIN_MAX: u32 = 10;
/// 60 share lookups per 60 s per IP.
const SHARE_MAX: u32 = 60;
const WINDOW: Duration = Duration::from_secs(60);

static LOGIN: OnceLock<RateLimiter> = OnceLock::new();
static SHARE: OnceLock<RateLimiter> = OnceLock::new();

/// The login brute-force limiter.
pub fn login_limiter() -> &'static RateLimiter {
    LOGIN.get_or_init(|| RateLimiter::new(LOGIN_MAX, WINDOW))
}

/// The share-lookup (token-enumeration) limiter.
pub fn share_limiter() -> &'static RateLimiter {
    SHARE.get_or_init(|| RateLimiter::new(SHARE_MAX, WINDOW))
}

// ---------------------------------------------------------------------------
// Axum middleware — share-lookup throttle (H16)
// ---------------------------------------------------------------------------

/// Resolve the client IP from a request, using the same rules as the login
/// handler: honor `X-Forwarded-For`/`X-Real-IP` only when
/// `trust_forwarded_headers` is set, otherwise fall back to the TCP peer from
/// `ConnectInfo`. Returns `"unknown"` if neither is available so the request is
/// still bucketed (and thus throttled) rather than bypassed.
fn client_ip(
    headers: &axum::http::HeaderMap,
    connect_info: Option<std::net::SocketAddr>,
) -> String {
    let trust = crate::config::Config::get().trust_forwarded_headers;
    crate::auth::resolve_ip_address(headers, connect_info, trust)
        .unwrap_or_else(|| "unknown".to_string())
}

/// `from_fn` middleware: throttle `/share/` lookups per client IP to blunt
/// share-token enumeration (H16). Applied only to the share-access route group,
/// so authenticated share-management routes (`/share`, `/share/with-me`,
/// `/:id/share/…`) and all other storage routes are unaffected.
pub async fn share_rate_limit(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let connect_info = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0);
    let ip = client_ip(request.headers(), connect_info);

    if share_limiter().check(&ip) {
        next.run(request).await
    } else {
        let retry_after = share_limiter().window().as_secs();
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                header::RETRY_AFTER,
                header::HeaderValue::from_str(&retry_after.to_string()).unwrap(),
            )],
            axum::Json(crate::web::ErrorResponse {
                error: "Too many requests".to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_requests_under_the_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        // The first three attempts within the window all pass.
        assert!(limiter.check("1.2.3.4"));
        assert!(limiter.check("1.2.3.4"));
        assert!(limiter.check("1.2.3.4"));
    }

    #[test]
    fn blocks_requests_over_the_limit() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.check("client"));
        assert!(limiter.check("client"));
        // The third is over the limit and is refused.
        assert!(!limiter.check("client"));
        // Further attempts stay refused until the window rolls over.
        assert!(!limiter.check("client"));
    }

    #[test]
    fn counts_per_key_independently() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.check("a"));
        // 'b' has its own budget — unaffected by 'a' hitting its limit.
        assert!(limiter.check("b"));
        assert!(!limiter.check("b"));
        // 'a' is still at its own limit.
        assert!(!limiter.check("a"));
    }

    #[test]
    fn window_reset_allows_again() {
        // A zero-duration window means every check lands in a fresh window, so
        // the limiter never blocks — exercising the rollover path without a
        // real sleep.
        let limiter = RateLimiter::new(1, Duration::from_millis(0));
        assert!(limiter.check("k"));
        // With a zero-length window the previous window is always "elapsed",
        // so the count resets and the request is allowed again.
        assert!(limiter.check("k"));
    }

    #[test]
    fn over_limit_stays_hot_within_window() {
        // An over-limit request still counts, keeping the window hot so a flood
        // cannot "spend" the budget and then immediately succeed.
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.check("k"));
        for _ in 0..10 {
            assert!(!limiter.check("k"));
        }
    }

    #[test]
    fn pruning_drops_expired_entries() {
        let limiter = RateLimiter::new(1, Duration::from_millis(0));
        limiter.check("idle");
        // A zero-length window means the "idle" entry is already expired on the
        // next check; run a check for a different key and confirm the expired
        // entry is pruned (the map shrinks back toward only live keys).
        limiter.check("other");
        let map = limiter.inner.lock().unwrap();
        // "idle" must have been pruned; at most the just-touched "other" remains.
        assert!(
            !map.contains_key("idle"),
            "expired entry should have been pruned"
        );
    }

    #[test]
    fn window_accessor_returns_configured_duration() {
        let limiter = RateLimiter::new(5, Duration::from_secs(42));
        assert_eq!(limiter.window(), Duration::from_secs(42));
    }

    #[test]
    fn global_limiters_are_singletons() {
        // Same address on every call — `get_or_init` returns the one instance.
        assert!(std::ptr::eq(login_limiter(), login_limiter()));
        assert!(std::ptr::eq(share_limiter(), share_limiter()));
        // And the two are distinct instances.
        assert!(!std::ptr::eq(login_limiter(), share_limiter()));
    }
}
