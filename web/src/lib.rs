use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use shared::Config;
use futures_util::{sink::SinkExt, stream::StreamExt};
use sea_orm::{DatabaseConnection, EntityTrait};
use shared::entities::dummy;
use std::sync::Arc;

pub struct AppState {
    pub db: DatabaseConnection,
}

pub async fn run(config: Config, db: DatabaseConnection) {
    let state = Arc::new(AppState { db });

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/ws", get(ws_handler))
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
