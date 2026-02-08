use crate::auth::Auth;
use crate::entity::tag;
use crate::web::{require_admin, AppState, ErrorResponse};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tag response
#[derive(Debug, Serialize)]
pub struct TagResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
}

impl From<tag::Model> for TagResponse {
    fn from(tag: tag::Model) -> Self {
        Self {
            id: tag.id,
            name: tag.name,
            description: tag.description,
        }
    }
}

/// Create tag request
#[derive(Debug, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    pub description: Option<String>,
}

/// Update tag request
#[derive(Debug, Deserialize)]
pub struct UpdateTagRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Create tag router with all tag-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_tags_handler).post(create_tag_handler))
        .route(
            "/:id",
            get(get_tag_handler)
                .put(update_tag_handler)
                .delete(delete_tag_handler),
        )
}

/// List all tags for the current user
/// GET /api/tag
async fn list_tags_handler(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TagResponse>>, (StatusCode, Json<ErrorResponse>)> {
    let tags = tag::Entity::find()
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

    Ok(Json(tags.into_iter().map(TagResponse::from).collect()))
}

/// Get tag by ID (only if it belongs to the user)
/// GET /api/tag/:id
async fn get_tag_handler(
    _auth: Auth,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<TagResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tag = tag::Entity::find_by_id(tag_id)
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
                error: format!("Tag {} not found", tag_id),
            }),
        ))?;

    Ok(Json(TagResponse::from(tag)))
}

/// Create new tag
/// POST /api/tag
async fn create_tag_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateTagRequest>,
) -> Result<Json<TagResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Check if tag with same name already exists
    let existing = tag::Entity::find()
        .filter(tag::Column::Name.eq(&payload.name))
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
                error: format!("Tag with name '{}' already exists", payload.name),
            }),
        ));
    }

    let new_tag = tag::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        ..Default::default()
    };

    let created_tag = new_tag.insert(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create tag".to_string(),
            }),
        )
    })?;

    Ok(Json(TagResponse::from(created_tag)))
}

/// Update tag
/// PUT /api/tag/:id
async fn update_tag_handler(
    auth: Auth,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateTagRequest>,
) -> Result<Json<TagResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Find the tag
    let tag = tag::Entity::find_by_id(tag_id)
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
                error: format!("Tag {} not found", tag_id),
            }),
        ))?;

    // Check if name is being changed and if it conflicts
    if let Some(ref new_name) = payload.name {
        if new_name != &tag.name {
            let existing = tag::Entity::find()
                .filter(tag::Column::Name.eq(new_name))
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
                        error: format!("Tag with name '{}' already exists", new_name),
                    }),
                ));
            }
        }
    }

    let mut active_tag: tag::ActiveModel = tag.into();

    if let Some(name) = payload.name {
        active_tag.name = Set(name);
    }
    if let Some(description) = payload.description {
        active_tag.description = Set(Some(description));
    }

    let updated_tag = active_tag.update(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to update tag".to_string(),
            }),
        )
    })?;

    Ok(Json(TagResponse::from(updated_tag)))
}

/// Delete tag
/// DELETE /api/tag/:id
async fn delete_tag_handler(
    auth: Auth,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Check if tag exists
    let tag = tag::Entity::find_by_id(tag_id)
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
                error: format!("Tag {} not found", tag_id),
            }),
        ))?;

    // Delete the tag
    tag::Entity::delete_by_id(tag_id)
        .exec(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to delete tag".to_string(),
                }),
            )
        })?;

    Ok(Json(serde_json::json!({
        "message": format!("Tag '{}' deleted successfully", tag.name),
    })))
}
