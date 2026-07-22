use crate::auth::Auth;
use crate::entity::tag;
use crate::web::{
    conflict, message, not_found, require_admin, save_or_err, tag_name_taken, ApiError, AppState,
    ErrorResponse, MessageResponse, Page, Pagination,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tag response
#[derive(Debug, Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateTagRequest {
    pub name: String,
    pub description: Option<String>,
}

/// Update tag request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
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
#[utoipa::path(
    get,
    path = "/api/tag",
    tag = "tag",
    params(Pagination),
    responses(
        (status = 200, description = "Paginated list of tags", body = Page<TagResponse>),
    )
)]
async fn list_tags_handler(
    _auth: Option<Auth>,
    Query(pagination): Query<Pagination>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Page<TagResponse>>, ApiError> {
    let paginator = tag::Entity::find().paginate(&state.db, pagination.per_page());
    let total = paginator.num_items().await?;
    let tags = paginator.fetch_page(pagination.page_index()).await?;
    let items = tags.into_iter().map(TagResponse::from).collect();

    Ok(Json(Page::new(items, total, &pagination)))
}

/// Get tag by ID (only if it belongs to the user)
/// GET /api/tag/:id
#[utoipa::path(
    get,
    path = "/api/tag/{id}",
    tag = "tag",
    params(("id" = i32, Path, description = "Tag ID")),
    responses(
        (status = 200, description = "Tag found", body = TagResponse),
        (status = 404, description = "Tag not found", body = ErrorResponse),
    )
)]
async fn get_tag_handler(
    _auth: Option<Auth>,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<TagResponse>, ApiError> {
    let tag = tag::Entity::find_by_id(tag_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Tag", tag_id))?;

    Ok(Json(TagResponse::from(tag)))
}

/// Create new tag
/// POST /api/tag
#[utoipa::path(
    post,
    path = "/api/tag",
    tag = "tag",
    request_body = CreateTagRequest,
    responses(
        (status = 200, description = "Tag created", body = TagResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 409, description = "Tag already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn create_tag_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateTagRequest>,
) -> Result<Json<TagResponse>, ApiError> {
    require_admin(&auth)?;

    if tag_name_taken(&state.db, &payload.name).await? {
        return Err(conflict(format!(
            "Tag with name '{}' already exists",
            payload.name
        )));
    }

    let new_tag = tag::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        ..Default::default()
    };

    let created_tag = new_tag.insert(&state.db).await?;

    Ok(Json(TagResponse::from(created_tag)))
}

/// Update tag
/// PUT /api/tag/:id
#[utoipa::path(
    put,
    path = "/api/tag/{id}",
    tag = "tag",
    params(("id" = i32, Path, description = "Tag ID")),
    request_body = UpdateTagRequest,
    responses(
        (status = 200, description = "Tag updated", body = TagResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Tag not found", body = ErrorResponse),
        (status = 409, description = "Tag name already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn update_tag_handler(
    auth: Auth,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateTagRequest>,
) -> Result<Json<TagResponse>, ApiError> {
    require_admin(&auth)?;

    let tag = tag::Entity::find_by_id(tag_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Tag", tag_id))?;

    if let Some(ref new_name) = payload.name {
        if new_name != &tag.name && tag_name_taken(&state.db, new_name).await? {
            return Err(conflict(format!(
                "Tag with name '{}' already exists",
                new_name
            )));
        }
    }

    let mut active_tag: tag::ActiveModel = tag.into();

    if let Some(name) = payload.name {
        active_tag.name = Set(name);
    }
    if let Some(description) = payload.description {
        active_tag.description = Set(Some(description));
    }

    let updated_tag = save_or_err(active_tag, &state.db).await?;

    Ok(Json(TagResponse::from(updated_tag)))
}

/// Delete tag
/// DELETE /api/tag/:id
#[utoipa::path(
    delete,
    path = "/api/tag/{id}",
    tag = "tag",
    params(("id" = i32, Path, description = "Tag ID")),
    responses(
        (status = 200, description = "Tag deleted"),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Tag not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn delete_tag_handler(
    auth: Auth,
    Path(tag_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    require_admin(&auth)?;

    let tag = tag::Entity::find_by_id(tag_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Tag", tag_id))?;

    tag::Entity::delete_by_id(tag_id).exec(&state.db).await?;

    Ok(message(format!("Tag '{}' deleted successfully", tag.name)))
}
