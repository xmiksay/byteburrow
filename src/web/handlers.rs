use crate::entity::storage;
use crate::web::{auth::*, AppState};
use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs;
use std::path::PathBuf;

/// Login request payload
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_in_days: i64,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Login endpoint - accepts Basic Auth or JSON payload
pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    use crate::entity::user;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    // Get salt from config
    let password_hash = hash_password(&payload.password, &state.config.salt);

    // Find user by username and password hash
    let user_record = user::Entity::find()
        .filter(user::Column::Username.eq(&payload.username))
        .filter(user::Column::Password.eq(password_hash))
        .one(&state.db)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Database error".to_string(),
                }),
            )
        })?;

    let user = user_record.ok_or((
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "Invalid credentials".to_string(),
        }),
    ))?;

    // Create token (30 days validity)
    let duration_days = 30;
    let token = create_token(user.id, duration_days, None, None, &state)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to create token".to_string(),
                }),
            )
        })?;

    Ok(Json(LoginResponse {
        token,
        expires_in_days: duration_days,
    }))
}

/// Protected handler - requires authentication
pub async fn protected_handler(
    auth_user: AuthUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let records = storage::Entity::find()
        .all(&state.db)
        .await
        .expect("Failed to fetch records from database");

    Json(serde_json::json!({
        "user": {
            "id": auth_user.id,
            "username": auth_user.username,
            "name": auth_user.name,
            "admin": auth_user.admin,
        },
        "storages": records,
    }))
}

/// Me endpoint - returns current user info
pub async fn me_handler(auth_user: AuthUser) -> impl IntoResponse {
    Json(serde_json::json!({
        "id": auth_user.id,
        "username": auth_user.username,
        "name": auth_user.name,
        "admin": auth_user.admin,
    }))
}

/// Health check endpoint (no auth required)
pub async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "cloud",
    }))
}

/// Thumbnail endpoint - serves thumbnail by entry hash (public, no auth)
/// GET /api/thumbnail/:hash
pub async fn thumbnail_handler(
    AxumPath(hash): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use crate::entity::entry;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    // Validate hash format (basic validation)
    if hash.is_empty() || hash.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "Invalid hash format".to_string()));
    }

    // Find entry by hash in database
    let entry_record = entry::Entity::find()
        .filter(entry::Column::Hash.eq(&hash))
        .one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error while looking up hash {}: {}", hash, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error".to_string())
        })?;

    let entry_entity = entry_record.ok_or((
        StatusCode::NOT_FOUND,
        "Entry not found for this hash".to_string(),
    ))?;

    // Construct thumbnail path using the hash
    let thumbnail_dir = PathBuf::from(&state.config.thumbnail_storage);
    let thumbnail_path = thumbnail_dir.join(&hash);

    // Check if thumbnail exists
    if !thumbnail_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Thumbnail file not found for entry: {}", entry_entity.path),
        ));
    }

    // Read the thumbnail file
    let thumbnail_data = fs::read(&thumbnail_path)
        .await
        .map_err(|e| {
            tracing::error!("Failed to read thumbnail {} for entry {}: {}", hash, entry_entity.path, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read thumbnail".to_string())
        })?;

    // Determine content type based on file extension or first bytes
    let content_type = determine_content_type(&thumbnail_path, &thumbnail_data);

    Ok((
        [(header::CONTENT_TYPE, content_type)],
        thumbnail_data,
    ))
}

/// Determine content type from file path and data
fn determine_content_type(path: &PathBuf, data: &[u8]) -> &'static str {
    // Try to determine from extension first
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        return match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "svg" => "image/svg+xml",
            _ => "application/octet-stream",
        };
    }

    // Fallback: detect from magic bytes
    if data.len() >= 4 {
        match &data[0..4] {
            [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
            [0x89, 0x50, 0x4E, 0x47] => "image/png",
            [0x47, 0x49, 0x46, ..] => "image/gif",
            [0x52, 0x49, 0x46, 0x46] => "image/webp",
            [0x42, 0x4D, ..] => "image/bmp",
            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}
