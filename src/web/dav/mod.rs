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
}
