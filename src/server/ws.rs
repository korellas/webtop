use crate::server::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use std::sync::Arc;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.broadcast_tx.subscribe();

    while let Ok(snapshot) = rx.recv().await {
        // A snapshot that fails to serialize (a NaN float would do it) is not
        // worth dropping the socket over — skip the sample and keep streaming.
        let json = match serde_json::to_string(&snapshot) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "skipping unserializable snapshot");
                continue;
            }
        };
        if socket.send(Message::Text(json.into())).await.is_err() {
            break;
        }
    }
}
