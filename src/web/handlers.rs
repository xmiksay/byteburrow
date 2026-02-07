use crate::web::AppState;
use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

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
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            )
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
    let thumbnail_data = fs::read(&thumbnail_path).await.map_err(|e| {
        tracing::error!(
            "Failed to read thumbnail {} for entry {}: {}",
            hash,
            entry_entity.path,
            e
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read thumbnail".to_string(),
        )
    })?;

    // Determine content type based on file extension or first bytes
    let content_type =
        crate::web::storage::determine_content_type(&thumbnail_path, &thumbnail_data);

    Ok(([(header::CONTENT_TYPE, content_type)], thumbnail_data))
}
