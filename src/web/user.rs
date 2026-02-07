use crate::auth::{Auth, AuthError};
use crate::entity::user;
use crate::web::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

/// User response (without password)
#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub username: String,
    pub enabled: bool,
    pub admin: bool,
}

impl From<user::Model> for UserResponse {
    fn from(user: user::Model) -> Self {
        Self {
            id: user.id,
            name: user.name,
            description: user.description,
            username: user.username,
            enabled: user.enabled,
            admin: user.admin,
        }
    }
}

/// Create user request
#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub description: Option<String>,
    pub username: String,
    pub password: String,
    pub enabled: Option<bool>,
    pub admin: Option<bool>,
}

/// Update user request
#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub enabled: Option<bool>,
    pub admin: Option<bool>,
}

/// Create user router with all user-related endpoints
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/login", post(login_handler))
        .route("/me", get(me_handler))
        .route("/", get(list_users_handler).post(create_user_handler))
        .route(
            "/:id",
            get(get_user_handler)
                .put(update_user_handler)
                .delete(delete_user_handler),
        )
}

/// Login endpoint - accepts JSON payload with username/password
/// POST /api/user/login
async fn login_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Extract user agent from headers
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract IP address from headers (try X-Forwarded-For first, then X-Real-IP)
    let ip_address = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .map(|s| s.trim().to_string());

    // Authenticate user
    let auth = Auth::from_user_password(&payload.username, &payload.password, &state.db)
        .await
        .map_err(|e| {
            let (status, message) = match e {
                AuthError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "Invalid credentials"),
                AuthError::UserDisabled => (StatusCode::FORBIDDEN, "User account is disabled"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "Authentication error"),
            };
            (
                status,
                Json(ErrorResponse {
                    error: message.to_string(),
                }),
            )
        })?;

    // Create token with user agent and IP address
    let config = crate::config::Config::get();
    let token = auth
        .create_token(&state.db, user_agent, ip_address)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to create token: {:?}", e),
                }),
            )
        })?;

    Ok(Json(LoginResponse {
        token,
        expires_in_days: config.token_expiration_days,
    }))
}

/// Me endpoint - returns current user info
/// GET /api/user/me
async fn me_handler(auth: Auth) -> impl IntoResponse {
    Json(serde_json::json!({
        "id": auth.user.id,
        "username": auth.user.username,
        "name": auth.user.name,
        "admin": auth.user.admin,
    }))
}

// ============================================================================
// CRUD Operations (Admin Only)
// ============================================================================

/// Helper function to check if user is admin
fn require_admin(auth: &Auth) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if !auth.user.admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Admin access required".to_string(),
            }),
        ));
    }
    Ok(())
}

/// List all users
/// GET /api/user
async fn list_users_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UserResponse>>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    let users = user::Entity::find()
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

    Ok(Json(users.into_iter().map(UserResponse::from).collect()))
}

/// Get user by ID
/// GET /api/user/:id
async fn get_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    let user = user::Entity::find_by_id(user_id)
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
                error: format!("User {} not found", user_id),
            }),
        ))?;

    Ok(Json(UserResponse::from(user)))
}

/// Create new user
/// POST /api/user
async fn create_user_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateUserRequest>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Check if username already exists
    let existing = user::Entity::find()
        .filter(user::Column::Username.eq(&payload.username))
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
                error: format!("Username '{}' already exists", payload.username),
            }),
        ));
    }

    // Hash password
    let password_hash = Auth::hash_string(&payload.password);

    let new_user = user::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        username: Set(payload.username),
        password: Set(password_hash),
        enabled: Set(payload.enabled.unwrap_or(true)),
        admin: Set(payload.admin.unwrap_or(false)),
        ..Default::default()
    };

    let created_user = new_user.insert(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create user".to_string(),
            }),
        )
    })?;

    Ok(Json(UserResponse::from(created_user)))
}

/// Update user
/// PUT /api/user/:id
async fn update_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Find the user
    let user = user::Entity::find_by_id(user_id)
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
                error: format!("User {} not found", user_id),
            }),
        ))?;

    // Check if username is being changed and if it conflicts
    if let Some(ref new_username) = payload.username {
        if new_username != &user.username {
            let existing = user::Entity::find()
                .filter(user::Column::Username.eq(new_username))
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
                        error: format!("Username '{}' already exists", new_username),
                    }),
                ));
            }
        }
    }

    let mut active_user: user::ActiveModel = user.into();

    if let Some(name) = payload.name {
        active_user.name = Set(name);
    }
    if let Some(description) = payload.description {
        active_user.description = Set(Some(description));
    }
    if let Some(username) = payload.username {
        active_user.username = Set(username);
    }
    if let Some(password) = payload.password {
        active_user.password = Set(Auth::hash_string(&password));
    }
    if let Some(enabled) = payload.enabled {
        active_user.enabled = Set(enabled);
    }
    if let Some(admin) = payload.admin {
        active_user.admin = Set(admin);
    }

    let updated_user = active_user.update(&state.db).await.map_err(|e| {
        tracing::error!("Database error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to update user".to_string(),
            }),
        )
    })?;

    Ok(Json(UserResponse::from(updated_user)))
}

/// Delete user
/// DELETE /api/user/:id
async fn delete_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    require_admin(&auth)?;

    // Prevent deleting yourself
    if auth.user.id == user_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Cannot delete your own account".to_string(),
            }),
        ));
    }

    // Check if user exists
    let user = user::Entity::find_by_id(user_id)
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
                error: format!("User {} not found", user_id),
            }),
        ))?;

    // Delete the user
    user::Entity::delete_by_id(user_id)
        .exec(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to delete user".to_string(),
                }),
            )
        })?;

    Ok(Json(serde_json::json!({
        "message": format!("User '{}' deleted successfully", user.username),
    })))
}
