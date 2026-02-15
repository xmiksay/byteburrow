use crate::auth::Auth;
use crate::entity::entry::EntryType;
use crate::entity::{entry, group, group_user, shared, storage, user};
use crate::storage::{
    determine_content_type, thumbnail, DirectoryEntry, Storage as StorageWrapper,
};
use crate::web::{require_admin, AppState, ErrorResponse};
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, Request, StatusCode},
    response::{Html, IntoResponse},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono;
use sea_orm::{
    sea_query::Expr, ActiveModelTrait, ColumnTrait, Condition, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tower::Service;
use tower_http::services::ServeFile;
use tracing::instrument;


/// Storage response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StorageResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub default_user: i32,
    pub default_group: i32,
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
}

/// Update storage request (all fields optional)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateStorageRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub path: Option<String>,
    pub default_user: Option<i32>,
    pub default_group: Option<i32>,
}

/// Validate that a path exists and is a directory
async fn validate_storage_path(path: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let path_buf = PathBuf::from(path);

    // Check if path exists
    if !path_buf.exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Path does not exist: {}", path),
            }),
        ));
    }

    // Check if it's a directory
    if !path_buf.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Path is not a directory: {}", path),
            }),
        ));
    }

    // Check read permissions by attempting to read directory
    let read_check = fs::read_dir(&path_buf).await;
    if read_check.is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Cannot read directory (permission denied): {}", path),
            }),
        ));
    }

    Ok(())
}

/// Create entry request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateEntryRequest {
    pub entry_type: EntryType,
}

/// Rename entry request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RenameEntryRequest {
    pub new_path: String,
}

/// Create storage router with all storage-related endpoints
pub fn router() -> Router<Arc<AppState>> {
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
        .route("/:id/remove/*path", delete(remove_entry_handler))
        .route("/:id/tags/*path", put(update_entry_tags_handler))
        .route("/:id/hash/*path", post(trigger_hash_handler))
        .route("/:id/share/*path", get(list_shares_handler))
        .route("/:id/share/*path", post(share_entry_handler))
        .route(
            "/share/:share_id",
            get(get_share_info_handler)
                .put(update_share_handler)
                .delete(delete_share_handler),
        )
        // Share-based access routes
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
            delete(share_remove_handler),
        )
        .route("/share/:share_id/tags/*path", put(share_tags_handler))
        .route("/thumbnail/:hash/:size", get(thumbnail_handler))
}

/// Thumbnail endpoint - serves thumbnail by entry hash (public, no auth)
/// GET /api/storage/thumbnail/:hash/:size
#[instrument(skip(state))]
async fn thumbnail_handler(
    AxumPath((hash, size)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Validate hash format (basic validation)
    if hash.is_empty() || hash.len() < 6 || hash.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "Invalid hash format".to_string()));
    }

    // Validate size
    if !["small", "large", "mini"].contains(&size.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "Invalid size".to_string()));
    }

    // Construct thumbnail path using the hash and size with distributed directory structure
    // /1234/56/rest_of_hash_size.png
    let thumbnail_dir = PathBuf::from(&state.config.thumbnail_storage);
    let thumbnail_path = thumbnail::get_thumbnail_path(&thumbnail_dir, &hash, &size);

    // Check if thumbnail exists
    if !thumbnail_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("Thumbnail file not found")));
    }

    // Read the thumbnail file
    let thumbnail_data = fs::read(&thumbnail_path).await.map_err(|e| {
        tracing::error!("Failed to read thumbnail {}/{}: {}", hash, size, e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read thumbnail".to_string(),
        )
    })?;

    // Determine content type based on file extension or first bytes
    let content_type = determine_content_type(&thumbnail_path, &thumbnail_data);

    Ok(([(header::CONTENT_TYPE, content_type)], thumbnail_data))
}

/// Create new storage
/// POST /api/storage
#[utoipa::path(
    post,
    path = "/api/storage",
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
) -> Result<Json<StorageResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Validate path exists and is accessible
    validate_storage_path(&payload.path).await?;

    // Check if path already exists in storage
    let existing_path = storage::Entity::find()
        .filter(storage::Column::Path.eq(&payload.path))
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    if existing_path.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: format!("Storage with path '{}' already exists", payload.path),
            }),
        ));
    }

    // Check if name already exists
    let existing_name = storage::Entity::find()
        .filter(storage::Column::Name.eq(&payload.name))
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    if existing_name.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: format!("Storage with name '{}' already exists", payload.name),
            }),
        ));
    }

    // Verify default_user exists
    let user_exists = user::Entity::find_by_id(payload.default_user)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    if user_exists.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("User with id {} not found", payload.default_user),
            }),
        ));
    }

    // Verify default_group exists
    let group_exists = group::Entity::find_by_id(payload.default_group)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    if group_exists.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Group with id {} not found", payload.default_group),
            }),
        ));
    }

    let new_storage = storage::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        path: Set(payload.path),
        default_user: Set(payload.default_user),
        default_group: Set(payload.default_group),
        ..Default::default()
    };

    let created_storage = new_storage.insert(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create storage".to_string(),
            }),
        )
    })?;

    Ok(Json(StorageResponse::from(created_storage)))
}

/// Get storage by ID
/// GET /api/storage/:id
#[utoipa::path(
    get,
    path = "/api/storage/{id}",
    params(("id" = i32, Path, description = "Storage ID")),
    responses(
        (status = 200, description = "Storage found", body = StorageResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Storage not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, auth))]
async fn get_storage_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<StorageResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Storage {} not found", storage_id),
            }),
        ))?;

    Ok(Json(StorageResponse::from(storage)))
}

/// Update storage
/// PUT /api/storage/:id
#[utoipa::path(
    put,
    path = "/api/storage/{id}",
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
) -> Result<Json<StorageResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Find the storage
    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Storage {} not found", storage_id),
            }),
        ))?;

    // Validate path if being changed
    if let Some(ref new_path) = payload.path {
        if new_path != &storage.path {
            validate_storage_path(new_path).await?;

            // Check if path already exists
            let existing = storage::Entity::find()
                .filter(storage::Column::Path.eq(new_path))
                .one(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "Database error".to_string(),
                        }),
                    )
                })?;

            if existing.is_some() {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!("Storage with path '{}' already exists", new_path),
                    }),
                ));
            }
        }
    }

    // Check if name is being changed and if it conflicts
    if let Some(ref new_name) = payload.name {
        if new_name != &storage.name {
            let existing = storage::Entity::find()
                .filter(storage::Column::Name.eq(new_name))
                .one(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!("Database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "Database error".to_string(),
                        }),
                    )
                })?;

            if existing.is_some() {
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: format!("Storage with name '{}' already exists", new_name),
                    }),
                ));
            }
        }
    }

    // Validate default_user if being changed
    if let Some(user_id) = payload.default_user {
        let user_exists = user::Entity::find_by_id(user_id)
            .one(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            })?;

        if user_exists.is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("User with id {} not found", user_id),
                }),
            ));
        }
    }

    // Validate default_group if being changed
    if let Some(group_id) = payload.default_group {
        let group_exists = group::Entity::find_by_id(group_id)
            .one(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            })?;

        if group_exists.is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Group with id {} not found", group_id),
                }),
            ));
        }
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

    let updated_storage = active_storage.update(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to update storage".to_string(),
            }),
        )
    })?;

    Ok(Json(StorageResponse::from(updated_storage)))
}

/// Delete storage
/// DELETE /api/storage/:id
#[utoipa::path(
    delete,
    path = "/api/storage/{id}",
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
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Check if storage exists
    let storage = storage::Entity::find_by_id(storage_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Storage {} not found", storage_id),
            }),
        ))?;

    // Check if storage has entries
    let entry_count = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .count(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    if entry_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: format!(
                    "Cannot delete storage with {} entries. Delete entries first.",
                    entry_count
                ),
            }),
        ));
    }

    // Delete the storage
    storage::Entity::delete_by_id(storage_id)
        .exec(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to delete storage".to_string(),
                }),
            )
        })?;

    Ok(Json(serde_json::json!({
        "message": format!("Storage '{}' deleted successfully", storage.name),
    })))
}

/// List all storages endpoint
/// GET /api/storage
#[utoipa::path(
    get,
    path = "/api/storage",
    responses(
        (status = 200, description = "List of all storages", body = Vec<StorageResponse>),
    ),
    security(("bearer" = []))
)]
#[instrument(skip(state, _auth))]
async fn list_storages_handler(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StorageResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let storages = storage::Entity::find().all(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
            }),
        )
    })?;

    Ok(Json(
        storages.into_iter().map(StorageResponse::from).collect(),
    ))
}

/// Helper handler for listing the root directory (no trailing slash)
async fn list_directory_root_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    state: State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    list_directory_handler(auth, AxumPath((storage_id, String::new())), state).await
}

/// Directory index root handler - serves Kodi-compatible directory listing
async fn directory_index_root_handler(
    _auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    directory_index_impl(storage_id, String::new(), state, req).await
}

/// Directory index handler - serves Kodi-compatible directory listing or file content
#[instrument(skip(state, _auth, req))]
async fn directory_index_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    directory_index_impl(storage_id, path, state, req).await
}

async fn directory_index_impl(
    storage_id: i32,
    path: String,
    state: Arc<AppState>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Storage {} not found", storage_id),
                    }),
                )
            } else {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            }
        })?;

    let normalized_path = path.trim_matches('/');
    let full_path = storage.get_full_path(normalized_path);

    // If it's a file, serve it directly with content-type
    if full_path.is_file() {
        let content_type = match fs::File::open(&full_path).await {
            Ok(mut file) => {
                let mut buffer = [0u8; 1024];
                let n = file.read(&mut buffer).await.unwrap_or(0);
                determine_content_type(&full_path, &buffer[..n])
            }
            Err(_) => "application/octet-stream",
        };

        let mut service = ServeFile::new(full_path);
        let mut res = Service::<Request<Body>>::call(&mut service, req)
            .await
            .map_err(|e| match e {})
            .unwrap();

        res.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(content_type),
        );

        return Ok(res.into_response());
    }

    // It's a directory - list entries and return HTML
    let entries = storage
        .list_directory(&state.db, normalized_path)
        .await
        .map_err(|e| {
            tracing::error!("Storage error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error listing directory: {}", e),
                }),
            )
        })?;

    entries
        .iter()
        .filter(|e| e.hash.is_none() && e.entry_type == EntryType::File)
        .for_each(|e| {
            state
                .job_sender
                .send(crate::job::Job::CheckFile {
                    storage_id: e.storage_id,
                    path: e.path.clone(),
                })
                .unwrap();
        });

    let html = generate_directory_index(&state, storage_id, normalized_path, &entries).map_err(
        |e| {
            tracing::error!("Template error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Internal server error".to_string(),
                }),
            )
        },
    )?;
    Ok(Html(html).into_response())
}

#[instrument(skip(state, _auth, req))]
async fn get_file_content_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    serve_file_with_content_type(storage_id, path, state, req, None).await
}

#[instrument(skip(state, _auth, req))]
async fn download_file_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    serve_file_with_content_type(
        storage_id,
        path,
        state,
        req,
        Some("application/octet-stream"),
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
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Find storage
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Storage {} not found", storage_id),
                    }),
                )
            } else {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            }
        })?;

    let full_path = storage.get_full_path(&path);

    if !full_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("File not found: {}", path),
            }),
        ));
    }

    if full_path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Requested path is a directory: {}", path),
            }),
        ));
    }

    let content_type = if let Some(ct) = forced_content_type {
        ct
    } else {
        // Read first few bytes to determine content type
        match fs::File::open(&full_path).await {
            Ok(mut file) => {
                let mut buffer = [0u8; 1024];
                let n = file.read(&mut buffer).await.unwrap_or(0);
                determine_content_type(&full_path, &buffer[..n])
            }
            Err(_) => "application/octet-stream",
        }
    };

    // Use tower-http's ServeFile which handles Range requests, ETag, etc.
    let mut service = ServeFile::new(full_path);

    let mut res = Service::<Request<Body>>::call(&mut service, req)
        .await
        .map_err(|e| {
            match e {} // e is Infallible
        })
        .unwrap();

    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );

    Ok(res.into_response())
}

#[instrument(skip(state, _auth, body))]
async fn update_file_content_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Find storage
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Storage {} not found", storage_id),
                    }),
                )
            } else {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            }
        })?;

    // Save file
    storage.save_file(&path, &body).await.map_err(|e| {
        tracing::error!("Error saving file: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Error saving file: {}", e),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "message": "File updated successfully",
    })))
}

#[instrument(skip(state, _auth))]
async fn create_entry_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateEntryRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    match payload.entry_type {
        EntryType::Directory => {
            storage.create_directory(&path).await.map_err(|e| {
                tracing::error!("Error creating directory: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Error creating directory: {}", e),
                    }),
                )
            })?;
        }
        EntryType::File => {
            storage.create_file(&path).await.map_err(|e| {
                tracing::error!("Error creating file: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Error creating file: {}", e),
                    }),
                )
            })?;
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Unsupported entry type".to_string(),
                }),
            ))
        }
    }

    Ok(Json(
        serde_json::json!({ "message": "Entry created successfully" }),
    ))
}

#[instrument(skip(state, _auth))]
async fn rename_entry_handler(
    _auth: Auth,
    AxumPath((storage_id, old_path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RenameEntryRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    storage
        .rename_entry(&old_path, &payload.new_path)
        .await
        .map_err(|e| {
            tracing::error!("Error renaming entry: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error renaming entry: {}", e),
                }),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "message": "Entry renamed successfully" }),
    ))
}

#[instrument(skip(state, _auth))]
async fn remove_entry_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    storage.remove_entry(&path).await.map_err(|e| {
        tracing::error!("Error removing entry: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Error removing entry: {}", e),
            }),
        )
    })?;

    Ok(Json(
        serde_json::json!({ "message": "Entry removed successfully" }),
    ))
}

#[instrument(skip(state, _auth))]
async fn list_directory_handler(
    _auth: Auth,
    AxumPath((id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    tracing::info!("Listing directory for storage {} at path {}", id, path);
    // Get storage wrapper
    let storage = StorageWrapper::find_by_id(&state.db, id)
        .await
        .map_err(|e| {
            if matches!(e, sea_orm::DbErr::RecordNotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: format!("Storage {} not found", id),
                    }),
                )
            } else {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Database error".to_string(),
                    }),
                )
            }
        })?;

    // List directory using merged FS/DB state
    let entries = storage
        .list_directory(&state.db, &path)
        .await
        .map_err(|e| {
            tracing::error!("Storage error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error listing directory: {}", e),
                }),
            )
        })?;

    entries
        .iter()
        .filter(|e| e.hash.is_none() && e.entry_type == EntryType::File)
        .for_each(|e| {
            state
                .job_sender
                .send(crate::job::Job::CheckFile {
                    storage_id: e.storage_id,
                    path: e.path.clone(),
                })
                .unwrap();
        });

    let normalized_path = path.trim_matches('/');

    Ok(Json(serde_json::json!({
        "storage_id": id,
        "path": normalized_path,
        "entries": entries,
    })))
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
        Some(path.rsplitn(2, '/').nth(1).unwrap_or(""))
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
            .last()
            .unwrap_or(&a.path);
        let b_name = b
            .path
            .trim_end_matches('/')
            .split('/')
            .last()
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

/// Update tags request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateEntryTagsRequest {
    pub tags: Vec<i32>,
}

/// Update entry tags handler
/// PUT /api/storage/:id/tags/*path
#[instrument(skip(state, auth))]
async fn update_entry_tags_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateEntryTagsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Find storage
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    // Normalize path and check filesystem
    let sub_path = path.trim_matches('/');
    let entry_id = storage
        .set_tags(&state.db, sub_path, payload.tags)
        .await
        .map_err(|e| {
            tracing::error!("Failed to set tags: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "id": entry_id, "message": "Tags updated successfully" }),
    ))
}

/// Share entry request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ShareEntryRequest {
    pub can_write: bool,
    pub expires_in_days: Option<u64>,
    pub public_link: bool,
    pub user_ids: Vec<i32>,
    pub group_ids: Vec<i32>,
}

/// Share response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ShareResponse {
    pub id: i32,
    pub storage_id: i32,
    pub path: String,
    pub token: Option<String>,
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
            token: m.token,
            can_write: m.can_write,
            user_ids: m.user_ids,
            group_ids: m.group_ids,
            expires_at: m.expires_at,
            created_at: m.created_at,
        }
    }
}

/// List shares handler
/// GET /api/storage/:id/share/*path
#[instrument(skip(state, _auth))]
async fn list_shares_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let _storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let sub_path = path.trim_matches('/');

    // Find entry
    let entry_record = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let entry_model = match entry_record {
        Some(m) => m,
        None => return Ok(Json(vec![])), // No entry, so no shares
    };

    let shares = shared::Entity::find()
        .filter(shared::Column::PathId.eq(entry_model.id))
        .all(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(
        shares
            .into_iter()
            .map(|s| ShareResponse::from_model(s, entry_model.clone()))
            .collect(),
    ))
}

/// List all user's shares
/// GET /api/storage/share
#[instrument(skip(state, auth))]
async fn list_all_user_shares_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let user_id = auth.user.id;

    // Find groups the user is in
    let user_groups = group_user::Entity::find()
        .filter(group_user::Column::UserId.eq(user_id))
        .all(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .into_iter()
        .map(|gu| gu.group_id)
        .collect::<Vec<i32>>();

    // Query shares using the provided logic:
    // This shared->entry->user.id = current_user.id
    // group_user.user_id = current_user.id and group_user.group_id = shared->entry.group_id
    let shares_with_entries = shared::Entity::find()
        .find_also_related(entry::Entity)
        .filter(
            Condition::any()
                .add(entry::Column::UserId.eq(user_id))
                .add(entry::Column::GroupId.is_in(user_groups)),
        )
        .all(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let response = shares_with_entries
        .into_iter()
        .filter_map(|(s, e)| e.map(|entry_model| ShareResponse::from_model(s, entry_model)))
        .collect();

    Ok(Json(response))
}

/// List shares shared with the current user
/// GET /api/storage/share/with-me
#[instrument(skip(state, auth))]
async fn list_shares_with_me_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ShareResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let user_id = auth.user.id;

    // Find groups the user is in
    let user_groups = group_user::Entity::find()
        .filter(group_user::Column::UserId.eq(user_id))
        .all(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .into_iter()
        .map(|gu| gu.group_id)
        .collect::<Vec<i32>>();

    // Query shares where user_ids contains current user OR group_ids contains one of user's groups
    // We need to use raw SQL or custom expressions for array containment
    let shares_with_entries = shared::Entity::find()
        .find_also_related(entry::Entity)
        .filter(
            Condition::any()
                .add(Expr::cust_with_expr("$1 = ANY(user_ids)", user_id))
                .add(Expr::cust_with_expr("group_ids && $1", user_groups.clone())),
        )
        .all(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let response = shares_with_entries
        .into_iter()
        .filter_map(|(s, e)| e.map(|entry_model| ShareResponse::from_model(s, entry_model)))
        .collect();

    Ok(Json(response))
}

/// Share entry handler
/// POST /api/storage/:id/share/*path
#[instrument(skip(state, _auth))]
async fn share_entry_handler(
    _auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShareEntryRequest>,
) -> Result<Json<ShareResponse>, (StatusCode, Json<ErrorResponse>)> {
    let storage = StorageWrapper::find_by_id(&state.db, storage_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let sub_path = path.trim_matches('/');

    // Ensure entry exists in DB (create if missing)
    // We can use storage.set_tags or similar if we wanted, but we just need entry_id
    // For now, let's just find or error
    let entry_record = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let entry_id = match entry_record {
        Some(m) => m.id,
        None => {
            // Create entry if it exists on disk
            let full_path = storage.get_full_path(sub_path);
            if !full_path.exists() {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: "Path not found".to_string(),
                    }),
                ));
            }
            // Use set_tags with empty tags to create the entry
            storage
                .set_tags(&state.db, sub_path, vec![])
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )
                })?
        }
    };

    let expires_at = payload
        .expires_in_days
        .map(|days| chrono::Utc::now().naive_utc() + chrono::Duration::days(days as i64));

    let token = if payload.public_link {
        Some(uuid::Uuid::new_v4().as_simple().to_string())
    } else {
        None
    };

    let new_shared = shared::ActiveModel {
        path_id: Set(entry_id),
        token: Set(token),
        can_write: Set(payload.can_write),
        user_ids: Set(payload.user_ids),
        group_ids: Set(payload.group_ids),
        expires_at: Set(expires_at),
        created_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };

    let created = new_shared.insert(&state.db).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Fetch the entry model to build the response
    let entry_model = entry::Entity::find_by_id(created.path_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Entry not found after sharing".to_string(),
                }),
            )
        })?;

    Ok(Json(ShareResponse::from_model(created, entry_model)))
}

/// Delete share handler
/// DELETE /api/storage/share/:share_id
#[instrument(skip(state, _auth))]
async fn delete_share_handler(
    _auth: Auth,
    AxumPath(share_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    shared::Entity::delete_by_id(share_id)
        .exec(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "message": "Share deleted successfully" }),
    ))
}

/// Update share handler
/// PUT /api/storage/share/:share_id
#[instrument(skip(state, _auth))]
async fn update_share_handler(
    _auth: Auth,
    AxumPath(share_id): AxumPath<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShareEntryRequest>,
) -> Result<Json<ShareResponse>, (StatusCode, Json<ErrorResponse>)> {
    let share = shared::Entity::find_by_id(share_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Share not found".to_string(),
                }),
            )
        })?;

    let expires_at = payload.expires_in_days.and_then(|days| {
        if days > 0 {
            Some(chrono::Utc::now().naive_utc() + chrono::Duration::days(days as i64))
        } else {
            None
        }
    });

    let token = if payload.public_link {
        if share.token.is_none() {
            Some(uuid::Uuid::new_v4().as_simple().to_string())
        } else {
            share.token.clone()
        }
    } else {
        None
    };

    let mut active_share: shared::ActiveModel = share.into();
    active_share.token = Set(token);
    active_share.can_write = Set(payload.can_write);
    active_share.user_ids = Set(payload.user_ids);
    active_share.group_ids = Set(payload.group_ids);
    active_share.expires_at = Set(expires_at);

    let updated = active_share.update(&state.db).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    let entry_model = entry::Entity::find_by_id(updated.path_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Entry not found".to_string(),
                }),
            )
        })?;

    Ok(Json(ShareResponse::from_model(updated, entry_model)))
}

/// Helper function to verify share access for the current user
async fn verify_share_access(
    auth: &Auth,
    share: &shared::Model,
    db: &sea_orm::DatabaseConnection,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let user_id = auth.user.id;

    // Check if user is in user_ids
    if share.user_ids.contains(&user_id) {
        return Ok(());
    }

    // Check if user's groups overlap with group_ids
    let user_groups = group_user::Entity::find()
        .filter(group_user::Column::UserId.eq(user_id))
        .all(db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .into_iter()
        .map(|gu| gu.group_id)
        .collect::<Vec<i32>>();

    for gid in &share.group_ids {
        if user_groups.contains(gid) {
            return Ok(());
        }
    }

    Err((
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: "Access denied to this share".to_string(),
        }),
    ))
}

/// Helper to get share, entry, and storage for share-based access
/// The share_identifier can be either a numeric ID or a token string
async fn get_share_context(
    share_identifier: &str,
    auth: Option<&Auth>,
    state: &AppState,
) -> Result<(shared::Model, entry::Model, StorageWrapper), (StatusCode, Json<ErrorResponse>)> {
    // Try to parse as numeric ID first, otherwise treat as token
    let share = if let Ok(share_id) = share_identifier.parse::<i32>() {
        shared::Entity::find_by_id(share_id)
            .one(&state.db)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
            })?
    } else {
        // Treat as token
        shared::Entity::find()
            .filter(shared::Column::Token.eq(share_identifier))
            .one(&state.db)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
            })?
    };

    let share = share.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Share not found".to_string(),
            }),
        )
    })?;

    // Check expiration
    if let Some(expires_at) = share.expires_at {
        if expires_at < chrono::Utc::now().naive_utc() {
            return Err((
                StatusCode::GONE,
                Json(ErrorResponse {
                    error: "Share has expired".to_string(),
                }),
            ));
        }
    }

    // Verify access
    // If we looked up by ID (numeric), we MUST verify auth/permissions.
    // If we looked up by token (string), access is granted (public link).

    let looked_up_by_id = share_identifier.parse::<i32>().is_ok();

    if looked_up_by_id {
        // Accessing by ID requires explicit authorization (even if it has a token)
        let auth_val = auth.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "Authentication required for share access by ID".to_string(),
                }),
            )
        })?;
        verify_share_access(auth_val, &share, &state.db).await?;
    } else {
        // Accessing by token
        // Ensure the share actually HAS a token (it should if we found it via token query)
        if share.token.is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Invalid share token".to_string(),
                }),
            ));
        }
        // Access granted for public token
    }

    // Get the entry
    let entry_model = entry::Entity::find_by_id(share.path_id)
        .one(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Shared entry not found".to_string(),
                }),
            )
        })?;

    // Get the storage
    let storage = StorageWrapper::find_by_id(&state.db, entry_model.storage_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok((share, entry_model, storage))
}

// Helper to get share info
/// GET /api/storage/share/:share_id
async fn get_share_info_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, _) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    Ok(Json(serde_json::json!({
        "id": share.id,
        "token": share.token,
        "can_write": share.can_write,
        "path": entry_model.path,
        "name": std::path::Path::new(&entry_model.path).file_name().and_then(|s| s.to_str()).unwrap_or(&entry_model.path),
        "entry_type": format!("{:?}", entry_model.entry_type),
        "expires_at": share.expires_at,
        "storage_id": entry_model.storage_id
    })))
}

/// Share list root handler
/// GET /api/storage/share/:share_id/list
#[instrument(skip(state, auth))]
async fn share_list_root_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    share_list_impl(auth, share_id, String::new(), state).await
}

/// Share list handler
/// GET /api/storage/share/:share_id/list/*path
#[instrument(skip(state, auth))]
async fn share_list_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    share_list_impl(auth, share_id, path, state).await
}

async fn share_list_impl(
    auth: Option<Auth>,
    share_id: String,
    sub_path: String,
    state: Arc<AppState>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Calculate full path relative to storage
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = sub_path.trim_matches('/');
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // List directory
    let entries = storage
        .list_directory(&state.db, &full_path)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error listing directory: {}", e),
                }),
            )
        })?;

    entries
        .iter()
        .filter(|e| e.hash.is_none() && e.entry_type == EntryType::File)
        .for_each(|e| {
            state
                .job_sender
                .send(crate::job::Job::CheckFile {
                    storage_id: e.storage_id,
                    path: e.path.clone(),
                })
                .unwrap();
        });

    // Transform entries to have paths relative to the share's base path
    let relative_entries: Vec<_> = entries
        .into_iter()
        .map(|mut entry| {
            // Remove the base path prefix from entry path
            let entry_path = entry.path.trim_matches('/');
            let relative_path = if base_path.is_empty() {
                entry_path.to_string()
            } else if entry_path.starts_with(base_path) {
                entry_path[base_path.len()..]
                    .trim_start_matches('/')
                    .to_string()
            } else {
                entry_path.to_string()
            };
            entry.path = relative_path;
            entry
        })
        .collect();

    Ok(Json(serde_json::json!({
        "share_id": share.id,
        "storage_id": entry_model.storage_id,
        "path": sub_path_clean,
        "entries": relative_entries,
    })))
}

/// Share index root handler - Kodi-compatible directory listing for shares
async fn share_index_root_handler(
    auth: Option<Auth>,
    AxumPath(share_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    share_index_impl(auth, share_id, String::new(), state, req).await
}

/// Share index handler - Kodi-compatible directory listing or file serving for shares
#[instrument(skip(state, auth, req))]
async fn share_index_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    share_index_impl(auth, share_id, path, state, req).await
}

async fn share_index_impl(
    auth: Option<Auth>,
    share_id: String,
    sub_path: String,
    state: Arc<AppState>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let ctx = get_share_context(&share_id, auth.as_ref(), &state).await;
    // If auth is required (numeric share ID) but missing, return proper basic auth challenge
    let (_share, entry_model, storage) = match ctx {
        Ok(ctx) => ctx,
        Err((StatusCode::UNAUTHORIZED, _)) => {
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
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    let abs_path = storage.get_full_path(&full_path);

    // If it's a file, serve it directly
    if abs_path.is_file() {
        let content_type = match fs::File::open(&abs_path).await {
            Ok(mut file) => {
                let mut buffer = [0u8; 1024];
                let n = file.read(&mut buffer).await.unwrap_or(0);
                determine_content_type(&abs_path, &buffer[..n])
            }
            Err(_) => "application/octet-stream",
        };

        let mut service = ServeFile::new(&abs_path);
        let mut res = Service::<Request<Body>>::call(&mut service, req)
            .await
            .map_err(|e| match e {})
            .unwrap();

        res.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(content_type),
        );

        return Ok(res.into_response());
    }

    // It's a directory - list entries and return HTML
    let entries = storage
        .list_directory(&state.db, &full_path)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error listing directory: {}", e),
                }),
            )
        })?;

    entries
        .iter()
        .filter(|e| e.hash.is_none() && e.entry_type == EntryType::File)
        .for_each(|e| {
            state
                .job_sender
                .send(crate::job::Job::CheckFile {
                    storage_id: e.storage_id,
                    path: e.path.clone(),
                })
                .unwrap();
        });

    // Transform entries to have paths relative to the share's base path
    let relative_entries: Vec<_> = entries
        .into_iter()
        .map(|mut entry| {
            let entry_path = entry.path.trim_matches('/');
            let relative_path = if base_path.is_empty() {
                entry_path.to_string()
            } else if entry_path.starts_with(base_path) {
                entry_path[base_path.len()..]
                    .trim_start_matches('/')
                    .to_string()
            } else {
                entry_path.to_string()
            };
            entry.path = relative_path;
            entry
        })
        .collect();

    let html = generate_directory_index(
        &state,
        entry_model.storage_id,
        sub_path_clean,
        &relative_entries,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Html(html).into_response())
}

/// Share show handler - get file content
/// GET /api/storage/share/:share_id/show/*path
#[instrument(skip(state, auth))]
async fn share_show_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (_share, entry_model, storage) =
        get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Calculate full path (relative to storage root)
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let relative_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // Security check: prevent path traversal
    if relative_path.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid path: traversal detected".to_string(),
            }),
        ));
    }

    let abs_path = storage.get_full_path(&relative_path);

    if !abs_path.exists() || !abs_path.is_file() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "File not found".to_string(),
            }),
        ));
    }

    // Detect content type
    let content_type = match fs::File::open(&abs_path).await {
        Ok(mut file) => {
            let mut buffer = [0u8; 1024];
            let n = file.read(&mut buffer).await.unwrap_or(0);
            determine_content_type(&abs_path, &buffer[..n])
        }
        Err(_) => "application/octet-stream",
    };

    // Use tower-http's ServeFile which handles Range requests
    let mut service = ServeFile::new(&abs_path);

    // Call the service
    // Note: ServeFile error type is Infallible in newer versions or io::Error in older?
    // Based on existing code usage, we assume Infallible if pattern matching empty enum works.
    let mut res = Service::<Request<Body>>::call(&mut service, req)
        .await
        .map_err(|e| match e {})
        .unwrap();

    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );

    Ok(res)
}

/// Share update handler - update file content
/// PUT /api/storage/share/:share_id/update/*path
#[instrument(skip(state, auth, body))]
async fn share_update_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Check write permission
    if !share.can_write {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "This share does not allow write access".to_string(),
            }),
        ));
    }

    // Calculate full path
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // Save the file
    storage.save_file(&full_path, &body).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Error saving file: {}", e),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "message": "File updated successfully",
    })))
}

/// Share create handler - create file or folder
/// POST /api/storage/share/:share_id/create/*path
#[instrument(skip(state, auth))]
async fn share_create_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateEntryRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Check write permission
    if !share.can_write {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "This share does not allow write access".to_string(),
            }),
        ));
    }

    // Calculate full path
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // Create the entry
    match payload.entry_type {
        EntryType::Directory => {
            storage.create_directory(&full_path).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Error creating directory: {}", e),
                    }),
                )
            })?;
        }
        EntryType::File => {
            storage.create_file(&full_path).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Error creating file: {}", e),
                    }),
                )
            })?;
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Unsupported entry type".to_string(),
                }),
            ))
        }
    }

    Ok(Json(serde_json::json!({
        "message": "Entry created successfully",
    })))
}

/// Share rename handler - rename file or folder
/// POST /api/storage/share/:share_id/rename/*path
#[instrument(skip(state, auth))]
async fn share_rename_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RenameEntryRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Check write permission
    if !share.can_write {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "This share does not allow write access".to_string(),
            }),
        ));
    }

    // Calculate full paths
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let old_full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // New path is relative to share, so we need to add base_path
    let new_sub_path = payload.new_path.trim_matches('/');
    let new_full_path = if base_path.is_empty() {
        new_sub_path.to_string()
    } else {
        format!("{}/{}", base_path, new_sub_path)
    };

    // Rename the entry
    storage
        .rename_entry(&old_full_path, &new_full_path)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error renaming entry: {}", e),
                }),
            )
        })?;

    Ok(Json(serde_json::json!({
        "message": "Entry renamed successfully",
    })))
}

/// Share remove handler - delete file or folder
/// DELETE /api/storage/share/:share_id/remove/*path
#[instrument(skip(state, auth))]
async fn share_remove_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    // Check write permission
    if !share.can_write {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "This share does not allow write access".to_string(),
            }),
        ));
    }

    // Calculate full path
    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Cannot delete share root".to_string(),
            }),
        ));
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    // Remove the entry
    storage.remove_entry(&full_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Error removing entry: {}", e),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "message": "Entry removed successfully",
    })))
}

/// Share tags handler - update tags on an entry within a share
/// PUT /api/storage/share/:share_id/tags/*path
#[instrument(skip(state, auth))]
async fn share_tags_handler(
    auth: Option<Auth>,
    AxumPath((share_id, path)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateEntryTagsRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let (share, entry_model, storage) = get_share_context(&share_id, auth.as_ref(), &state).await?;

    if !share.can_write {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "This share does not allow write access".to_string(),
            }),
        ));
    }

    let base_path = entry_model.path.trim_matches('/');
    let sub_path_clean = path.trim_matches('/');
    let full_path = if base_path.is_empty() {
        sub_path_clean.to_string()
    } else if sub_path_clean.is_empty() {
        base_path.to_string()
    } else {
        format!("{}/{}", base_path, sub_path_clean)
    };

    let entry_id = storage
        .set_tags(&state.db, &full_path, payload.tags)
        .await
        .map_err(|e| {
            tracing::error!("Failed to set tags: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "id": entry_id, "message": "Tags updated successfully" }),
    ))
}

/// Trigger hash calculation job for a file
/// POST /api/storage/:id/hash/*path
async fn trigger_hash_handler(
    auth: Auth,
    AxumPath((storage_id, path)): AxumPath<(i32, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    state
        .job_sender
        .send(crate::job::Job::CheckFile {
            storage_id,
            path: path.trim_matches('/').to_string(),
        })
        .map_err(|e| {
            tracing::error!("Failed to send job: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to queue job".to_string(),
                }),
            )
        })?;

    Ok(Json(
        serde_json::json!({ "message": "Hash calculation queued" }),
    ))
}
