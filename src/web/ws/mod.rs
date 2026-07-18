use crate::web::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use std::sync::Arc;

/// WebSocket upgrade handler
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

/// Handle WebSocket connection
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
