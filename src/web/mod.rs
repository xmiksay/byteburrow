pub mod group;
pub mod storage;
pub mod user;
pub mod ws;
pub mod tag;

use crate::auth::Auth;
use crate::config::Config;
use axum::{extract::State, response::IntoResponse, http::StatusCode, routing::get, Json, Router};
use sea_orm::{DatabaseConnection, ConnectionTrait, Statement};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use minijinja::Environment;
use crate::storage::format_size;

/// Error response
#[derive(Debug, Serialize)]
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
}

pub async fn run(config: Config, db: DatabaseConnection) {
    let mut jinja = Environment::new();
    jinja.set_loader(minijinja::path_loader("templates"));
    
    // Add filters
    jinja.add_filter("format_size", |bytes: i64| {
        format_size(bytes)
    });
    
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

    let app = Router::new()
        .nest("/api", api_router)
        .nest_service(
            "/",
            ServeDir::new(&config.frontend_dist).fallback(ServeFile::new(index_path)),
        )
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
    let db_status = match state.db.execute(Statement::from_string(db_backend, "SELECT 1")).await {
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
