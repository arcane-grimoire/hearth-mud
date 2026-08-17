mod accounts;
mod db;
mod engine;
mod locks;
mod net;
mod softcode;
mod world;

use std::path::Path;

use tokio::sync::mpsc;

use db::Database;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hearth_mud=info".parse().unwrap()),
        )
        .init();

    let db_path = Path::new("hearth.db");
    let db = Database::open(db_path).expect("Failed to open database");
    tracing::info!(?db_path, "Database opened");

    let (engine_tx, engine_rx) = mpsc::unbounded_channel();
    let engine = engine::Engine::new(engine_rx, db);

    let telnet_tx = engine_tx.clone();
    let telnet_handle = tokio::spawn(async move {
        if let Err(e) = net::start_telnet("0.0.0.0:4000", telnet_tx).await {
            tracing::error!(error = %e, "Telnet server failed");
        }
    });

    let web_tx = engine_tx.clone();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = net::start_web("0.0.0.0:8000", web_tx).await {
            tracing::error!(error = %e, "Web server failed");
        }
    });

    let engine_handle = tokio::spawn(engine.run());

    tracing::info!("Hearth MUD running — telnet :4000 | web :8000");

    tokio::select! {
        _ = engine_handle => tracing::info!("Engine stopped"),
        _ = telnet_handle => tracing::info!("Telnet stopped"),
        _ = web_handle => tracing::info!("Web server stopped"),
    }
}
