//! CardDAV (RFC 6352) — address book collections stored as directories of
//! `.vcf` (vCard) files inside a ByteBurrow storage.
//!
//! As with CalDAV, the HTTP method surface is reused from [`super::webdav`];
//! CardDAV adds the `addressbook-query` / `addressbook-multiget` REPORTs and
//! the notion of an "addressbook collection". We mark an addressbook
//! directory with a sidecar `.carddav-addressbook` file so PROPFIND can report
//! `<addressbook/>` in its `resourcetype`.

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

/// Marker file placed in an address book collection's directory.
pub const ADDRESSBOOK_MARKER: &str = ".carddav-addressbook";

/// Dispatcher for CardDAV `REPORT` requests. Called as the fallback from
/// [`super::caldav::report_dispatcher`] when the report isn't a CalDAV one.
pub async fn report_dispatcher(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    let report = parse_report_element(body);
    match report.as_deref() {
        Some("addressbook-query") => addressbook_query(auth, state, storage, path, body).await,
        Some("addressbook-multiget") => {
            addressbook_multiget(auth, state, storage, path, body).await
        }
        _ => Err(crate::web::bad_request(format!(
            "Unsupported REPORT: {report:?}"
        ))),
    }
}

/// `REPORT addressbook-query` — return matching vCard resources within an
/// address book collection.
async fn addressbook_query(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    require_storage_path_access(auth, &storage.model, path, &state.db).await?;

    let want_address_data = body_contains(body, b"address-data");
    let hrefs = object_resources_in(storage, path).await?;

    let mut responses = Vec::new();
    for rel in hrefs {
        let full = storage.get_full_path(&rel);
        let Ok(data) = tokio::fs::read(&full).await else {
            continue;
        };
        let mut props = vec![DavProp::text("getcontenttype", "text/vcard")];
        if want_address_data {
            props.push(DavProp::text(
                "address-data",
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

/// `REPORT addressbook-multiget` — return the vcards at the explicitly listed
/// `<href>`s.
async fn addressbook_multiget(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    _path: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    let want_address_data = body_contains(body, b"address-data");
    let requested_hrefs = extract_hrefs(body);

    let mut responses = Vec::new();
    for h in requested_hrefs {
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
                let mut props = vec![DavProp::text("getcontenttype", "text/vcard")];
                if want_address_data {
                    props.push(DavProp::text(
                        "address-data",
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

/// Enumerate `.vcf` files directly inside the address book collection `path`.
async fn object_resources_in(storage: &Storage, path: &str) -> Result<Vec<String>, ApiError> {
    let entries = storage
        .list_directory_fs(path)
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(entries
        .into_iter()
        .filter(|e| {
            matches!(e.entry_type, crate::entity::entry::EntryType::File)
                && e.path.ends_with(".vcf")
        })
        .map(|e| e.path)
        .collect())
}

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

fn body_contains(body: &[u8], needle: &[u8]) -> bool {
    std::str::from_utf8(body)
        .map(|s| s.contains(std::str::from_utf8(needle).unwrap_or("")))
        .unwrap_or(false)
}

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

/// Create an address book collection at `path`: a directory containing the
/// marker file. Called from MKCOL-with-body when the body declares an
/// addressbook resourcetype; here we expose it directly for tests and for a
/// future typed MKCOL handler.
pub async fn mk_addressbook(
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
    storage
        .save_file(&format!("{path}/{ADDRESSBOOK_MARKER}"), b"")
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

/// Is this path an address book collection? (Directory containing the marker.)
pub async fn is_addressbook_collection(storage: &Storage, path: &str) -> bool {
    let marker = if path.is_empty() {
        ADDRESSBOOK_MARKER.to_string()
    } else {
        format!("{path}/{ADDRESSBOOK_MARKER}")
    };
    tokio::fs::try_exists(storage.get_full_path(&marker))
        .await
        .unwrap_or(false)
}
