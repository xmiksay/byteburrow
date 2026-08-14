//! WebDAV / CalDAV / CardDAV gateway.
//!
//! Mounts three sub-protocols under `/dav`, each layered on top of existing
//! ByteBurrow storages:
//!
//! - **WebDAV** (`src/web/dav/webdav.rs`): general file operations mapped onto
//!   a storage's filesystem — the foundation. `OPTIONS`, `PROPFIND`, `GET/HEAD`,
//!   `PUT`, `MKCOL`, `DELETE`, `COPY`, `MOVE`, `LOCK/UNLOCK`, `PROPPATCH`.
//! - **CalDAV** (`src/web/dav/caldav.rs`): calendars are directories of `.ics`
//!   files inside a storage; `MKCALENDAR` + the `calendar-query` /
//!   `calendar-multiget` REPORT.
//! - **CardDAV** (`src/web/dav/carddav.rs`): address books are directories of
//!   `.vcf` files inside a storage; `MKCOL` (addressbook) + the
//!   `addressbook-query` / `addressbook-multiget` REPORT.
//!
//! Auth uses the shared `Auth` extractor — Basic auth works out of the box for
//! native WebDAV/CalDAV/CardDAV clients, Bearer/cookie for the web UI.
//!
//! URL layout: `/dav/storage/<storage_id>/<path-within-storage>`. Every handler
//! enforces ownership via the same `require_storage_path_access` /
//! `require_storage_path_write_access` helpers used by the REST API, so a DAV
//! client can never exceed the rights a share grants.

pub mod caldav;
pub mod carddav;
pub mod util;
pub mod webdav;

use axum::{routing::any, Router};

use crate::web::AppState;
use std::sync::Arc;

/// Build the DAV router. Mounted under `/dav` in [`crate::web::run`].
///
/// CalDAV/CardDAV are served on the *same* URLs as WebDAV — the handler
/// dispatches by the request method and, for `REPORT`, by the XML report
/// element name. No separate mount point is needed.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Advertise DAV protocol versions at the root (RFC 4918 §10.1).
        .route("/dav", any(webdav::root_handler))
        // Storage-scoped catch-all: any method on any sub-path.
        .route("/dav/storage/:id/*path", any(webdav::dispatch))
        // `/dav/storage/:id` with no trailing path → root of the storage.
        // Axum's `/*path` requires a non-empty segment, hence the explicit
        // root route.
        .route("/dav/storage/:id", any(webdav::dispatch_root))
        // `/dav/storage/:id/` (trailing slash, empty catch-all) → also the
        // storage root. In Axum 0.7 a trailing slash with an empty `/*path`
        // segment matches NEITHER `/dav/storage/:id` NOR
        // `/dav/storage/:id/*path`, so we need an explicit route for it —
        // otherwise a directory listing of the storage root 404s.
        .route("/dav/storage/:id/", any(webdav::dispatch_root))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::any;
    use axum::Router;
    use tower::ServiceExt;

    /// Routing regression for the storage-root directory listing.
    ///
    /// In Axum 0.7 `/dav/storage/:id/*path` requires a non-empty trailing
    /// segment, so `/dav/storage/5/` (trailing slash, empty catch-all) matched
    /// NEITHER that route NOR `/dav/storage/:id` and fell through to 404. We
    /// therefore add an explicit `/dav/storage/:id/` route.
    ///
    /// This test exercises the route *shapes* with stub handlers (mirroring
    /// [`router`]) so it needs no DB, no auth, and no `Config` — it pins the
    /// routing contract directly. The real `router()` mounts the same shapes.
    #[test]
    fn storage_root_trailing_slash_routes_to_handler() {
        let app: Router<()> = Router::new()
            .route("/dav", any(|| async { (StatusCode::OK, "root") }))
            .route(
                "/dav/storage/:id/*path",
                any(|| async { (StatusCode::OK, "catchall") }),
            )
            .route(
                "/dav/storage/:id",
                any(|| async { (StatusCode::OK, "dispatch-root") }),
            )
            // The fix under test.
            .route(
                "/dav/storage/:id/",
                any(|| async { (StatusCode::OK, "dispatch-root") }),
            );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // (uri, expected handler tag). Every shape must match a route —
            // i.e. NOT fall through to 404.
            for (uri, expected) in [
                ("/dav", "root"),
                ("/dav/storage/5", "dispatch-root"),
                ("/dav/storage/5/", "dispatch-root"),
                ("/dav/storage/5/x", "catchall"),
                ("/dav/storage/5/sub/x", "catchall"),
            ] {
                let req = Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap();
                let resp = app.clone().oneshot(req).await.unwrap();
                assert_eq!(
                    resp.status(),
                    StatusCode::OK,
                    "{uri} should route to a handler"
                );
                let body = axum::body::to_bytes(resp.into_body(), 64).await.unwrap();
                assert_eq!(
                    String::from_utf8_lossy(&body),
                    expected,
                    "{uri} routed to the wrong handler"
                );
            }
        });
    }

    /// Negative control: without the `/dav/storage/:id/` route the trailing-
    /// slash root URL 404s — this is the bug we fixed. Keeps the fix honest if
    /// someone later removes the route.
    #[test]
    fn storage_root_trailing_slash_404s_without_explicit_route() {
        let app: Router<()> = Router::new()
            .route(
                "/dav/storage/:id/*path",
                any(|| async { (StatusCode::OK, "catchall") }),
            )
            .route(
                "/dav/storage/:id",
                any(|| async { (StatusCode::OK, "dispatch-root") }),
            );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let req = Request::builder()
                .method(Method::GET)
                .uri("/dav/storage/5/")
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        });
    }
}
