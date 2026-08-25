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
    pub game_web_dir: Option<String>,
    pub max_characters: u8,
    /// Whether to load `game_dir`'s TOML+`.luau` content into the world at
    /// boot, same as it always has. Defaults to `true` — the current
    /// behaviour — so existing configs are unaffected.
    ///
    /// See docs/plans/program-authoring.md Stage 4: once `@import` /
    /// `hearth import` cover installing world content into the DB
    /// explicitly, a maintainer can flip this to `false` so boot reads the
    /// DB only and files become a pure distribution format. That switch is
    /// deliberately *not* made here — this option exists so it is a
    /// one-line config change when a maintainer decides to make it, not a
    /// silent behaviour change bundled with Stage 4. Shipped lib modules
    /// (`lib/`, embedded stdlib) and `.ink` files are unaffected either way;
    /// they are never persisted to the DB (see Stage 2's "`require`
    /// resolution").
    pub load_world_files: bool,
    /// File-key/area prefixes whose managed objects are stamped
    /// `system:locked` at load — their definition is file-authoritative and
    /// read-only to in-game authoring (the builder, REST, and `@` commands
    /// refuse to edit them; edit the source file and `@reload-world`).
    ///
    /// Runtime *state* (attrs a hook sets during play) is never blocked —
    /// only the authoring surface. See `docs/plans/archetypes.md`. Typically
    /// `["std", "system"]`: the code tier is locked, world content is live.
    pub locked: Vec<String>,
    /// Allow-list of browser origins permitted to call the web/API server
    /// cross-origin. `None`/empty (the default) uses a permissive CORS policy,
    /// which is fine for local development but risky in deployment — a page on
    /// any origin could drive the authenticated REST API. Set an explicit list
    /// (e.g. `["https://play.example.com"]`) in production.
    pub cors_allowed_origins: Option<Vec<String>>,
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
            game_web_dir: None,
            max_characters: 3,
            load_world_files: true,
            locked: Vec::new(),
            cors_allowed_origins: None,
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
