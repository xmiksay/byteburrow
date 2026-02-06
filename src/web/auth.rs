use crate::entity::{token, user};
use crate::web::AppState;
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    RequestPartsExt,
};
use axum_extra::{
    headers::{authorization::Basic, authorization::Bearer, Authorization},
    TypedHeader,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Authenticated user extracted from the request
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: u32,
    pub username: String,
    pub name: String,
    pub admin: bool,
}

/// Authentication error
pub enum AuthError {
    InvalidCredentials,
    InvalidToken,
    TokenExpired,
    MissingCredentials,
    DatabaseError,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "Invalid credentials"),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid token"),
            AuthError::TokenExpired => (StatusCode::UNAUTHORIZED, "Token expired"),
            AuthError::MissingCredentials => {
                (StatusCode::UNAUTHORIZED, "Missing authentication credentials")
            }
            AuthError::DatabaseError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
        };

        (status, message).into_response()
    }
}

/// Hash password using SHA256 and salt
pub fn hash_password(password: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(password.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extractor for Basic Auth and Bearer Token
#[async_trait]
impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Try Bearer token first
        if let Ok(TypedHeader(Authorization(bearer))) =
            parts.extract::<TypedHeader<Authorization<Bearer>>>().await
        {
            return verify_bearer_token(bearer.token(), state).await;
        }

        // Try Basic Auth
        if let Ok(TypedHeader(Authorization(basic))) =
            parts.extract::<TypedHeader<Authorization<Basic>>>().await
        {
            return verify_basic_auth(&basic, state).await;
        }

        Err(AuthError::MissingCredentials)
    }
}

/// Verify Basic Auth credentials
async fn verify_basic_auth(
    basic: &Basic,
    state: &Arc<AppState>,
) -> Result<AuthUser, AuthError> {
    let username = basic.username();
    let password = basic.password();

    // Get salt from config
    let password_hash = hash_password(password, &state.config.salt);

    // Find user by username and password hash
    let user_record = user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .filter(user::Column::Password.eq(password_hash))
        .one(&state.db)
        .await
        .map_err(|_| AuthError::DatabaseError)?;

    match user_record {
        Some(user) => {
            // Check if user is enabled
            if !user.enabled {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(AuthUser {
                id: user.id,
                username: user.username,
                name: user.name,
                admin: user.admin,
            })
        }
        None => Err(AuthError::InvalidCredentials),
    }
}

/// Verify Bearer token
async fn verify_bearer_token(
    token_str: &str,
    state: &Arc<AppState>,
) -> Result<AuthUser, AuthError> {
    // Find token in database
    let token_record = token::Entity::find()
        .filter(token::Column::Nonce.eq(token_str))
        .one(&state.db)
        .await
        .map_err(|_| AuthError::DatabaseError)?;

    let token = token_record.ok_or(AuthError::InvalidToken)?;

    // Check if token is expired
    let now = chrono::Utc::now().naive_utc();
    if token.expires_at < now {
        return Err(AuthError::TokenExpired);
    }

    // Get user associated with token
    let user_record = user::Entity::find_by_id(token.user_id)
        .one(&state.db)
        .await
        .map_err(|_| AuthError::DatabaseError)?;

    match user_record {
        Some(user) => {
            // Check if user is enabled
            if !user.enabled {
                return Err(AuthError::InvalidToken);
            }
            Ok(AuthUser {
                id: user.id,
                username: user.username,
                name: user.name,
                admin: user.admin,
            })
        }
        None => Err(AuthError::InvalidToken),
    }
}

/// Create a new bearer token for a user
pub async fn create_token(
    user_id: u32,
    duration_days: i64,
    user_agent: Option<String>,
    ip_address: Option<String>,
    state: &Arc<AppState>,
) -> Result<String, AuthError> {
    use token::ActiveModel;
    use sea_orm::ActiveValue::Set;

    // Generate a secure random token
    let nonce = uuid::Uuid::new_v4().to_string();

    // Calculate expiration
    let now = chrono::Utc::now().naive_utc();
    let expires_at = now + chrono::Duration::days(duration_days);

    // Create token in database
    let new_token = ActiveModel {
        user_id: Set(user_id),
        nonce: Set(nonce.clone()),
        expires_at: Set(expires_at),
        user_agent: Set(user_agent),
        ip_address: Set(ip_address),
        last_activity: Set(Some(now)),
        ..Default::default()
    };

    new_token
        .insert(&state.db)
        .await
        .map_err(|_| AuthError::DatabaseError)?;

    Ok(nonce)
}

/// Update token last activity timestamp
pub async fn update_token_activity(
    token_str: &str,
    state: &Arc<AppState>,
) -> Result<(), AuthError> {
    use sea_orm::ActiveValue::Set;

    let token_record = token::Entity::find()
        .filter(token::Column::Nonce.eq(token_str))
        .one(&state.db)
        .await
        .map_err(|_| AuthError::DatabaseError)?;

    if let Some(token) = token_record {
        let mut active_token: token::ActiveModel = token.into();
        active_token.last_activity = Set(Some(chrono::Utc::now().naive_utc()));

        active_token
            .update(&state.db)
            .await
            .map_err(|_| AuthError::DatabaseError)?;
    }

    Ok(())
}
