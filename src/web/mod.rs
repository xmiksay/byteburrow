pub mod group;
pub mod photo;
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
    http::{header, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use minijinja::Environment;
use rust_embed::Embed;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde::Serialize;
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

pub async fn run(config: Config, db: DatabaseConnection, job_sender: JobSender) {
    let mut jinja = Environment::new();
    jinja
        .add_template(
            "directory_index.html",
            include_str!("../../templates/directory_index.html"),
        )
        .unwrap();

    // Add filters
    jinja.add_filter("format_size", |bytes: i64| format_size(bytes));

    jinja.add_filter("format_datetime", |dt: String| {
        // This is a bit hacky because minijinja's Value doesn't easily pass chrono types
        // unless we use custom Object. For now, we'll assume it's passed as ISO string or
        // handle the type in the filter if possible.
        // Actually, minijinja can handle Serialized chrono types as strings or ints.
        dt
    });

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
    });

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
        .merge(SwaggerUi::new("/api/docs/").url("/api/docs/openapi.json", ApiDoc::openapi()))
        .fallback(get(frontend_handler));

    let app = app
        .layer(build_cors_layer(&config))
        .layer(TraceLayer::new_for_http())
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

/// Health check endpoint (no auth required)
pub async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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

    Json(serde_json::json!({
        "status": "ok",
        "service": "byteburrow",
        "database": db_status,
    }))
}

/// Version endpoint
pub async fn version_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "commit": env!("GIT_COMMIT"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
