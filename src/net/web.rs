use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Json, State, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

use crate::engine::{ApiRequest, ApiResponse, ClientMessage, EngineMessage};
use crate::markup;

#[derive(Clone)]
struct AppState {
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
}

pub async fn start_web(
    addr: &str,
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
    game_web_dir: Option<&str>,
) -> std::io::Result<()> {
    let state = AppState {
        engine_tx: engine_tx.clone(),
    };

    let base = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api", post(api_handler))
        .with_state(state)
        .layer(CorsLayer::permissive());

    let web_dir = game_web_dir
        .map(std::path::Path::new)
        .filter(|p| p.is_dir())
        .or_else(|| {
            let p = std::path::Path::new("web/dist");
            if p.is_dir() { Some(p) } else { None }
        });

    let app = if let Some(dir) = web_dir {
        tracing::info!(dir = %dir.display(), "Serving web client");
        let serve = ServeDir::new(dir)
            .not_found_service(ServeFile::new(dir.join("index.html")));
        base.fallback_service(serve)
    } else {
        tracing::info!("Using embedded web client (run `npm run build` in web/ for the full UI)");
        base.route("/", get(index_handler))
            .route("/play", get(index_handler))
    };

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

    let (tx, mut rx) = mpsc::unbounded_channel::<ClientMessage>();

    let _ = engine_tx.send(EngineMessage::PlayerConnected {
        session_id: session_id.clone(),
        tx,
    });

    use futures_util::{SinkExt, StreamExt};

    let write_session_id = session_id.clone();
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match &msg {
                ClientMessage::Text { text } => {
                    let html = markup::to_html(text);
                    if html.is_empty() { continue; }
                    serde_json::json!({"type": "text", "text": html}).to_string()
                }
                ClientMessage::Prompt { echo } => {
                    serde_json::json!({"type": "prompt", "echo": echo}).to_string()
                }
                other => {
                    match serde_json::to_string(other) {
                        Ok(j) => j,
                        Err(_) => continue,
                    }
                }
            };
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
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

