use crate::entity::{group, token, user};
use axum::{
    async_trait,
    extract::{ConnectInfo, FromRequestParts},
    http::{request::Parts, HeaderMap, StatusCode},
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
use std::net::SocketAddr;
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
    /// Hashing a password with Argon2id failed.
    PasswordHashFailed,
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
            AuthError::PasswordHashFailed => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to hash password")
            }
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

        let peer = parts
            .extract::<ConnectInfo<SocketAddr>>()
            .await
            .ok()
            .map(|ConnectInfo(addr)| addr);
        let trust_forwarded_headers = crate::config::Config::get().trust_forwarded_headers;
        let ip_address = resolve_ip_address(&parts.headers, peer, trust_forwarded_headers);

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
pub(crate) async fn extract_token(parts: &mut Parts) -> Option<String> {
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

/// Resolve the client IP to record for logging/auditing purposes.
///
/// `X-Forwarded-For` / `X-Real-IP` are only trusted when
/// `trust_forwarded_headers` (config-driven at the call site) is `true` —
/// i.e. a reverse proxy is known to set them itself — otherwise any client
/// could spoof the IP recorded against their own session by sending these
/// headers directly. Falls back to the real TCP peer address from
/// `ConnectInfo`.
pub(crate) fn resolve_ip_address(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trust_forwarded_headers: bool,
) -> Option<String> {
    if trust_forwarded_headers {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
            .map(|s| s.trim().to_string());
        if forwarded.is_some() {
            return forwarded;
        }
    }

    peer.map(|addr| addr.ip().to_string())
}

// ============================================================================
// Auth Core
// ============================================================================

pub struct Auth {
    pub user: user::Model,
}

impl Auth {
    /// Hash a string using SHA256 with salt from config.
    ///
    /// Only appropriate for high-entropy random values (session tokens),
    /// where fast unsalted-per-value hashing is not a brute-force risk. Do
    /// not use for passwords — see [`Auth::hash_password`].
    pub fn hash_string(s: &str) -> String {
        let salt = crate::config::Config::get().salt.clone();
        let mut hasher = sha2::Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(s.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Hash a password with Argon2id and a fresh per-user random salt,
    /// returning a self-describing PHC string (algorithm + params + salt +
    /// hash all encoded together).
    pub fn hash_password(password: &str) -> Result<String, AuthError> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::Argon2;

        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AuthError::PasswordHashFailed)
    }

    /// Verify a password against a stored hash.
    ///
    /// Accepts both current Argon2id PHC hashes (`$argon2id$...`) and
    /// legacy SHA-256 + global-salt hashes (plain 64-char hex, no `$`
    /// prefix) so existing accounts keep working — see
    /// [`Auth::from_user_password`] for the transparent rehash-on-login
    /// migration path (issue #8).
    fn verify_password(password: &str, stored: &str) -> bool {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        use argon2::Argon2;

        if let Ok(parsed) = PasswordHash::new(stored) {
            return Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok();
        }

        stored == Self::hash_string(password)
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

        if !Self::verify_password(password, &user.password) {
            return Err(AuthError::InvalidCredentials);
        }

        if !user.enabled {
            return Err(AuthError::UserDisabled);
        }

        let user = Self::rehash_if_legacy(user, password, db).await;

        Ok(Self::new(user))
    }

    /// Transparently upgrade a legacy SHA-256 password hash to Argon2id
    /// after a successful login (issue #8 migration path). Best-effort: a
    /// failure here must not fail an otherwise-valid login.
    async fn rehash_if_legacy<C: ConnectionTrait>(
        user: user::Model,
        password: &str,
        db: &C,
    ) -> user::Model {
        if argon2::password_hash::PasswordHash::new(&user.password).is_ok() {
            return user;
        }

        let Ok(rehashed) = Self::hash_password(password) else {
            return user;
        };

        let user_id = user.id;
        let fallback = user.clone();
        let mut active: user::ActiveModel = user.into();
        active.password = Set(rehashed);
        match active.update(db).await {
            Ok(updated) => updated,
            Err(err) => {
                tracing::warn!("failed to rehash legacy password for user {user_id}: {err}");
                fallback
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use std::sync::{Arc, Once};

    /// `Config` is a process-wide `OnceLock` — initialize it once for all
    /// tests in this binary that need `Auth::hash_string`.
    fn init_test_config() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            crate::config::Config::set(Arc::new(crate::config::Config {
                database_url: "postgres://unused".to_string(),
                salt: "test-salt".to_string(),
                server_addr: "0.0.0.0:3000".to_string(),
                thumbnail_storage: "/tmp/thumbnails".to_string(),
                base_url: "http://localhost:3000".to_string(),
                token_expiration_days: 30,
                token_length: 32,
                plugin_dir: "/etc/byteburrow/plugins".to_string(),
                ignore_patterns: vec![],
                cors_allowed_origins: String::new(),
                trust_forwarded_headers: false,
                face_match_threshold: 0.8,
                face_match_margin: 0.05,
            }));
        });
    }

    #[test]
    fn hash_string_is_deterministic() {
        init_test_config();
        assert_eq!(Auth::hash_string("hello"), Auth::hash_string("hello"));
    }

    #[test]
    fn hash_string_differs_for_different_input() {
        init_test_config();
        assert_ne!(Auth::hash_string("hello"), Auth::hash_string("world"));
    }

    #[test]
    fn hash_string_is_not_the_plain_input() {
        init_test_config();
        assert_ne!(Auth::hash_string("password123"), "password123");
    }

    #[test]
    fn hash_password_produces_argon2id_phc_string() {
        let hash = Auth::hash_password("hunter2").expect("hashing should succeed");
        assert!(hash.starts_with("$argon2id$"));
    }

    #[test]
    fn hash_password_uses_a_fresh_salt_each_time() {
        let a = Auth::hash_password("hunter2").expect("hashing should succeed");
        let b = Auth::hash_password("hunter2").expect("hashing should succeed");
        assert_ne!(a, b);
    }

    #[test]
    fn verify_password_accepts_correct_argon2_hash() {
        let hash = Auth::hash_password("hunter2").expect("hashing should succeed");
        assert!(Auth::verify_password("hunter2", &hash));
    }

    #[test]
    fn verify_password_rejects_wrong_password_against_argon2_hash() {
        let hash = Auth::hash_password("hunter2").expect("hashing should succeed");
        assert!(!Auth::verify_password("wrong-password", &hash));
    }

    #[test]
    fn verify_password_accepts_legacy_sha256_hash() {
        init_test_config();
        let legacy = Auth::hash_string("hunter2");
        assert!(Auth::verify_password("hunter2", &legacy));
    }

    #[test]
    fn verify_password_rejects_wrong_password_against_legacy_hash() {
        init_test_config();
        let legacy = Auth::hash_string("hunter2");
        assert!(!Auth::verify_password("wrong-password", &legacy));
    }

    async fn parts_for_uri(uri: &str) -> Parts {
        Request::builder().uri(uri).body(()).unwrap().into_parts().0
    }

    #[tokio::test]
    async fn extract_token_from_bearer_header() {
        let mut parts = parts_for_uri("/anything").await;
        parts.headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer my-bearer-token".parse().unwrap(),
        );

        assert_eq!(
            extract_token(&mut parts).await,
            Some("my-bearer-token".to_string())
        );
    }

    #[tokio::test]
    async fn extract_token_from_query_param() {
        let mut parts = parts_for_uri("/files?foo=bar&token=abc123&baz=qux").await;

        assert_eq!(extract_token(&mut parts).await, Some("abc123".to_string()));
    }

    #[tokio::test]
    async fn extract_token_from_cookie() {
        let mut parts = parts_for_uri("/anything").await;
        parts.headers.insert(
            axum::http::header::COOKIE,
            "other=1; session_token=cookie-token; more=2"
                .parse()
                .unwrap(),
        );

        assert_eq!(
            extract_token(&mut parts).await,
            Some("cookie-token".to_string())
        );
    }

    #[tokio::test]
    async fn extract_token_prefers_bearer_over_query_and_cookie() {
        let mut parts = parts_for_uri("/anything?token=query-token").await;
        parts.headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer bearer-token".parse().unwrap(),
        );
        parts.headers.insert(
            axum::http::header::COOKIE,
            "session_token=cookie-token".parse().unwrap(),
        );

        assert_eq!(
            extract_token(&mut parts).await,
            Some("bearer-token".to_string())
        );
    }

    #[tokio::test]
    async fn extract_token_prefers_query_over_cookie() {
        let mut parts = parts_for_uri("/anything?token=query-token").await;
        parts.headers.insert(
            axum::http::header::COOKIE,
            "session_token=cookie-token".parse().unwrap(),
        );

        assert_eq!(
            extract_token(&mut parts).await,
            Some("query-token".to_string())
        );
    }

    #[tokio::test]
    async fn extract_token_returns_none_when_absent() {
        let mut parts = parts_for_uri("/anything").await;

        assert_eq!(extract_token(&mut parts).await, None);
    }

    fn headers_with_forwarded_for(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    fn peer_addr() -> SocketAddr {
        "203.0.113.7:12345".parse().unwrap()
    }

    #[test]
    fn resolve_ip_ignores_spoofed_forwarded_header_when_untrusted() {
        let headers = headers_with_forwarded_for("6.6.6.6");

        // Untrusted by default: any client-supplied X-Forwarded-For must be
        // ignored in favor of the real TCP peer address.
        assert_eq!(
            resolve_ip_address(&headers, Some(peer_addr()), false),
            Some("203.0.113.7".to_string())
        );
    }

    #[test]
    fn resolve_ip_uses_forwarded_header_when_trusted() {
        let headers = headers_with_forwarded_for("198.51.100.1, 10.0.0.1");

        assert_eq!(
            resolve_ip_address(&headers, Some(peer_addr()), true),
            Some("198.51.100.1".to_string())
        );
    }

    #[test]
    fn resolve_ip_falls_back_to_peer_when_trusted_but_header_absent() {
        let headers = HeaderMap::new();

        assert_eq!(
            resolve_ip_address(&headers, Some(peer_addr()), true),
            Some("203.0.113.7".to_string())
        );
    }

    #[test]
    fn resolve_ip_returns_none_when_no_peer_and_untrusted() {
        let headers = headers_with_forwarded_for("6.6.6.6");

        assert_eq!(resolve_ip_address(&headers, None, false), None);
    }
}
