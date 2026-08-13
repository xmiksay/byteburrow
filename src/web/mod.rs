pub mod dav;
pub mod group;
pub mod photo;
pub mod rate_limit;
pub mod storage;
pub mod tag;
pub mod user;
pub mod ws;

use crate::auth::Auth;
use crate::config::Config;
use crate::job::JobSender;
use crate::storage::format_size;
use axum::{
    extract::State,
    http::{header, HeaderValue, Method, StatusCode, Uri},
    middleware::from_fn,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use minijinja::Environment;
use rust_embed::Embed;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

async fn frontend_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first, then fall back to index.html (SPA routing)
    let file = if path.is_empty() {
        FrontendAssets::get("index.html")
    } else {
        FrontendAssets::get(path).or_else(|| FrontendAssets::get("index.html"))
    };

    match file {
        Some(content) => {
            let mime = mime_guess::from_path(if FrontendAssets::get(path).is_some() {
                path
            } else {
                "index.html"
            })
            .first_or_octet_stream();

            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// Error response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

/// A generic `{ "message": "..." }` acknowledgement body.
///
/// The single envelope for endpoints whose only useful response is a
/// human-readable confirmation (deletes, queued jobs, tag updates, …).
/// Replaces the scattered ad-hoc `Json(json!({ "message": ... }))` so every
/// such endpoint shares one typed, OpenAPI-documented shape.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MessageResponse {
    pub message: String,
}

/// Build a `Json(MessageResponse { .. })` from anything string-like.
pub fn message<S: Into<String>>(msg: S) -> Json<MessageResponse> {
    Json(MessageResponse {
        message: msg.into(),
    })
}

/// Query parameters shared by every paginated list endpoint:
/// `?page=1&per_page=50`. `page` is 1-based. The accessors clamp both values,
/// so a missing, zero, or oversized parameter can never produce an unbounded
/// or malformed query.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Pagination {
    /// 1-based page number (default 1).
    pub page: Option<u64>,
    /// Items per page (default 50, capped at 200).
    pub per_page: Option<u64>,
}

impl Pagination {
    /// Default page size when the client does not specify `per_page`.
    pub const DEFAULT_PER_PAGE: u64 = 50;
    /// Hard upper bound on page size — the guard against unbounded payloads.
    pub const MAX_PER_PAGE: u64 = 200;

    /// The requested 1-based page, never below 1.
    pub fn page(&self) -> u64 {
        self.page.unwrap_or(1).max(1)
    }

    /// The requested page size, clamped to `1..=MAX_PER_PAGE`.
    pub fn per_page(&self) -> u64 {
        self.per_page
            .unwrap_or(Self::DEFAULT_PER_PAGE)
            .clamp(1, Self::MAX_PER_PAGE)
    }

    /// 0-based page index for SeaORM's `fetch_page` / manual `OFFSET`.
    pub fn page_index(&self) -> u64 {
        self.page() - 1
    }
}

/// A single page of results plus the metadata a client needs to walk the rest.
/// The uniform envelope returned by every paginated list endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Page<T> {
    /// Items in this page.
    pub items: Vec<T>,
    /// 1-based page number this response represents.
    pub page: u64,
    /// Page size used to build this response.
    pub per_page: u64,
    /// Total number of items across all pages.
    pub total: u64,
    /// Total number of pages at this `per_page` (at least 1).
    pub total_pages: u64,
}

impl<T> Page<T> {
    /// Assemble a page from a fetched slice and the total item count.
    pub fn new(items: Vec<T>, total: u64, pagination: &Pagination) -> Self {
        let per_page = pagination.per_page();
        Self {
            items,
            page: pagination.page(),
            per_page,
            total,
            total_pages: total.div_ceil(per_page).max(1),
        }
    }
}

/// Unified error type for the web layer.
///
/// Every variant logs internally (in [`IntoResponse`]) and renders as a JSON
/// `ErrorResponse` body, so handlers can simply use `?` and `.map_err(...)`
/// without repeating the boilerplate tuple `(StatusCode, Json<ErrorResponse>)`.
#[derive(Debug)]
pub enum ApiError {
    /// A database error (logged, maps to 500 Internal Server Error).
    Db(sea_orm::DbErr),
    /// An entity lookup returned no row (404 Not Found).
    NotFound { entity: &'static str, id: i32 },
    /// A 404 with a fully custom message (e.g. "Photo not found").
    NotFoundMsg { msg: String },
    /// A conflicting resource already exists (409 Conflict).
    Conflict { msg: String },
    /// The request was malformed / invalid (400 Bad Request).
    BadRequest { msg: String },
    /// The caller is not permitted to perform the action (403 Forbidden).
    Forbidden { msg: String },
    /// The caller must authenticate to perform the action (401 Unauthorized).
    Unauthorized,
    /// The caller must authenticate; carries a specific message body
    /// (401 Unauthorized + WWW-Authenticate header).
    UnauthorizedMsg { msg: String },
    /// The requested share has expired (410 Gone).
    Gone { msg: String },
    /// The resource is locked by another user (423 Locked). Carries the
    /// conflicting lock so the body can include its token per RFC 4918 §10.6.
    /// (C4 Part B.)
    Locked { lock_token: String },
    /// The caller has sent too many requests and is being throttled
    /// (429 Too Many Requests). Carries the window length so the response can
    /// advise the client how long to wait via `Retry-After`. (M1 + H16.)
    RateLimited { retry_after_secs: u64 },
    /// Any other internal failure (500 Internal Server Error).
    Internal { msg: String },
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            ApiError::Db(e) => {
                tracing::error!("Database error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            ApiError::NotFound { entity, id } => {
                (StatusCode::NOT_FOUND, format!("{entity} {id} not found"))
            }
            ApiError::NotFoundMsg { msg } => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::Conflict { msg } => (StatusCode::CONFLICT, msg.clone()),
            ApiError::BadRequest { msg } => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Forbidden { msg } => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Authentication required".to_string(),
            ),
            ApiError::UnauthorizedMsg { msg } => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Gone { msg } => (StatusCode::GONE, msg.clone()),
            ApiError::Locked { lock_token } => {
                // RFC 4918 §10.6: a 423 response carries an XML error body
                // naming the lock that blocked the request.
                let body = format!(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                     <D:error xmlns:D=\"DAV:\">\
                     <D:lock-token-submitted>\
                     <D:locktoken><D:href>{token}</D:href></D:locktoken>\
                     </D:lock-token-submitted>\
                     </D:error>",
                    token = lock_token
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                );
                let mut resp = (
                    StatusCode::LOCKED,
                    [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                )
                    .into_response();
                *resp.body_mut() = axum::body::Body::from(body);
                return resp;
            }
            ApiError::RateLimited { retry_after_secs } => {
                // 429 Too Many Requests with a `Retry-After` hint (RFC 9110
                // §15.6.4) so well-behaved clients back off for the configured
                // window before retrying.
                let body = Json(ErrorResponse {
                    error: "Too many requests".to_string(),
                });
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(
                        header::RETRY_AFTER,
                        HeaderValue::from_str(&retry_after_secs.to_string()).unwrap(),
                    )],
                    body,
                )
                    .into_response();
            }
            ApiError::Internal { msg } => {
                tracing::error!("Internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
        };

        if matches!(self, ApiError::Unauthorized) {
            (
                status,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"Cloud\"")],
                Json(ErrorResponse { error: message }),
            )
                .into_response()
        } else {
            (status, Json(ErrorResponse { error: message })).into_response()
        }
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(e: sea_orm::DbErr) -> Self {
        if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
            ApiError::NotFoundMsg { msg: e.to_string() }
        } else {
            ApiError::Db(e)
        }
    }
}

/// Construct a database error (used as `.map_err(db_err)?` for readability).
pub fn db_err(e: sea_orm::DbErr) -> ApiError {
    ApiError::Db(e)
}

/// Construct a forbidden error with a specific message.
pub fn forbidden<S: Into<String>>(msg: S) -> ApiError {
    ApiError::Forbidden { msg: msg.into() }
}

/// Construct a not-found error for a given entity and id.
pub fn not_found(entity: &'static str, id: i32) -> ApiError {
    ApiError::NotFound { entity, id }
}

/// Construct a not-found error with a custom message body.
pub fn not_found_msg<S: Into<String>>(msg: S) -> ApiError {
    ApiError::NotFoundMsg { msg: msg.into() }
}

/// Construct a conflict error with a message.
pub fn conflict<S: Into<String>>(msg: S) -> ApiError {
    ApiError::Conflict { msg: msg.into() }
}

/// Construct a bad-request error with a message.
pub fn bad_request<S: Into<String>>(msg: S) -> ApiError {
    ApiError::BadRequest { msg: msg.into() }
}

/// Construct an internal-error with a message.
pub fn internal<S: Into<String>>(msg: S) -> ApiError {
    ApiError::Internal { msg: msg.into() }
}

/// Construct an unauthorized error (401 + WWW-Authenticate) with a message.
pub fn unauthorized_msg<S: Into<String>>(msg: S) -> ApiError {
    ApiError::UnauthorizedMsg { msg: msg.into() }
}

/// Construct a rate-limited error (429 + `Retry-After`). `retry_after_secs`
/// should be the limiter's window length, so well-behaved clients know how long
/// to back off before retrying.
pub fn rate_limited(retry_after_secs: u64) -> ApiError {
    ApiError::RateLimited { retry_after_secs }
}

// ---------------------------------------------------------------------------
// Unique-name validation (DRY-2)
// ---------------------------------------------------------------------------

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

/// Per-entity uniqueness checks. Each helper queries the right column and
/// returns `Ok(true)` if the name/path is already in use. Combine with the
/// [`conflict`] helper at call sites:
///
/// ```ignore
/// if user_name_taken(db, &payload.username).await? {
///     return Err(conflict(format!("Username '{}' already exists", payload.username)));
/// }
/// ```
pub async fn user_name_taken(db: &DatabaseConnection, name: &str) -> Result<bool, ApiError> {
    use crate::entity::user;
    Ok(user::Entity::find()
        .filter(user::Column::Username.eq(name))
        .one(db)
        .await?
        .is_some())
}

pub async fn group_name_taken(db: &DatabaseConnection, name: &str) -> Result<bool, ApiError> {
    use crate::entity::group;
    Ok(group::Entity::find()
        .filter(group::Column::Name.eq(name))
        .one(db)
        .await?
        .is_some())
}

pub async fn tag_name_taken(db: &DatabaseConnection, name: &str) -> Result<bool, ApiError> {
    use crate::entity::tag;
    Ok(tag::Entity::find()
        .filter(tag::Column::Name.eq(name))
        .one(db)
        .await?
        .is_some())
}

pub async fn storage_name_taken(db: &DatabaseConnection, name: &str) -> Result<bool, ApiError> {
    use crate::entity::storage;
    Ok(storage::Entity::find()
        .filter(storage::Column::Name.eq(name))
        .one(db)
        .await?
        .is_some())
}

pub async fn storage_path_taken(db: &DatabaseConnection, path: &str) -> Result<bool, ApiError> {
    use crate::entity::storage;
    Ok(storage::Entity::find()
        .filter(storage::Column::Path.eq(path))
        .one(db)
        .await?
        .is_some())
}

// ---------------------------------------------------------------------------
// Generic SeaORM persistence helpers (DRY-3)
// ---------------------------------------------------------------------------

use sea_orm::{ActiveModelBehavior, ActiveModelTrait, IntoActiveModel};

/// Insert an active model, mapping any DB error to [`ApiError`].
pub async fn insert_or_err<A, E>(am: A, db: &DatabaseConnection) -> Result<E::Model, ApiError>
where
    E: EntityTrait,
    A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send + Sync + 'static,
    E::Model: IntoActiveModel<A>,
{
    Ok(am.insert(db).await?)
}

/// Save (update) an active model, mapping any DB error to [`ApiError`].
pub async fn save_or_err<A, E>(am: A, db: &DatabaseConnection) -> Result<E::Model, ApiError>
where
    E: EntityTrait,
    A: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send + Sync + 'static,
    E::Model: IntoActiveModel<A>,
{
    Ok(am.update(db).await?)
}

/// If `value` is `Some`, overwrite `*field` with it. Used to apply optional
/// `Option<T>` fields from update payloads without repeating `if let Some`.
pub fn apply_optional<T>(field: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *field = v;
    }
}

/// Helper function to check if user is admin
pub fn require_admin(auth: &Auth) -> Result<(), ApiError> {
    if !auth.user.admin {
        return Err(forbidden("Admin access required"));
    }
    Ok(())
}

/// Helper function to verify a user exists by ID
pub async fn require_user_exists(user_id: i32, db: &DatabaseConnection) -> Result<(), ApiError> {
    let exists = crate::auth::user_exists(user_id, db).await?;
    if !exists {
        return Err(bad_request(format!("User with id {user_id} not found")));
    }
    Ok(())
}

/// Helper function to verify a group exists by ID
pub async fn require_group_exists(group_id: i32, db: &DatabaseConnection) -> Result<(), ApiError> {
    let exists = crate::auth::group_exists(group_id, db).await?;
    if !exists {
        return Err(bad_request(format!("Group with id {group_id} not found")));
    }
    Ok(())
}

/// Load all group ids the given user belongs to.
pub async fn user_group_ids(db: &DatabaseConnection, user_id: i32) -> Result<Vec<i32>, ApiError> {
    use crate::entity::group_user;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    Ok(group_user::Entity::find()
        .filter(group_user::Column::UserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|gu| gu.group_id)
        .collect())
}

/// Authorization check for accessing a storage.
///
/// A user may access a storage if any of the following holds:
/// - they are an admin;
/// - they are the storage's default owner (`storage.default_user`);
/// - they are a member of the storage's default group (`storage.default_group`);
/// - there is a share (in the `shared` table) whose `user_ids` contains the user
///   or whose `group_ids` intersects the user's groups, referencing any entry
///   within this storage.
///
/// This only establishes that the user has *some* connection to the storage
/// (e.g. for metadata/listing purposes) — a share only entitles its holder to
/// the shared entry's own subtree, not the whole storage. Handlers that serve
/// or mutate content at a specific path must use [`require_storage_path_access`]
/// or [`require_storage_path_write_access`] instead, which enforce that
/// scoping.
///
/// Use this on every storage file/entry handler to prevent IDOR. Returns `Ok`
/// on success; returns [`ApiError::Forbidden`] on denial.
///
/// The overall share-access model (share ⇒ shared entry's subtree, with this
/// function's storage-metadata visibility as the deliberate exception) is
/// recorded in ADR 0005 (`docs/adr/0005-share-access-scope.md`).
pub async fn require_storage_access(
    auth: &Auth,
    storage: &crate::entity::storage::Model,
    db: &DatabaseConnection,
) -> Result<(), ApiError> {
    // Admins can access everything.
    if auth.user.admin {
        return Ok(());
    }
    let user_id = auth.user.id;

    // Direct ownership.
    if is_owner(storage, user_id) {
        return Ok(());
    }

    // The user's group memberships — loaded ONCE and reused for both the
    // storage-default-group check and the share-recipient check.
    let groups = user_group_ids(db, user_id).await?;

    // Membership in the storage's default group.
    if groups.contains(&storage.default_group) {
        return Ok(());
    }

    // Shares referencing any entry in this storage.
    if has_share_for_storage(db, storage.id, user_id, &groups).await? {
        return Ok(());
    }

    Err(forbidden("Access denied to this storage"))
}

/// Load the set of storage ids a non-admin user is authorized to *see*
/// (for listing purposes, mirroring [`require_storage_access`]): they own
/// the storage, are in its default group, or have a share referencing any
/// entry in it.
///
/// Returns `Ok(None)` for admins — meaning "no filter, all storages". For
/// non-admins returns `Ok(Some(ids))`, which is empty when the user has
/// access to no storage at all. Callers use the result to scope photo
/// listings (see `src/web/photo.rs`): a photo is visible to a non-admin iff
/// at least one of its entries lives in an accessible storage.
pub async fn accessible_storage_ids(
    auth: &Auth,
    db: &DatabaseConnection,
) -> Result<Option<Vec<i32>>, ApiError> {
    if auth.user.admin {
        return Ok(None);
    }
    let user_id = auth.user.id;
    let groups = user_group_ids(db, user_id).await?;

    let mut ids = Vec::new();
    for storage in crate::entity::storage::Entity::find().all(db).await? {
        if is_owner(&storage, user_id)
            || groups.contains(&storage.default_group)
            || has_share_for_storage(db, storage.id, user_id, &groups).await?
        {
            ids.push(storage.id);
        }
    }
    Ok(Some(ids))
}

/// The user is the storage's default owner.
fn is_owner(storage: &crate::entity::storage::Model, user_id: i32) -> bool {
    storage.default_user == user_id
}

/// The user belongs to at least one of the storage's groups.
#[allow(dead_code)]
fn is_group_member(groups: &[i32], group_id: i32) -> bool {
    groups.contains(&group_id)
}

/// There is a share in this storage whose `user_ids` contains the user or
/// whose `group_ids` intersects `groups`.
async fn has_share_for_storage(
    db: &DatabaseConnection,
    storage_id: i32,
    user_id: i32,
    groups: &[i32],
) -> Result<bool, ApiError> {
    use crate::entity::{entry, shared};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    // Any entry id in this storage (shares reference entries by id via path_id).
    let entry_ids: Vec<i32> = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .all(db)
        .await?
        .into_iter()
        .map(|e| e.id)
        .collect();

    if entry_ids.is_empty() {
        return Ok(false);
    }

    // Load all shares targeting entries in this storage, then check
    // user/group membership in Rust (the user_ids/group_ids are Postgres
    // array columns; materializing them is simplest and correct).
    let shares = shared::Entity::find()
        .filter(shared::Column::PathId.is_in(entry_ids))
        .all(db)
        .await?;

    for share in shares {
        if share.user_ids.contains(&user_id) {
            return Ok(true);
        }
        if share.group_ids.iter().any(|g| groups.contains(g)) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Authorization check for accessing a specific path within a storage
/// (read: browsing/downloading). Unlike [`require_storage_access`], a
/// share only grants access to the shared entry's own subtree, not the
/// whole storage.
pub async fn require_storage_path_access(
    auth: &Auth,
    storage: &crate::entity::storage::Model,
    path: &str,
    db: &DatabaseConnection,
) -> Result<(), ApiError> {
    require_storage_path_access_impl(auth, storage, path, false, db).await
}

/// Authorization check for mutating a specific path within a storage
/// (create/rename/remove/update content). Same subtree scoping as
/// [`require_storage_path_access`], plus the share must have `can_write`.
pub async fn require_storage_path_write_access(
    auth: &Auth,
    storage: &crate::entity::storage::Model,
    path: &str,
    db: &DatabaseConnection,
) -> Result<(), ApiError> {
    require_storage_path_access_impl(auth, storage, path, true, db).await
}

async fn require_storage_path_access_impl(
    auth: &Auth,
    storage: &crate::entity::storage::Model,
    path: &str,
    require_write: bool,
    db: &DatabaseConnection,
) -> Result<(), ApiError> {
    // Admins can access everything.
    if auth.user.admin {
        return Ok(());
    }
    let user_id = auth.user.id;

    // Direct ownership and default-group membership grant full read/write
    // access to the whole storage, regardless of shares.
    if is_owner(storage, user_id) {
        return Ok(());
    }
    let groups = user_group_ids(db, user_id).await?;
    if groups.contains(&storage.default_group) {
        return Ok(());
    }

    if has_share_for_path(db, storage.id, path, user_id, &groups, require_write).await? {
        return Ok(());
    }

    Err(forbidden("Access denied to this path"))
}

/// The ordered list of ancestor paths of `path`, from the storage root
/// (`""`) down to `path` itself, e.g. `"a/b/c"` -> `["", "a", "a/b", "a/b/c"]`.
fn path_prefixes(path: &str) -> Vec<String> {
    let normalized = path.trim_matches('/');
    let mut prefixes = vec![String::new()];
    let mut acc = String::new();
    for segment in normalized.split('/').filter(|s| !s.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(segment);
        prefixes.push(acc.clone());
    }
    prefixes
}

/// There is a share, on an entry that is `path` itself or an ancestor
/// directory of `path`, whose `user_ids` contains the user or whose
/// `group_ids` intersects `groups`. When `require_write` is set, only
/// shares with `can_write` count.
async fn has_share_for_path(
    db: &DatabaseConnection,
    storage_id: i32,
    path: &str,
    user_id: i32,
    groups: &[i32],
    require_write: bool,
) -> Result<bool, ApiError> {
    use crate::entity::{entry, shared};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    // Entries in this storage whose path is an ancestor of (or equal to)
    // the requested path — i.e. the requested path lies within their subtree.
    let entry_ids: Vec<i32> = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.is_in(path_prefixes(path)))
        .all(db)
        .await?
        .into_iter()
        .map(|e| e.id)
        .collect();

    if entry_ids.is_empty() {
        return Ok(false);
    }

    let shares = shared::Entity::find()
        .filter(shared::Column::PathId.is_in(entry_ids))
        .all(db)
        .await?;

    for share in shares {
        if require_write && !share.can_write {
            continue;
        }
        if share.user_ids.contains(&user_id) {
            return Ok(true);
        }
        if share.group_ids.iter().any(|g| groups.contains(g)) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Authorization check for managing shares (list/create/update/delete) on an entry.
///
/// A user may manage shares on an entry if any of the following holds:
/// - they are an admin;
/// - they are the entry's owner (`entry.user_id`);
/// - they are a member of the entry's owning group (`entry.group_id`).
///
/// This is deliberately stricter than [`require_storage_access`]: merely having
/// a share of the entry (i.e. being a recipient) does not grant the right to
/// manage that entry's shares.
pub async fn require_entry_owner(
    auth: &Auth,
    entry: &crate::entity::entry::Model,
    db: &DatabaseConnection,
) -> Result<(), ApiError> {
    if auth.user.admin {
        return Ok(());
    }
    let user_id = auth.user.id;

    if entry.user_id == user_id {
        return Ok(());
    }

    let groups = user_group_ids(db, user_id).await?;
    if groups.contains(&entry.group_id) {
        return Ok(());
    }

    Err(forbidden("Access denied to this entry's shares"))
}

pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Config,
    pub jinja: Environment<'static>,
    pub job_sender: JobSender,
    /// Signal to the inotify handler that watched entries changed and should
    /// be reloaded immediately (H12 — avoids up to a 60s delay before a newly
    /// watched directory is actually watched).
    pub notify_reload: Arc<tokio::sync::Notify>,
}

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ByteBurrow API",
        version = "1.0.0",
        description = "REST API for ByteBurrow personal cloud storage",
    ),
    paths(
        // Meta endpoints
        health_handler,
        version_handler,
        // User endpoints
        user::login_handler,
        user::logout_handler,
        user::me_handler,
        user::list_users_handler,
        user::get_user_handler,
        user::create_user_handler,
        user::update_user_handler,
        user::delete_user_handler,
        user::change_password_handler,
        // Group endpoints
        group::list_groups_handler,
        group::get_group_handler,
        group::create_group_handler,
        group::update_group_handler,
        group::delete_group_handler,
        // Tag endpoints
        tag::list_tags_handler,
        tag::get_tag_handler,
        tag::create_tag_handler,
        tag::update_tag_handler,
        tag::delete_tag_handler,
        // Storage CRUD endpoints
        storage::list_storages_handler,
        storage::get_storage_handler,
        storage::create_storage_handler,
        storage::update_storage_handler,
        storage::delete_storage_handler,
        // File content endpoints
        storage::get_file_content_handler,
        storage::download_file_handler,
        storage::update_file_content_handler,
        // Entry management endpoints
        storage::create_entry_handler,
        storage::rename_entry_handler,
        storage::update_entry_tags_handler,
        storage::remove_entry_handler,
        storage::list_directory_handler,
        // Share endpoints
        storage::list_shares_handler,
        storage::list_all_user_shares_handler,
        storage::list_shares_with_me_handler,
        storage::share_entry_handler,
        storage::delete_share_handler,
        storage::update_share_handler,
        storage::get_share_info_handler,
        storage::share_list_root_handler,
        storage::share_list_handler,
        storage::share_show_handler,
        storage::share_update_handler,
        storage::share_create_handler,
        storage::share_rename_handler,
        storage::share_remove_handler,
        storage::share_update_entry_tags_handler,
        // Thumbnail endpoints
        storage::thumbnail_handler,
        storage::trigger_hash_handler,
        // Meta endpoints
        storage::get_meta_handler,
        // Photo endpoints
        photo::list_photos,
        photo::list_by_year,
        photo::list_by_year_month,
        photo::list_by_year_month_day,
        photo::regenerate_thumbnail,
    ),
    components(
        schemas(
            ErrorResponse,
            MessageResponse,
            HealthResponse,
            VersionResponse,
            Page<user::UserResponse>,
            Page<group::GroupResponse>,
            Page<tag::TagResponse>,
            Page<storage::StorageResponse>,
            user::MeResponse,
            user::LoginRequest,
            user::LoginResponse,
            user::UserResponse,
            user::CreateUserRequest,
            user::UpdateUserRequest,
            user::ChangePasswordRequest,
            group::GroupResponse,
            group::CreateGroupRequest,
            group::UpdateGroupRequest,
            tag::TagResponse,
            tag::CreateTagRequest,
            tag::UpdateTagRequest,
            storage::StorageResponse,
            storage::CreateStorageRequest,
            storage::UpdateStorageRequest,
            storage::CreateEntryRequest,
            storage::RenameEntryRequest,
            storage::UpdateEntryTagsRequest,
            storage::ShareEntryRequest,
            storage::ShareResponse,
            crate::entity::entry::EntryType,
            crate::storage::DirectoryEntry,
            photo::PhotoResponse,
            storage::MetaResponse,
            storage::DirectoryListingResponse,
            storage::ShareInfoResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "user", description = "User management endpoints"),
        (name = "group", description = "Group management endpoints"),
        (name = "tag", description = "Tag management endpoints"),
        (name = "storage", description = "Storage CRUD endpoints"),
        (name = "file", description = "File content operations"),
        (name = "entry", description = "Entry management (create, rename, remove, list)"),
        (name = "share", description = "Sharing operations"),
        (name = "thumbnail", description = "Thumbnail and hash operations"),
        (name = "meta", description = "File meta information endpoints"),
        (name = "photo", description = "Photo management endpoints"),
    )
)]
struct ApiDoc;

/// Serialize the OpenAPI spec as pretty-printed JSON.
///
/// Exposed so tooling (the `byteburrow-cli openapi` command) can dump the exact
/// same document served at `/api/docs/openapi.json` without starting the server
/// or touching the database. The frontend TypeScript client is generated from
/// this output, so it stays the single source of truth for request/response
/// types and endpoints.
pub fn openapi_json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI document is always serializable")
}

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Stateless CSRF defense (C5)
// ---------------------------------------------------------------------------
//
// `SameSite=Strict` + `HttpOnly` cookies mitigate CSRF at the browser level,
// but if an operator ever opens `CORS_ALLOWED_ORIGINS` broadly (the CORS layer
// uses `allow_credentials(true)`), credentialed cross-origin mutation becomes
// possible. This is a server-side, stateless, OWASP-recommended backstop using
// the `Sec-Fetch-Site` header (primary) with an `Origin` fallback. No tokens,
// no storage, no frontend changes.
//
// Only unsafe methods (POST/PUT/DELETE/PATCH) are checked; safe methods and
// OPTIONS preflight always pass through.

/// An HTTP method that mutates server state and therefore requires a CSRF
/// check. GET/HEAD/OPTIONS/TRACE are never checked.
fn is_unsafe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    )
}

/// Derive the server's own origin (scheme + host + port, no path) from
/// `base_url`. Returns the trimmed origin string, or the raw `base_url` if it
/// has no path component to strip (the common case where `base_url` is already
/// just an origin like `http://localhost:3000`).
fn origin_from_base_url(base_url: &str) -> String {
    // base_url is `http(s)://host[:port]` — almost always already origin-only.
    // Strip a trailing path/query if one is present.
    let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    let scheme_prefix_len = base_url.len() - after_scheme.len();
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    base_url[..scheme_prefix_len + authority.len()].to_owned()
}

/// The pure decision core of the CSRF guard. Extracted so it can be unit-tested
/// in isolation without spinning up a server.
///
/// Inputs:
/// - `method`: the HTTP method.
/// - `sec_fetch_site`: the value of the `Sec-Fetch-Site` request header, if
///   the client sent one.
/// - `origin`: the value of the `Origin` request header, if present.
/// - `own_origin`: the server's own origin derived from `base_url`.
/// - `allowed_origins`: the trusted cross-origin allowlist (from
///   `cors_allowed_origins`), as an iterator of trimmed origin strings.
///
/// Returns `true` to allow, `false` to reject with 403.
fn csrf_decision<'a, I>(
    method: &Method,
    sec_fetch_site: Option<&str>,
    origin: Option<&str>,
    own_origin: &str,
    allowed_origins: I,
) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    // Safe methods are never checked.
    if !is_unsafe_method(method) {
        return true;
    }

    // 1. Sec-Fetch-Site is the primary signal (modern browsers send it on all
    //    fetches). It is unspoofable from a cross-origin context.
    if let Some(site) = sec_fetch_site {
        return matches!(site, "same-origin" | "same-site" | "none");
    }

    // 2. No Sec-Fetch-Site → either a non-browser client (curl, desktop DAV
    //    app) or an older browser. Fall back to Origin.
    let Some(origin) = origin else {
        // No Origin header at all → legitimate non-browser API client. Allow.
        return true;
    };

    // Origin present: allow if it is our own origin or explicitly trusted.
    origin == own_origin || allowed_origins.into_iter().any(|o| o == origin)
}

/// The `Origin`/`Sec-Fetch-Site` headers are case-insensitive names but we read
/// them once here to avoid repeating the lookup. [`HeaderMap::get`] returns the
/// first value; both headers are single-valued in practice.
fn read_csrf_headers(headers: &axum::http::HeaderMap) -> (Option<String>, Option<String>) {
    let sec_fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (sec_fetch_site, origin)
}

/// Axum middleware: stateless CSRF guard for unsafe methods.
///
/// See [`csrf_decision`] for the decision logic. Reads config from the global
/// `Config` singleton (the server sets it before serving).
async fn csrf_guard(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().clone();

    if !is_unsafe_method(&method) {
        return next.run(request).await;
    }

    let (sec_fetch_site, origin) = read_csrf_headers(request.headers());
    let config = Config::get();
    let own_origin = origin_from_base_url(&config.base_url);
    let allowed: Vec<&str> = config
        .cors_allowed_origins
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let allowed_decision = csrf_decision(
        &method,
        sec_fetch_site.as_deref(),
        origin.as_deref(),
        &own_origin,
        allowed.iter().copied(),
    );

    if allowed_decision {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Cross-site request blocked".to_string(),
            }),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// Security response headers (M2) + HSTS
// ---------------------------------------------------------------------------
//
// A single `from_fn` middleware stamps a baseline set of browser security
// headers on *every* response (200s, 4xx from CSRF/CORS, 5xx errors). Being
// the outermost layer, it runs after TraceLayer/CORS/CSRF and decorates even
// short-circuited responses. The heavy lifting is split into pure helpers so
// the HSTS conditional and the exact directive strings are unit-testable
// without a server.
//
// `Referrer-Policy: no-referrer` doubles as the H16 fix: share tokens ride in
// the URL path (`/s/<token>`), and `no-referrer` guarantees no `Referer` header
// leaks them to any third-party origin the rendered page might contact.

/// `Strict-Transport-Security` value, sent only over HTTPS deployments.
///
/// HSTS is a *positive* instruction: a browser that sees it over plain HTTP
/// would simply ignore it, but a man-in-the-middle on the first HTTP hop could
/// strip it. We therefore only emit it when the operator has explicitly told us
/// the service is fronted by HTTPS (`base_url` starts with `https://`), so we
/// never imply a guarantee the deployment can't back up.
fn hsts_header(base_url: &str) -> Option<&'static str> {
    base_url
        .starts_with("https://")
        .then_some("max-age=31536000; includeSubDomains")
}

/// Strict Content-Security-Policy.
///
/// `frontend/dist/index.html` references only an external `type="module"`
/// script under `/assets/` — no inline `<script>`, no inline event handlers —
/// so `script-src 'self'` is safe *without* `'unsafe-inline'`. This is
/// defense-in-depth: even if the C2 DOMPurify escape hatch let markup through,
/// or a `/dav/` directory listing rendered an attacker-controlled filename,
/// inline `<script>`/`on*=` are blocked at the browser.
fn csp_header() -> &'static str {
    "default-src 'self'; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; \
     font-src 'self'; \
     connect-src 'self'; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'"
}

/// Pure decision core for the security-headers middleware: returns the value
/// to set (or `None` to leave the header unset) for a given header name.
/// Extracted so the per-header policy — including the HSTS conditional — is
/// unit-testable in isolation.
fn security_header_value(name: &header::HeaderName, base_url: &str) -> Option<&'static str> {
    match *name {
        header::X_CONTENT_TYPE_OPTIONS => Some("nosniff"),
        header::X_FRAME_OPTIONS => Some("DENY"),
        header::REFERRER_POLICY => Some("no-referrer"),
        header::CONTENT_SECURITY_POLICY => Some(csp_header()),
        // HSTS is conditional on the deployment scheme — see `hsts_header`.
        _ if *name == header::STRICT_TRANSPORT_SECURITY => hsts_header(base_url),
        _ => None,
    }
}

/// Stamp the security-headers baseline onto a response. Inlined into the
/// middleware below; kept as a function so the header set + HSTS conditional
/// is exercised by unit tests without standing up an HTTP server.
fn apply_security_headers(
    mut response: axum::response::Response,
    base_url: &str,
) -> axum::response::Response {
    let headers = response.headers_mut();
    for name in [
        header::X_CONTENT_TYPE_OPTIONS,
        header::X_FRAME_OPTIONS,
        header::REFERRER_POLICY,
        header::CONTENT_SECURITY_POLICY,
        header::STRICT_TRANSPORT_SECURITY,
    ] {
        if let Some(value) = security_header_value(&name, base_url) {
            // `insert` overwrites any prior value so we always own the
            // security posture, even if an inner handler set one.
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    response
}

/// `from_fn` middleware: applies the security-headers baseline to every
/// response, including errors short-circuited by inner layers (CSRF, CORS).
pub async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // `base_url` is the only config field the header policy consults; reading
    // it here keeps the middleware stateless (no clone into AppState).
    let base_url = Config::get().base_url.clone();
    let response = next.run(request).await;
    apply_security_headers(response, &base_url)
}

pub async fn run(
    config: Config,
    db: DatabaseConnection,
    job_sender: JobSender,
    notify_reload: Arc<tokio::sync::Notify>,
) {
    let mut jinja = Environment::new();
    jinja
        .add_template(
            "directory_index.html",
            include_str!("../../templates/directory_index.html"),
        )
        .unwrap();

    // Add filters
    jinja.add_filter("format_size", |bytes: i64| format_size(bytes));

    jinja.add_filter("basename", |path: String| {
        path.trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or(&path)
            .to_string()
    });

    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        jinja,
        job_sender,
        notify_reload: notify_reload.clone(),
    });

    // Give the inotify handler a way to trigger an immediate reload. We
    // already hold one; the web layer signals via `state.notify_reload`.

    // API router - all API routes under /api
    let api_router = Router::new()
        .route("/health", get(health_handler))
        .route("/version", get(version_handler))
        .route("/ws", get(ws::ws_handler))
        .nest("/user", user::router())
        .nest("/group", group::router())
        .nest("/storage", storage::router())
        .nest("/tag", tag::router())
        .nest("/photo", photo::router());

    let app = Router::new()
        .nest("/api", api_router)
        // DAV gateway mounts at /dav (not under /api) — WebDAV/CalDAV/CardDAV
        // clients expect a clean path root.
        .merge(dav::router())
        .merge(SwaggerUi::new("/api/docs/").url("/api/docs/openapi.json", ApiDoc::openapi()))
        .fallback(get(frontend_handler));

    let app = app
        .layer(from_fn(csrf_guard))
        .layer(build_cors_layer(&config))
        .layer(TraceLayer::new_for_http())
        // Outermost layer (last `.layer()`): decorates every response,
        // including errors short-circuited by CSRF/CORS, with the security
        // baseline (M2) — nosniff / frame-deny / no-referrer / CSP / HSTS.
        .layer(from_fn(security_headers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.server_addr)
        .await
        .unwrap();
    tracing::info!("Server listening on {}", listener.local_addr().unwrap());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}

/// Build the CORS layer from `cors_allowed_origins`. Same-origin requests are
/// never subject to CORS at all, so an empty allowlist (the default) simply
/// means no *cross*-origin caller is granted access — it does not affect the
/// app's own frontend or the Vite dev proxy (which forwards server-to-server,
/// outside the browser's CORS enforcement).
fn build_cors_layer(config: &Config) -> tower_http::cors::CorsLayer {
    use axum::http::{HeaderValue, Method};
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let origins: Vec<HeaderValue> = config
        .cors_allowed_origins
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_credentials(true)
}

/// Health check response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// Overall service status (always `ok` if the process is serving).
    pub status: String,
    /// Service identifier.
    pub service: String,
    /// Database connectivity: `ok` or `error`.
    pub database: String,
}

/// Build/version response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VersionResponse {
    /// Embedded git commit hash of the running binary.
    pub commit: String,
    /// Cargo package version.
    pub version: String,
}

/// Health check endpoint (no auth required)
#[utoipa::path(
    get,
    path = "/api/health",
    tag = "meta",
    responses((status = 200, description = "Service health", body = HealthResponse)),
)]
pub async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let db_backend = state.db.get_database_backend();
    let db_status = match state
        .db
        .execute(Statement::from_string(db_backend, "SELECT 1"))
        .await
    {
        Ok(_) => "ok",
        Err(e) => {
            tracing::error!("Health check database error: {}", e);
            "error"
        }
    };

    Json(HealthResponse {
        status: "ok".to_string(),
        service: "byteburrow".to_string(),
        database: db_status.to_string(),
    })
}

/// Version endpoint
#[utoipa::path(
    get,
    path = "/api/version",
    tag = "meta",
    responses((status = 200, description = "Build/version info", body = VersionResponse)),
)]
pub async fn version_handler() -> Json<VersionResponse> {
    Json(VersionResponse {
        commit: env!("GIT_COMMIT").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pag(page: Option<u64>, per_page: Option<u64>) -> Pagination {
        Pagination { page, per_page }
    }

    #[test]
    fn pagination_defaults_when_unset() {
        let p = pag(None, None);
        assert_eq!(p.page(), 1);
        assert_eq!(p.per_page(), Pagination::DEFAULT_PER_PAGE);
        assert_eq!(p.page_index(), 0);
    }

    #[test]
    fn pagination_clamps_page_and_per_page() {
        // page 0 is bumped to 1; per_page beyond the cap is clamped down.
        let p = pag(Some(0), Some(10_000));
        assert_eq!(p.page(), 1);
        assert_eq!(p.per_page(), Pagination::MAX_PER_PAGE);

        // per_page 0 is clamped up to 1 — never an unbounded/zero query.
        assert_eq!(pag(None, Some(0)).per_page(), 1);
    }

    #[test]
    fn page_envelope_reports_total_pages() {
        // 25 items at 10 per page => 3 pages.
        let page = Page::new(vec![0u8; 10], 25, &pag(Some(1), Some(10)));
        assert_eq!(page.total, 25);
        assert_eq!(page.per_page, 10);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.page, 1);

        // An empty result set still reports at least one page.
        let empty: Page<u8> = Page::new(vec![], 0, &pag(None, None));
        assert_eq!(empty.total, 0);
        assert_eq!(empty.total_pages, 1);
        assert!(empty.items.is_empty());
    }

    // --- CSRF guard (C5) ---------------------------------------------------

    use axum::http::Method;

    #[test]
    fn origin_from_base_url_strips_path() {
        // The common case: base_url is already just an origin.
        assert_eq!(
            origin_from_base_url("http://localhost:3000"),
            "http://localhost:3000"
        );
        // A trailing path/query is stripped down to the origin.
        assert_eq!(
            origin_from_base_url("https://cloud.example.com/"),
            "https://cloud.example.com"
        );
        assert_eq!(
            origin_from_base_url("https://cloud.example.com/some/path?x=1"),
            "https://cloud.example.com"
        );
        // Default scheme + no port.
        assert_eq!(
            origin_from_base_url("https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn csrf_safe_methods_always_allowed() {
        // GET/HEAD/OPTIONS bypass the guard regardless of headers.
        let own = "http://localhost:3000";
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(
                csrf_decision(
                    &m,
                    Some("cross-site"),
                    Some("https://evil.example"),
                    own,
                    std::iter::empty(),
                ),
                "{m:?} should bypass the CSRF guard"
            );
        }
    }

    #[test]
    fn csrf_sec_fetch_site_allows_same_origin_same_site_none() {
        let own = "http://localhost:3000";
        for ok in ["same-origin", "same-site", "none"] {
            assert!(
                csrf_decision(
                    &Method::POST,
                    Some(ok),
                    Some("https://evil.example"),
                    own,
                    std::iter::empty(),
                ),
                "Sec-Fetch-Site={ok} on an unsafe method should be allowed"
            );
        }
    }

    #[test]
    fn csrf_sec_fetch_site_rejects_cross_site() {
        assert!(!csrf_decision(
            &Method::POST,
            Some("cross-site"),
            None,
            "http://localhost:3000",
            std::iter::empty(),
        ));
        // Applies to every unsafe method.
        for m in [Method::PUT, Method::DELETE, Method::PATCH] {
            assert!(
                !csrf_decision(
                    &m,
                    Some("cross-site"),
                    None,
                    "http://localhost:3000",
                    std::iter::empty(),
                ),
                "{m:?} with Sec-Fetch-Site=cross-site should be blocked"
            );
        }
    }

    #[test]
    fn csrf_no_headers_means_non_browser_client_allowed() {
        // curl / a desktop DAV client sends neither header → allow.
        assert!(csrf_decision(
            &Method::POST,
            None,
            None,
            "http://localhost:3000",
            std::iter::empty(),
        ));
        assert!(csrf_decision(
            &Method::DELETE,
            None,
            None,
            "http://localhost:3000",
            std::iter::empty(),
        ));
    }

    #[test]
    fn csrf_origin_fallback_allows_own_origin() {
        let own = "http://localhost:3000";
        assert!(csrf_decision(
            &Method::POST,
            None, // no Sec-Fetch-Site (older browser)
            Some(own),
            own,
            std::iter::empty(),
        ));
    }

    #[test]
    fn csrf_origin_fallback_allows_trusted_cors_origin() {
        let own = "http://localhost:3000";
        let allowed = ["http://localhost:5173", "https://app.example.com"];
        assert!(csrf_decision(
            &Method::PUT,
            None,
            Some("http://localhost:5173"),
            own,
            allowed.iter().copied(),
        ));
        assert!(csrf_decision(
            &Method::DELETE,
            None,
            Some("https://app.example.com"),
            own,
            allowed.iter().copied(),
        ));
    }

    #[test]
    fn csrf_origin_fallback_rejects_untrusted_origin() {
        let own = "http://localhost:3000";
        let allowed = ["http://localhost:5173"];
        assert!(!csrf_decision(
            &Method::POST,
            None,
            Some("https://evil.example"),
            own,
            allowed.iter().copied(),
        ));
        // An Origin that looks like our host but on a different scheme/port is
        // not trusted (https vs http, 3000 vs 3001).
        assert!(!csrf_decision(
            &Method::POST,
            None,
            Some("https://localhost:3000"),
            own,
            allowed.iter().copied(),
        ));
        assert!(!csrf_decision(
            &Method::POST,
            None,
            Some("http://localhost:3001"),
            own,
            allowed.iter().copied(),
        ));
    }

    #[test]
    fn csrf_sec_fetch_site_takes_precedence_over_origin() {
        // Sec-Fetch-Site=cross-site must block even if Origin happens to match.
        assert!(!csrf_decision(
            &Method::POST,
            Some("cross-site"),
            Some("http://localhost:3000"),
            "http://localhost:3000",
            std::iter::empty(),
        ));
    }

    #[test]
    fn csrf_empty_allowed_origins_blocks_all_cross_origin() {
        // Default config: no CORS allowlist → only same-origin is trusted.
        let own = "http://localhost:3000";
        assert!(!csrf_decision(
            &Method::POST,
            None,
            Some("http://localhost:5173"),
            own,
            // empty iterator — simulates default empty cors_allowed_origins
            "".split(',').filter(|s| !s.is_empty()),
        ));
    }

    // --- Security response headers (M2) -----------------------------------

    #[test]
    fn hsts_only_when_base_url_is_https() {
        // HTTPS deployment → HSTS is advertised.
        assert_eq!(
            hsts_header("https://cloud.example.com"),
            Some("max-age=31536000; includeSubDomains")
        );
        // Plain-HTTP deployment → never emit HSTS (a browser would ignore it,
        // but more importantly an MITM on the first hop could strip it).
        assert_eq!(hsts_header("http://localhost:3000"), None);
        // A string that merely contains "https" but isn't an https:// URL must
        // not trigger HSTS.
        assert_eq!(hsts_header("ftp://example.com/https"), None);
    }

    #[test]
    fn security_headers_unconditional_set() {
        // These headers apply regardless of deployment scheme.
        assert_eq!(
            security_header_value(&header::X_CONTENT_TYPE_OPTIONS, "http://localhost:3000"),
            Some("nosniff")
        );
        assert_eq!(
            security_header_value(&header::X_FRAME_OPTIONS, "http://localhost:3000"),
            Some("DENY")
        );
        assert_eq!(
            security_header_value(&header::REFERRER_POLICY, "http://localhost:3000"),
            Some("no-referrer")
        );
    }

    #[test]
    fn security_headers_csp_is_strict_no_unsafe_inline_script() {
        let csp = security_header_value(&header::CONTENT_SECURITY_POLICY, "http://localhost:3000")
            .expect("CSP always set");
        // The Vue build ships only an external type=module script (verified in
        // frontend/dist/index.html), so script-src must be 'self' only.
        assert!(csp.contains("script-src 'self'"));
        assert!(
            !csp.contains("script-src 'self' 'unsafe-inline'"),
            "script-src must NOT include 'unsafe-inline'"
        );
        // Defense-in-depth directives that neutralize reflected/stored XSS
        // even if sanitization (C2 DOMPurify) is bypassed.
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("base-uri 'self'"));
        assert!(csp.contains("form-action 'self'"));
    }

    #[test]
    fn security_headers_hsts_via_helper() {
        // HSTS rides on the same dispatch as the unconditional headers.
        assert_eq!(
            security_header_value(&header::STRICT_TRANSPORT_SECURITY, "https://x"),
            Some("max-age=31536000; includeSubDomains")
        );
        assert_eq!(
            security_header_value(&header::STRICT_TRANSPORT_SECURITY, "http://x"),
            None
        );
    }

    #[test]
    fn security_headers_unknown_header_returns_none() {
        assert_eq!(security_header_value(&header::ACCEPT, "https://x"), None);
    }

    #[test]
    fn apply_security_headers_overwrites_inner_values() {
        // A handler may have set its own (weaker) value; the middleware owns
        // the posture and must overwrite rather than append.
        let mut resp = (StatusCode::OK, "ok").into_response();
        resp.headers_mut().insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("weak"),
        );
        let resp = apply_security_headers(resp, "http://localhost:3000");
        assert_eq!(
            resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        // HSTS absent under http, present under https.
        assert!(resp
            .headers()
            .get(header::STRICT_TRANSPORT_SECURITY)
            .is_none());
    }

    #[test]
    fn apply_security_headers_all_present_on_https() {
        let resp = (StatusCode::OK, "ok").into_response();
        let resp = apply_security_headers(resp, "https://cloud.example.com");
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
        assert_eq!(
            h.get(header::STRICT_TRANSPORT_SECURITY).unwrap(),
            "max-age=31536000; includeSubDomains"
        );
    }
}
