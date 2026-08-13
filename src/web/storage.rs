use crate::auth::Auth;
use crate::entity::entry::EntryType;
use crate::entity::{entry, meta, shared, storage};
use crate::storage::{
    determine_content_type, thumbnail, validate_storage_path, DirectoryEntry,
    Storage as StorageWrapper,
};
use crate::web::{
    bad_request, conflict, forbidden, internal, message, not_found, not_found_msg, require_admin,
    require_entry_owner, require_group_exists, require_storage_access, require_storage_path_access,
    require_storage_path_write_access, require_user_exists, save_or_err, storage_name_taken,
    storage_path_taken, unauthorized_msg, user_group_ids, ApiError, AppState, ErrorResponse,
    MessageResponse, Page, Pagination,
};
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, Request, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post, put},
    Json, Router,
};
use sea_orm::{
    sea_query::Expr, ActiveModelTrait, ColumnTrait, Condition, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tower::Service;
use tower_http::services::ServeFile;
use tracing::instrument;

/// Storage response
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct StorageResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub default_user: i32,
    pub default_group: i32,
    pub ignore_patterns: String,
}

impl From<storage::Model> for StorageResponse {
    fn from(storage: storage::Model) -> Self {
        Self {
            id: storage.id,
            name: storage.name,
            description: storage.description,
            path: storage.path,
            default_user: storage.default_user,
            default_group: storage.default_group,
            ignore_patterns: storage.ignore_patterns,
        }
    }
}

/// Create storage request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateStorageRequest {
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub default_user: i32,
    pub default_group: i32,
    pub ignore_patterns: Option<String>,
}

/// Update storage request (all fields optional)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateStorageRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub path: Option<String>,
    pub default_user: Option<i32>,
    pub default_group: Option<i32>,
    pub ignore_patterns: Option<String>,
}

/// Create entry request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateEntryRequest {
    pub entry_type: EntryType,
    #[serde(default)]
    pub notify: bool,
}

/// Rename entry request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RenameEntryRequest {
    pub new_path: String,
}

/// Update entry tags request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateEntryTagsRequest {
    pub tags: Vec<i32>,
}

/// Meta response
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct MetaResponse {
    pub hash: String,
    pub tags: Vec<i32>,
    pub keywords: Vec<String>,
    pub custom: serde_json::Value,
}

impl From<meta::Model> for MetaResponse {
    fn from(m: meta::Model) -> Self {
        Self {
            hash: hex::encode(&m.hash),
            tags: m.tags,
            keywords: m.keywords,
            custom: m.custom,
        }
    }
}

// ---------------------------------------------------------------------------
// Local helpers (DRY-1: map StorageWrapper errors preserving legacy messages)
// ---------------------------------------------------------------------------

/// Map a `StorageWrapper::find_by_id` DbErr to an ApiError, preserving the
/// legacy "Storage {id} not found" message for the RecordNotFound case (the
/// wrapper's own message is "Storage with id X not found").
fn storage_lookup_err(e: sea_orm::DbErr, storage_id: i32) -> ApiError {
    if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
        not_found_msg(format!("Storage {} not found", storage_id))
    } else {
        ApiError::Db(e)
    }
}

/// Dispatch background `ProcessFile` jobs for any unhashed files in `entries`.
fn dispatch_hash_jobs(state: &AppState, entries: &[DirectoryEntry]) {
    entries
        .iter()
        .filter(|e| e.hash.is_none() && e.entry_type == EntryType::File)
        .for_each(|e| {
            state
                .job_sender
                .try_send(crate::job::Job::ProcessFile {
                    storage_id: e.storage_id,
                    path: e.path.clone(),
                    mode: crate::job::ProcessMode::Auto,
                })
                .ok();
        });
}

/// Build a one-shot [`ServeFile`] response with a forced Content-Type header.
async fn serve_file_response(
    full_path: std::path::PathBuf,
    content_type: &'static str,
    req: Request<Body>,
) -> axum::response::Response {
    let mut service = ServeFile::new(full_path);
    let mut res = Service::<Request<Body>>::call(&mut service, req)
        .await
        .unwrap_or_else(|e| match e {})
        .into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    res
}

/// Detect content type from a file's first kilobyte, falling back to
/// `application/octet-stream` if the file cannot be opened.
async fn detect_content_type(full_path: &std::path::Path) -> &'static str {
    match fs::File::open(full_path).await {
        Ok(mut file) => {
            let mut buffer = [0u8; 1024];
            let n = file.read(&mut buffer).await.unwrap_or(0);
            determine_content_type(full_path, &buffer[..n])
        }
        Err(_) => "application/octet-stream",
    }
}

/// Map a path-resolution [`io::Error`] (from `resolve_safe_path*`) to an
/// `ApiError`: `..` traversal escapes surface as 400, missing files as 404.
fn map_path_err(e: std::io::Error, path: &str) -> ApiError {
    match e.kind() {
        std::io::ErrorKind::NotFound => not_found_msg(format!("File not found: {path}")),
        std::io::ErrorKind::PermissionDenied => bad_request(format!("Invalid path: {path}")),
        _ => internal(format!("Error accessing path '{path}': {e}")),
    }
}

/// Verify the caller may read content-addressed metadata for `hash`.
///
/// Meta rows are content-addressed and not owned directly, so access is granted
/// when the user can access at least one storage holding an entry with this
/// hash. Admins always pass.
async fn require_hash_access(auth: &Auth, hash: &[u8], state: &AppState) -> Result<(), ApiError> {
    if auth.user.admin {
        return Ok(());
    }

    let entries = entry::Entity::find()
        .filter(entry::Column::Hash.eq(hash.to_vec()))
        .all(&state.db)
        .await?;

    for e in entries {
        let storage = StorageWrapper::find_by_id(&state.db, e.storage_id)
            .await
            .map_err(|err| storage_lookup_err(err, e.storage_id))?;
        if require_storage_path_access(auth, &storage.model, &e.path, &state.db)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }

    Err(forbidden("Access denied to this content"))
}

/// Generate a new public-link share token: a fresh random plaintext (UUIDv4,
/// 122 bits of entropy) and its hash for storage. Only the hash is persisted
/// to `shared.token` — the plaintext must be returned to the caller now, as
/// it cannot be recovered from the stored hash later (issue #10).
fn generate_share_token() -> (String, String) {
    let plaintext = uuid::Uuid::new_v4().as_simple().to_string();
    let hash = Auth::hash_string(&plaintext);
    (plaintext, hash)
}

/// Build a fully-qualified path under a share's base entry.
fn join_share_path(base_path: &str, sub_path: &str) -> String {
    let base = base_path.trim_matches('/');
    let sub = sub_path.trim_matches('/');
    match (base.is_empty(), sub.is_empty()) {
        (true, _) => sub.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}/{sub}"),
    }
}

/// Collapse `.` and `..` segments lexically on a relative path string. Empty
/// and `.` segments are dropped; `..` pops the last accumulated segment. A `..`
/// that would pop above the root is also dropped — we are rooted at the share
/// base, so an escape surfaces as a normalized result that no longer shares the
/// base's prefix (and is therefore rejected by [`require_within_share`]). (H6)
fn normalize_lexical(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Verify that `full_path` is lexically contained within `base_path`'s subtree
/// (i.e. equals `base_path` or is nested under it), after normalizing `..`
/// segments. Returns a forbidden error otherwise. This confines share write
/// handlers to the shared folder even when `..` still resolves inside the
/// wider storage root. (H6)
fn require_within_share(base_path: &str, full_path: &str) -> Result<(), ApiError> {
    let base = normalize_lexical(base_path.trim_matches('/'));
    let full = normalize_lexical(full_path.trim_matches('/'));
    if full == base || full.starts_with(&format!("{base}/")) || base.is_empty() {
        Ok(())
    } else {
        Err(forbidden("Path escapes the shared folder"))
    }
}

/// Upsert the `tags` column of the `meta` row keyed by `entry.hash`,
/// preserving any existing `keywords`/`custom` classification data.
/// Fails with 409 if the entry hasn't been hashed yet (tags are
/// content-addressed, same as the rest of `meta`).
async fn set_entry_tags(
    db: &sea_orm::DatabaseConnection,
    entry_model: &entry::Model,
    tags: Vec<i32>,
) -> Result<(), ApiError> {
    let hash = entry_model
        .hash
        .clone()
        .ok_or_else(|| conflict("Entry has not been hashed yet; tags are unavailable"))?;

    let existing = meta::Entity::find_by_id(hash.clone()).one(db).await?;

    match existing {
        Some(m) => {
            let active = meta::ActiveModel {
                hash: Set(hash),
                tags: Set(tags),
                keywords: Set(m.keywords),
                custom: Set(m.custom),
            };
            active.update(db).await?;
        }
        None => {
            let active = meta::ActiveModel {
                hash: Set(hash),
                tags: Set(tags),
                keywords: Set(vec![]),
                custom: Set(serde_json::Value::Object(serde_json::Map::new())),
            };
            active.insert(db).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Create storage router with all storage-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    // Share-based access routes — every handler resolves the share via
    // `get_share_context`, where a public token is accepted without auth.
    // That makes the `{share_id}` segment a brute-force/enumeration target
    // (H16), so this group is wrapped in the per-IP share rate limiter.
    // Authenticated share-management routes (`/share`, `/share/with-me`,
    // `/:id/share/…`) deliberately live outside this group and are not
    // throttled, since they require a valid session.
    let share_access = Router::new()
        .route(
            "/share/:share_id",
            get(get_share_info_handler)
                .put(update_share_handler)
                .delete(delete_share_handler),
        )
        .route("/share/:share_id/list", get(share_list_root_handler))
        .route("/share/:share_id/list/", get(share_list_root_handler))
        .route("/share/:share_id/list/*path", get(share_list_handler))
        .route("/share/:share_id/index", get(share_index_root_handler))
        .route("/share/:share_id/index/", get(share_index_root_handler))
        .route("/share/:share_id/index/*path", get(share_index_handler))
        .route("/share/:share_id/show/*path", get(share_show_handler))
        .route("/share/:share_id/update/*path", put(share_update_handler))
        .route("/share/:share_id/create/*path", post(share_create_handler))
        .route("/share/:share_id/rename/*path", post(share_rename_handler))
        .route(
            "/share/:share_id/remove/*path",
            axum::routing::delete(share_remove_handler),
        )
        .route(
            "/share/:share_id/tags/*path",
            put(share_update_entry_tags_handler),
        )
        .layer(axum::middleware::from_fn(
            crate::web::rate_limit::share_rate_limit,
        ));

    Router::new()
        .route("/", get(list_storages_handler).post(create_storage_handler))
        .route("/share", get(list_all_user_shares_handler))
        .route("/share/with-me", get(list_shares_with_me_handler))
        .route(
            "/:id",
            get(get_storage_handler)
                .put(update_storage_handler)
                .delete(delete_storage_handler),
        )
        .route("/:id/list", get(list_directory_root_handler))
        .route("/:id/list/", get(list_directory_root_handler))
        .route("/:id/list/*path", get(list_directory_handler))
        .route("/:id/index", get(directory_index_root_handler))
        .route("/:id/index/", get(directory_index_root_handler))
        .route("/:id/index/*path", get(directory_index_handler))
        .route("/:id/show/*path", get(get_file_content_handler))
        .route("/:id/update/*path", put(update_file_content_handler))
        .route("/:id/raw/*path", get(download_file_handler))
        .route("/:id/create/*path", post(create_entry_handler))
        .route("/:id/rename/*path", post(rename_entry_handler))
        .route(
            "/:id/remove/*path",
            axum::routing::delete(remove_entry_handler),
        )
        .route("/:id/hash/*path", post(trigger_hash_handler))
        .route("/:id/tags/*path", put(update_entry_tags_handler))
        .route("/:id/share/*path", get(list_shares_handler))
        .route("/:id/share/*path", post(share_entry_handler))
        .merge(share_access)
        .route("/thumbnail/:hash/:size", get(thumbnail_handler))
        .route("/meta/:hash", get(get_meta_handler))
}

// ---------------------------------------------------------------------------
// Meta + thumbnail handlers
// ---------------------------------------------------------------------------

/// Get meta information by hash
/// GET /api/storage/meta/:hash
#[utoipa::path(
    get,
    path = "/api/storage/meta/{hash}",
    tag = "meta",
    params(
        ("hash" = String, Path, description = "File content hash (hex-encoded)"),
    ),
    responses(
        (status = 200, description = "Meta information", body = MetaResponse),
        (status = 400, description = "Invalid hash format", body = ErrorResponse),
        (status = 404, description = "Meta not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(auth, state))]
pub(crate) async fn get_meta_handler(
    auth: Auth,
    AxumPath(hash): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MetaResponse>, ApiError> {
    let hash_bytes = hex::decode(&hash).map_err(|_| bad_request("Invalid hex hash format"))?;

    require_hash_access(&auth, &hash_bytes, &state).await?;

    let meta = meta::Entity::find_by_id(hash_bytes)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg("Meta not found"))?;

    Ok(Json(MetaResponse::from(meta)))
}

/// Thumbnail endpoint - serves thumbnail by entry hash (requires access to a
/// storage entry with this content hash, same gate as `get_meta_handler`)
/// GET /api/storage/thumbnail/:hash/:size
#[utoipa::path(
    get,
    path = "/api/storage/thumbnail/{hash}/{size}",
    tag = "thumbnail",
    params(
        ("hash" = String, Path, description = "Entry hash"),
        ("size" = String, Path, description = "Thumbnail size (small, large, mini)"),
    ),
    responses(
        (status = 200, description = "Thumbnail image", content_type = "image/png"),
        (status = 400, description = "Invalid hash or size"),
        (status = 403, description = "Access denied to this content", body = ErrorResponse),
        (status = 404, description = "Thumbnail not found"),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(auth, state))]
pub(crate) async fn thumbnail_handler(
    auth: Auth,
    AxumPath((hash, size)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate hash format (basic validation)
    if hash.is_empty() || hash.len() < 6 || hash.len() > 64 {
        return Err(bad_request("Invalid hash format"));
    }

    // Validate size
    if !["small", "large", "mini"].contains(&size.as_str()) {
        return Err(bad_request("Invalid size"));
    }

    let hash_bytes = hex::decode(&hash).map_err(|_| bad_request("Invalid hex hash format"))?;
    require_hash_access(&auth, &hash_bytes, &state).await?;

    // Construct thumbnail path using the hash and size with distributed directory structure
    let thumbnail_dir = std::path::PathBuf::from(&state.config.thumbnail_storage);
    let thumbnail_path = thumbnail::get_thumbnail_path(&thumbnail_dir, &hash, &size);

    if !thumbnail_path.exists() {
        return Err(not_found_msg("Thumbnail file not found"));
    }

    let thumbnail_data = fs::read(&thumbnail_path)
        .await
        .map_err(|e| internal(format!("Failed to read thumbnail {hash}/{size}: {e}")))?;

    let content_type = determine_content_type(&thumbnail_path, &thumbnail_data);

    Ok(([(header::CONTENT_TYPE, content_type)], thumbnail_data))
}

// ---------------------------------------------------------------------------
// Storage CRUD
// ---------------------------------------------------------------------------

/// Create new storage
/// POST /api/storage
#[utoipa::path(
    post,
    path = "/api/storage",
    tag = "storage",
    request_body = CreateStorageRequest,
    responses(
        (status = 200, description = "Storage created", body = StorageResponse),
        (status = 400, description = "Invalid path", body = ErrorResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 409, description = "Storage already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn create_storage_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateStorageRequest>,
) -> Result<Json<StorageResponse>, ApiError> {
    require_admin(&auth)?;

    // Validate path exists and is accessible
    validate_storage_path(&payload.path)
        .await
        .map_err(|e| bad_request(e.to_string()))?;

    // DRY-2: uniqueness checks
    if storage_path_taken(&state.db, &payload.path).await? {
        return Err(conflict(format!(
            "Storage with path '{}' already exists",
            payload.path
        )));
    }
    if storage_name_taken(&state.db, &payload.name).await? {
        return Err(conflict(format!(
            "Storage with name '{}' already exists",
            payload.name
        )));
    }

    // Verify default_user and default_group exist
    require_user_exists(payload.default_user, &state.db).await?;
    require_group_exists(payload.default_group, &state.db).await?;

    let ignore_patterns = payload
        .ignore_patterns
        .unwrap_or_else(|| crate::config::Config::get().ignore_patterns.join(","));

    let new_storage = storage::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        path: Set(payload.path),
        default_user: Set(payload.default_user),
        default_group: Set(payload.default_group),
        ignore_patterns: Set(ignore_patterns),
        ..Default::default()
    };

    let created_storage = new_storage.insert(&state.db).await?;

    Ok(Json(StorageResponse::from(created_storage)))
}

/// Get storage by ID
/// GET /api/storage/:id
#[utoipa::path(
    get,
    path = "/api/storage/{id}",
    tag = "storage",
    params(("id" = i32, Path, description = "Storage ID")),
    responses(
        (status = 200, description = "Storage found", body = StorageResponse),
        (status = 403, description = "Access denied to this storage", body = ErrorResponse),
        (status = 404, description = "Storage not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
async fn get_storage_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<StorageResponse>, ApiError> {
    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Storage", storage_id))?;

    require_storage_access(&auth, &storage, &state.db).await?;

    Ok(Json(StorageResponse::from(storage)))
}

/// Update storage
/// PUT /api/storage/:id
#[utoipa::path(
    put,
    path = "/api/storage/{id}",
    tag = "storage",
    params(("id" = i32, Path, description = "Storage ID")),
    request_body = UpdateStorageRequest,
    responses(
        (status = 200, description = "Storage updated", body = StorageResponse),
        (status = 400, description = "Invalid path", body = ErrorResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Storage not found", body = ErrorResponse),
        (status = 409, description = "Storage path/name already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
async fn update_storage_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateStorageRequest>,
) -> Result<Json<StorageResponse>, ApiError> {
    require_admin(&auth)?;

    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Storage", storage_id))?;

    // Validate path if being changed
    if let Some(ref new_path) = payload.path {
        if new_path != &storage.path {
            validate_storage_path(new_path)
                .await
                .map_err(|e| bad_request(e.to_string()))?;

            if storage_path_taken(&state.db, new_path).await? {
                return Err(conflict(format!(
                    "Storage with path '{}' already exists",
                    new_path
                )));
            }
        }
    }

    // Check name conflict if changed
    if let Some(ref new_name) = payload.name {
        if new_name != &storage.name && storage_name_taken(&state.db, new_name).await? {
            return Err(conflict(format!(
                "Storage with name '{}' already exists",
                new_name
            )));
        }
    }

    // Validate default_user if being changed
    if let Some(user_id) = payload.default_user {
        require_user_exists(user_id, &state.db).await?;
    }

    // Validate default_group if being changed
    if let Some(group_id) = payload.default_group {
        require_group_exists(group_id, &state.db).await?;
    }

    let mut active_storage: storage::ActiveModel = storage.into();

    if let Some(name) = payload.name {
        active_storage.name = Set(name);
    }
    if let Some(description) = payload.description {
        active_storage.description = Set(Some(description));
    }
    if let Some(path) = payload.path {
        active_storage.path = Set(path);
    }
    if let Some(default_user) = payload.default_user {
        active_storage.default_user = Set(default_user);
    }
    if let Some(default_group) = payload.default_group {
        active_storage.default_group = Set(default_group);
    }
    if let Some(ignore_patterns) = payload.ignore_patterns {
        active_storage.ignore_patterns = Set(ignore_patterns);
    }

    let updated_storage = save_or_err(active_storage, &state.db).await?;

    Ok(Json(StorageResponse::from(updated_storage)))
}

/// Delete storage
/// DELETE /api/storage/:id
#[utoipa::path(
    delete,
    path = "/api/storage/{id}",
    tag = "storage",
    params(("id" = i32, Path, description = "Storage ID")),
    responses(
        (status = 200, description = "Storage deleted"),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Storage not found", body = ErrorResponse),
        (status = 409, description = "Cannot delete storage with entries", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
async fn delete_storage_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    require_admin(&auth)?;

    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Storage", storage_id))?;

    let entry_count = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .count(&state.db)
        .await?;

    if entry_count > 0 {
        return Err(conflict(format!(
            "Cannot delete storage with {} entries. Delete entries first.",
            entry_count
        )));
    }

    storage::Entity::delete_by_id(storage_id)
        .exec(&state.db)
        .await?;

    Ok(message(format!(
        "Storage '{}' deleted successfully",
        storage.name
    )))
}

/// List storages the caller can access (owner, group member, share recipient,
/// or admin — mirrors `require_storage_access`) endpoint
/// GET /api/storage
#[utoipa::path(
    get,
    path = "/api/storage",
    tag = "storage",
    params(Pagination),
    responses(
        (status = 200, description = "Paginated list of accessible storages", body = Page<StorageResponse>),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
async fn list_storages_handler(
    auth: Auth,
    Query(pagination): Query<Pagination>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Page<StorageResponse>>, ApiError> {
    let storages = storage::Entity::find().all(&state.db).await?;

    // Access is enforced per-row in Rust (mirrors `require_storage_access`),
    // so pagination is applied to the filtered set rather than at the SQL layer.
    let mut accessible = Vec::with_capacity(storages.len());
    for storage in storages {
        if require_storage_access(&auth, &storage, &state.db)
            .await
            .is_ok()
        {
            accessible.push(StorageResponse::from(storage));
        }
    }

    let total = accessible.len() as u64;
    let start = (pagination.page_index() * pagination.per_page()) as usize;
    let items = accessible
        .into_iter()
        .skip(start)
        .take(pagination.per_page() as usize)
        .collect();

    Ok(Json(Page::new(items, total, &pagination)))
}

// ---------------------------------------------------------------------------
// Directory listing / index (KISS-5: collapse wrappers, keep shared impl)
// ---------------------------------------------------------------------------

/// Helper handler for listing the root directory (no trailing slash).
async fn list_directory_root_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    state: State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    list_directory_handler(auth, AxumPath((storage_id, String::new())), state).await
}

/// Directory index root handler - serves Kodi-compatible directory listing
async fn directory_index_root_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    directory_index_impl(storage_id, String::new(), state, req, &auth).await
}

/// Directory index handler - serves Kodi-compatible directory listing or file content
#[instrument(skip(state, auth, req))]
async fn directory_index_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    directory_index_impl(storage_id, path, state, req, &auth).await
}

async fn directory_index_impl(
    storage_id: i32,
    path: String,
    state: Arc<AppState>,
    req: Request<Body>,
    auth: &Auth,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    let normalized_path = path.trim_matches('/');

    // Authorization: must have access to this path within the storage.
    require_storage_path_access(auth, &storage.model, normalized_path, &state.db).await?;
    let full_path = storage.get_full_path(normalized_path);

    // If it's a file, serve it directly with content-type
    if full_path.is_file() {
        let content_type = detect_content_type(&full_path).await;
        return Ok(serve_file_response(full_path, content_type, req)
            .await
            .into_response());
    }

    // It's a directory - list entries and return HTML
    let entries = storage
        .list_directory(&state.db, normalized_path)
        .await
        .map_err(|e| internal(format!("Error listing directory: {e}")))?;

    dispatch_hash_jobs(&state, &entries);

    let html =
        generate_directory_index(&state, storage_id, normalized_path, &entries).map_err(|e| {
            tracing::error!("Template error: {}", e);
            internal("Internal server error")
        })?;

    Ok(Html(html).into_response())
}

/// Get file content with detected content type
/// GET /api/storage/:id/show/*path
#[utoipa::path(
    get,
    path = "/api/storage/{id}/show/{path}",
    tag = "file",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "File path"),
    ),
    responses(
        (status = 200, description = "File content"),
        (status = 404, description = "File not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth, req))]
pub(crate) async fn get_file_content_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    serve_file_with_content_type(storage_id, path, state, req, None, &auth).await
}

/// Download file as octet-stream
/// GET /api/storage/:id/raw/*path
#[utoipa::path(
    get,
    path = "/api/storage/{id}/raw/{path}",
    tag = "file",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "File path"),
    ),
    responses(
        (status = 200, description = "File download", content_type = "application/octet-stream"),
        (status = 404, description = "File not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth, req))]
pub(crate) async fn download_file_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    serve_file_with_content_type(
        storage_id,
        path,
        state,
        req,
        Some("application/octet-stream"),
        &auth,
    )
    .await
}

/// Helper to serve file with specific or detected content type
async fn serve_file_with_content_type(
    storage_id: i32,
    path: String,
    state: Arc<AppState>,
    req: Request<Body>,
    forced_content_type: Option<&'static str>,
    auth: &Auth,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_access(auth, &storage.model, &path, &state.db).await?;

    // Canonicalizing resolution rejects `..` traversal escaping the storage root.
    let full_path = storage
        .resolve_safe_path(&path)
        .await
        .map_err(|e| map_path_err(e, &path))?;

    if full_path.is_dir() {
        return Err(bad_request(format!(
            "Requested path is a directory: {path}"
        )));
    }

    let content_type = match forced_content_type {
        Some(ct) => ct,
        None => detect_content_type(&full_path).await,
    };

    let res = serve_file_response(full_path, content_type, req).await;
    Ok(res)
}

/// Update file content
/// PUT /api/storage/:id/update/*path
#[utoipa::path(
    put,
    path = "/api/storage/{id}/update/{path}",
    tag = "file",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "File path"),
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "File updated"),
        (status = 404, description = "Storage not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth, body))]
pub(crate) async fn update_file_content_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_write_access(&auth, &storage.model, &path, &state.db).await?;

    // `save_file` resolves the path lexically and rejects `..` traversal escapes.
    storage
        .save_file(&path, &body)
        .await
        .map_err(|e| map_path_err(e, &path))?;

    Ok(message("File updated successfully"))
}

/// Create a new file or directory entry
/// POST /api/storage/:id/create/*path
#[utoipa::path(
    post,
    path = "/api/storage/{id}/create/{path}",
    tag = "entry",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Entry path"),
    ),
    request_body = CreateEntryRequest,
    responses(
        (status = 200, description = "Entry created"),
        (status = 400, description = "Unsupported entry type", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn create_entry_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateEntryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_write_access(&auth, &storage.model, &path, &state.db).await?;

    match payload.entry_type {
        EntryType::Directory => {
            storage
                .create_directory(&path)
                .await
                .map_err(|e| internal(format!("Error creating directory: {e}")))?;
            if payload.notify {
                storage
                    .set_notify(&state.db, &path, true)
                    .await
                    .map_err(|e| internal(format!("Error setting notify: {e}")))?;
                // H12: trigger an immediate inotify reload so the new watch
                // dir is picked up without waiting for the 15s timer.
                state.notify_reload.notify_one();
            }
        }
        EntryType::File => {
            storage
                .create_file(&path)
                .await
                .map_err(|e| internal(format!("Error creating file: {e}")))?;
        }
        _ => return Err(bad_request("Unsupported entry type")),
    }

    Ok(message("Entry created successfully"))
}

/// Rename a file or directory
/// POST /api/storage/:id/rename/*path
#[utoipa::path(
    post,
    path = "/api/storage/{id}/rename/{path}",
    tag = "entry",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Current entry path"),
    ),
    request_body = RenameEntryRequest,
    responses(
        (status = 200, description = "Entry renamed"),
        (status = 500, description = "Error renaming entry", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn rename_entry_handler(
    auth: Auth,
    AxumPath((storage_id, old_path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RenameEntryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_write_access(&auth, &storage.model, &old_path, &state.db).await?;
    require_storage_path_write_access(&auth, &storage.model, &payload.new_path, &state.db).await?;

    storage
        .rename_entry(&old_path, &payload.new_path)
        .await
        .map_err(|e| internal(format!("Error renaming entry: {e}")))?;

    Ok(message("Entry renamed successfully"))
}

/// Set the tags on a file or directory
/// PUT /api/storage/:id/tags/*path
#[utoipa::path(
    put,
    path = "/api/storage/{id}/tags/{path}",
    tag = "entry",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Entry path"),
    ),
    request_body = UpdateEntryTagsRequest,
    responses(
        (status = 200, description = "Tags updated"),
        (status = 404, description = "Entry not found", body = ErrorResponse),
        (status = 409, description = "Entry has not been hashed yet", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn update_entry_tags_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateEntryTagsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_write_access(&auth, &storage.model, &path, &state.db).await?;

    let sub_path = path.trim_matches('/');

    let entry_model = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg(format!("File not found: {path}")))?;

    set_entry_tags(&state.db, &entry_model, payload.tags).await?;

    Ok(message("Tags updated successfully"))
}

/// Remove a file or directory
/// DELETE /api/storage/:id/remove/*path
#[utoipa::path(
    delete,
    path = "/api/storage/{id}/remove/{path}",
    tag = "entry",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Entry path"),
    ),
    responses(
        (status = 200, description = "Entry removed"),
        (status = 500, description = "Error removing entry", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn remove_entry_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    require_storage_path_write_access(&auth, &storage.model, &path, &state.db).await?;

    storage
        .remove_entry(&path)
        .await
        .map_err(|e| internal(format!("Error removing entry: {e}")))?;

    Ok(message("Entry removed successfully"))
}

/// Directory listing response: the entries under a storage path.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DirectoryListingResponse {
    pub storage_id: i32,
    /// Normalized (slash-trimmed) directory path within the storage.
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
}

/// List directory contents (JSON)
/// GET /api/storage/:id/list/*path
#[utoipa::path(
    get,
    path = "/api/storage/{id}/list/{path}",
    tag = "entry",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Directory path"),
    ),
    responses(
        (status = 200, description = "Directory listing", body = DirectoryListingResponse),
        (status = 404, description = "Storage not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn list_directory_handler(
    auth: Auth,
    AxumPath((id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<DirectoryListingResponse>, ApiError> {
    tracing::info!("Listing directory for storage {} at path {}", id, path);

    let storage = StorageWrapper::find_by_id(&state.db, id)
        .await
        .map_err(|e| storage_lookup_err(e, id))?;

    require_storage_path_access(&auth, &storage.model, &path, &state.db).await?;

    let entries = storage
        .list_directory(&state.db, &path)
        .await
        .map_err(|e| internal(format!("Error listing directory: {e}")))?;

    dispatch_hash_jobs(&state, &entries);

    Ok(Json(DirectoryListingResponse {
        storage_id: id,
        path: path.trim_matches('/').to_string(),
        entries,
    }))
}

/// Generate HTML directory index
fn generate_directory_index(
    state: &AppState,
    storage_id: i32,
    path: &str,
    entries: &[DirectoryEntry],
) -> Result<String, minijinja::Error> {
    let title = if path.is_empty() {
        format!("Index of Storage {}/", storage_id)
    } else {
        format!("Index of Storage {}/{}/", storage_id, path)
    };

    let parent_path = if !path.is_empty() {
        Some(path.rsplit_once('/').map(|x| x.0).unwrap_or(""))
    } else {
        None
    };

    // Sort entries: directories first, then by name
    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| {
        let a_name = a
            .path
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or(&a.path);
        let b_name = b
            .path
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .unwrap_or(&b.path);

        match (
            a.entry_type == EntryType::Directory,
            b.entry_type == EntryType::Directory,
        ) {
            (true, true) | (false, false) => a_name.cmp(b_name),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
        }
    });

    let template = state.jinja.get_template("directory_index.html")?;
    template.render(serde_json::json!({
        "title": title,
        "storage_id": storage_id,
        "path": path,
        "parent_path": parent_path.unwrap_or(""),
        "entries": sorted_entries,
    }))
}

// ---------------------------------------------------------------------------
// Shares
// ---------------------------------------------------------------------------

/// Share entry request
///
/// The same body is used for create (`POST`) and update (`PUT`); both apply
/// the fields with replace semantics — the request fully describes the share.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ShareEntryRequest {
    pub can_write: bool,
    /// Days until the share expires, counted from now.
    ///
    /// One expiry contract shared by create and update (issue #9 / B8): a
    /// positive value sets `expires_at = now + days`; `0`, `null`, or an
    /// absent field means the share never expires. Create and update treat
    /// this value identically.
    #[schema(minimum = 0)]
    pub expires_in_days: Option<u64>,
    pub public_link: bool,
    pub user_ids: Vec<i32>,
    pub group_ids: Vec<i32>,
}

/// Resolve a share's absolute expiry timestamp from the request's
/// `expires_in_days`.
///
/// Shared by create and update so their semantics cannot drift (issue #9 /
/// B8). A positive day count yields `now + days`; `0`, `null`, and an absent
/// field all mean "never expires" (`None`). Previously create treated `0` as
/// "expire immediately" (`now + 0`) and an absent field as "never", while
/// update treated both `0` and absent as "never".
fn resolve_share_expiry(expires_in_days: Option<u64>) -> Option<chrono::NaiveDateTime> {
    expires_in_days
        .filter(|&days| days > 0)
        .map(|days| chrono::Utc::now().naive_utc() + chrono::Duration::days(days as i64))
}

/// Share response
///
/// `shared.token` stores a hash of the public-link token (issue #10), not the
/// plaintext, so it can no longer be read back from the database. `token` is
/// therefore only ever populated with the plaintext immediately after it is
/// generated (on creation, or on regeneration via update) — callers must
/// capture it then, as it cannot be recovered later. `has_public_link`
/// reflects whether a public link exists at all, independent of whether the
/// plaintext is known in this response.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ShareResponse {
    pub id: i32,
    pub storage_id: i32,
    pub path: String,
    pub owner_id: i32,
    pub token: Option<String>,
    pub has_public_link: bool,
    pub can_write: bool,
    pub user_ids: Vec<i32>,
    pub group_ids: Vec<i32>,
    pub expires_at: Option<chrono::NaiveDateTime>,
    pub created_at: chrono::NaiveDateTime,
}

impl ShareResponse {
    pub fn from_model(m: shared::Model, e: entry::Model) -> Self {
        Self {
            id: m.id,
            storage_id: e.storage_id,
            path: e.path,
            owner_id: m.owner_id,
            has_public_link: m.token.is_some(),
            token: None,
            can_write: m.can_write,
            user_ids: m.user_ids,
            group_ids: m.group_ids,
            expires_at: m.expires_at,
            created_at: m.created_at,
        }
    }
}

/// List shares for an entry
/// GET /api/storage/:id/share/*path
#[utoipa::path(
    get,
    path = "/api/storage/{id}/share/{path}",
    tag = "share",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Entry path"),
    ),
    responses(
        (status = 200, description = "List of shares", body = Vec<ShareResponse>),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn list_shares_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, ApiError> {
    let _storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    let sub_path = path.trim_matches('/');

    let entry_record = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await?;

    let entry_model = match entry_record {
        Some(m) => m,
        None => return Ok(Json(vec![])),
    };

    require_entry_owner(&auth, &entry_model, &state.db).await?;

    let shares = shared::Entity::find()
        .filter(shared::Column::PathId.eq(entry_model.id))
        .all(&state.db)
        .await?;

    Ok(Json(
        shares
            .into_iter()
            .map(|s| ShareResponse::from_model(s, entry_model.clone()))
            .collect(),
    ))
}

/// List all shares owned by the current user
/// GET /api/storage/share
#[utoipa::path(
    get,
    path = "/api/storage/share",
    tag = "share",
    responses(
        (status = 200, description = "List of user's shares", body = Vec<ShareResponse>),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn list_all_user_shares_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, ApiError> {
    let user_id = auth.user.id;

    // Shares this user created, keyed off the authoritative `owner_id` column
    // (issue #32 / G1) rather than derived from the backing entry's ownership.
    let shares_with_entries = shared::Entity::find()
        .find_also_related(entry::Entity)
        .filter(shared::Column::OwnerId.eq(user_id))
        .all(&state.db)
        .await?;

    let response = shares_with_entries
        .into_iter()
        .filter_map(|(s, e)| e.map(|entry_model| ShareResponse::from_model(s, entry_model)))
        .collect();

    Ok(Json(response))
}

/// List shares shared with the current user
/// GET /api/storage/share/with-me
#[utoipa::path(
    get,
    path = "/api/storage/share/with-me",
    tag = "share",
    responses(
        (status = 200, description = "List of shares shared with user", body = Vec<ShareResponse>),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn list_shares_with_me_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, ApiError> {
    let user_id = auth.user.id;

    // DRY-4: shared group-id loader
    let user_groups = user_group_ids(&state.db, user_id).await?;

    // Query shares where user_ids contains the user OR group_ids intersects.
    let shares_with_entries = shared::Entity::find()
        .find_also_related(entry::Entity)
        .filter(
            Condition::any()
                .add(Expr::cust_with_expr("$1 = ANY(user_ids)", user_id))
                .add(Expr::cust_with_expr("group_ids && $1", user_groups.clone())),
        )
        .all(&state.db)
        .await?;

    let response = shares_with_entries
        .into_iter()
        .filter_map(|(s, e)| e.map(|entry_model| ShareResponse::from_model(s, entry_model)))
        .collect();

    Ok(Json(response))
}

/// Create a share for an entry
/// POST /api/storage/:id/share/*path
#[utoipa::path(
    post,
    path = "/api/storage/{id}/share/{path}",
    tag = "share",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "Entry path"),
    ),
    request_body = ShareEntryRequest,
    responses(
        (status = 200, description = "Share created", body = ShareResponse),
        (status = 404, description = "Path not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_entry_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShareEntryRequest>,
) -> Result<Json<ShareResponse>, ApiError> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, storage_id))?;

    let sub_path = path.trim_matches('/');

    // Ensure entry exists in DB (create if missing)
    let entry_model = match entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await?
    {
        Some(m) => m,
        None => {
            // Create entry if it exists on disk
            storage
                .ensure_entry(&state.db, sub_path)
                .await
                .map_err(|e| {
                    if e.to_string().contains("not found on disk") {
                        not_found_msg("Path not found")
                    } else {
                        internal(e.to_string())
                    }
                })?
        }
    };

    require_entry_owner(&auth, &entry_model, &state.db).await?;

    let entry_id = entry_model.id;

    let expires_at = resolve_share_expiry(payload.expires_in_days);

    let (plaintext_token, token_hash) = if payload.public_link {
        let (plaintext, hash) = generate_share_token();
        (Some(plaintext), Some(hash))
    } else {
        (None, None)
    };

    let new_shared = shared::ActiveModel {
        path_id: Set(entry_id),
        owner_id: Set(auth.user.id),
        token: Set(token_hash),
        can_write: Set(payload.can_write),
        user_ids: Set(payload.user_ids),
        group_ids: Set(payload.group_ids),
        expires_at: Set(expires_at),
        created_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };

    let created = new_shared.insert(&state.db).await?;

    let entry_model = entry::Entity::find_by_id(created.path_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| internal("Entry not found after sharing"))?;

    let mut response = ShareResponse::from_model(created, entry_model);
    response.token = plaintext_token;

    Ok(Json(response))
}

/// Delete a share
/// DELETE /api/storage/share/:share_id
#[utoipa::path(
    delete,
    path = "/api/storage/share/{share_id}",
    tag = "share",
    params(("share_id" = i32, Path, description = "Share ID")),
    responses(
        (status = 200, description = "Share deleted"),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn delete_share_handler(
    auth: Auth,
    AxumPath(share_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    let share = shared::Entity::find_by_id(share_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg("Share not found"))?;

    let entry_model = entry::Entity::find_by_id(share.path_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| internal("Entry not found"))?;

    require_entry_owner(&auth, &entry_model, &state.db).await?;

    shared::Entity::delete_by_id(share_id)
        .exec(&state.db)
        .await?;

    Ok(message("Share deleted successfully"))
}

/// Update a share
/// PUT /api/storage/share/:share_id
#[utoipa::path(
    put,
    path = "/api/storage/share/{share_id}",
    tag = "share",
    params(("share_id" = i32, Path, description = "Share ID")),
    request_body = ShareEntryRequest,
    responses(
        (status = 200, description = "Share updated", body = ShareResponse),
        (status = 404, description = "Share not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
pub(crate) async fn update_share_handler(
    auth: Auth,
    AxumPath(share_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShareEntryRequest>,
) -> Result<Json<ShareResponse>, ApiError> {
    let share = shared::Entity::find_by_id(share_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg("Share not found"))?;

    let owning_entry = entry::Entity::find_by_id(share.path_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| internal("Entry not found"))?;

    require_entry_owner(&auth, &owning_entry, &state.db).await?;

    let expires_at = resolve_share_expiry(payload.expires_in_days);

    let (plaintext_token, token_hash) = if payload.public_link {
        if share.token.is_none() {
            let (plaintext, hash) = generate_share_token();
            (Some(plaintext), Some(hash))
        } else {
            // Keep the existing hash: we don't have the plaintext to hand
            // back, but the previously issued link must keep working.
            (None, share.token.clone())
        }
    } else {
        (None, None)
    };

    let mut active_share: shared::ActiveModel = share.into();
    active_share.token = Set(token_hash);
    active_share.can_write = Set(payload.can_write);
    active_share.user_ids = Set(payload.user_ids);
    active_share.group_ids = Set(payload.group_ids);
    active_share.expires_at = Set(expires_at);

    let updated = save_or_err(active_share, &state.db).await?;

    let entry_model = entry::Entity::find_by_id(updated.path_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| internal("Entry not found"))?;

    let mut response = ShareResponse::from_model(updated, entry_model);
    response.token = plaintext_token;

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Share-based access helpers
// ---------------------------------------------------------------------------

/// Helper function to verify share access for the current user.
///
/// Uses the shared [`user_group_ids`] helper (DRY-4) rather than re-querying
/// the group_user table inline.
async fn verify_share_access(
    auth: &Auth,
    share: &shared::Model,
    db: &sea_orm::DatabaseConnection,
) -> Result<(), ApiError> {
    let user_id = auth.user.id;

    if share.user_ids.contains(&user_id) {
        return Ok(());
    }

    let user_groups = user_group_ids(db, user_id).await?;
    if share.group_ids.iter().any(|g| user_groups.contains(g)) {
        return Ok(());
    }

    Err(forbidden("Access denied to this share"))
}

/// Resolve the `(share, entry, storage)` triple for a share-based request.
///
/// `share_identifier` may be either a numeric share ID (requires authorization)
/// or a token string (public link — access granted).
async fn get_share_context(
    share_identifier: &str,
    auth: Option<&Auth>,
    state: &AppState,
) -> Result<(shared::Model, entry::Model, StorageWrapper), ApiError> {
    let looked_up_by_id = share_identifier.parse::<i32>().is_ok();

    // Look up the share by id or token. Tokens are hashed at rest (issue
    // #10), so a token-based lookup compares against the hash of the
    // supplied identifier, never the plaintext.
    let share = if let Ok(share_id) = share_identifier.parse::<i32>() {
        shared::Entity::find_by_id(share_id).one(&state.db).await?
    } else {
        shared::Entity::find()
            .filter(shared::Column::Token.eq(Auth::hash_string(share_identifier)))
            .one(&state.db)
            .await?
    }
    .ok_or_else(|| not_found_msg("Share not found"))?;

    // Check expiration.
    if let Some(expires_at) = share.expires_at {
        if expires_at < chrono::Utc::now().naive_utc() {
            return Err(ApiError::Gone {
                msg: "Share has expired".to_string(),
            });
        }
    }

    // Verify access.
    if looked_up_by_id {
        let auth_val =
            auth.ok_or_else(|| unauthorized_msg("Authentication required for share access by ID"))?;
        verify_share_access(auth_val, &share, &state.db).await?;
    } else if share.token.is_none() {
        return Err(not_found_msg("Invalid share token"));
    }

    // Get the entry.
    let entry_model = entry::Entity::find_by_id(share.path_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg("Shared entry not found"))?;

    // Get the storage.
    let storage = StorageWrapper::find_by_id(&state.db, entry_model.storage_id)
        .await
        .map_err(|e| storage_lookup_err(e, entry_model.storage_id))?;

    Ok((share, entry_model, storage))
}

/// Public metadata about a single share (no secret token is ever included).
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ShareInfoResponse {
    pub id: i32,
    pub can_write: bool,
    pub path: String,
    /// Basename of the shared entry.
    pub name: String,
    /// `File` or `Directory`.
    pub entry_type: String,
    pub expires_at: Option<chrono::NaiveDateTime>,
    pub storage_id: i32,
}

/// Get share info
/// GET /api/storage/share/:share_id
#[utoipa::path(
    get,
    path = "/api/storage/share/{share_id}",
    tag = "share",
    params(("share_id" = String, Path, description = "Share ID or token")),
    responses(
        (status = 200, description = "Share info", body = ShareInfoResponse),
        (status = 404, description = "Share not found", body = ErrorResponse),
    )
)]
pub(crate) async fn get_share_info_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<ShareInfoResponse>, ApiError> {
    let (share, entry_model, _) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Never echo back `share.token` — it's a hash now (issue #10), and the
    // caller already knows the plaintext identifier they used to get here.
    let name = std::path::Path::new(&entry_model.path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&entry_model.path)
        .to_string();

    Ok(Json(ShareInfoResponse {
        id: share.id,
        can_write: share.can_write,
        path: entry_model.path.clone(),
        name,
        entry_type: format!("{:?}", entry_model.entry_type),
        expires_at: share.expires_at,
        storage_id: entry_model.storage_id,
    }))
}

/// List share root directory
/// GET /api/storage/share/:share_id/list
#[utoipa::path(
    get,
    path = "/api/storage/share/{share_id}/list",
    tag = "share",
    params(("share_id" = String, Path, description = "Share ID or token")),
    responses(
        (status = 200, description = "Directory listing"),
        (status = 404, description = "Share not found", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_list_root_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    share_list_impl(auth, share_id, String::new(), state).await
}

/// List share subdirectory
/// GET /api/storage/share/:share_id/list/*path
#[utoipa::path(
    get,
    path = "/api/storage/share/{share_id}/list/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "Subdirectory path"),
    ),
    responses(
        (status = 200, description = "Directory listing"),
        (status = 404, description = "Share not found", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_list_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    share_list_impl(auth, share_id, path, state).await
}

async fn share_list_impl(
    auth: Option<Auth>,
    share_id: String,
    sub_path: String,
    state: Arc<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let (_share, entry_model, storage) =
        get_share_context(&share_id, auth.as_ref(), &state).await?;

    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = sub_path.trim_matches('/');
    let full_path = join_share_path(base_path, sub_path_clean);

    let entries = storage
        .list_directory(&state.db, &full_path)
        .await
        .map_err(|e| internal(format!("Error listing directory: {e}")))?;

    dispatch_hash_jobs(&state, &entries);

    let relative_entries = make_entries_relative(entries, base_path);

    Ok(Json(DirectoryListingResponse {
        storage_id: entry_model.storage_id,
        path: sub_path_clean.to_string(),
        entries: relative_entries,
    }))
}

/// Share index root handler - Kodi-compatible directory listing for shares
async fn share_index_root_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    share_index_impl(auth, share_id, String::new(), state, req).await
}

/// Share index handler - Kodi-compatible directory listing or file serving for shares
#[instrument(skip(state, auth, req))]
async fn share_index_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    share_index_impl(auth, share_id, path, state, req).await
}

async fn share_index_impl(
    auth: Option<Auth>,
    share_id: String,
    sub_path: String,
    state: Arc<AppState>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    let ctx = get_share_context(&share_id, auth.as_ref(), &state).await;
    let (_share, entry_model, storage) = match ctx {
        Ok(c) => c,
        Err(ApiError::Unauthorized | ApiError::UnauthorizedMsg { .. }) => {
            // Issue a Basic-auth challenge for missing auth.
            return Ok((
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"Cloud\"")],
                "Authentication required",
            )
                .into_response());
        }
        Err(e) => return Err(e),
    };

    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = sub_path.trim_matches('/');
    let full_path = join_share_path(base_path, sub_path_clean);

    if full_path.contains("..") {
        return Err(bad_request("Invalid path: traversal detected"));
    }

    let abs_path = storage.get_full_path(&full_path);

    // If it's a file, serve it directly
    if abs_path.is_file() {
        let content_type = detect_content_type(&abs_path).await;
        return Ok(serve_file_response(abs_path, content_type, req)
            .await
            .into_response());
    }

    // Directory listing → HTML
    let entries = storage
        .list_directory(&state.db, &full_path)
        .await
        .map_err(|e| internal(format!("Error listing directory: {e}")))?;

    dispatch_hash_jobs(&state, &entries);

    let relative_entries = make_entries_relative(entries, base_path);

    let html = generate_directory_index(
        &state,
        entry_model.storage_id,
        sub_path_clean,
        &relative_entries,
    )
    .map_err(|e| {
        tracing::error!("Template error: {}", e);
        internal("Internal server error")
    })?;

    Ok(Html(html).into_response())
}

/// Transform directory entries to have paths relative to a base path.
fn make_entries_relative(entries: Vec<DirectoryEntry>, base_path: &str) -> Vec<DirectoryEntry> {
    entries
        .into_iter()
        .map(|mut entry| {
            let entry_path = entry.path.trim_matches('/');
            let relative_path = if base_path.is_empty() {
                entry_path.to_string()
            } else if let Some(rest) = entry_path.strip_prefix(base_path) {
                rest.trim_start_matches('/').to_string()
            } else {
                entry_path.to_string()
            };
            entry.path = relative_path;
            entry
        })
        .collect()
}

/// Get file content via share
/// GET /api/storage/share/:share_id/show/*path
#[utoipa::path(
    get,
    path = "/api/storage/share/{share_id}/show/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "File path"),
    ),
    responses(
        (status = 200, description = "File content"),
        (status = 404, description = "File not found", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_show_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, ApiError> {
    let (_share, entry_model, storage) =
        get_share_context(&share_id, auth.as_ref(), &state).await?;

    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let relative_path = join_share_path(base_path, sub_path_clean);

    if relative_path.contains("..") {
        return Err(bad_request("Invalid path: traversal detected"));
    }

    let abs_path = storage.get_full_path(&relative_path);

    if !abs_path.exists() || !abs_path.is_file() {
        return Err(not_found_msg("File not found"));
    }

    let content_type = detect_content_type(&abs_path).await;
    let res = serve_file_response(abs_path, content_type, req).await;
    Ok(res)
}

/// Update file content via share
/// PUT /api/storage/share/:share_id/update/*path
#[utoipa::path(
    put,
    path = "/api/storage/share/{share_id}/update/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "File path"),
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 200, description = "File updated"),
        (status = 403, description = "Write access denied", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth, body))]
pub(crate) async fn share_update_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err(forbidden("This share does not allow write access"));
    }

    let full_path = join_share_path(&entry_model.path, &path);
    require_within_share(&entry_model.path, &full_path)?;

    storage
        .save_file(&full_path, &body)
        .await
        .map_err(|e| internal(format!("Error saving file: {e}")))?;

    Ok(message("File updated successfully"))
}

/// Create file or folder via share
/// POST /api/storage/share/:share_id/create/*path
#[utoipa::path(
    post,
    path = "/api/storage/share/{share_id}/create/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "Entry path"),
    ),
    request_body = CreateEntryRequest,
    responses(
        (status = 200, description = "Entry created"),
        (status = 403, description = "Write access denied", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_create_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateEntryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err(forbidden("This share does not allow write access"));
    }

    let full_path = join_share_path(&entry_model.path, &path);
    require_within_share(&entry_model.path, &full_path)?;

    match payload.entry_type {
        EntryType::Directory => {
            storage
                .create_directory(&full_path)
                .await
                .map_err(|e| internal(format!("Error creating directory: {e}")))?;
        }
        EntryType::File => {
            storage
                .create_file(&full_path)
                .await
                .map_err(|e| internal(format!("Error creating file: {e}")))?;
        }
        _ => return Err(bad_request("Unsupported entry type")),
    }

    Ok(message("Entry created successfully"))
}

/// Rename file or folder via share
/// POST /api/storage/share/:share_id/rename/*path
#[utoipa::path(
    post,
    path = "/api/storage/share/{share_id}/rename/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "Current entry path"),
    ),
    request_body = RenameEntryRequest,
    responses(
        (status = 200, description = "Entry renamed"),
        (status = 403, description = "Write access denied", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_rename_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RenameEntryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err(forbidden("This share does not allow write access"));
    }

    let base_path = entry_model.path.trim_matches('/');
    let old_full_path = join_share_path(base_path, &path);
    let new_full_path = join_share_path(base_path, &payload.new_path);
    require_within_share(&entry_model.path, &old_full_path)?;
    require_within_share(&entry_model.path, &new_full_path)?;

    storage
        .rename_entry(&old_full_path, &new_full_path)
        .await
        .map_err(|e| internal(format!("Error renaming entry: {e}")))?;

    Ok(message("Entry renamed successfully"))
}

/// Delete file or folder via share
/// DELETE /api/storage/share/:share_id/remove/*path
#[utoipa::path(
    delete,
    path = "/api/storage/share/{share_id}/remove/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "Entry path"),
    ),
    responses(
        (status = 200, description = "Entry removed"),
        (status = 403, description = "Write access denied", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_remove_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err(forbidden("This share does not allow write access"));
    }

    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let full_path = if sub_path_clean.is_empty() {
        return Err(bad_request("Cannot delete share root"));
    } else {
        join_share_path(base_path, sub_path_clean)
    };
    require_within_share(&entry_model.path, &full_path)?;

    storage
        .remove_entry(&full_path)
        .await
        .map_err(|e| internal(format!("Error removing entry: {e}")))?;

    Ok(message("Entry removed successfully"))
}

/// Set the tags on a file or directory via share
/// PUT /api/storage/share/:share_id/tags/*path
#[utoipa::path(
    put,
    path = "/api/storage/share/{share_id}/tags/{path}",
    tag = "share",
    params(
        ("share_id" = String, Path, description = "Share ID or token"),
        ("path" = String, Path, description = "Entry path"),
    ),
    request_body = UpdateEntryTagsRequest,
    responses(
        (status = 200, description = "Tags updated"),
        (status = 403, description = "Write access denied", body = ErrorResponse),
        (status = 404, description = "Entry not found", body = ErrorResponse),
        (status = 409, description = "Entry has not been hashed yet", body = ErrorResponse),
    )
)]
#[instrument(skip(state, auth))]
pub(crate) async fn share_update_entry_tags_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateEntryTagsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (share, entry_model, _storage) =
        get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err(forbidden("This share does not allow write access"));
    }

    let full_path = join_share_path(&entry_model.path, &path);

    let target_entry = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(entry_model.storage_id))
        .filter(entry::Column::Path.eq(full_path.clone()))
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found_msg(format!("File not found: {full_path}")))?;

    set_entry_tags(&state.db, &target_entry, payload.tags).await?;

    Ok(message("Tags updated successfully"))
}

#[derive(Deserialize)]
pub(crate) struct ProcessQuery {
    /// Processing mode: "auto" (default), "force", or "hash_only"
    mode: Option<String>,
}

/// Trigger file processing job
/// POST /api/storage/:id/hash/*path?mode=auto|force|hash_only
#[utoipa::path(
    post,
    path = "/api/storage/{id}/hash/{path}",
    tag = "thumbnail",
    params(
        ("id" = i32, Path, description = "Storage ID"),
        ("path" = String, Path, description = "File path"),
        ("mode" = Option<String>, Query, description = "Processing mode: auto (default), force, hash_only"),
    ),
    responses(
        (status = 200, description = "Processing job queued"),
    ),
    security(("bearer" = []))
)]
pub(crate) async fn trigger_hash_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    Query(query): Query<ProcessQuery>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    require_admin(&auth)?;

    let mode = match query.mode.as_deref() {
        Some("force") => crate::job::ProcessMode::ForceClassify,
        Some("hash_only") => crate::job::ProcessMode::HashOnly,
        _ => crate::job::ProcessMode::Auto,
    };

    state
        .job_sender
        .try_send(crate::job::Job::ProcessFile {
            storage_id,
            path: path.trim_matches('/').to_string(),
            mode,
        })
        .map_err(|_| internal("Job queue is full or closed"))?;

    Ok(message("Hash calculation queued"))
}

#[cfg(test)]
mod tests {
    use super::{join_share_path, normalize_lexical, require_within_share, resolve_share_expiry};

    // Issue #9 / B8: create and update share one expiry contract.
    #[test]
    fn positive_days_sets_future_expiry() {
        let before = chrono::Utc::now().naive_utc();
        let expiry = resolve_share_expiry(Some(7)).expect("positive days must yield an expiry");
        let expected = before + chrono::Duration::days(7);
        // Allow a small clock delta between `before` and the value computed
        // inside the helper.
        assert!((expiry - expected).num_seconds().abs() <= 1);
        assert!(expiry > before);
    }

    #[test]
    fn zero_days_never_expires() {
        assert_eq!(resolve_share_expiry(Some(0)), None);
    }

    #[test]
    fn absent_never_expires() {
        assert_eq!(resolve_share_expiry(None), None);
    }

    // H6: share-subtree write containment helpers.
    #[test]
    fn normalize_lexical_drops_dot_and_resolves_dotdot() {
        assert_eq!(normalize_lexical("a/b"), "a/b");
        assert_eq!(normalize_lexical("a/./b"), "a/b");
        assert_eq!(normalize_lexical("a/sub/../file"), "a/file");
        // A leading ".." pops nothing (rooted at the share base) and is dropped.
        assert_eq!(normalize_lexical("../sibling"), "sibling");
        assert_eq!(normalize_lexical("dir_a/../../dir_b/file"), "dir_b/file");
        assert_eq!(normalize_lexical(""), "");
        assert_eq!(normalize_lexical("/"), "");
    }

    #[test]
    fn require_within_share_allows_descendants() {
        // base "dir_a", plain file
        let full = join_share_path("dir_a", "file.txt");
        assert!(require_within_share("dir_a", &full).is_ok());
        // base "dir_a", nested file with internal ".."
        let full = join_share_path("dir_a", "sub/../file");
        assert!(require_within_share("dir_a", &full).is_ok());
        // base "dir_a", exactly the base
        let full = join_share_path("dir_a", "");
        assert!(require_within_share("dir_a", &full).is_ok());
    }

    #[test]
    fn require_within_share_rejects_escape() {
        // dir_a/../../dir_b/file -> dir_b/file -> not under dir_a
        let full = join_share_path("dir_a", "../../dir_b/file");
        assert!(require_within_share("dir_a", &full).is_err());
        // ../sibling escapes to a sibling of the base
        let full = join_share_path("dir_a", "../sibling");
        assert!(require_within_share("dir_a", &full).is_err());
    }

    #[test]
    fn require_within_share_empty_base_shares_whole_storage() {
        // base "" (share of storage root): anything is allowed.
        let full = join_share_path("", "anywhere/file");
        assert!(require_within_share("", &full).is_ok());
        assert!(require_within_share("", "dir_b/file").is_ok());
    }
}
