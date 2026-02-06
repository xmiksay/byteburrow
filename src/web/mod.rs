use crate::entities::dummy;
use crate::Config;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;

use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

pub struct AppState {
    pub db: DatabaseConnection,
}

pub async fn run(config: Config, db: DatabaseConnection) {
    let state = Arc::new(AppState { db });

    let index_path = PathBuf::from(&config.frontend_dist).join("index.html");

    let app = Router::new()
        .route("/api/dummy", get(root_handler))
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
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn root_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let records = dummy::Entity::find()
        .all(&state.db)
        .await
        .expect("Failed to fetch records from database");

    Json(records)
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
