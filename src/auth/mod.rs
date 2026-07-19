use crate::entity::{group, token, user};
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
use chrono::{Duration, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use sha2::Digest;
use std::sync::Arc;

use crate::web::AppState;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug)]
pub enum AuthError {
    InvalidToken,
    TokenExpired,
    InvalidCredentials,
    MissingCredentials,
    UserDisabled,
    /// Generating a fresh random token failed (RNG unavailable).
    TokenGenerationFailed,
    DbError(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for AuthError {
    fn from(err: sea_orm::DbErr) -> Self {
        AuthError::DbError(err)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "Invalid credentials"),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid token"),
            AuthError::TokenExpired => (StatusCode::UNAUTHORIZED, "Token expired"),
            AuthError::MissingCredentials => (
                StatusCode::UNAUTHORIZED,
                "Missing authentication credentials",
            ),
            AuthError::UserDisabled => (StatusCode::FORBIDDEN, "User account is disabled"),
            AuthError::TokenGenerationFailed => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate token",
            ),
            AuthError::DbError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database error"),
        };

        if status == StatusCode::UNAUTHORIZED {
            (
                status,
                [(
                    axum::http::header::WWW_AUTHENTICATE,
                    "Basic realm=\"Cloud\"",
                )],
                message,
            )
                .into_response()
        } else {
            (status, message).into_response()
        }
    }
}

/// Extractor for Basic Auth and Bearer Token
#[async_trait]
impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Extract user agent from headers
        let user_agent = parts
            .headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Extract IP address from headers (try X-Forwarded-For first, then X-Real-IP)
        let ip_address = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .or_else(|| parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
            .map(|s| s.trim().to_string());

        // KISS-7: Basic auth needs the DB (verifies a password), so check it
        // first as a special case. Bearer / query-param / cookie are unified
        // into [`extract_token`].
        if let Ok(TypedHeader(Authorization(basic))) =
            parts.extract::<TypedHeader<Authorization<Basic>>>().await
        {
            return Auth::from_basic_auth(&basic, &state.db).await;
        }

        let token = extract_token(parts)
            .await
            .ok_or(AuthError::MissingCredentials)?;
        Self::from_token(&token, &state.db, user_agent, ip_address).await
    }
}

/// Try the non-DB credential sources in priority order and return the first
/// token string found, or `None` when no credentials are present.
///
/// Sources checked (in order):
///   1. `Authorization: Bearer <token>` header
///   2. `?token=` query parameter
///   3. `session_token=<...>` cookie
///
/// (Basic auth is handled separately in [`Auth::from_request_parts`] because it
/// requires database access to verify the password.)
async fn extract_token(parts: &mut Parts) -> Option<String> {
    // 1. Bearer token in Authorization header.
    if let Ok(TypedHeader(Authorization(bearer))) =
        parts.extract::<TypedHeader<Authorization<Bearer>>>().await
    {
        return Some(bearer.token().to_string());
    }

    // 2. Query parameter ?token=...
    if let Some(query) = parts.uri.query() {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "token" {
                    return Some(value.to_string());
                }
            }
        }
    }

    // 3. Cookie: session_token=...
    if let Some(cookie_header) = parts.headers.get(axum::http::header::COOKIE) {
        if let Ok(cookies) = cookie_header.to_str() {
            for cookie in cookies.split(';') {
                let cookie = cookie.trim();
                if let Some(token_value) = cookie.strip_prefix("session_token=") {
                    return Some(token_value.trim().to_string());
                }
            }
        }
    }

    None
}

// ============================================================================
// Auth Core
// ============================================================================

pub struct Auth {
    pub user: user::Model,
}

impl Auth {
    /// Hash a string using SHA256 with salt from config
    pub fn hash_string(s: &str) -> String {
        let salt = crate::config::Config::get().salt.clone();
        let mut hasher = sha2::Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(s.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Create Auth from a user model
    pub fn new(user: user::Model) -> Self {
        Self { user }
    }

    // ------------------------------------------------------------------------
    // Authentication Methods
    // ------------------------------------------------------------------------

    /// Authenticate using basic auth credentials
    pub async fn from_basic_auth<C: ConnectionTrait>(
        basic: &Basic,
        db: &C,
    ) -> Result<Self, AuthError> {
        let username = basic.username();
        let password = basic.password();

        Self::from_user_password(username, password, db).await
    }

    /// Authenticate using raw token string
    pub async fn from_token<C: ConnectionTrait>(
        token: &str,
        db: &C,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<Self, AuthError> {
        let token_hash = Self::hash_string(token);

        let token = token::Entity::find_by_id(token_hash)
            .one(db)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        // Check if token is expired
        if token.expires_at < Utc::now() {
            return Err(AuthError::TokenExpired);
        }

        let user = user::Entity::find_by_id(token.user_id)
            .one(db)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        if !user.enabled {
            return Err(AuthError::UserDisabled);
        }

        // Update last activity timestamp, user agent, and IP address
        let mut active_token: token::ActiveModel = token.into();
        active_token.last_activity = Set(Some(Utc::now()));
        active_token.user_agent = Set(user_agent);
        active_token.ip_address = Set(ip_address);
        active_token.update(db).await?;

        Ok(Self::new(user))
    }

    /// Authenticate using username and password
    pub async fn from_user_password<C: ConnectionTrait>(
        username: &str,
        password: &str,
        db: &C,
    ) -> Result<Self, AuthError> {
        let user = user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;

        if user.password != Self::hash_string(password) {
            return Err(AuthError::InvalidCredentials);
        }

        if !user.enabled {
            return Err(AuthError::UserDisabled);
        }

        Ok(Self::new(user))
    }

    // ------------------------------------------------------------------------
    // Token Management
    // ------------------------------------------------------------------------

    /// Create a new authentication token
    pub async fn create_token<C: ConnectionTrait>(
        &self,
        db: &C,
        user_agent: Option<String>,
        ip_address: Option<String>,
    ) -> Result<String, AuthError> {
        let config = crate::config::Config::get();

        // KISS-7: generate `token_length` random bytes and hex-encode them,
        // giving a token of length `2 * token_length`. We previously
        // concatenated N UUIDs and truncated — a single `getrandom` fill is
        // both simpler and cryptographically sound.
        //
        // `token_length` is interpreted as the *raw* byte count to match the
        // historical effective entropy (the UUID loop produced ~32 hex chars
        // per UUID and was truncated to `token_length`, so the final token had
        // `token_length` hex chars ≈ `token_length/2` bytes of entropy). To
        // keep the generated token length equal to the configured
        // `token_length`, we produce `ceil(token_length / 2)` random bytes.
        let byte_count = config.token_length.div_ceil(2);
        let mut buf = vec![0u8; byte_count];
        // getrandom::fill writes random bytes into the buffer or returns an
        // error; on supported platforms (Linux getrandom, /dev/urandom, etc.)
        // it cannot fail in practice for non-huge sizes.
        getrandom::fill(&mut buf).map_err(|_| AuthError::TokenGenerationFailed)?;
        let raw_token = hex::encode(&buf);

        let now = Utc::now();

        let token = token::ActiveModel {
            nonce: Set(Self::hash_string(&raw_token)),
            user_id: Set(self.user.id),
            expires_at: Set(now + Duration::days(config.token_expiration_days)),
            user_agent: Set(user_agent),
            ip_address: Set(ip_address),
            last_activity: Set(Some(now)),
            created_at: Set(now),
        };

        token.insert(db).await?;

        Ok(raw_token)
    }

    /// Update the last activity timestamp for a token
    pub async fn update_token_activity<C: ConnectionTrait>(
        token_str: &str,
        db: &C,
    ) -> Result<(), AuthError> {
        let token_hash = Self::hash_string(token_str);

        let token_record = token::Entity::find_by_id(token_hash).one(db).await?;

        if let Some(token) = token_record {
            let mut active_token: token::ActiveModel = token.into();
            active_token.last_activity = Set(Some(Utc::now()));
            active_token.update(db).await?;
        }

        Ok(())
    }

    /// Revoke (delete) a token
    pub async fn revoke_token<C: ConnectionTrait>(
        token_str: &str,
        db: &C,
    ) -> Result<(), AuthError> {
        let token_hash = Self::hash_string(token_str);
        token::Entity::delete_by_id(token_hash).exec(db).await?;
        Ok(())
    }

    /// Revoke all tokens for the current user
    pub async fn revoke_all_tokens<C: ConnectionTrait>(&self, db: &C) -> Result<(), AuthError> {
        token::Entity::delete_many()
            .filter(token::Column::UserId.eq(self.user.id))
            .exec(db)
            .await?;
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a user exists by ID
pub async fn user_exists<C: ConnectionTrait>(user_id: i32, db: &C) -> Result<bool, sea_orm::DbErr> {
    let user = user::Entity::find_by_id(user_id).one(db).await?;
    Ok(user.is_some())
}

/// Check if a group exists by ID
pub async fn group_exists<C: ConnectionTrait>(
    group_id: i32,
    db: &C,
) -> Result<bool, sea_orm::DbErr> {
    let group = group::Entity::find_by_id(group_id).one(db).await?;
    Ok(group.is_some())
}
