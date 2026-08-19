use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Json, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

use crate::engine::{ApiRequest, ApiResponse, ClientMessage, EngineMessage};
use crate::markup;

#[cfg(feature = "bundle-web")]
use rust_embed::Embed;

#[cfg(feature = "bundle-web")]
#[derive(Embed)]
#[folder = "web/dist/"]
struct EmbeddedAssets;

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
        web_fallback(base)
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

#[cfg(feature = "bundle-web")]
fn web_fallback(base: Router) -> Router {
    tracing::info!("Serving bundled web client");
    base.fallback(embedded_handler)
}

#[cfg(not(feature = "bundle-web"))]
fn web_fallback(base: Router) -> Router {
    tracing::info!("No web client (build with --features bundle-web or run `npm run build` in web/)");
    base
}

#[cfg(feature = "bundle-web")]
async fn embedded_handler(uri: axum::http::Uri) -> axum::response::Response {
    use axum::http::{header, StatusCode};

    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = EmbeddedAssets::get(path) {
        let mime = match path.rsplit('.').next().unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "application/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "ico" => "image/x-icon",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "json" => "application/json",
            "wasm" => "application/wasm",
            _ => "application/octet-stream",
        };
        return (StatusCode::OK, [(header::CONTENT_TYPE, mime)], file.data.to_vec())
            .into_response();
    }

    // SPA fallback
    if let Some(file) = EmbeddedAssets::get("index.html") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data.to_vec(),
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.engine_tx))
}

async fn api_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ApiRequest>,
) -> impl IntoResponse {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    if state
        .engine_tx
        .send(EngineMessage::ApiRequest {
            request,
            token,
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

