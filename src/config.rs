use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub telnet_addr: String,
    pub web_addr: String,
    pub db_path: String,
    pub autosave_secs: u64,
    pub tick_secs: u64,
    pub spawn_room: String,
    pub game_dir: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            telnet_addr: "0.0.0.0:4000".into(),
            web_addr: "0.0.0.0:8000".into(),
            db_path: "hearth.db".into(),
            autosave_secs: 300,
            tick_secs: 1,
            spawn_room: "starter/town_square".into(),
            game_dir: None,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    tracing::info!(?path, "Config loaded");
                    config
                }
                Err(e) => {
                    tracing::warn!(?path, error = %e, "Failed to parse config, using defaults");
                    Self::default()
                }
            },
            Err(_) => {
                tracing::info!(?path, "No config file found, using defaults");
                Self::default()
            }
        }
    }
}
