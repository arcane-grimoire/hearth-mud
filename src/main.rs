mod accounts;
mod ansi;
mod config;
mod db;
mod dungeon;
mod grid;
mod loader;
mod engine;
mod locks;
mod map_template;
mod net;
mod noise;
mod softcode;
mod theme;
mod world;

use std::path::Path;

use tokio::sync::mpsc;

use config::Config;
use db::Database;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hearth_mud=info".parse().unwrap()),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "hearth.toml".into());
    let config = Config::load(Path::new(&config_path));

    let db = Database::open(Path::new(&config.db_path)).expect("Failed to open database");
    tracing::info!(db_path = %config.db_path, "Database opened");

    let (engine_tx, engine_rx) = mpsc::unbounded_channel();
    let engine = engine::Engine::new(engine_rx, db, &config);

    let telnet_addr = config.telnet_addr.clone();
    let telnet_tx = engine_tx.clone();
    let telnet_handle = tokio::spawn(async move {
        if let Err(e) = net::start_telnet(&telnet_addr, telnet_tx).await {
            tracing::error!(error = %e, "Telnet server failed");
        }
    });

    let web_addr = config.web_addr.clone();
    let web_tx = engine_tx.clone();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = net::start_web(&web_addr, web_tx).await {
            tracing::error!(error = %e, "Web server failed");
        }
    });

    let engine_handle = tokio::spawn(engine.run());

    tracing::info!(
        telnet = %config.telnet_addr,
        web = %config.web_addr,
        "Hearth MUD running"
    );

    tokio::select! {
        _ = engine_handle => tracing::info!("Engine stopped"),
        _ = telnet_handle => tracing::info!("Telnet stopped"),
        _ = web_handle => tracing::info!("Web server stopped"),
    }
}
