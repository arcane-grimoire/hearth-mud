//! Hook names and per-object script storage.
//!
//! Each Object carries at most one [`ObjectScript`] — a single Luau chunk that
//! runs once and *defines* its hooks as top-level functions sharing one scope
//! (helpers, constants, `require`d modules). This is the Godot model: the
//! object is the unit, hooks are its methods, and the file-level scope is the
//! shared "class body." `can_` hooks gate an action (return `false` to veto),
//! `on_` hooks react after it happened, and `cmd_` hooks define player-typeable
//! commands (see ADR 0004).
//!
//! `lib_<name>` modules are a *separate* concern: a lib is a standalone chunk
//! that `return`s a value and is loaded via `require("<name>")`, so it cannot
//! be a function in the shared script scope. Those live in [`GameObject::libs`]
//! as [`LibModule`]s, one per name.

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

/// Whether `name` is a hook an object script may define.
///
/// Any `can_` hook must be one of [`KNOWN_HOOKS`]. `on_` and `cmd_`-prefixed
/// names are open-ended — `on_reply`, `on_buy`, `cmd_talk` are all valid.
/// `lib_<name>` is *not* a hook — it is a [`LibModule`] (see this module's
/// docs) — so it is deliberately excluded here.
pub fn is_valid_hook_name(name: &str) -> bool {
    KNOWN_HOOKS.contains(&name)
        || (name.starts_with("cmd_") && name.len() > 4)
        || (name.starts_with("on_") && name.len() > 3)
}

/// Whether `name` names a library module (`lib_<name>`), authored on a
/// `Kind::Code` object and loaded via `require("<name>")`.
pub fn is_valid_lib_name(name: &str) -> bool {
    name.starts_with("lib_") && name.len() > 4
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

/// Where a script's source came from — which authority owns it.
///
/// The loader reconciles only [`ProgramOrigin::File`] scripts against the game
/// files. [`ProgramOrigin::InGame`] scripts are database-owned: they survive
/// `@reload-world` and restarts, and a file load never deletes or overwrites
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramOrigin {
    /// Installed from a TOML definition or `.luau` file under `game_dir`.
    #[default]
    File,
    /// Written at runtime — `@program`, the REST API, or softcode's
    /// `set_script`.
    InGame,
}

/// The single Luau script attached to an Object.
///
/// `source` is one chunk that, when run, defines each hook as a top-level
/// function (`function on_get(this, actor, room) ... end`). The engine runs
/// the body once, then looks up whichever hook is firing by name. `hooks` is a
/// derived index of the hook functions the source defines (see
/// [`derive_hooks`]) so the engine can answer "does this object respond to X?"
/// without running the script. `state` is the object's persistent state,
/// shared across all its hooks (Godot member vars).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectScript {
    pub source: String,
    pub enabled: bool,
    #[serde(default)]
    pub state: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub origin: ProgramOrigin,
    /// Derived at set-time: the hook names this source defines as functions.
    #[serde(default)]
    pub hooks: Vec<String>,
}

impl ObjectScript {
    /// Whether this (enabled) script defines `hook`.
    pub fn defines(&self, hook: &str) -> bool {
        self.enabled && self.hooks.iter().any(|h| h == hook)
    }
}

/// A `require`able library module authored on a `Kind::Code` object.
///
/// Unlike a hook, a lib is a standalone chunk that `return`s a value; it is
/// registered under its bare `<name>` (no `lib_` prefix) and loaded via
/// `require("<name>")`. See `crate::softcode::mod::install_require`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibModule {
    pub source: String,
    #[serde(default)]
    pub origin: ProgramOrigin,
}

/// The top-level hook functions a script's `source` defines.
///
/// Parses `source` as Luau (via `full_moon`) and collects the names of
/// **global** function definitions declared at the top level of the chunk —
/// both `function <name>(...)` declarations and `<name> = function(...)`
/// assignments. Because it works on the parsed AST rather than raw text, a
/// `function on_get(...)` that appears *inside a string literal or comment*
/// (e.g. a template `set_script`'d onto another object) is correctly ignored.
///
/// `local function` and table methods (`function t.m()` / `t:m()`) are
/// excluded — they aren't dispatchable hooks. Only names accepted by
/// [`is_valid_hook_name`] are returned, in first-seen order without
/// duplicates. Hooks installed by metaprogramming (assigning into the
/// environment dynamically) are not detected — the authoring contract is
/// "define each hook as a named top-level function." Source that doesn't
/// parse yields no hooks (a broken script has no working hooks anyway).
pub fn derive_hooks(source: &str) -> Vec<String> {
    // `full_moon` is a recursive-descent parser and can use deep stack on
    // large/nested scripts — enough to overflow a default 2 MiB thread stack
    // (the engine runs on tokio worker threads of that size, and a runtime
    // `set_script` parses on that thread). Run the parse on a dedicated thread
    // with a large stack so parsing a script can never crash the server.
    // Parsing only happens on writes/boot, never per-tick, so the thread cost
    // is irrelevant.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn_scoped(scope, || derive_hooks_inner(source))
            .expect("spawn hook-derivation thread")
            .join()
            .unwrap_or_default()
    })
}

fn derive_hooks_inner(source: &str) -> Vec<String> {
    use full_moon::ast::{Stmt, Var};

    let Ok(ast) = full_moon::parse(source) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if is_valid_hook_name(&name) && !out.iter().any(|h| *h == name) {
            out.push(name);
        }
    };
    for stmt in ast.nodes().stmts() {
        match stmt {
            // `function <name>(...) ... end` — a global function declaration.
            // Skip anything with dotted names or a `:method` (not a hook).
            Stmt::FunctionDeclaration(decl) => {
                let fname = decl.name();
                let names: Vec<_> = fname.names().iter().collect();
                if fname.method_name().is_none()
                    && names.len() == 1
                {
                    push(names[0].token().to_string());
                }
            }
            // `<name> = function(...) ... end` — a global assignment whose
            // value is a function expression.
            Stmt::Assignment(assign) => {
                let vars: Vec<_> = assign.variables().iter().collect();
                let exprs: Vec<_> = assign.expressions().iter().collect();
                if vars.len() == 1
                    && exprs.len() == 1
                    && matches!(exprs[0], full_moon::ast::Expression::Function(_))
                    && let Var::Name(name) = vars[0]
                {
                    push(name.token().to_string());
                }
            }
            _ => {}
        }
    }
    out
}

/// Attach (or replace) `obj`'s script from in-game authoring.
pub fn set_script(obj: &mut GameObject, source: String) {
    set_script_with_origin(obj, source, ProgramOrigin::InGame)
}

/// Attach (or replace) `obj`'s script, recording where the source came from.
///
/// Replacing the script preserves the object's accumulated `state` — rewriting
/// behavior shouldn't reset what an `on_tick` has been remembering, and the
/// loader reinstalls file scripts on every startup. The hook index is
/// re-derived from the new source.
pub fn set_script_with_origin(obj: &mut GameObject, source: String, origin: ProgramOrigin) {
    let state = obj
        .script
        .as_ref()
        .map(|prev| prev.state.clone())
        .unwrap_or_default();
    let hooks = derive_hooks(&source);
    obj.script = Some(ObjectScript {
        source,
        enabled: true,
        state,
        origin,
        hooks,
    });
}

/// Remove `obj`'s script, if any. Returns whether one was removed.
pub fn clear_script(obj: &mut GameObject) -> bool {
    obj.script.take().is_some()
}

/// `obj`'s script, if it has an enabled one.
pub fn get_script(obj: &GameObject) -> Option<&ObjectScript> {
    obj.script.as_ref().filter(|s| s.enabled)
}

/// Whether `obj` has an enabled script defining `hook`.
pub fn object_defines_hook(obj: &GameObject, hook: &str) -> bool {
    obj.script.as_ref().is_some_and(|s| s.defines(hook))
}

/// Attach (or replace) a `require`able lib module on `obj`, keyed by its bare
/// `<name>` (no `lib_` prefix).
pub fn set_lib(obj: &mut GameObject, name: &str, source: String, origin: ProgramOrigin) {
    obj.libs
        .insert(name.to_string(), LibModule { source, origin });
}

/// Remove the lib module `<name>` on `obj`, if any. Returns whether one was
/// removed.
pub fn remove_lib(obj: &mut GameObject, name: &str) -> bool {
    obj.libs.remove(name).is_some()
}

/// Find the first object among `candidates` whose script defines an enabled
/// `cmd_<command>` hook, per the command-resolution order in ADR 0004 (callers
/// control the order via `candidates`).
pub fn find_cmd_hook<'a>(
    candidates: impl IntoIterator<Item = &'a GameObject>,
    command: &str,
) -> Option<(&'a GameObject, &'a ObjectScript)> {
    let hook = format!("cmd_{}", command);
    candidates.into_iter().find_map(|obj| {
        obj.script
            .as_ref()
            .filter(|s| s.defines(&hook))
            .map(|s| (obj, s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_hooks_finds_top_level_functions() {
        let src = "local x = 1\nfunction on_get(this, actor, room) end\nfunction cmd_talk(this, actor, room, args) end\non_look = function(this, actor, room) end";
        let hooks = derive_hooks(src);
        assert!(hooks.contains(&"on_get".to_string()));
        assert!(hooks.contains(&"cmd_talk".to_string()));
        assert!(hooks.contains(&"on_look".to_string()));
    }

    #[test]
    fn derive_hooks_ignores_functions_inside_string_literals() {
        // A template `set_script`'d onto another object at runtime — the
        // `function cmd_talk` here is a string value, not a hook on THIS
        // object. A text scanner would wrongly report it; the parser must not.
        let src = r#"
local GUIDE_TALK = [[
function cmd_talk(this, actor, room, args)
  emit(actor, "hi")
end
]]
function on_enter(this, actor, room)
  set_script(get_attr(this, "guide"), GUIDE_TALK)
end
"#;
        let hooks = derive_hooks(src);
        assert_eq!(hooks, vec!["on_enter".to_string()]);
        assert!(!hooks.contains(&"cmd_talk".to_string()));
    }

    #[test]
    fn derive_hooks_excludes_local_functions_and_methods() {
        let src = "local function helper() end\nfunction M.method() end\nfunction on_use(this, actor, room) end";
        let hooks = derive_hooks(src);
        assert_eq!(hooks, vec!["on_use".to_string()]);
    }

    #[test]
    fn derive_hooks_ignores_non_hook_names() {
        // `can_*` must be a KNOWN hook; a bare word is not a hook at all.
        let src = "function frobnicate() end\nfunction can_frobnicate(this, actor) end\nfunction can_get(this, actor) end";
        let hooks = derive_hooks(src);
        assert_eq!(hooks, vec!["can_get".to_string()]);
    }
}
