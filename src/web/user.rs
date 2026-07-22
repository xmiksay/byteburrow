use crate::auth::{Auth, AuthError};
use crate::entity::user;
use crate::web::{
    bad_request, conflict, forbidden, internal, message, not_found, require_admin, save_or_err,
    user_name_taken, ApiError, AppState, ErrorResponse, MessageResponse, Page, Pagination,
};
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use utoipa::ToSchema;

/// Login request payload
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
///
/// The session token itself is never returned in the body (issue #10) — it's
/// only ever carried in the HttpOnly `session_token` cookie set alongside
/// this response, so it stays out of reach of any XSS-injected JavaScript.
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub expires_in_days: i64,
}

/// User response (without password)
#[derive(Debug, Serialize, ToSchema)]
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
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    pub name: String,
    pub description: Option<String>,
    pub username: String,
    pub password: String,
    pub enabled: Option<bool>,
    pub admin: Option<bool>,
}

/// Update user request
#[derive(Debug, Deserialize, ToSchema)]
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
        .route("/logout", post(logout_handler))
        .route("/me", get(me_handler))
        .route("/", get(list_users_handler).post(create_user_handler))
        .route(
            "/:id",
            get(get_user_handler)
                .put(update_user_handler)
                .delete(delete_user_handler),
        )
        .route("/:id/password", post(change_password_handler))
}

/// Change password request
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub password: String,
}

/// Change password handler
/// POST /api/user/:id/password
#[utoipa::path(
    post,
    path = "/api/user/{id}/password",
    tag = "user",
    params(("id" = i32, Path, description = "User ID")),
    request_body = ChangePasswordRequest,
    responses(
        (status = 200, description = "Password changed successfully"),
        (status = 403, description = "Permission denied", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn change_password_handler(
    auth: Auth,
    Path(id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, AuthOrApiError> {
    // Check permission: user can change own password, admin can change any.
    if auth.user.id != id && !auth.user.admin {
        return Err(AuthOrApiError::Api(forbidden("Permission denied")));
    }

    // Find user
    let user = user::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("User", id))?;

    // Update password
    let mut active_user: user::ActiveModel = user.into();
    active_user.password = Set(Auth::hash_password(&payload.password)
        .map_err(|_| AuthOrApiError::Api(internal("Failed to hash password")))?);
    active_user.update(&state.db).await?;

    Ok(message("Password changed successfully"))
}

/// Login endpoint - accepts JSON payload with username/password
/// POST /api/user/login
#[utoipa::path(
    post,
    path = "/api/user/login",
    tag = "user",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ErrorResponse),
        (status = 403, description = "User disabled", body = ErrorResponse),
    )
)]
async fn login_handler(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<impl IntoResponse, AuthOrApiError> {
    // Extract user agent from headers
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let trust_forwarded_headers = crate::config::Config::get().trust_forwarded_headers;
    let ip_address = crate::auth::resolve_ip_address(&headers, Some(peer), trust_forwarded_headers);

    // Authenticate user
    let auth = Auth::from_user_password(&payload.username, &payload.password, &state.db)
        .await
        .map_err(AuthOrApiError::Auth)?;

    // Create token with user agent and IP address
    let config = crate::config::Config::get();
    let token = auth
        .create_token(&state.db, user_agent, ip_address)
        .await
        .map_err(|e| AuthOrApiError::Api(internal(format!("Failed to create token: {e:?}"))))?;

    // Build Set-Cookie header for HttpOnly session cookie. This is the only
    // place the raw token is ever sent to the client (issue #10) — it never
    // appears in the JSON body.
    let max_age = config.token_expiration_days * 24 * 60 * 60;
    let secure_flag = if config.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let cookie_value = format!(
        "session_token={}; HttpOnly; SameSite=Strict; Path=/api; Max-Age={}{}",
        token, max_age, secure_flag
    );

    Ok((
        [(header::SET_COOKIE, cookie_value)],
        Json(LoginResponse {
            expires_in_days: config.token_expiration_days,
        }),
    ))
}

/// Logout endpoint - revokes the current session token and clears the cookie
/// POST /api/user/logout
#[utoipa::path(
    post,
    path = "/api/user/logout",
    tag = "user",
    responses(
        (status = 200, description = "Logged out successfully"),
    )
)]
async fn logout_handler(
    mut parts: axum::http::request::Parts,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(token) = crate::auth::extract_token(&mut parts).await {
        // Best-effort: an already-expired/invalid token isn't an error here.
        let _ = Auth::revoke_token(&token, &state.db).await;
    }

    let cookie_value = "session_token=; HttpOnly; SameSite=Strict; Path=/api; Max-Age=0";

    (
        [(header::SET_COOKIE, cookie_value)],
        message("Logged out successfully"),
    )
}

/// Current-user summary returned by `GET /api/user/me`.
#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub id: i32,
    pub username: String,
    pub name: String,
    pub admin: bool,
}

/// Me endpoint - returns current user info
/// GET /api/user/me
#[utoipa::path(
    get,
    path = "/api/user/me",
    tag = "user",
    responses(
        (status = 200, description = "Current user info", body = MeResponse),
    ),
    security(("bearer" = []))
)]
async fn me_handler(auth: Auth) -> Json<MeResponse> {
    Json(MeResponse {
        id: auth.user.id,
        username: auth.user.username,
        name: auth.user.name,
        admin: auth.user.admin,
    })
}

// ============================================================================
// CRUD Operations (Admin Only)
// ============================================================================

/// List all users
/// GET /api/user
#[utoipa::path(
    get,
    path = "/api/user",
    tag = "user",
    params(Pagination),
    responses(
        (status = 200, description = "Paginated list of users", body = Page<UserResponse>),
        (status = 403, description = "Admin access required", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn list_users_handler(
    auth: Auth,
    Query(pagination): Query<Pagination>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Page<UserResponse>>, ApiError> {
    require_admin(&auth)?;

    let paginator = user::Entity::find().paginate(&state.db, pagination.per_page());
    let total = paginator.num_items().await?;
    let users = paginator.fetch_page(pagination.page_index()).await?;
    let items = users.into_iter().map(UserResponse::from).collect();

    Ok(Json(Page::new(items, total, &pagination)))
}

/// Get user by ID
/// GET /api/user/:id
#[utoipa::path(
    get,
    path = "/api/user/{id}",
    tag = "user",
    params(("id" = i32, Path, description = "User ID")),
    responses(
        (status = 200, description = "User found", body = UserResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn get_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<UserResponse>, ApiError> {
    require_admin(&auth)?;

    let user = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("User", user_id))?;

    Ok(Json(UserResponse::from(user)))
}

/// Create new user
/// POST /api/user
#[utoipa::path(
    post,
    path = "/api/user",
    tag = "user",
    request_body = CreateUserRequest,
    responses(
        (status = 200, description = "User created", body = UserResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 409, description = "Username already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn create_user_handler(
    auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    require_admin(&auth)?;

    if user_name_taken(&state.db, &payload.username).await? {
        return Err(conflict(format!(
            "Username '{}' already exists",
            payload.username
        )));
    }

    // Hash password
    let password_hash =
        Auth::hash_password(&payload.password).map_err(|_| internal("Failed to hash password"))?;

    let new_user = user::ActiveModel {
        name: Set(payload.name),
        description: Set(payload.description),
        username: Set(payload.username),
        password: Set(password_hash),
        enabled: Set(payload.enabled.unwrap_or(true)),
        admin: Set(payload.admin.unwrap_or(false)),
        ..Default::default()
    };

    let created_user = new_user.insert(&state.db).await?;

    Ok(Json(UserResponse::from(created_user)))
}

/// Update user
/// PUT /api/user/:id
#[utoipa::path(
    put,
    path = "/api/user/{id}",
    tag = "user",
    params(("id" = i32, Path, description = "User ID")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "User updated", body = UserResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse),
        (status = 409, description = "Username already exists", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn update_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    require_admin(&auth)?;

    let user = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("User", user_id))?;

    if let Some(ref new_username) = payload.username {
        if new_username != &user.username && user_name_taken(&state.db, new_username).await? {
            return Err(conflict(format!(
                "Username '{}' already exists",
                new_username
            )));
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
        active_user.password =
            Set(Auth::hash_password(&password).map_err(|_| internal("Failed to hash password"))?);
    }
    if let Some(enabled) = payload.enabled {
        active_user.enabled = Set(enabled);
    }
    if let Some(admin) = payload.admin {
        active_user.admin = Set(admin);
    }

    let updated_user = save_or_err(active_user, &state.db).await?;

    Ok(Json(UserResponse::from(updated_user)))
}

/// Delete user
/// DELETE /api/user/:id
#[utoipa::path(
    delete,
    path = "/api/user/{id}",
    tag = "user",
    params(("id" = i32, Path, description = "User ID")),
    responses(
        (status = 200, description = "User deleted"),
        (status = 400, description = "Cannot delete own account", body = ErrorResponse),
        (status = 403, description = "Admin access required", body = ErrorResponse),
        (status = 404, description = "User not found", body = ErrorResponse),
    ),
    security(("bearer" = []))
)]
async fn delete_user_handler(
    auth: Auth,
    Path(user_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<MessageResponse>, ApiError> {
    require_admin(&auth)?;

    // Prevent deleting yourself
    if auth.user.id == user_id {
        return Err(bad_request("Cannot delete your own account"));
    }

    let user = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| not_found("User", user_id))?;

    user::Entity::delete_by_id(user_id).exec(&state.db).await?;

    Ok(message(format!(
        "User '{}' deleted successfully",
        user.username
    )))
}

// ---------------------------------------------------------------------------
// Local error wrapper: handlers that authenticate via username/password must
// return `AuthError` directly (it carries the WWW-Authenticate header on 401),
// while everything else uses `ApiError`. This enum picks either.
// ---------------------------------------------------------------------------

enum AuthOrApiError {
    Auth(AuthError),
    Api(ApiError),
}

impl IntoResponse for AuthOrApiError {
    fn into_response(self) -> axum::response::Response {
        match self {
            AuthOrApiError::Auth(e) => e.into_response(),
            AuthOrApiError::Api(e) => e.into_response(),
        }
    }
}

impl From<sea_orm::DbErr> for AuthOrApiError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Api(ApiError::Db(e))
    }
}

impl From<ApiError> for AuthOrApiError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
