pub mod group;
pub mod storage;
pub mod tag;
pub mod user;
pub mod ws;

use crate::auth::Auth;
use crate::config::Config;
use crate::job::JobSender;
use crate::storage::format_size;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use minijinja::Environment;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};
use utoipa_swagger_ui::SwaggerUi;

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

pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Config,
    pub jinja: Environment<'static>,
    pub job_sender: JobSender,
}

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
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
        // Storage endpoints
        storage::list_storages_handler,
        storage::get_storage_handler,
        storage::create_storage_handler,
        storage::update_storage_handler,
        storage::delete_storage_handler,
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
            storage::UpdateEntryTagsRequest,
            storage::ShareEntryRequest,
            storage::ShareResponse,
            crate::entity::entry::EntryType,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "user", description = "User management endpoints"),
        (name = "group", description = "Group management endpoints"),
        (name = "tag", description = "Tag management endpoints"),
        (name = "storage", description = "Storage management endpoints"),
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

    let index_path = PathBuf::from(&config.frontend_dist).join("index.html");

    // API router - all API routes under /api
    let api_router = Router::new()
        .route("/health", get(health_handler))
        .route("/version", get(version_handler))
        .route("/ws", get(ws::ws_handler))
        .nest("/user", user::router())
        .nest("/group", group::router())
        .nest("/storage", storage::router())
        .nest("/tag", tag::router());

    let app = Router::new().nest("/api", api_router).nest_service(
        "/",
        ServeDir::new(&config.frontend_dist).fallback(ServeFile::new(index_path)),
    );

    // Merge Swagger UI - convert explicitly with type annotation
    let swagger_router = Router::<Arc<AppState>>::from(
        SwaggerUi::new("/swagger-ui").url("/api-doc/openapi.json", ApiDoc::openapi()),
    );

    let app = app
        .merge(swagger_router)
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
        "service": "cloud",
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
