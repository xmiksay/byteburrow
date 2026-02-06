pub mod auth;
pub mod handlers;

use crate::config::Config;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use sea_orm::DatabaseConnection;
use std::sync::Arc;

use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Config,
}

pub async fn run(config: Config, db: DatabaseConnection) {
    let state = Arc::new(AppState {
        db,
        config: config.clone(),
    });

    let index_path = PathBuf::from(&config.frontend_dist).join("index.html");

    // Public routes (no authentication required)
    let public_routes = Router::new()
        .route("/api/health", get(handlers::health_handler))
        .route("/api/login", post(handlers::login_handler))
        .route("/api/thumbnail/:hash", get(handlers::thumbnail_handler));

    // Protected routes (authentication required)
    let protected_routes = Router::new()
        .route("/api/me", get(handlers::me_handler))
        .route("/api/storages", get(handlers::protected_handler));

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .route("/ws", get(ws_handler))
        .nest_service(
            "/",
            ServeDir::new(&config.frontend_dist).fallback(ServeFile::new(index_path)),
        )
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.server_addr)
        .await
        .unwrap();
    tracing::info!("Server listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket))
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            match msg {
                Message::Text(text) => {
                    tracing::debug!("Received text: {}", text);
                    if socket
                        .send(Message::Text(format!("Echo: {}", text)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Message::Binary(bin) => {
                    tracing::debug!("Received binary: {} bytes", bin.len());
                    if socket.send(Message::Binary(bin)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => {
                    tracing::debug!("Client closed connection");
                    break;
                }
                _ => {}
            }
        } else {
            tracing::debug!("Client disconnected");
            break;
        }
    }
}
