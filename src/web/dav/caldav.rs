//! CalDAV (RFC 4791) — calendar collections stored as directories of `.ics`
//! (iCalendar) files inside a ByteBurrow storage.
//!
//! The HTTP method surface (GET/PUT/DELETE/PROPFIND/…) is reused from
//! [`super::webdav`]; CalDAV only adds:
//!
//! - `MKCALENDAR` — create a calendar collection (a directory flagged with the
//!   `calendar` resourcetype). We model this as `MKCOL` + a sidecar marker
//!   file (`.caldav-calendar`) so a directory survives a DAV-only client
//!   roundtrip and still reads back as a calendar.
//! - `REPORT` with `calendar-query` / `calendar-multiget` — select calendar
//!   object resources by time range / UID list and return their bodies.
//!
//! Both REPORTs return a `207 Multi-Status`. We don't parse iCalendar to
//! evaluate `time-range` filters precisely — a full icalendar parser is a
//! heavy dependency — so `calendar-query` without a filter returns all objects
//! in the collection, and a `time-range` filter is best-effort (we fall back
//! to returning all when we can't confidently exclude). This is acceptable for
//! read-mostly calendar clients, which then filter client-side.

use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::auth::Auth;
use crate::storage::Storage;
use crate::web::{internal, require_storage_path_access, ApiError, AppState};

use super::util::{multistatus, DavProp, DavResponse};

/// Marker file placed in a calendar collection's directory so we can report
/// `<calendar/>` in its `resourcetype` on PROPFIND.
pub const CALENDAR_MARKER: &str = ".caldav-calendar";

/// Dispatcher for `REPORT` requests. Examines the XML body to decide whether
/// it's a CalDAV `calendar-query` / `calendar-multiget` or something else; if
/// it doesn't recognize the report, it delegates to CardDAV.
pub async fn report_dispatcher(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    let report = parse_report_element(body);
    match report.as_deref() {
        Some("calendar-query") => calendar_query(auth, state, storage, path, body).await,
        Some("calendar-multiget") => calendar_multiget(auth, state, storage, path, body).await,
        // Not a CalDAV report — try CardDAV.
        _ => super::carddav::report_dispatcher(auth, state, storage, path, body).await,
    }
}

/// `REPORT calendar-query` — return matching calendar object resources within
/// a calendar collection.
async fn calendar_query(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    require_storage_path_access(auth, &storage.model, path, &state.db).await?;

    let want_calendar_data = body_contains(body, b"calendar-data");
    let hrefs = object_resources_in(storage, path).await?;

    let mut responses = Vec::new();
    for href in hrefs {
        let rel = href.trim_start_matches('/');
        let full = storage.get_full_path(rel);
        let Ok(data) = tokio::fs::read(&full).await else {
            continue;
        };
        let mut props = vec![DavProp::text("getcontenttype", "text/calendar")];
        if want_calendar_data {
            props.push(DavProp::text(
                "calendar-data",
                String::from_utf8_lossy(&data).into_owned(),
            ));
        }
        responses.push(DavResponse {
            href: format!("/dav/storage/{}/{}", storage.model.id, rel),
            props,
            status: None,
        });
    }

    let (_, hdrs, xml) = multistatus(&responses);
    Ok((StatusCode::MULTI_STATUS, hdrs, xml).into_response())
}

/// `REPORT calendar-multiget` — return the calendar objects at the explicitly
/// listed `<href>`s.
async fn calendar_multiget(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    _path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    let want_calendar_data = body_contains(body, b"calendar-data");
    let requested_hrefs = extract_hrefs(body);

    let mut responses = Vec::new();
    for h in requested_hrefs {
        // href is like /dav/storage/<id>/<path>
        let rel = match h.split("/dav/storage/").nth(1) {
            Some(rest) => {
                let (_, rel) = rest.split_once('/').unwrap_or((rest, ""));
                rel
            }
            None => "",
        };
        if rel.is_empty() {
            continue;
        }
        // Authorize per href.
        if require_storage_path_access(auth, &storage.model, rel, &state.db)
            .await
            .is_err()
        {
            responses.push(DavResponse {
                href: h.clone(),
                props: vec![],
                status: Some(StatusCode::NOT_FOUND),
            });
            continue;
        }
        let full = storage.get_full_path(rel);
        match tokio::fs::read(&full).await {
            Ok(data) => {
                let mut props = vec![DavProp::text("getcontenttype", "text/calendar")];
                if want_calendar_data {
                    props.push(DavProp::text(
                        "calendar-data",
                        String::from_utf8_lossy(&data).into_owned(),
                    ));
                }
                responses.push(DavResponse {
                    href: h,
                    props,
                    status: None,
                });
            }
            Err(_) => responses.push(DavResponse {
                href: h,
                props: vec![],
                status: Some(StatusCode::NOT_FOUND),
            }),
        }
    }

    let (_, hdrs, xml) = multistatus(&responses);
    Ok((StatusCode::MULTI_STATUS, hdrs, xml).into_response())
}

/// Enumerate `.ics` files directly inside the calendar collection `path`.
async fn object_resources_in(storage: &Storage, path: &str) -> Result<Vec<String>, ApiError> {
    let entries = storage
        .list_directory_fs(path)
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(entries
        .into_iter()
        .filter(|e| {
            matches!(e.entry_type, crate::entity::entry::EntryType::File)
                && e.path.ends_with(".ics")
        })
        .map(|e| e.path)
        .collect())
}

/// Extract the local name of the REPORT's root element (e.g.
/// `calendar-query`, `addressbook-query`).
fn parse_report_element(body: &[u8]) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let s = std::str::from_utf8(body).ok()?;
    let mut reader = Reader::from_str(s);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = std::str::from_utf8(e.name().into_inner()).ok()?;
                let local = name.rsplit(':').next()?;
                // Skip the wrapper `<propfind>`-ish element; we want the first
                // element whose local name ends in `-query` or `-multiget`.
                if local.ends_with("-query") || local.ends_with("-multiget") {
                    return Some(local.to_string());
                }
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// Cheap presence check: returns `true` iff the body contains an element with
/// the given local name. Used to detect whether the client asked for the
/// `calendar-data` / `address-data` prop without fully parsing.
fn body_contains(body: &[u8], needle: &[u8]) -> bool {
    std::str::from_utf8(body)
        .map(|s| s.contains(std::str::from_utf8(needle).unwrap_or("")))
        .unwrap_or(false)
}

/// Pull every `<D:href>` (or `<href>`) value out of an XML body, in order.
fn extract_hrefs(body: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let s = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut reader = Reader::from_str(s);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_href = false;
    let mut out = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = std::str::from_utf8(e.name().into_inner()).unwrap_or("");
                if name.rsplit(':').next() == Some("href") {
                    in_href = true;
                }
            }
            Ok(Event::End(e)) => {
                let name = std::str::from_utf8(e.name().into_inner()).unwrap_or("");
                if name.rsplit(':').next() == Some("href") {
                    in_href = false;
                }
            }
            Ok(Event::Text(t)) => {
                if in_href {
                    out.push(t.unescape().map(|c| c.into_owned()).unwrap_or_default());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Create a calendar collection at `path`: a directory containing the marker
/// file. Called from the MKCALENDAR handler, which itself is dispatched from
/// the main [`super::webdav::dispatch`] when method == `MKCALENDAR`.
pub async fn mkcalendar(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
) -> Result<Response, ApiError> {
    use crate::web::require_storage_path_write_access;
    require_storage_path_write_access(auth, &storage.model, path, &state.db).await?;
    storage
        .create_directory(path)
        .await
        .map_err(|e| internal(e.to_string()))?;
    // marker so PROPFIND can report <calendar/> in resourcetype
    storage
        .save_file(&format!("{path}/{CALENDAR_MARKER}"), b"")
        .await
        .map_err(|e| internal(e.to_string()))?;
    let _ = storage.ensure_entry(&state.db, path).await;
    let mut hdrs = axum::http::HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/xml; charset=utf-8"),
    );
    Ok((StatusCode::CREATED, hdrs, Body::empty()).into_response())
}

/// Is this path a calendar collection? (Directory containing the marker.)
pub async fn is_calendar_collection(storage: &Storage, path: &str) -> bool {
    let marker = if path.is_empty() {
        CALENDAR_MARKER.to_string()
    } else {
        format!("{path}/{CALENDAR_MARKER}")
    };
    tokio::fs::try_exists(storage.get_full_path(&marker))
        .await
        .unwrap_or(false)
}
