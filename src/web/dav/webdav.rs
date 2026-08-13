//! Core WebDAV (RFC 4918) handler — the backbone of the `/dav` gateway.
//!
//! Every method is mapped onto ByteBurrow's existing [`Storage`] abstraction
//! and goes through the same authorization helpers as the REST API, so a DAV
//! client cannot exceed the rights a share grants.

use axum::{
    body::{Body, Bytes},
    extract::{Path as AxumPath, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::SystemTime;

use crate::auth::Auth;
use crate::entity::entry::EntryType;
use crate::storage::{determine_content_type, Storage};
use crate::web::{
    bad_request, conflict, internal, not_found_msg, require_storage_path_access,
    require_storage_path_write_access, ApiError, AppState,
};

use super::util::{
    check_lock_for_write, delete_lock_async, if_header_tokens, multistatus, parse_timeout,
    persist_lock, DavProp, DavResponse, LockManager, PropFind,
};

/// Maximum WebDAV request body we'll buffer in memory (e.g. a PUT). Larger
/// uploads should use chunked streaming, which a future iteration would add.
const MAX_DAV_BODY: usize = 256 * 1024 * 1024;

// Non-standard header names used by WebDAV (RFC 4918 §10).
const H_DAV: HeaderName = HeaderName::from_static("dav");
const H_DEPTH: HeaderName = HeaderName::from_static("depth");
const H_DESTINATION: HeaderName = HeaderName::from_static("destination");
const H_IF: HeaderName = HeaderName::from_static("if");
const H_LOCK_TOKEN: HeaderName = HeaderName::from_static("lock-token");
const H_OVERWRITE: HeaderName = HeaderName::from_static("overwrite");
const H_TIMEOUT: HeaderName = HeaderName::from_static("timeout");
const H_MS_AUTHOR_VIA: HeaderName = HeaderName::from_static("ms-author-via");

/// `ANY /dav` — advertise the DAV protocol versions this server implements
/// (RFC 4918 §10.1). No auth: clients probe this before deciding to talk to us.
pub async fn root_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (H_DAV, HeaderValue::from_static("1, 2, calendar-access, addressbook")),
            (H_MS_AUTHOR_VIA, HeaderValue::from_static("DAV")),
            (
                header::ALLOW,
                HeaderValue::from_static(
                    "OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, MOVE, COPY, PROPFIND, PROPPATCH, LOCK, UNLOCK, REPORT",
                ),
            ),
        ],
    )
}

/// `ANY /dav/storage/:id` (no trailing path) — treat the path as the storage
/// root (`""`). Axum's `/*path` requires a non-empty segment, so we add an
/// explicit root route.
pub async fn dispatch_root(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<i32>,
    request: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, MAX_DAV_BODY)
        .await
        .map_err(|e| internal(e.to_string()))?;
    dispatch_inner(auth, &state, id, "", parts.method, parts.headers, &body).await
}

/// `ANY /dav/storage/:id/*path` — the main WebDAV entry point.
pub async fn dispatch(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    AxumPath((id, path)): AxumPath<(i32, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, MAX_DAV_BODY)
        .await
        .map_err(|e| internal(e.to_string()))?;
    dispatch_inner(auth, &state, id, &path, parts.method, parts.headers, &body).await
}

async fn dispatch_inner(
    auth: Auth,
    state: &Arc<AppState>,
    storage_id: i32,
    path: &str,
    method: Method,
    headers: HeaderMap,
    body: &Bytes,
) -> Result<Response, ApiError> {
    let storage = Storage::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| match e {
            sea_orm::DbErr::RecordNotFound(_) => not_found_msg("Storage not found"),
            other => internal(other.to_string()),
        })?;

    // Normalize: leading slash stripped, trailing slash kept as a "is
    // collection" hint. Empty path = storage root (a collection).
    let path = path.trim_start_matches('/');
    let trailing_slash = path.ends_with('/');
    let path = path.trim_end_matches('/');

    // WebDAV/CalDAV/CardDAV use extension methods (PROPFIND, MKCALENDAR,
    // REPORT, …) that aren't in the HTTP standard set, so we dispatch on the
    // string form rather than `Method` variants.
    match method.as_str() {
        "OPTIONS" => Ok(options_response()),
        "GET" => get_handler(&auth, state, &storage, path, trailing_slash).await,
        "HEAD" => head_handler(&auth, state, &storage, path, trailing_slash).await,
        "PUT" => put_handler(&auth, state, &storage, path, &headers, body).await,
        "DELETE" => delete_handler(&auth, state, &storage, path, &headers).await,
        "MKCOL" => mkcol_handler(&auth, state, &storage, path, &headers).await,
        "MKCALENDAR" => super::caldav::mkcalendar(&auth, state, &storage, path).await,
        "COPY" => copy_move_handler(&auth, state, &storage, path, &headers, false).await,
        "MOVE" => copy_move_handler(&auth, state, &storage, path, &headers, true).await,
        "PROPFIND" => {
            propfind_handler(&auth, state, &storage, path, trailing_slash, &headers, body).await
        }
        "PROPPATCH" => proppatch_handler(&auth, state, &storage, path, body).await,
        "LOCK" => lock_handler(&auth, state, &storage, path, trailing_slash, &headers, body).await,
        "UNLOCK" => unlock_handler(&auth, state, storage.model.id, path, &headers).await,
        "REPORT" => {
            super::caldav::report_dispatcher(&auth, state, &storage, path, body.as_ref()).await
        }
        _ => Err(bad_request(format!("Unsupported DAV method: {method}"))),
    }
}

// ---------------------------------------------------------------------------
// OPTIONS
// ---------------------------------------------------------------------------

fn options_response() -> Response {
    (
        StatusCode::OK,
        [
            (H_DAV, HeaderValue::from_static("1, 2, calendar-access, addressbook")),
            (
                header::ALLOW,
                HeaderValue::from_static(
                    "OPTIONS, GET, HEAD, PUT, DELETE, MKCOL, MOVE, COPY, PROPFIND, PROPPATCH, LOCK, UNLOCK, REPORT",
                ),
            ),
        ],
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET / HEAD
// ---------------------------------------------------------------------------

async fn get_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    trailing_slash: bool,
) -> Result<Response, ApiError> {
    require_storage_path_access(auth, &storage.model, path, &state.db).await?;

    let full = storage.get_full_path(path);
    let metadata = tokio::fs::metadata(&full)
        .await
        .map_err(|e| map_io_to_api(e, path))?;

    if metadata.is_dir() {
        // A directory GET without trailing slash → redirect clients to add
        // one (RFC 4918 §8.3). Many clients rely on this to know the resource
        // is a collection.
        if !trailing_slash && !path.is_empty() {
            return Ok(redirect_to_collection(storage.model.id, path));
        }
        return Ok(directory_listing(storage, path).await);
    }

    serve_file(storage, path, metadata).await
}

async fn head_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    trailing_slash: bool,
) -> Result<Response, ApiError> {
    // HEAD shares authorization and content-type logic with GET; we just drop
    // the body. Reusing `get_handler` then emptying the body keeps them in
    // sync.
    let mut resp = get_handler(auth, state, storage, path, trailing_slash).await?;
    if resp.status() == StatusCode::OK {
        *resp.body_mut() = Body::empty();
    }
    Ok(resp)
}

/// Stream a file's bytes (GET) via `tower_http::services::ServeFile`, which
/// handles range requests and content-type detection. We override Content-Type
/// with our own detector and add WebDAV-relevant headers (ETag, Last-Modified).
async fn serve_file(
    storage: &Storage,
    path: &str,
    metadata: std::fs::Metadata,
) -> Result<Response, ApiError> {
    use tower::Service;
    use tower_http::services::ServeFile;

    let full_path = storage
        .resolve_safe_path(path)
        .await
        .map_err(|e| map_io_to_api(e, path))?;
    let content_type = determine_content_type(&full_path, &[]);
    let etag = weak_etag(metadata.len(), &metadata);

    let mut service = ServeFile::new(full_path);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .map_err(|e| internal(e.to_string()))?;
    let mut resp = Service::<Request<Body>>::call(&mut service, req)
        .await
        .map_err(|e| internal(format!("ServeFile failed: {e:?}")))?
        .into_response();

    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap(),
    );
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    Ok(resp)
}

/// Build a minimal `text/html` directory listing for a storage path. This is
/// what a browser sees when GETting a collection directly over `/dav/...`.
async fn directory_listing(storage: &Storage, path: &str) -> Response {
    let entries = storage.list_directory_fs(path).await.unwrap_or_default();
    let mut html = String::from("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>");
    html.push_str(&escape_html(path));
    html.push_str("</title></head><body><h1>");
    html.push_str(&escape_html(if path.is_empty() { "/" } else { path }));
    html.push_str("</h1><ul>");
    if !path.is_empty() {
        html.push_str("<li><a href=\"../\">../</a></li>");
    }
    for e in entries {
        let name = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
        let slash = if matches!(e.entry_type, EntryType::Directory) {
            "/"
        } else {
            ""
        };
        // The `href` is an attribute context: percent-encode the name (so it
        // is a valid URL and contains no `"` to break the attribute), then the
        // slash is a literal path separator. The link text is element content,
        // so it only needs HTML-escaping.
        let href_name = encode_path_segment(&name);
        let text_name = escape_html(&name);
        html.push_str(&format!(
            "<li><a href=\"{href_name}{slash}\">{text_name}{slash}</a></li>",
        ));
    }
    html.push_str("</ul></body></html>");

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    (StatusCode::OK, headers, html).into_response()
}

fn redirect_to_collection(storage_id: i32, path: &str) -> Response {
    let loc = format!("/dav/storage/{storage_id}/{path}/");
    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, HeaderValue::from_str(&loc).unwrap());
    (StatusCode::MOVED_PERMANENTLY, headers, Body::empty()).into_response()
}

// ---------------------------------------------------------------------------
// PUT
// ---------------------------------------------------------------------------

async fn put_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(bad_request("Cannot PUT to storage root"));
    }
    require_storage_path_write_access(auth, &storage.model, path, &state.db).await?;

    // C4 Part B: enforce exclusive locks held by other users before mutating.
    enforce_lock_for_write(
        storage.model.id,
        path,
        auth,
        &if_header_tokens(headers.get(&H_IF)),
    )?;

    storage
        .save_file(path, body)
        .await
        .map_err(|e| internal(format!("Failed to write file: {e}")))?;
    // Keep the DB entry table in sync — DAV PUT is a real file create/overwrite.
    let _ = storage.ensure_entry(&state.db, path).await;

    let metadata = tokio::fs::metadata(storage.get_full_path(path))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let etag = weak_etag(metadata.len(), &metadata);

    let mut headers = HeaderMap::new();
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok((StatusCode::CREATED, headers, Body::empty()).into_response())
}

// ---------------------------------------------------------------------------
// DELETE
// ---------------------------------------------------------------------------

async fn delete_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(bad_request("Cannot DELETE storage root"));
    }
    require_storage_path_write_access(auth, &storage.model, path, &state.db).await?;

    enforce_lock_for_write(
        storage.model.id,
        path,
        auth,
        &if_header_tokens(headers.get(&H_IF)),
    )?;

    storage
        .remove_entry(path)
        .await
        .map_err(|e| map_io_to_api(e, path))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// MKCOL
// ---------------------------------------------------------------------------

async fn mkcol_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(bad_request("Cannot MKCOL storage root"));
    }
    require_storage_path_write_access(auth, &storage.model, path, &state.db).await?;

    enforce_lock_for_write(
        storage.model.id,
        path,
        auth,
        &if_header_tokens(headers.get(&H_IF)),
    )?;

    let exists = storage.get_full_path(path);
    if tokio::fs::try_exists(&exists).await.unwrap_or(false) {
        return Err(conflict("A resource already exists at this path"));
    }

    storage
        .create_directory(path)
        .await
        .map_err(|e| internal(format!("MKCOL failed: {e}")))?;
    let _ = storage.ensure_entry(&state.db, path).await;
    Ok(StatusCode::CREATED.into_response())
}

// ---------------------------------------------------------------------------
// COPY / MOVE
// ---------------------------------------------------------------------------

async fn copy_move_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    src: &str,
    headers: &HeaderMap,
    is_move: bool,
) -> Result<Response, ApiError> {
    if src.is_empty() {
        return Err(bad_request("Source path required"));
    }

    // Parse Destination — it's a full URI; we only care about the path.
    let dest = parse_destination(headers)?;
    let dest = dest
        .strip_prefix("/dav/storage/")
        .ok_or_else(|| bad_request("Destination must be under /dav/storage/"))?;
    let (dest_storage_id, rest) = dest
        .split_once('/')
        .ok_or_else(|| bad_request("Destination must include a storage id and path"))?;
    let dest_storage_id: i32 = dest_storage_id
        .parse()
        .map_err(|_| bad_request("Invalid storage id in Destination"))?;
    let dest_path = rest.trim_end_matches('/');

    if dest_storage_id != storage.model.id && is_move {
        return Err(bad_request("Cross-storage MOVE not supported"));
    }

    // Authorize: read the source, write the destination.
    require_storage_path_access(auth, &storage.model, src, &state.db).await?;
    if dest_storage_id == storage.model.id {
        require_storage_path_write_access(auth, &storage.model, dest_path, &state.db).await?;
    } else {
        let dst_storage = Storage::find_by_id(&state.db, dest_storage_id)
            .await
            .map_err(|e| match e {
                sea_orm::DbErr::RecordNotFound(_) => not_found_msg("Destination storage not found"),
                other => internal(other.to_string()),
            })?;
        require_storage_path_write_access(auth, &dst_storage.model, dest_path, &state.db).await?;
    }

    // C4 Part B: enforce exclusive locks. MOVE removes the source, so it needs
    // a lock check on both src and dest; COPY only needs the dest check.
    let if_tokens = if_header_tokens(headers.get(&H_IF));
    if is_move {
        enforce_lock_for_write(storage.model.id, src, auth, &if_tokens)?;
    }
    enforce_lock_for_write(dest_storage_id, dest_path, auth, &if_tokens)?;

    let overwrite = headers
        .get(&H_OVERWRITE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("t") || s.eq_ignore_ascii_case("true"))
        .unwrap_or(true); // default Overwrite: T (RFC 4918 §9.8.3)
    let dest_exists = tokio::fs::try_exists(storage.get_full_path(dest_path))
        .await
        .unwrap_or(false);
    if dest_exists && !overwrite {
        return Err(conflict("Destination exists and Overwrite: F"));
    }

    if is_move {
        storage
            .rename_entry(src, dest_path)
            .await
            .map_err(|e| internal(format!("MOVE failed: {e}")))?;
    } else {
        copy_tree(storage, src, dest_path).await?;
    }
    let _ = storage.ensure_entry(&state.db, dest_path).await;

    let status = if dest_exists {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    Ok(status.into_response())
}

/// Recursively copy `src` → `dest` within the same storage.
async fn copy_tree(storage: &Storage, src: &str, dest: &str) -> Result<(), ApiError> {
    let src_full = storage
        .resolve_safe_path(src)
        .await
        .map_err(|e| internal(e.to_string()))?;
    let dest_full = storage
        .resolve_safe_path_lexical(dest)
        .await
        .map_err(|e| internal(e.to_string()))?;

    let meta = tokio::fs::metadata(&src_full)
        .await
        .map_err(|e| internal(e.to_string()))?;
    if meta.is_dir() {
        tokio::fs::create_dir_all(&dest_full)
            .await
            .map_err(|e| internal(e.to_string()))?;
        let mut rd = tokio::fs::read_dir(&src_full)
            .await
            .map_err(|e| internal(e.to_string()))?;
        while let Some(entry) = rd.next_entry().await.map_err(|e| internal(e.to_string()))? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child_src = format!("{src}/{name}");
            let child_dest = format!("{dest}/{name}");
            Box::pin(copy_tree(storage, &child_src, &child_dest)).await?;
        }
    } else {
        tokio::fs::copy(&src_full, &dest_full)
            .await
            .map_err(|e| internal(e.to_string()))?;
    }
    Ok(())
}

fn parse_destination(headers: &HeaderMap) -> Result<String, ApiError> {
    let raw = headers
        .get(&H_DESTINATION)
        .ok_or_else(|| bad_request("Destination header required"))?
        .to_str()
        .map_err(|_| bad_request("Invalid Destination header"))?;
    let parsed: Uri = raw
        .parse()
        .map_err(|_| bad_request("Invalid Destination URI"))?;
    Ok(parsed.path().to_string())
}

// ---------------------------------------------------------------------------
// PROPFIND
// ---------------------------------------------------------------------------

async fn propfind_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    trailing_slash: bool,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, ApiError> {
    require_storage_path_access(auth, &storage.model, path, &state.db).await?;

    let depth = parse_depth(headers);
    let pf = PropFind::parse(body);

    let full = storage.get_full_path(path);
    let metadata = tokio::fs::metadata(&full)
        .await
        .map_err(|e| map_io_to_api(e, path))?;

    let mut responses = Vec::new();

    responses.push(build_response(
        storage,
        path,
        &metadata,
        trailing_slash || metadata.is_dir(),
        &pf,
    ));

    // Depth: 1 → immediate children; infinity → whole subtree; 0 → self only.
    if depth != 0 && metadata.is_dir() {
        let entries = storage
            .list_directory_fs(path)
            .await
            .map_err(|e| internal(e.to_string()))?;
        for e in entries {
            let child_path = &e.path;
            let child_full = storage.get_full_path(child_path);
            if let Ok(child_meta) = tokio::fs::metadata(&child_full).await {
                responses.push(build_response(
                    storage,
                    child_path,
                    &child_meta,
                    matches!(e.entry_type, EntryType::Directory),
                    &pf,
                ));
                if depth == 255 && child_meta.is_dir() {
                    let mut stack = vec![child_path.clone()];
                    while let Some(d) = stack.pop() {
                        let sub = storage
                            .list_directory_fs(&d)
                            .await
                            .map_err(|e| internal(e.to_string()))?;
                        for s in sub {
                            let sfull = storage.get_full_path(&s.path);
                            if let Ok(sm) = tokio::fs::metadata(&sfull).await {
                                responses.push(build_response(
                                    storage,
                                    &s.path,
                                    &sm,
                                    matches!(s.entry_type, EntryType::Directory),
                                    &pf,
                                ));
                                if sm.is_dir() {
                                    stack.push(s.path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let (_, hdrs, xml) = multistatus(&responses);
    Ok((StatusCode::MULTI_STATUS, hdrs, xml).into_response())
}

fn parse_depth(headers: &HeaderMap) -> u8 {
    headers
        .get(&H_DEPTH)
        .and_then(|v| v.to_str().ok())
        .map(|s| match s.trim() {
            "0" => 0,
            "1" => 1,
            "infinity" => 255,
            _ => 1,
        })
        .unwrap_or(1)
    // RFC 4918 §9.1 actually says the default Depth for PROPFIND is infinity,
    // but many clients omit it and a huge-tree PROPFIND would OOM us. We
    // default to 1; clients wanting infinity must ask explicitly.
}

/// Build a single `<response>` element for a resource.
fn build_response(
    storage: &Storage,
    path: &str,
    metadata: &std::fs::Metadata,
    is_collection: bool,
    pf: &PropFind,
) -> DavResponse {
    let href = format!(
        "/dav/storage/{}/{}{}",
        storage.model.id,
        path,
        if is_collection { "/" } else { "" }
    );

    let mut props = Vec::new();

    if pf.wants("resourcetype") {
        let rt = if is_collection {
            "<D:collection/>".to_string()
        } else {
            String::new()
        };
        props.push(DavProp::raw("resourcetype", rt));
    }
    if pf.wants("displayname") {
        let name = path.rsplit('/').next().unwrap_or("").to_string();
        props.push(DavProp::text("displayname", name));
    }
    if pf.wants("getcontentlength") {
        props.push(DavProp::text(
            "getcontentlength",
            metadata.len().to_string(),
        ));
    }
    if pf.wants("getcontenttype") && !is_collection {
        let ct = determine_content_type(&storage.get_full_path(path), &[]);
        props.push(DavProp::text("getcontenttype", ct));
    }
    if pf.wants("getlastmodified") {
        let s = metadata
            .modified()
            .ok()
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
            })
            .unwrap_or_default();
        props.push(DavProp::text("getlastmodified", s));
    }
    if pf.wants("creationdate") {
        let s = metadata
            .created()
            .ok()
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            })
            .unwrap_or_default();
        props.push(DavProp::text("creationdate", s));
    }
    if pf.wants("getetag") {
        props.push(DavProp::text(
            "getetag",
            weak_etag(metadata.len(), metadata),
        ));
    }
    if pf.wants("supportedlock") {
        props.push(DavProp::raw(
            "supportedlock",
            "<D:lockentry><D:lockscope><D:exclusive/></D:lockscope>\
             <D:locktype><D:write/></D:locktype></D:lockentry>"
                .to_string(),
        ));
    }
    if pf.wants("lockdiscovery") {
        if let Some(lock) = LockManager::active(storage.model.id, path) {
            let expires_secs = lock
                .expires
                .duration_since(SystemTime::now())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let xml = format!(
                "<D:activelock>\
                 <D:locktype><D:write/></D:locktype>\
                 <D:lockscope><D:exclusive/></D:lockscope>\
                 <D:depth>{}</D:depth>\
                 <D:owner>{}</D:owner>\
                 <D:timeout>Second-{}</D:timeout>\
                 <D:locktoken><D:href>{}</D:href></D:locktoken>\
                 <D:lockroot><D:href>{}</D:href></D:lockroot>\
                 </D:activelock>",
                if lock.depth == 255 { "infinity" } else { "0" },
                escape_html(&lock.owner),
                expires_secs,
                escape_html(&lock.token),
                escape_html(&href),
            );
            props.push(DavProp::raw("lockdiscovery", xml));
        } else {
            props.push(DavProp::raw("lockdiscovery", String::new()));
        }
    }

    DavResponse {
        href,
        props,
        status: None,
    }
}

/// A weak validator ETag from size + mtime — cheap, no hash job needed on
/// every PROPFIND. Format: `W/"<hex-size>-<secs>"`.
fn weak_etag(size: u64, metadata: &std::fs::Metadata) -> String {
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("W/\"{size:x}-{mtime:x}\"")
}

// ---------------------------------------------------------------------------
// PROPPATCH
// ---------------------------------------------------------------------------

async fn proppatch_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    _body: &Bytes,
) -> Result<Response, ApiError> {
    require_storage_path_access(auth, &storage.model, path, &state.db).await?;

    // We don't persist arbitrary dead properties — the filesystem is the
    // source of truth for the live ones we expose. Per RFC 4918 §15 we must
    // still return a 207 acknowledging the request.
    let resp = DavResponse {
        href: format!("/dav/storage/{}/{}", storage.model.id, path),
        props: vec![],
        status: Some(StatusCode::OK),
    };
    let (_, hdrs, xml) = multistatus(&[resp]);
    Ok((StatusCode::MULTI_STATUS, hdrs, xml).into_response())
}

// ---------------------------------------------------------------------------
// LOCK / UNLOCK
// ---------------------------------------------------------------------------

async fn lock_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage: &Storage,
    path: &str,
    trailing_slash: bool,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, ApiError> {
    require_storage_path_write_access(auth, &storage.model, path, &state.db).await?;

    // If the resource doesn't exist yet, create an empty one (a lock can
    // "create" a resource per RFC 4918 §8.10.4).
    let full = storage.get_full_path(path);
    if !tokio::fs::try_exists(&full).await.unwrap_or(false) {
        if trailing_slash {
            storage
                .create_directory(path)
                .await
                .map_err(|e| internal(e.to_string()))?;
        } else {
            storage
                .save_file(path, b"")
                .await
                .map_err(|e| internal(e.to_string()))?;
        }
        let _ = storage.ensure_entry(&state.db, path).await;
    }

    let depth = parse_depth(headers);
    let timeout = parse_timeout(headers.get(&H_TIMEOUT));
    let owner = parse_lock_owner(body);
    let expires = std::time::SystemTime::now() + std::time::Duration::from_secs(timeout);

    let token = LockManager::lock(
        storage.model.id,
        path,
        owner.clone(),
        depth,
        timeout,
        auth.user.id,
    );

    // C4 Part C: persist the lock so it survives a restart. Best-effort.
    persist_lock(
        &state.db,
        storage.model.id,
        path,
        depth,
        &owner,
        auth.user.id,
        expires,
        &token,
    )
    .await;

    let lock_xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <D:prop xmlns:D=\"DAV:\">\
         <D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype>\
         <D:lockscope><D:exclusive/></D:lockscope>\
         <D:depth>{}</D:depth>\
         <D:timeout>Second-{}</D:timeout>\
         <D:locktoken><D:href>{}</D:href></D:locktoken>\
         <D:lockroot><D:href>/dav/storage/{}/{}</D:href></D:lockroot>\
         </D:activelock></D:lockdiscovery></D:prop>",
        if depth == 255 { "infinity" } else { "0" },
        timeout,
        escape_html(&token),
        storage.model.id,
        path,
    );

    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    hdrs.insert(
        H_LOCK_TOKEN,
        HeaderValue::from_str(&format!("<{}>", token)).unwrap(),
    );
    Ok((StatusCode::OK, hdrs, lock_xml).into_response())
}

fn parse_lock_owner(body: &Bytes) -> String {
    // Minimal: grab text inside `<D:owner>...</D:owner>` (or unqualified
    // `<owner>`). Falls back to empty.
    let s = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    for tag in ["<D:owner>", "<owner>"] {
        if let Some(start) = s.find(tag) {
            let rest = &s[start + tag.len()..];
            let end_tag = tag.replace('<', "</");
            if let Some(end) = rest.find(&end_tag) {
                return rest[..end].trim().to_string();
            }
        }
    }
    String::new()
}

async fn unlock_handler(
    auth: &Auth,
    state: &Arc<AppState>,
    storage_id: i32,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let token = headers
        .get(&H_LOCK_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
        .ok_or_else(|| bad_request("Lock-Token header required"))?;

    // C4 Part A: only the lock owner (or an admin) may release the token.
    if LockManager::unlock(storage_id, path, &token, auth.user.id, auth.user.admin) {
        // C4 Part C: best-effort delete of the persisted durability row.
        delete_lock_async(&state.db, &token).await;
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(conflict("Lock token not found or already released"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Enforce WebDAV exclusive locks before a write mutation (C4 Part B).
///
/// Translates a blocking [`LockConflict`] into an `ApiError::Locked` (→ 423)
/// so handlers can use `?`. RFC 4918 §10.6 requires the lock token in the body.
fn enforce_lock_for_write(
    storage_id: i32,
    path: &str,
    auth: &Auth,
    if_header_tokens: &[String],
) -> Result<(), ApiError> {
    match check_lock_for_write(
        storage_id,
        path,
        auth.user.id,
        auth.user.admin,
        if_header_tokens,
    ) {
        Ok(()) => Ok(()),
        Err(conflict) => Err(ApiError::Locked {
            lock_token: conflict.lock.token,
        }),
    }
}

fn map_io_to_api(e: std::io::Error, path: &str) -> ApiError {
    match e.kind() {
        std::io::ErrorKind::NotFound => not_found_msg(format!("Resource not found: {path}")),
        std::io::ErrorKind::PermissionDenied => internal(format!("Permission denied: {path}")),
        _ => internal(e.to_string()),
    }
}

/// HTML-escape `&`, `<`, `>`, and `"`.
///
/// This escapes `"` as `&quot;` unconditionally. Escaping a double quote in
/// HTML element content is harmless, and escaping it inside an attribute value
/// (e.g. `href="…"`) is *required* — a filename containing `"` would otherwise
/// break out of the attribute and allow attribute-injection / XSS. Keep this
/// in sync with [`super::util::push_escaped`] (which also escapes quotes when
/// `is_attr` is set, though there that flag is always-on for hrefs).
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Percent-encode a single path segment for use inside an `href="…"` attribute.
///
/// Filenames may contain spaces, `#`, `?`, `"`, and other characters that are
/// either invalid in a URL or would break out of the attribute; we encode
/// everything except the unreserved set plus `/` (so a directory name with a
/// literal slash — impossible on POSIX, but harmless — round-trips). The result
/// still contains only ASCII, so it is safe to interpolate into an HTML
/// attribute without further escaping.
fn encode_path_segment(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

    // Reserved + delimiters that have meaning in a URL path. We keep `/` so a
    // segment containing a slash round-trips; everything else that's special
    // in a URL or HTML attribute gets percent-encoded.
    const SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');

    utf8_percent_encode(s, SEGMENT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_escapes_quotes() {
        // Regression for H5: a double quote must be escaped so it cannot break
        // out of an `href="…"` attribute.
        assert_eq!(escape_html(r#"a"b"#), "a&quot;b");
        assert_eq!(escape_html("<>&\""), "&lt;&gt;&amp;&quot;");
        // & is escaped first, so a literal &quot; survives intact.
        assert_eq!(escape_html("&"), "&amp;");
    }

    #[test]
    fn encode_path_segment_handles_dangerous_chars() {
        // A quote must be percent-encoded so the attribute can't be broken,
        // even though we also HTML-escape it elsewhere.
        assert_eq!(encode_path_segment(r#"a"b"#), "a%22b");
        // Spaces, fragments, and queries are encoded so the URL stays valid.
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("a#b"), "a%23b");
        assert_eq!(encode_path_segment("a?b"), "a%3Fb");
        // Unreserved characters pass through untouched.
        assert_eq!(encode_path_segment("plain.txt"), "plain.txt");
        assert_eq!(encode_path_segment("café.md"), "caf%C3%A9.md");
    }

    /// Render just the `<li>` line for one entry, mirroring `directory_listing`.
    fn render_li(name: &str, is_dir: bool) -> String {
        let slash = if is_dir { "/" } else { "" };
        let href_name = encode_path_segment(name);
        let text_name = escape_html(name);
        format!("<li><a href=\"{href_name}{slash}\">{text_name}{slash}</a></li>")
    }

    #[test]
    fn directory_li_escapes_attribute_breakout() {
        // The original bug: a filename with a `"` broke out of the href
        // attribute. Both the href (percent-encoded) and the text
        // (HTML-escaped) must now be inert.
        let li = render_li(r#"evil".txt"#, false);
        // The href must contain no raw double-quote between its opening and
        // closing quote — i.e. the quote must be percent-encoded as %22.
        assert!(
            li.contains("href=\"evil%22.txt\""),
            "quote must be percent-encoded in href: {li}"
        );
        // And the link text must have the quote HTML-escaped.
        assert!(li.contains(">evil&quot;.txt<"));
        // No raw `"` may appear anywhere except as the attribute delimiters.
        assert_eq!(
            li.matches('"').count(),
            2,
            "only the two href attribute delimiters may be raw quotes: {li}"
        );
    }

    #[test]
    fn directory_li_keeps_url_valid_for_spaces() {
        // A space in a filename must not produce a raw space in the href.
        let li = render_li("my file.txt", false);
        assert!(li.contains("href=\"my%20file.txt\""));
        assert!(li.contains(">my file.txt<"));
    }

    #[test]
    fn directory_li_for_directory_has_trailing_slash() {
        let li = render_li("subdir", true);
        assert!(li.contains("href=\"subdir/\""));
        assert!(li.contains(">subdir/</a>"));
    }
}
