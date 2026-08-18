//! Dungeon theme data, loaded from TOML files in `<game_dir>/themes/`.
//!
//! A theme defines the flavor for a dungeon zone: room description text
//! pools, per-depth encounter tables, and per-depth loot tables. Pure data —
//! no Rust logic lives here. See `docs/dungeon-generation-design.md` in the
//! game repo for the full design.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeFile {
    pub theme: Theme,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Theme {
    pub name: String,
    pub title_prefix: String,
    pub room_descriptions: Vec<RoomDescriptions>,
    pub encounters: Vec<EncounterTable>,
    pub loot: Vec<LootTable>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoomDescriptions {
    pub shape: String,
    pub texts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EncounterTable {
    pub depth: [u32; 2],
    pub entries: Vec<EncounterEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EncounterEntry {
    pub monster: String,
    pub count: [u32; 2],
    pub weight: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LootTable {
    pub depth: [u32; 2],
    pub items: Vec<String>,
}

/// Load every `*.toml` file under `<game_dir>/themes/`, keyed by
/// `theme.name`. Missing directory or unparseable files are logged and
/// skipped rather than treated as fatal — theme loading never blocks engine
/// startup.
pub fn load_themes(game_dir: &Path) -> HashMap<String, Theme> {
    let mut themes = HashMap::new();
    let themes_dir = game_dir.join("themes");
    if !themes_dir.exists() {
        return themes;
    }
    if let Ok(entries) = std::fs::read_dir(&themes_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str::<ThemeFile>(&content) {
                        Ok(tf) => {
                            tracing::info!(theme = %tf.theme.name, path = %path.display(), "loaded theme");
                            themes.insert(tf.theme.name.clone(), tf.theme);
                        }
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "failed to parse theme");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to read theme file");
                    }
                }
            }
        }
    }
    themes
}
