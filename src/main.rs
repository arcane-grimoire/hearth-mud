#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

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
mod markup;
mod net;
mod noise;
mod softcode;
mod theme;
mod world;

use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use config::Config;
use db::Database;

/// How long to wait for the engine's final checkpoint before giving up.
///
/// Container runtimes allow a grace period after SIGTERM (10s by default for
/// Docker and Kubernetes) before following up with SIGKILL, so this is a
/// backstop for a wedged engine rather than a budget to spend.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Resolves when the process is asked to stop — ctrl-c, or SIGTERM on Unix.
///
/// SIGTERM is what a container runtime sends on `docker stop` or a pod
/// eviction. With no handler installed the default disposition terminates the
/// process immediately, so the engine's shutdown checkpoint never runs and
/// every deploy discards world state back to the last autosave.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "Failed to install ctrl-c handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

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

    let game_web_dir = config.game_web_dir.as_ref().map(|d| {
        if let Some(game_dir) = &config.game_dir {
            let game_root = std::path::Path::new(game_dir)
                .parent()
                .unwrap_or(std::path::Path::new(game_dir));
            game_root.join(d).to_string_lossy().to_string()
        } else {
            d.clone()
        }
    });
    let web_addr = config.web_addr.clone();
    let web_tx = engine_tx.clone();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = net::start_web(&web_addr, web_tx, game_web_dir.as_deref()).await {
            tracing::error!(error = %e, "Web server failed");
        }
    });

    let mut engine_handle = tokio::spawn(engine.run());

    tracing::info!(
        telnet = %config.telnet_addr,
        web = %config.web_addr,
        "Hearth MUD running"
    );

    // The engine owns the final checkpoint, so a stop signal has to reach it
    // rather than felling the process — see `shutdown_signal`.
    let engine_already_stopped = tokio::select! {
        _ = &mut engine_handle => true,
        _ = shutdown_signal() => {
            tracing::info!("Shutdown signal received");
            false
        }
        _ = telnet_handle => {
            tracing::info!("Telnet stopped");
            false
        }
        _ = web_handle => {
            tracing::info!("Web server stopped");
            false
        }
    };

    if engine_already_stopped {
        tracing::info!("Engine stopped");
        return;
    }

    let _ = engine_tx.send(engine::EngineMessage::Shutdown);
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, engine_handle).await {
        Ok(_) => tracing::info!("World checkpointed, shutdown complete"),
        Err(_) => tracing::error!(
            timeout_secs = SHUTDOWN_TIMEOUT.as_secs(),
            "Engine did not finish its shutdown checkpoint in time; \
             world state may have been lost back to the last autosave"
        ),
    }
}
