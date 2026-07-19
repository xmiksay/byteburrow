use crate::auth::Auth;
use crate::entity::group;
use crate::web::{
    conflict, group_name_taken, not_found, require_admin, save_or_err, ApiError, AppState,
    ErrorResponse,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Group response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GroupResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
}

impl From<group::Model> for GroupResponse {
    fn from(group: group::Model) -> Self {
        Self {
            id: group.id,
            name: group.name,
            description: group.description,
        }
    }
}

/// Create group request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
}

/// Update group request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateGroupRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Create group router with all group-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_groups_handler).post(create_group_handler))
        .route(
            "/:id",
            get(get_group_handler)
                .put(update_group_handler)
                .delete(delete_group_handler),
        )
}

/// List all groups
/// GET /api/group
#[utoipa::path(
    get,
    path = "/api/group",
    tag = "group",
    responses(
        (status = 200, description = "List of all groups", body = Vec<GroupResponse>),
        (status = 403, description = "Admin access required", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn list_groups_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GroupResponse>>, ApiError> {
    require_admin(&auth)?;

    let groups = group::Entity::find().all(&state.db).await?;

    Ok(Json(groups.into_iter().map(GroupResponse::from).collect()))
}

/// Get group by ID
/// GET /api/group/:id
#[utoipa::path(
    get,
    path = "/api/group/{id}",
    tag = "group",
    params(("id" = i32, Path, description = "Group ID")),
    responses(
        (status = 200, description = "Group found", body = GroupResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Group not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn get_group_handler(
    auth: Auth,
    Path(group_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<GroupResponse>, ApiError> {
    require_admin(&auth)?;

    let group = group::Entity::find_by_id(group_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Group", group_id))?;

    Ok(Json(GroupResponse::from(group)))
}

/// Create new group
/// POST /api/group
#[utoipa::path(
    post,
    path = "/api/group",
    tag = "group",
    request_body = CreateGroupRequest,
    responses(
        (status = 200, description = "Group created", body = GroupResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 409, description = "Group already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn create_group_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateGroupRequest>,
) -> Result<Json<GroupResponse>, ApiError> {
    require_admin(&auth)?;

    if group_name_taken(&state.db, &payload.name).await? {
        return Err(conflict(format!(
            "Group with name '{}' already exists",
            payload.name
        )));
    }

    let new_group = group::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        ..Default::default()
    };

    let created_group = new_group.insert(&state.db).await?;

    Ok(Json(GroupResponse::from(created_group)))
}

/// Update group
/// PUT /api/group/:id
#[utoipa::path(
    put,
    path = "/api/group/{id}",
    tag = "group",
    params(("id" = i32, Path, description = "Group ID")),
    request_body = UpdateGroupRequest,
    responses(
        (status = 200, description = "Group updated", body = GroupResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Group not found", body = ErrorResponse),
        (status = 409, description = "Group name already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn update_group_handler(
    auth: Auth,
    Path(group_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateGroupRequest>,
) -> Result<Json<GroupResponse>, ApiError> {
    require_admin(&auth)?;

    let group = group::Entity::find_by_id(group_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Group", group_id))?;

    if let Some(ref new_name) = payload.name {
        if new_name != &group.name && group_name_taken(&state.db, new_name).await? {
            return Err(conflict(format!(
                "Group with name '{}' already exists",
                new_name
            )));
        }
    }

    let mut active_group: group::ActiveModel = group.into();

    if let Some(name) = payload.name {
        active_group.name = Set(name);
    }
    if let Some(description) = payload.description {
        active_group.description = Set(Some(description));
    }

    let updated_group = save_or_err(active_group, &state.db).await?;

    Ok(Json(GroupResponse::from(updated_group)))
}

/// Delete group
/// DELETE /api/group/:id
#[utoipa::path(
    delete,
    path = "/api/group/{id}",
    tag = "group",
    params(("id" = i32, Path, description = "Group ID")),
    responses(
        (status = 200, description = "Group deleted"),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "Group not found", body = ErrorResponse),
        (status = 409, description = "Cannot delete group with users", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn delete_group_handler(
    auth: Auth,
    Path(group_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let group = group::Entity::find_by_id(group_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("Group", group_id))?;

    group::Entity::delete_by_id(group_id)
        .exec(&state.db)
        .await?;

    Ok(Json(serde_json::json!({
        "message": format!("Group '{}' deleted successfully", group.name),
    })))
}
