use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Json, State, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use crate::engine::{ApiRequest, ApiResponse, EngineMessage};

#[derive(Clone)]
struct AppState {
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
}

pub async fn start_web(
    addr: &str,
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
) -> std::io::Result<()> {
    let state = AppState {
        engine_tx: engine_tx.clone(),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/play", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api", post(api_handler))
        .with_state(state)
        .layer(CorsLayer::permissive());

    let addr: SocketAddr = addr.parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{}", e))
    })?;

    tracing::info!(%addr, "Web server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

async fn index_handler() -> impl IntoResponse {
    Html(include_str!("web_client.html"))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.engine_tx))
}

async fn api_handler(
    State(state): State<AppState>,
    Json(request): Json<ApiRequest>,
) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    if state
        .engine_tx
        .send(EngineMessage::ApiRequest {
            request,
            reply: reply_tx,
        })
        .is_err()
    {
        return Json(ApiResponse {
            ok: false,
            data: None,
            error: Some("Engine unavailable".into()),
        });
    }

    match reply_rx.await {
        Ok(response) => Json(response),
        Err(_) => Json(ApiResponse {
            ok: false,
            data: None,
            error: Some("Engine did not respond".into()),
        }),
    }
}

async fn handle_ws(socket: WebSocket, engine_tx: mpsc::UnboundedSender<EngineMessage>) {
    let session_id = Uuid::new_v4().to_string();
    let (mut ws_tx, mut ws_rx) = socket.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let _ = engine_tx.send(EngineMessage::PlayerConnected {
        session_id: session_id.clone(),
        tx,
    });

    use futures_util::{SinkExt, StreamExt};

    let write_session_id = session_id.clone();
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
        tracing::debug!(session_id = %write_session_id, "WS write loop ended");
    });

    let engine_tx_read = engine_tx.clone();
    let read_session_id = session_id.clone();
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let input = text.trim().to_string();
                if !input.is_empty() {
                    let _ = engine_tx_read.send(EngineMessage::PlayerInput {
                        session_id: read_session_id.clone(),
                        input,
                    });
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = engine_tx.send(EngineMessage::PlayerDisconnected {
        session_id: session_id.clone(),
    });

    write_handle.abort();
}
