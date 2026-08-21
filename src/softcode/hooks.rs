//! Hook names and Program storage.
//!
//! A Program is a Luau script attached to an Object via a named Hook.
//! `can_` hooks gate permission for an action (return `false` to veto).
//! `on_` hooks run after an action has already happened, for reactive flavor.
//! `cmd_` hooks define player-typeable commands that dispatch resolves to
//! when nothing builtin matches (see ADR 0004).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::world::GameObject;

/// The fixed set of non-`cmd_` hooks the engine knows how to fire on its own.
/// `cmd_*` hooks are open-ended — any suffix is a valid command name.
pub const KNOWN_HOOKS: &[&str] = &[
    "can_get",
    "on_get",
    "can_drop",
    "on_drop",
    "can_put",
    "on_put",
    "can_traverse",
    "can_enter",
    "on_enter",
    "on_leave",
    "can_look",
    "on_look",
    "can_say",
    "on_say",
    "can_use",
    "on_use",
    "can_see",
    "on_move",
    "on_destroy",
    "on_connect",
    "on_disconnect",
    "on_whisper",
    "on_emote",
    "on_receive",
    "on_damage",
    "on_death",
    "on_tick",
    "on_startup",
    "on_shutdown",
    "on_reload",
    "on_save",
    "on_create",
];

/// Whether `name` is a hook the engine will accept on `@program`.
///
/// Any `can_` hook must be one of [`KNOWN_HOOKS`]. `on_` and `cmd_`-prefixed
/// names are open-ended — `on_reply`, `on_buy`, `cmd_talk` are all valid.
pub fn is_valid_hook_name(name: &str) -> bool {
    KNOWN_HOOKS.contains(&name)
        || (name.starts_with("cmd_") && name.len() > 4)
        || (name.starts_with("on_") && name.len() > 3)
}

/// A short human-readable description of a hook, for `@programs` output and
/// error messages. Falls back to a generic description for `cmd_*` hooks.
pub fn describe_hook(name: &str) -> &'static str {
    match name {
        "can_get" => "gates whether an actor may pick this object up",
        "on_get" => "runs after an actor successfully picks this object up",
        "can_drop" => "gates whether an actor may drop this object",
        "on_drop" => "runs after an actor drops this object",
        "can_put" => "gates whether an actor may put an item into this container",
        "on_put" => "runs after an actor puts an item into this container",
        "can_traverse" => "gates whether an actor may use this exit",
        "on_enter" => "runs after an actor enters this room",
        "on_leave" => "runs after an actor leaves this room",
        "can_look" => "gates whether an actor may look at this object",
        "on_look" => "runs after an actor looks at this object",
        "can_say" => "gates whether an actor may speak in this room",
        "on_say" => "runs after an actor speaks in this room",
        "can_use" => "gates whether an actor may use this object",
        "on_use" => "runs after an actor uses this object",
        "can_see" => "gates whether an actor can see this hidden object",
        "can_enter" => "gates whether an actor can enter this room",
        "on_move" => "runs when this object is moved to a new location",
        "on_destroy" => "runs before this object is destroyed",
        "on_connect" => "runs when a player connects (on room or player)",
        "on_disconnect" => "runs when a player disconnects (on room or player)",
        "on_whisper" => "runs when an actor whispers in this room",
        "on_emote" => "runs when an actor emotes in this room",
        "on_receive" => "runs when an item is placed in this object",
        "on_damage" => "runs when this object takes damage",
        "on_death" => "runs when this object dies",
        "on_tick" => "runs every N ticks (set tick_interval attr)",
        "on_startup" => "runs once when the engine starts",
        "on_shutdown" => "runs once when the engine is shutting down",
        "on_reload" => "runs after @reload-world completes",
        "on_save" => "runs before each world save (autosave or manual)",
        "on_create" => "runs when this object is first created at runtime",
        _ => "a custom player command (cmd_<name>)",
    }
}

/// Where a Program's source came from — which authority owns it.
///
/// The loader reconciles only [`ProgramOrigin::File`] programs against the
/// game files. [`ProgramOrigin::InGame`] programs are database-owned: they
/// survive `@reload-world` and restarts, and a file load never deletes or
/// overwrites one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramOrigin {
    /// Installed from a TOML definition or `.luau` file under `game_dir`.
    ///
    /// This is the *deserialization* default so that programs stored before
    /// provenance existed stay under loader management — on a managed object
    /// they can only have come from files, because every startup reconciled
    /// them away otherwise. Records written since always set the field.
    #[default]
    File,
    /// Written at runtime — `@program`, the REST API, or softcode's
    /// `set_program`.
    InGame,
}

/// A Program stored on an Object under a Hook name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramRecord {
    pub hook: String,
    pub source: String,
    pub enabled: bool,
    #[serde(default)]
    pub state: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub origin: ProgramOrigin,
}

impl ProgramRecord {
    /// A new in-game Program. File-loaded programs come from
    /// [`ProgramRecord::from_file`].
    pub fn new(hook: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            hook: hook.into(),
            source: source.into(),
            enabled: true,
            state: HashMap::new(),
            origin: ProgramOrigin::InGame,
        }
    }

    /// A new Program owned by the game files, subject to loader reconciliation.
    pub fn from_file(hook: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            origin: ProgramOrigin::File,
            ..Self::new(hook, source)
        }
    }
}

/// Attach (or replace) a Program on `obj` at `hook`.
///
/// Returns an error if `hook` isn't a recognized hook name. Does not
/// validate the Luau source — callers should run it through
/// [`crate::softcode::SoftcodeRuntime::check_syntax`] first so builders get a
/// syntax error instead of a silently broken program.
pub fn set_program(obj: &mut GameObject, hook: &str, source: String) -> Result<(), String> {
    set_program_with_origin(obj, hook, source, ProgramOrigin::InGame)
}

/// Attach (or replace) a Program on `obj` at `hook`, recording where the
/// source came from.
///
/// Replacing a Program preserves the accumulated per-program `state` map —
/// rewriting the source of an `on_tick` hook shouldn't reset what it has been
/// remembering, and the loader reinstalls file programs on every startup.
pub fn set_program_with_origin(
    obj: &mut GameObject,
    hook: &str,
    source: String,
    origin: ProgramOrigin,
) -> Result<(), String> {
    if !is_valid_hook_name(hook) {
        return Err(format!(
            "Unknown hook '{}'. Known hooks: {}, or cmd_<name>.",
            hook,
            KNOWN_HOOKS.join(", ")
        ));
    }
    let state = obj
        .programs
        .get(hook)
        .map(|prev| prev.state.clone())
        .unwrap_or_default();
    let mut record = ProgramRecord::new(hook, source);
    record.origin = origin;
    record.state = state;
    obj.programs.insert(hook.to_string(), record);
    Ok(())
}

/// Remove the Program at `hook` on `obj`, if any. Returns whether one was
/// removed.
pub fn remove_program(obj: &mut GameObject, hook: &str) -> bool {
    obj.programs.remove(hook).is_some()
}

/// Look up an enabled Program at `hook` on `obj`.
pub fn get_program<'a>(obj: &'a GameObject, hook: &str) -> Option<&'a ProgramRecord> {
    obj.programs.get(hook).filter(|p| p.enabled)
}

/// All Programs on `obj`, sorted by hook name for stable display.
pub fn list_programs(obj: &GameObject) -> Vec<&ProgramRecord> {
    let mut programs: Vec<&ProgramRecord> = obj.programs.values().collect();
    programs.sort_by(|a, b| a.hook.cmp(&b.hook));
    programs
}

/// Find the first object among `candidates` carrying an enabled `cmd_<name>`
/// hook, per the command-resolution order in ADR 0004 (room contents first,
/// then inventory — callers control that order via `candidates`).
pub fn find_cmd_hook<'a>(
    candidates: impl IntoIterator<Item = &'a GameObject>,
    command: &str,
) -> Option<(&'a GameObject, &'a ProgramRecord)> {
    let hook = format!("cmd_{}", command);
    candidates
        .into_iter()
        .find_map(|obj| get_program(obj, &hook).map(|p| (obj, p)))
}
