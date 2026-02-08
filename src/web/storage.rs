use crate::auth::Auth;
use crate::entity::entry::EntryType;
use crate::entity::{entry, group, group_user, storage, user, shared};
use crate::storage::{Storage as StorageWrapper, DirectoryEntry, determine_content_type, thumbnail};
use crate::web::{require_admin, AppState, ErrorResponse};
use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{header, Request, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, put, post, delete},
    body::Body,
    Json, Router,
};
use tower::Service;
use tower_http::services::ServeFile;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set, Condition};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tracing::instrument;
use chrono;

/// Directory listing query parameters
#[derive(Debug, Deserialize)]
pub struct ListDirQuery {
    /// Output format: "json" or "html" (default: json)
    format: Option<String>,
}


/// Storage response
#[derive(Debug, Serialize)]
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
#[derive(Debug, Deserialize)]
pub struct CreateStorageRequest {
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub default_user: i32,
    pub default_group: i32,
}

/// Update storage request (all fields optional)
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct CreateEntryRequest {
    pub entry_type: EntryType,
}

/// Rename entry request
#[derive(Debug, Deserialize)]
pub struct RenameEntryRequest {
    pub new_path: String,
}

/// Create storage router with all storage-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_storages_handler).post(create_storage_handler))
        .route("/share", get(list_all_user_shares_handler))
        .route(
            "/:id",
            get(get_storage_handler)
                .put(update_storage_handler)
                .delete(delete_storage_handler),
        )
        .route("/:id/list", get(list_directory_root_handler))
        .route("/:id/list/", get(list_directory_root_handler))
        .route("/:id/list/*path", get(list_directory_handler))
        .route("/:id/show/*path", get(get_file_content_handler))
        .route("/:id/update/*path", put(update_file_content_handler))
        .route("/:id/raw/*path", get(download_file_handler))
        .route("/:id/create/*path", post(create_entry_handler))
        .route("/:id/rename/*path", post(rename_entry_handler))
        .route("/:id/remove/*path", delete(remove_entry_handler))
        .route("/:id/tags/*path", put(update_entry_tags_handler))
        .route("/:id/share/*path", get(list_shares_handler))
        .route("/:id/share/*path", post(share_entry_handler))
        .route("/share/:share_id", put(update_share_handler).delete(delete_share_handler))
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
        return Err((
            StatusCode::NOT_FOUND,
            format!("Thumbnail file not found"),
        ));
    }

    // Read the thumbnail file
    let thumbnail_data = fs::read(&thumbnail_path).await.map_err(|e| {
        tracing::error!(
            "Failed to read thumbnail {}/{}: {}",
            hash,
            size,
            e
        );
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
#[instrument(skip(state, _auth))]
async fn list_storages_handler(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StorageResponse>>, (StatusCode, Json<ErrorResponse>)> {

    let storages = storage::Entity::find()
        .all(&state.db)
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

    Ok(Json(
        storages.into_iter().map(StorageResponse::from).collect(),
    ))
}

/// Helper handler for listing the root directory (no trailing slash)
async fn list_directory_root_handler(
    auth: Auth,
    AxumPath(storage_id): AxumPath<i32>,
    query: Query<ListDirQuery>,
    state: State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    list_directory_handler(auth, AxumPath((storage_id, String::new())), query, state).await
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
    serve_file_with_content_type(storage_id, path, state, req, Some("application/octet-stream")).await
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
    
    let mut res = Service::<Request<Body>>::call(&mut service, req).await.map_err(|e| {
        match e {} // e is Infallible
    }).unwrap();

    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type)
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
    storage.save_file(&path, &body).await
        .map_err(|e| {
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
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Database error".to_string() }))
        })?;

    match payload.entry_type {
        EntryType::Directory => {
            storage.create_directory(&path).await.map_err(|e| {
                tracing::error!("Error creating directory: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: format!("Error creating directory: {}", e) }))
            })?;
        }
        EntryType::File => {
            storage.create_file(&path).await.map_err(|e| {
                tracing::error!("Error creating file: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: format!("Error creating file: {}", e) }))
            })?;
        }
        _ => return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Unsupported entry type".to_string() }))),
    }

    Ok(Json(serde_json::json!({ "message": "Entry created successfully" })))
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
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Database error".to_string() }))
        })?;

    storage.rename_entry(&old_path, &payload.new_path).await.map_err(|e| {
        tracing::error!("Error renaming entry: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: format!("Error renaming entry: {}", e) }))
    })?;

    Ok(Json(serde_json::json!({ "message": "Entry renamed successfully" })))
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
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Database error".to_string() }))
        })?;

    storage.remove_entry(&path).await.map_err(|e| {
        tracing::error!("Error removing entry: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: format!("Error removing entry: {}", e) }))
    })?;

    Ok(Json(serde_json::json!({ "message": "Entry removed successfully" })))
}

#[instrument(skip(state, _auth))]
async fn list_directory_handler(
    _auth: Auth,
    AxumPath((id, path)): AxumPath<(i32, String)>,
    Query(query): Query<ListDirQuery>,
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
    let entries = storage.list_directory(&state.db, &path).await
        .map_err(|e| {
            tracing::error!("Storage error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Error listing directory: {}", e),
                }),
            )
        })?;

    // Determine output format
    let format = query.format.as_deref().unwrap_or("json");
    let normalized_path = path.trim_matches('/');

    match format {
        "html" => {
            let html = generate_directory_index(&state, id, normalized_path, &entries)
                .map_err(|e| {
                    tracing::error!("Template error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "Internal server error".to_string(),
                        }),
                    )
                })?;
            Ok(Html(html).into_response())
        }
        _ => {
            // Default to JSON
            Ok(Json(serde_json::json!({
                "storage_id": id,
                "path": normalized_path,
                "entries": entries,
            }))
            .into_response())
        }
    }
}

/// Generate HTML directory index
fn generate_directory_index(state: &AppState, storage_id: i32, path: &str, entries: &[DirectoryEntry]) -> Result<String, minijinja::Error> {
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
        let a_name = a.path.trim_end_matches('/').split('/').last().unwrap_or(&a.path);
        let b_name = b.path.trim_end_matches('/').split('/').last().unwrap_or(&b.path);
        
        match (a.entry_type == EntryType::Directory, b.entry_type == EntryType::Directory) {
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
#[derive(Debug, Deserialize)]
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
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Database error".to_string() }))
        })?;

    // Normalize path and check filesystem
    let sub_path = path.trim_matches('/');
    let entry_id = storage.set_tags(&state.db, sub_path, payload.tags).await
        .map_err(|e| {
            tracing::error!("Failed to set tags: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() }))
        })?;

    Ok(Json(serde_json::json!({ "id": entry_id, "message": "Tags updated successfully" })))
}

/// Share entry request
#[derive(Debug, Deserialize)]
pub struct ShareEntryRequest {
    pub can_write: bool,
    pub expires_in_days: Option<u64>,
    pub public_link: bool,
    pub user_ids: Vec<i32>,
    pub group_ids: Vec<i32>,
}

/// Share response
#[derive(Debug, Serialize)]
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let sub_path = path.trim_matches('/');
    
    // Find entry
    let entry_record = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let entry_model = match entry_record {
        Some(m) => m,
        None => return Ok(Json(vec![])), // No entry, so no shares
    };

    let shares = shared::Entity::find()
        .filter(shared::Column::PathId.eq(entry_model.id))
        .all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    Ok(Json(shares.into_iter().map(|s| ShareResponse::from_model(s, entry_model.clone())).collect()))
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?
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
                .add(entry::Column::GroupId.is_in(user_groups))
        )
        .all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let response = shares_with_entries.into_iter()
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let sub_path = path.trim_matches('/');
    
    // Ensure entry exists in DB (create if missing)
    // We can use storage.set_tags or similar if we wanted, but we just need entry_id
    // For now, let's just find or error
    let entry_record = entry::Entity::find()
        .filter(entry::Column::StorageId.eq(storage_id))
        .filter(entry::Column::Path.eq(sub_path))
        .one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let entry_id = match entry_record {
        Some(m) => m.id,
        None => {
            // Create entry if it exists on disk
            let full_path = storage.get_full_path(sub_path);
            if !full_path.exists() {
                return Err((StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Path not found".to_string() })));
            }
            // Use set_tags with empty tags to create the entry
            storage.set_tags(&state.db, sub_path, vec![]).await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?
        }
    };

    let expires_at = payload.expires_in_days.map(|days| {
        chrono::Utc::now().naive_utc() + chrono::Duration::days(days as i64)
    });

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

    let created = new_shared.insert(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    // Fetch the entry model to build the response
    let entry_model = entry::Entity::find_by_id(created.path_id)
        .one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Entry not found after sharing".to_string() })))?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    Ok(Json(serde_json::json!({ "message": "Share deleted successfully" })))
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Share not found".to_string() })))?;

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
    
    let updated = active_share.update(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?;

    let entry_model = entry::Entity::find_by_id(updated.path_id)
        .one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: e.to_string() })))?
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Entry not found".to_string() })))?;

    Ok(Json(ShareResponse::from_model(updated, entry_model)))
}

