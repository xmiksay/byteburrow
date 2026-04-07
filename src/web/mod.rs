pub mod group;
pub mod photo;
pub mod storage;
pub mod tag;
pub mod user;
pub mod ws;

use crate::auth::Auth;
use crate::config::Config;
use crate::job::JobSender;
use crate::storage::format_size;
use axum::{
    extract::State,
    http::{header, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use minijinja::Environment;
use rust_embed::Embed;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde::Serialize;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

async fn frontend_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first, then fall back to index.html (SPA routing)
    let file = if path.is_empty() {
        FrontendAssets::get("index.html")
    } else {
        FrontendAssets::get(path).or_else(|| FrontendAssets::get("index.html"))
    };

    match file {
        Some(content) => {
            let mime = mime_guess::from_path(if FrontendAssets::get(path).is_some() {
                path
            } else {
                "index.html"
            })
            .first_or_octet_stream();

            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// Error response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

/// Helper function to check if user is admin
pub fn require_admin(auth: &Auth) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
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

/// Helper function to verify a user exists by ID
pub async fn require_user_exists(
    user_id: i32,
    db: &DatabaseConnection,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let exists = crate::auth::user_exists(user_id, db).await.map_err(|e| {
        tracing::error!("Database error checking user existence: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
            }),
        )
    })?;

    if !exists {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("User with id {} not found", user_id),
            }),
        ));
    }

    Ok(())
}

/// Helper function to verify a group exists by ID
pub async fn require_group_exists(
    group_id: i32,
    db: &DatabaseConnection,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let exists = crate::auth::group_exists(group_id, db).await.map_err(|e| {
        tracing::error!("Database error checking group existence: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
            }),
        )
    })?;

    if !exists {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Group with id {} not found", group_id),
            }),
        ));
    }

    Ok(())
}

pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Config,
    pub jinja: Environment<'static>,
    pub job_sender: JobSender,
}

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ByteBurrow API",
        version = "1.0.0",
        description = "REST API for ByteBurrow personal cloud storage",
    ),
    paths(
        // User endpoints
        user::login_handler,
        user::me_handler,
        user::list_users_handler,
        user::get_user_handler,
        user::create_user_handler,
        user::update_user_handler,
        user::delete_user_handler,
        user::change_password_handler,
        // Group endpoints
        group::list_groups_handler,
        group::get_group_handler,
        group::create_group_handler,
        group::update_group_handler,
        group::delete_group_handler,
        // Tag endpoints
        tag::list_tags_handler,
        tag::get_tag_handler,
        tag::create_tag_handler,
        tag::update_tag_handler,
        tag::delete_tag_handler,
        // Storage CRUD endpoints
        storage::list_storages_handler,
        storage::get_storage_handler,
        storage::create_storage_handler,
        storage::update_storage_handler,
        storage::delete_storage_handler,
        // File content endpoints
        storage::get_file_content_handler,
        storage::download_file_handler,
        storage::update_file_content_handler,
        // Entry management endpoints
        storage::create_entry_handler,
        storage::rename_entry_handler,
        storage::remove_entry_handler,
        storage::list_directory_handler,
        // Share endpoints
        storage::list_shares_handler,
        storage::list_all_user_shares_handler,
        storage::list_shares_with_me_handler,
        storage::share_entry_handler,
        storage::delete_share_handler,
        storage::update_share_handler,
        storage::get_share_info_handler,
        storage::share_list_root_handler,
        storage::share_list_handler,
        storage::share_show_handler,
        storage::share_update_handler,
        storage::share_create_handler,
        storage::share_rename_handler,
        storage::share_remove_handler,
        // Thumbnail endpoints
        storage::thumbnail_handler,
        storage::trigger_hash_handler,
        // Meta endpoints
        storage::get_meta_handler,
        // Photo endpoints
        photo::list_photos,
        photo::list_by_year,
        photo::list_by_year_month,
        photo::list_by_year_month_day,
        photo::regenerate_thumbnail,
    ),
    components(
        schemas(
            ErrorResponse,
            user::LoginRequest,
            user::LoginResponse,
            user::UserResponse,
            user::CreateUserRequest,
            user::UpdateUserRequest,
            user::ChangePasswordRequest,
            group::GroupResponse,
            group::CreateGroupRequest,
            group::UpdateGroupRequest,
            tag::TagResponse,
            tag::CreateTagRequest,
            tag::UpdateTagRequest,
            storage::StorageResponse,
            storage::CreateStorageRequest,
            storage::UpdateStorageRequest,
            storage::CreateEntryRequest,
            storage::RenameEntryRequest,
            storage::ShareEntryRequest,
            storage::ShareResponse,
            crate::entity::entry::EntryType,
            crate::storage::DirectoryEntry,
            photo::PhotoResponse,
            storage::MetaResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "user", description = "User management endpoints"),
        (name = "group", description = "Group management endpoints"),
        (name = "tag", description = "Tag management endpoints"),
        (name = "storage", description = "Storage CRUD endpoints"),
        (name = "file", description = "File content operations"),
        (name = "entry", description = "Entry management (create, rename, remove, list)"),
        (name = "share", description = "Sharing operations"),
        (name = "thumbnail", description = "Thumbnail and hash operations"),
        (name = "meta", description = "File meta information endpoints"),
        (name = "photo", description = "Photo management endpoints"),
    )
)]
struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            )
        }
    }
}

pub async fn run(config: Config, db: DatabaseConnection, job_sender: JobSender) {
    let mut jinja = Environment::new();
    jinja
        .add_template(
            "directory_index.html",
            include_str!("../../templates/directory_index.html"),
        )
        .unwrap();

    // Add filters
    jinja.add_filter("format_size", |bytes: i64| format_size(bytes));

    jinja.add_filter("format_datetime", |dt: String| {
        // This is a bit hacky because minijinja's Value doesn't easily pass chrono types
        // unless we use custom Object. For now, we'll assume it's passed as ISO string or
        // handle the type in the filter if possible.
        // Actually, minijinja can handle Serialized chrono types as strings or ints.
        dt
    });

    jinja.add_filter("basename", |path: String| {
        path.trim_end_matches('/')
            .split('/')
            .last()
            .unwrap_or(&path)
            .to_string()
    });

    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        jinja,
        job_sender,
    });

    // API router - all API routes under /api
    let api_router = Router::new()
        .route("/health", get(health_handler))
        .route("/version", get(version_handler))
        .route("/ws", get(ws::ws_handler))
        .nest("/user", user::router())
        .nest("/group", group::router())
        .nest("/storage", storage::router())
        .nest("/tag", tag::router())
        .nest("/photo", photo::router());

    let app = Router::new()
        .nest("/api", api_router)
        .merge(SwaggerUi::new("/api/docs/").url("/api/docs/openapi.json", ApiDoc::openapi()))
        .fallback(get(frontend_handler));

    let app = app
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.server_addr)
        .await
        .unwrap();
    tracing::info!("Server listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

/// Health check endpoint (no auth required)
pub async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_backend = state.db.get_database_backend();
    let db_status = match state
        .db
        .execute(Statement::from_string(db_backend, "SELECT 1"))
        .await
    {
        Ok(_) => "ok",
        Err(e) => {
            tracing::error!("Health check database error: {}", e);
            "error"
        }
    };

    Json(serde_json::json!({
        "status": "ok",
        "service": "byteburrow",
        "database": db_status,
    }))
}

/// Version endpoint
pub async fn version_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "commit": env!("GIT_COMMIT"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
