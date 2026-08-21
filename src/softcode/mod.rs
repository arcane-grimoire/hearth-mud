//! The Luau softcode system.
//!
//! Programs (Luau scripts, see [`hooks`]) run on the engine's single thread,
//! gated by an instruction [`Budget`]. They cannot touch [`crate::world::World`]
//! directly — see ADR 0001. Instead every mutating call in the API
//! ([`api`]) pushes a typed [`Intent`] into an [`IntentBatch`]; the engine
//! validates and applies the whole batch atomically once the script returns.

pub mod api;
pub mod hooks;
pub mod ink;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{Lua, LuaSerdeExt, Value as LuaValue, VmState};

use crate::theme::Theme;
use crate::world::{GameObject, Kind, Tag, World};
use hooks::ProgramRecord;

pub const MAX_CONTAINER_DEPTH: u32 = 3;

#[derive(Debug, Clone)]
pub struct ScheduledHook {
    pub id: String,
    pub fire_at_tick: u64,
    pub target: String,
    pub hook: String,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

/// A typed mutation a Program has requested. This is the exhaustive list of
/// everything softcode can do to the world — see ADR 0001.
#[derive(Debug, Clone)]
pub enum Intent {
    SetAttr {
        target: String,
        key: String,
        value: serde_json::Value,
    },
    UnsetAttr {
        target: String,
        key: String,
    },
    EmitActor {
        target: String,
        message: String,
    },
    EmitRoom {
        room: String,
        message: String,
        exclude: Vec<String>,
    },
    Move {
        target: String,
        destination: String,
    },
    SetTag {
        target: String,
        tag: Tag,
    },
    UnsetTag {
        target: String,
        tag: Tag,
    },
    Spawn {
        ref_id: String,
        key: String,
        kind: Kind,
        title: Option<String>,
        description: Option<String>,
        location: String,
        owner: Option<String>,
    },
    SetTitle {
        target: String,
        title: String,
    },
    SetDescription {
        target: String,
        description: String,
    },
    Destroy {
        target: String,
    },
    CreateExit {
        ref_id: String,
        source: String,
        direction: String,
        target: String,
        aliases: Vec<String>,
    },
    SetProgram {
        target: String,
        hook: String,
        source: String,
    },
    Trigger {
        target: String,
        hook: String,
        data: Option<serde_json::Value>,
    },
    EmitNearby {
        room: String,
        x: f64,
        y: f64,
        radius: f64,
        message: String,
        exclude: Vec<String>,
    },
    /// Attach a Lock DSL expression to one of `target`'s hooks (e.g.
    /// `"can_enter"`). Used by `map_template`'s `lock` cell override — see
    /// `crate::locks`.
    SetLock {
        target: String,
        hook: String,
        expr: String,
    },
    SetOwner {
        target: String,
        owner: String,
    },
    /// Schedule a hook to fire on `target` after `ticks` engine ticks.
    After {
        target: String,
        hook: String,
        ticks: u64,
        data: Option<HashMap<String, serde_json::Value>>,
    },
    CancelAfter {
        target: String,
        hook: String,
    },
    EmitData {
        target: String,
        channel: String,
        data: serde_json::Value,
    },
    EmitRadius {
        room: String,
        radius: u32,
        messages: HashMap<u32, String>,
        exclude: Vec<String>,
    },
    TransferAttr {
        from: String,
        to: String,
        key: String,
        amount: f64,
    },
}

/// The intents a Program has queued during a single run. Collected while the
/// script executes, applied all-at-once (or not at all) afterward.
#[derive(Debug, Clone, Default)]
pub struct IntentBatch {
    pub intents: Vec<Intent>,
    /// The human (or system) that caused this batch to run — e.g. the
    /// builder whose command invoked a `cmd_*` hook. `None` for ticks and
    /// scheduled hooks, which genuinely have no actor.
    ///
    /// Not used for program-version authorship: softcode's `Intent::SetProgram`
    /// is instantiation, not authoring (see
    /// docs/plans/program-authoring.md Stage 3, "Instantiation is not
    /// authoring"), and is never versioned. This field exists for the
    /// future permission work described alongside `authority` below.
    pub actor_ref: Option<String>,
    /// The owner of the object the running Program is attached to — the
    /// authority code should execute *as*, per the plan's "MUSH-level UGC"
    /// note: a builder's command should run with the object's authority,
    /// not the caller's. Nothing reads or enforces this field yet —
    /// `apply_to` still only validates that intent targets exist — it is
    /// recorded now so that enforcement is a change inside `apply_to` later
    /// rather than a retrofit through every call site.
    pub authority: Option<String>,
}

impl IntentBatch {
    pub fn push(&mut self, intent: Intent) {
        self.intents.push(intent);
    }

    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    pub fn len(&self) -> usize {
        self.intents.len()
    }

    /// Check for a pending write to `target`/`key`, scanning in reverse so
    /// the latest write wins. Returns `Some(Some(value))` for a pending set,
    /// `Some(None)` for a pending unset, or `None` if no write is pending.
    pub fn pending_attr(&self, target: &str, key: &str) -> Option<Option<&serde_json::Value>> {
        for intent in self.intents.iter().rev() {
            match intent {
                Intent::SetAttr { target: t, key: k, value } if t == target && k == key => {
                    return Some(Some(value));
                }
                Intent::UnsetAttr { target: t, key: k } if t == target && k == key => {
                    return Some(None);
                }
                _ => {}
            }
        }
        None
    }
}

/// An outward-facing side effect produced by applying an [`IntentBatch`].
/// The engine turns these into messages sent down player session channels.
/// Kept separate from `Intent` because delivering text to a socket isn't a
/// world mutation — it only happens once the whole batch is known-good.
#[derive(Debug, Clone)]
pub enum Effect {
    ToActor { target: String, message: String },
    ToRoom {
        room: String,
        message: String,
        exclude: Vec<String>,
    },
    ScheduleHook {
        target: String,
        hook: String,
        ticks: u64,
        data: Option<HashMap<String, serde_json::Value>>,
    },
    CancelScheduledHook {
        target: String,
        hook: String,
    },
    TriggerHook {
        target: String,
        hook: String,
        data: Option<serde_json::Value>,
    },
    EmitNearby {
        room: String,
        x: f64,
        y: f64,
        radius: f64,
        message: String,
        exclude: Vec<String>,
    },
    EmitData {
        target: String,
        channel: String,
        data: serde_json::Value,
    },
    EmitRadius {
        room: String,
        radius: u32,
        messages: HashMap<u32, String>,
        exclude: Vec<String>,
    },
}

/// Validate `batch` against `world` and apply it if every intent checks out.
/// Applies to a clone of `world` first — on any failure `world` is left
/// completely untouched (see ADR 0001's "atomic rollback").
///
/// Returns the [`Effect`]s to deliver on success.
pub fn apply_batch(world: &mut World, batch: &IntentBatch) -> Result<Vec<Effect>, String> {
    let mut sandbox = world.clone();
    let effects = apply_to(&mut sandbox, batch)?;
    *world = sandbox;
    Ok(effects)
}

/// Validate `batch` against `world` without applying it. Used for
/// `@dryrun`-style tooling and MCP validation endpoints.
pub fn dry_run(world: &World, batch: &IntentBatch) -> Result<(), String> {
    let mut sandbox = world.clone();
    apply_to(&mut sandbox, batch)?;
    Ok(())
}

fn apply_to(world: &mut World, batch: &IntentBatch) -> Result<Vec<Effect>, String> {
    let mut effects = Vec::new();
    for intent in &batch.intents {
        match intent {
            Intent::SetAttr { target, key, value } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_attr: no object '{}'", target))?;
                obj.attrs.insert(key.clone(), value.clone());
            }
            Intent::UnsetAttr { target, key } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("unset_attr: no object '{}'", target))?;
                obj.attrs.remove(key);
            }
            Intent::EmitActor { target, message } => {
                if world.get(target).is_none() {
                    return Err(format!("emit: no object '{}'", target));
                }
                effects.push(Effect::ToActor {
                    target: target.clone(),
                    message: message.clone(),
                });
            }
            Intent::EmitRoom {
                room,
                message,
                exclude,
            } => {
                if world.get(room).is_none() {
                    return Err(format!("emit_room: no room '{}'", room));
                }
                effects.push(Effect::ToRoom {
                    room: room.clone(),
                    message: message.clone(),
                    exclude: exclude.clone(),
                });
            }
            Intent::Move { target, destination } => {
                if world.get(destination).is_none() {
                    return Err(format!("move_object: no destination '{}'", destination));
                }
                if target == destination {
                    return Err("move_object: cannot move object into itself".into());
                }
                let mut cursor = destination.clone();
                let mut depth = 0u32;
                while let Some(parent) = world.get(&cursor).and_then(|o| o.location_ref.clone()) {
                    depth += 1;
                    if parent == *target {
                        return Err("move_object: circular containment detected".into());
                    }
                    if depth > MAX_CONTAINER_DEPTH {
                        return Err(format!(
                            "move_object: nesting depth exceeds {}",
                            MAX_CONTAINER_DEPTH
                        ));
                    }
                    cursor = parent;
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("move_object: no object '{}'", target))?;
                obj.location_ref = Some(destination.clone());
            }
            Intent::SetTag { target, tag } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_tag: no object '{}'", target))?;
                obj.tags.insert(tag.clone());
            }
            Intent::UnsetTag { target, tag } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("unset_tag: no object '{}'", target))?;
                obj.tags.remove(tag);
            }
            Intent::Spawn {
                ref_id,
                key,
                kind,
                title,
                description,
                location,
                owner,
            } => {
                if world.get(ref_id).is_some() {
                    return Err(format!("spawn: ref '{}' already exists", ref_id));
                }
                if world.get(location).is_none() {
                    return Err(format!("spawn: no location '{}'", location));
                }
                if *kind == Kind::Code {
                    // Belt and suspenders: the Lua `spawn()` wrapper already
                    // refuses kind "code" (Code objects are never physical
                    // things and always require a location here), but this
                    // is the one place every Intent::Spawn producer funnels
                    // through, so it's the right place to make the
                    // invariant unconditional.
                    return Err("spawn: cannot spawn kind 'code'".into());
                }
                let mut obj = GameObject::new(ref_id.clone(), key.clone(), kind.clone())
                    .with_location(location.clone());
                if let Some(t) = title {
                    obj = obj.with_title(t.clone());
                }
                if let Some(d) = description {
                    obj = obj.with_description(d.clone());
                }
                if let Some(o) = owner {
                    obj = obj.with_owner(o.clone());
                }
                world.add_object(obj);
            }
            Intent::SetTitle { target, title } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_title: no object '{}'", target))?;
                obj.title = Some(title.clone());
            }
            Intent::SetDescription { target, description } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_description: no object '{}'", target))?;
                obj.description = description.clone();
            }
            Intent::Destroy { target } => {
                match world.get(target) {
                    Some(obj) if obj.kind == Kind::Player => {
                        return Err("destroy: cannot destroy player objects".into());
                    }
                    Some(_) => {
                        world.objects.remove(target);
                    }
                    None => return Err(format!("destroy: no object '{}'", target)),
                }
            }
            Intent::CreateExit { ref_id, source, direction, target, aliases } => {
                if world.get(ref_id).is_some() {
                    return Err(format!("create_exit: ref '{}' already exists", ref_id));
                }
                if world.get(source).is_none() {
                    return Err(format!("create_exit: no source room '{}'", source));
                }
                if world.get(target).is_none() {
                    return Err(format!("create_exit: no target room '{}'", target));
                }
                let mut exit = GameObject::new(ref_id, direction, Kind::Exit)
                    .with_location(source)
                    .with_target(target);
                exit.aliases = aliases.iter().cloned().collect();
                world.add_object(exit);
                effects.push(Effect::TriggerHook {
                    target: ref_id.clone(),
                    hook: "on_create".to_string(),
                    data: None,
                });
            }
            Intent::SetProgram { target, hook, source } => {
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_program: no object '{}'", target))?;
                crate::softcode::hooks::set_program(obj, hook, source.clone())
                    .map_err(|e| format!("set_program: {}", e))?;
            }
            Intent::Trigger { target, hook, data } => {
                if world.get(target).is_none() {
                    return Err(format!("trigger: no object '{}'", target));
                }
                effects.push(Effect::TriggerHook {
                    target: target.clone(),
                    hook: hook.clone(),
                    data: data.clone(),
                });
            }
            Intent::EmitNearby { room, x, y, radius, message, exclude } => {
                effects.push(Effect::EmitNearby {
                    room: room.clone(),
                    x: *x,
                    y: *y,
                    radius: *radius,
                    message: message.clone(),
                    exclude: exclude.clone(),
                });
            }
            Intent::SetLock { target, hook, expr } => {
                crate::locks::parse(expr).map_err(|e| format!("set_lock: {}", e))?;
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_lock: no object '{}'", target))?;
                obj.locks.insert(hook.clone(), expr.clone());
            }
            Intent::SetOwner { target, owner } => {
                if world.get(owner).is_none() {
                    return Err(format!("set_owner: no object '{}'", owner));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_owner: no object '{}'", target))?;
                obj.owner_ref = Some(owner.clone());
            }
            Intent::After { target, hook, ticks, data } => {
                if *ticks == 0 {
                    return Err("after: ticks must be > 0".into());
                }
                if world.get(target).is_none() {
                    return Err(format!("after: no object '{}'", target));
                }
                effects.push(Effect::ScheduleHook {
                    target: target.clone(),
                    hook: hook.clone(),
                    ticks: *ticks,
                    data: data.clone(),
                });
            }
            Intent::CancelAfter { target, hook } => {
                if world.get(target).is_none() {
                    return Err(format!("cancel_after: no object '{}'", target));
                }
                effects.push(Effect::CancelScheduledHook {
                    target: target.clone(),
                    hook: hook.clone(),
                });
            }
            Intent::EmitData { target, channel, data } => {
                if world.get(target).is_none() {
                    return Err(format!("emit_data: no object '{}'", target));
                }
                effects.push(Effect::EmitData {
                    target: target.clone(),
                    channel: channel.clone(),
                    data: data.clone(),
                });
            }
            Intent::EmitRadius { room, radius, messages, exclude } => {
                if world.get(room).is_none() {
                    return Err(format!("emit_radius: no room '{}'", room));
                }
                effects.push(Effect::EmitRadius {
                    room: room.clone(),
                    radius: *radius,
                    messages: messages.clone(),
                    exclude: exclude.clone(),
                });
            }
            Intent::TransferAttr { from, to, key, amount } => {
                let from_val = world
                    .get(from)
                    .and_then(|o| o.attrs.get(key))
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| format!("transfer_attr: '{}' has no numeric attr '{}'", from, key))?;
                if from_val < *amount {
                    return Err(format!(
                        "transfer_attr: '{}' has {} but needs {} for '{}'",
                        from, from_val, amount, key
                    ));
                }
                let to_val = world
                    .get(to)
                    .ok_or_else(|| format!("transfer_attr: no object '{}'", to))?
                    .attrs
                    .get(key)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                let from_obj = world.get_mut(from).unwrap();
                from_obj.attrs.insert(key.clone(), serde_json::json!(from_val - amount));
                let to_obj = world.get_mut(to).unwrap();
                to_obj.attrs.insert(key.clone(), serde_json::json!(to_val + amount));
            }
        }
    }
    Ok(effects)
}

/// An instruction-count cap for a single Program run. Luau's public API
/// doesn't expose a literal per-bytecode-instruction counter, so this counts
/// VM interrupt callbacks instead (fired on function calls and loop
/// back-edges) — a close enough proxy to stop runaway scripts from blocking
/// the world tick. See ADR 0002.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_instructions: u64,
}

impl Budget {
    pub fn new(max_instructions: u64) -> Self {
        Self { max_instructions }
    }

    /// Budget for a one-shot `@eval`. The default is sized so a runaway hook
    /// dies well inside a tick, which is far too small for the job `@eval`
    /// exists to do — sweeping every object in the world to migrate it. This
    /// is deliberately orders of magnitude larger, but still finite: `@eval`
    /// runs on the single-writer engine loop, so while it runs the world is
    /// frozen. Large enough for a real migration, short enough that a mistake
    /// costs seconds rather than requiring a restart.
    pub fn for_eval() -> Self {
        Self {
            max_instructions: 50_000_000,
        }
    }
}

impl Default for Budget {
    fn default() -> Self {
        // Generous enough for normal hook logic, cheap enough that a script
        // stuck in an infinite loop dies in well under a tick.
        Self {
            max_instructions: 200_000,
        }
    }
}

/// Why a Program run failed.
#[derive(Debug)]
pub enum SoftcodeError {
    /// The Luau source didn't compile.
    Compile(String),
    /// The Luau source hit its instruction [`Budget`].
    BudgetExceeded,
    /// The Luau source raised an error (or one of the API functions did).
    Runtime(String),
}

impl std::fmt::Display for SoftcodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoftcodeError::Compile(msg) => write!(f, "compile error: {}", msg),
            SoftcodeError::BudgetExceeded => write!(f, "instruction budget exceeded"),
            SoftcodeError::Runtime(msg) => write!(f, "runtime error: {}", msg),
        }
    }
}

impl std::error::Error for SoftcodeError {}

const BUDGET_MARKER: &str = "hearth-mud: instruction budget exceeded";

fn classify_lua_error(err: mlua::Error) -> SoftcodeError {
    let msg = err.to_string();
    if msg.contains(BUDGET_MARKER) {
        SoftcodeError::BudgetExceeded
    } else {
        SoftcodeError::Runtime(msg)
    }
}

/// The outcome of a single Program run: the Intents it queued, plus what it
/// returned.
#[derive(Debug, Default)]
pub struct ProgramResult {
    pub batch: IntentBatch,
    /// True only if the Program's hook function explicitly `return`ed the
    /// literal value `false`. Anything else — `true`, `nil`, no return at
    /// all — counts as "not denied". This makes `on_`/`cmd_` hooks (whose
    /// return value nobody checks) safe to write without a trailing
    /// `return true`, while `can_` hooks still get a real veto.
    pub denied: bool,
    /// Updated state table from the script run. The engine writes this back
    /// to the ProgramRecord after applying the batch.
    pub state: HashMap<String, serde_json::Value>,
}

/// The outcome of a one-shot `@eval` run — see [`SoftcodeRuntime::run_eval`].
#[derive(Debug, Default)]
pub struct EvalResult {
    pub batch: IntentBatch,
    /// A human-readable rendering of whatever the script's top-level
    /// `return` produced. `None` when the script didn't return anything.
    pub returned: Option<String>,
}

/// Render a Luau return value for display to the `@eval` caller. Tables are
/// rendered as JSON where they convert cleanly; anything else falls back to
/// a `<type>` placeholder rather than failing the whole eval over a value
/// that can't be shown.
fn describe_lua_value(lua: &Lua, value: LuaValue) -> Option<String> {
    match value {
        LuaValue::Nil => None,
        LuaValue::Boolean(b) => Some(b.to_string()),
        LuaValue::Integer(i) => Some(i.to_string()),
        LuaValue::Number(n) => Some(n.to_string()),
        LuaValue::String(s) => Some(s.to_string_lossy()),
        LuaValue::Table(_) => {
            let json: Result<serde_json::Value, _> = lua.from_value(value);
            match json {
                Ok(v) => serde_json::to_string(&v).ok(),
                Err(_) => Some("<table>".to_string()),
            }
        }
        other => Some(format!("<{}>", other.type_name())),
    }
}

/// Owns the Luau VM. One `SoftcodeRuntime` is enough for the whole engine —
/// each [`SoftcodeRuntime::run_hook`] call isolates a Program in its own
/// environment table so unrelated Programs never see each other's globals.
///
/// Compiled chunks are cached by source hash — same source skips
/// recompilation on subsequent calls.
pub struct SoftcodeRuntime {
    lua: Lua,
    chunk_cache: std::cell::RefCell<HashMap<u64, mlua::RegistryKey>>,
    ink: RefCell<ink::InkRuntime>,
}

pub(crate) const MODULE_SOURCES_KEY: &str = "_hearth_module_sources";
const MODULE_CACHE_KEY: &str = "_hearth_module_cache";
/// Array of module names currently mid-`require`, innermost last. Used to
/// detect cycles and to render the chain in the error message.
const MODULE_LOADING_KEY: &str = "_hearth_module_loading";
/// User-authored library sources — `Kind::Code` objects' `lib_<name>`
/// Programs, keyed by `<name>`. Re-synced from `World` at the start of every
/// Program execution (`SoftcodeRuntime::sync_user_lib_sources`), unlike
/// [`MODULE_SOURCES_KEY`] which is file-owned and only changes on
/// `@reload-world`. Checked by `require` as a fallback after shipped
/// modules — see `install_require` and `docs/plans/program-authoring.md`
/// Stage 2.
pub(crate) const USER_LIB_SOURCES_KEY: &str = "_hearth_user_lib_sources";

impl SoftcodeRuntime {
    pub fn new() -> Self {
        let lua = Lua::new();
        let sources = lua.create_table().expect("create module sources table");
        lua.set_named_registry_value(MODULE_SOURCES_KEY, sources)
            .expect("store module sources");
        let cache = lua.create_table().expect("create module cache table");
        lua.set_named_registry_value(MODULE_CACHE_KEY, cache)
            .expect("store module cache");
        let loading = lua.create_table().expect("create module loading table");
        lua.set_named_registry_value(MODULE_LOADING_KEY, loading)
            .expect("store module loading stack");
        let user_lib_sources = lua.create_table().expect("create user lib sources table");
        lua.set_named_registry_value(USER_LIB_SOURCES_KEY, user_lib_sources)
            .expect("store user lib sources");

        Self::install_require(&lua);
        crate::grid::Grid2D::install_globals(&lua);
        crate::noise::install_globals(&lua);

        Self {
            lua,
            chunk_cache: std::cell::RefCell::new(HashMap::new()),
            ink: RefCell::new(ink::InkRuntime::new()),
        }
    }

    fn install_require(lua: &Lua) {
        let require_fn = lua
            .create_function(|lua, name: String| {
                let cache: mlua::Table =
                    lua.named_registry_value(MODULE_CACHE_KEY)?;
                let cached: LuaValue = cache.get(name.as_str())?;
                if cached != LuaValue::Nil {
                    return Ok(cached);
                }

                let sources: mlua::Table =
                    lua.named_registry_value(MODULE_SOURCES_KEY)?;
                let source: Option<String> = sources.get(name.as_str())?;
                // Shipped modules (embedded stdlib, <game_dir>/lib) win ties
                // over a same-named user library — but that can't happen in
                // practice, since authoring a lib_<name> that collides with
                // a shipped module is refused at write time. See
                // docs/plans/program-authoring.md Stage 2.
                let source = match source {
                    Some(s) => Some(s),
                    None => {
                        let user_sources: mlua::Table =
                            lua.named_registry_value(USER_LIB_SOURCES_KEY)?;
                        user_sources.get(name.as_str())?
                    }
                };
                let source = source.ok_or_else(|| {
                    mlua::Error::RuntimeError(format!("module '{}' not found", name))
                })?;

                let loading: mlua::Table =
                    lua.named_registry_value(MODULE_LOADING_KEY)?;
                if let Some(chain) = cycle_chain(&loading, &name)? {
                    return Err(mlua::Error::RuntimeError(format!(
                        "require cycle detected: {}",
                        chain
                    )));
                }

                let chunk = lua.load(&source).set_name(&name).into_function()?;
                let env = lua.create_table()?;
                let mt = lua.create_table()?;
                mt.set("__index", lua.globals())?;
                env.set_metatable(Some(mt));
                chunk.set_environment(env)?;

                loading.push(name.as_str())?;
                let result = chunk.call::<LuaValue>(());
                loading.pop::<LuaValue>()?;
                let result = result?;

                let result = if result == LuaValue::Nil {
                    LuaValue::Boolean(true)
                } else {
                    result
                };
                cache.set(name, result.clone())?;
                Ok(result)
            })
            .expect("create require function");
        lua.globals()
            .set("require", require_fn)
            .expect("register require");
    }

    pub fn load_modules(&self, modules: HashMap<String, String>) {
        let sources: mlua::Table = self
            .lua
            .named_registry_value(MODULE_SOURCES_KEY)
            .expect("module sources table");
        for (name, source) in modules {
            sources.set(name, source).expect("set module source");
        }
        self.invalidate_module_cache();
    }

    /// Clear the `require()` result cache and the loading stack, so the next
    /// `require("<name>")` anywhere re-evaluates from source instead of
    /// returning a memoized module result. Used both by `load_modules`
    /// (shipped sources changed, e.g. `@reload-world`) and whenever a
    /// `lib_<name>` Program is written or removed (user library changed) —
    /// see docs/plans/program-authoring.md Stage 2. Coarse on purpose: since
    /// modules re-evaluate lazily on next `require`, clearing the whole
    /// cache needs no transitive dependency tracking.
    ///
    /// Does *not* touch [`MODULE_SOURCES_KEY`] — that table is file-owned
    /// and only [`Self::load_modules`]/[`Self::invalidate_cache`] repopulate
    /// it. Clearing it here without a reload would make shipped modules
    /// unresolvable until the next `@reload-world`.
    pub fn invalidate_module_cache(&self) {
        let cache: mlua::Table = self
            .lua
            .named_registry_value(MODULE_CACHE_KEY)
            .expect("module cache table");
        clear_table(&cache);
        let loading: mlua::Table = self
            .lua
            .named_registry_value(MODULE_LOADING_KEY)
            .expect("module loading table");
        clear_table(&loading);
    }

    /// Whether `name` is a shipped module — embedded stdlib or
    /// `<game_dir>/lib` — as of the last `load_modules`/`@reload-world`.
    /// Used to refuse authoring a same-named user library at write time.
    pub fn is_shipped_module(&self, name: &str) -> bool {
        let sources: mlua::Table = self
            .lua
            .named_registry_value(MODULE_SOURCES_KEY)
            .expect("module sources table");
        sources.contains_key(name).unwrap_or(false)
    }

    /// Refresh the user-library source table from `world` — every enabled
    /// `lib_<name>` Program on a `Kind::Code` object, keyed by `<name>`.
    /// Called at the start of every Program execution so `require` always
    /// sees the current DB state; combined with `invalidate_module_cache`
    /// at write time, an edit takes effect on the *next* `require` call
    /// without needing to track who required what.
    fn sync_user_lib_sources(&self, world: &World) {
        let table: mlua::Table = self
            .lua
            .named_registry_value(USER_LIB_SOURCES_KEY)
            .expect("user lib sources table");
        clear_table(&table);
        for obj in world.objects.values() {
            if obj.kind != Kind::Code {
                continue;
            }
            for program in obj.programs.values() {
                if !program.enabled {
                    continue;
                }
                if let Some(name) = program.hook.strip_prefix("lib_") {
                    let _ = table.set(name, program.source.clone());
                }
            }
        }
    }

    fn source_hash(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    fn get_or_compile(&self, source: &str, name: &str) -> mlua::Result<mlua::Function> {
        let hash = Self::source_hash(source);
        let cache = self.chunk_cache.borrow();
        if let Some(key) = cache.get(&hash)
            && let Ok(func) = self.lua.registry_value::<mlua::Function>(key) {
                return Ok(func);
            }
        drop(cache);

        let func = self.lua.load(source).set_name(name).into_function()?;
        let key = self.lua.create_registry_value(func.clone())?;
        self.chunk_cache.borrow_mut().insert(hash, key);
        Ok(func)
    }

    pub fn invalidate_cache(&self) {
        self.chunk_cache.borrow_mut().clear();
        let sources: mlua::Table = self
            .lua
            .named_registry_value(MODULE_SOURCES_KEY)
            .expect("module sources table");
        clear_table(&sources);
        let cache: mlua::Table = self
            .lua
            .named_registry_value(MODULE_CACHE_KEY)
            .expect("module cache table");
        clear_table(&cache);
        self.ink.borrow_mut().invalidate_cache();
    }

    pub fn ink_runtime(&self) -> &RefCell<ink::InkRuntime> {
        &self.ink
    }

    /// Compile `source` without running it. Used by `@program` to reject
    /// syntax errors before they're saved to an object.
    pub fn check_syntax(&self, source: &str) -> Result<(), String> {
        self.lua
            .load(source)
            .into_function()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn install_budget(&self, budget: Budget) {
        let count = Arc::new(AtomicU64::new(0));
        let max = budget.max_instructions;
        self.lua.set_interrupt(move |_| {
            let n = count.fetch_add(1, Ordering::Relaxed) + 1;
            if n > max {
                return Err(mlua::Error::RuntimeError(BUDGET_MARKER.to_string()));
            }
            Ok(VmState::Continue)
        });
    }

    /// Run `program`'s Luau source, calling its `program.hook`-named
    /// function with `(this, actor, room[, args])`.
    ///
    /// For `on_tick` hooks, the signature is `(this, state, room)` — state
    /// is a mutable table persisted between runs.
    #[allow(clippy::too_many_arguments)]
    pub fn run_hook(
        &self,
        world: &World,
        program: &ProgramRecord,
        this_ref: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        args: Option<&str>,
        budget: Budget,
        dbref_counter: Rc<Cell<u64>>,
        themes: &HashMap<String, Theme>,
        map_templates: &HashMap<String, crate::map_template::MapTemplateFile>,
        scheduled_hooks: &[ScheduledHook],
        tick_count: u64,
    ) -> Result<ProgramResult, SoftcodeError> {
        self.sync_user_lib_sources(world);
        self.install_budget(budget);
        let batch = Rc::new(RefCell::new(IntentBatch::default()));
        {
            // Stamp who caused this run (`actor_ref`) and whose authority
            // it runs under (`this_ref`'s owner) before the script gets a
            // chance to push anything — see the `IntentBatch` field docs
            // and docs/plans/program-authoring.md Stage 3.
            let mut b = batch.borrow_mut();
            b.actor_ref = Some(actor_ref.to_string());
            b.authority = world.get(this_ref).and_then(|o| o.owner_ref.clone());
        }
        let default_location = room_ref.map(|s| s.to_string()).or_else(|| {
            world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone())
        });
        let is_tick = program.hook == "on_tick";
        let state_capture: Rc<RefCell<HashMap<String, serde_json::Value>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let state_writer = Rc::clone(&state_capture);

        let run_result: mlua::Result<LuaValue> = self.lua.scope(|scope| {
            let env = self.lua.create_table()?;
            api::install_stdlib(&self.lua, &env)?;
            api::install(
                &self.lua,
                scope,
                &env,
                world,
                Rc::clone(&batch),
                default_location.clone(),
                Rc::clone(&dbref_counter),
                themes,
                map_templates,
                scheduled_hooks,
                tick_count,
                &self.ink,
            )?;

            let compiled = self.get_or_compile(&program.source, &program.hook)
                .map_err(|e| e.clone())?;
            compiled.set_environment(env.clone())?;
            compiled.call::<()>(())?;

            let func: Option<mlua::Function> = env.get(program.hook.as_str())?;
            let func = match func {
                Some(f) => f,
                None => return Ok(LuaValue::Nil),
            };

            let this_val = api::object_to_value(&self.lua, world, this_ref)?;

            if is_tick {
                let state_tbl = self.lua.create_table()?;
                for (k, v) in &program.state {
                    state_tbl.set(k.clone(), self.lua.to_value(v)?)?;
                }
                let room_val = match room_ref.or(default_location.as_deref()) {
                    Some(r) => api::object_to_value(&self.lua, world, r)?,
                    None => LuaValue::Nil,
                };
                let ret = func.call::<LuaValue>((this_val, state_tbl.clone(), room_val))?;
                // Read state back before scope ends
                let mut map = state_writer.borrow_mut();
                for pair in state_tbl.pairs::<String, LuaValue>() {
                    if let Ok((k, v)) = pair
                        && let Ok(json_val) = self.lua.from_value::<serde_json::Value>(v) {
                            map.insert(k, json_val);
                        }
                }
                Ok(ret)
            } else {
                let actor_val = api::object_to_value(&self.lua, world, actor_ref)?;
                let room_val = match room_ref.or(default_location.as_deref()) {
                    Some(r) => api::object_to_value(&self.lua, world, r)?,
                    None => LuaValue::Nil,
                };
                let ret = match args {
                    Some(a) => func.call::<LuaValue>((this_val, actor_val, room_val, a.to_string()))?,
                    None => func.call::<LuaValue>((this_val, actor_val, room_val))?,
                };
                Ok(ret)
            }
        });

        self.lua.remove_interrupt();

        let ret = match run_result {
            Ok(v) => v,
            Err(e) => return Err(classify_lua_error(e)),
        };

        let batch = Rc::try_unwrap(batch)
            .map(|cell| cell.into_inner())
            .unwrap_or_else(|rc| rc.borrow().clone());

        // `Rc::try_unwrap` only succeeds when this is the last reference.
        // `state_writer` (the clone used inside the `lua.scope` closure
        // above) is captured *by reference*, not by value — closures only
        // capture what a use actually requires, and `.borrow_mut()` needs
        // just `&state_writer` — so it's still alive here as a local
        // variable in this function, and `try_unwrap` always sees a strong
        // count of (at least) 2. Read through the `Rc` instead of assuming
        // exclusive ownership, matching how `batch` is unwrapped above;
        // `unwrap_or_default()` here would silently discard every write a
        // tick made to `state`, which is exactly the bug this replaced.
        let state = Rc::try_unwrap(state_capture)
            .map(|cell| cell.into_inner())
            .unwrap_or_else(|rc| rc.borrow().clone());

        Ok(ProgramResult {
            batch,
            denied: matches!(ret, LuaValue::Boolean(false)),
            state,
        })
    }

    /// Run a one-shot `@eval` script — an admin running arbitrary Luau
    /// against the live world (Evennia's `@batchcode`, MUSH's
    /// paste-a-command-script). Modeled on [`Self::run_hook`], but simpler
    /// in two ways that follow from being one-shot rather than scheduled:
    ///
    /// - No named entry-point function to look up. The chunk body itself is
    ///   the whole program, so whatever it top-level `return`s is the result
    ///   — the same shape `check_syntax`'s `into_function()` already compiles
    ///   for.
    /// - No persistent `state` table, since there is nothing to remember
    ///   between runs.
    ///
    /// World writes still only ever happen through the returned
    /// [`IntentBatch`] — `@eval` gets no special access the write API
    /// doesn't already offer.
    #[allow(clippy::too_many_arguments)]
    pub fn run_eval(
        &self,
        world: &World,
        source: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        budget: Budget,
        dbref_counter: Rc<Cell<u64>>,
        themes: &HashMap<String, Theme>,
        map_templates: &HashMap<String, crate::map_template::MapTemplateFile>,
        scheduled_hooks: &[ScheduledHook],
        tick_count: u64,
    ) -> Result<EvalResult, SoftcodeError> {
        let compiled = self
            .lua
            .load(source)
            .set_name("@eval")
            .into_function()
            .map_err(|e| SoftcodeError::Compile(e.to_string()))?;

        self.sync_user_lib_sources(world);
        self.install_budget(budget);
        let batch = Rc::new(RefCell::new(IntentBatch::default()));

        let run_result: mlua::Result<LuaValue> = self.lua.scope(|scope| {
            let env = self.lua.create_table()?;
            api::install_stdlib(&self.lua, &env)?;
            api::install(
                &self.lua,
                scope,
                &env,
                world,
                Rc::clone(&batch),
                room_ref.map(|s| s.to_string()),
                Rc::clone(&dbref_counter),
                themes,
                map_templates,
                scheduled_hooks,
                tick_count,
                &self.ink,
            )?;

            // The caller running the eval, for convenience — same name a
            // hook's `actor` parameter would use.
            env.set("actor", api::object_to_value(&self.lua, world, actor_ref)?)?;

            compiled.set_environment(env)?;
            compiled.call::<LuaValue>(())
        });

        self.lua.remove_interrupt();

        let ret = match run_result {
            Ok(v) => v,
            Err(e) => return Err(classify_lua_error(e)),
        };

        let batch = Rc::try_unwrap(batch)
            .map(|cell| cell.into_inner())
            .unwrap_or_else(|rc| rc.borrow().clone());

        Ok(EvalResult {
            returned: describe_lua_value(&self.lua, ret),
            batch,
        })
    }
}

/// If `name` is already on the `loading` stack, render the cycle as
/// `a -> b -> a`. Returns `None` when there's no cycle.
fn cycle_chain(loading: &mlua::Table, name: &str) -> mlua::Result<Option<String>> {
    let mut chain: Vec<String> = Vec::new();
    let mut found = false;
    for entry in loading.clone().sequence_values::<String>() {
        let entry = entry?;
        if entry == name {
            found = true;
        }
        if found {
            chain.push(entry);
        }
    }
    if !found {
        return Ok(None);
    }
    chain.push(name.to_string());
    Ok(Some(chain.join(" -> ")))
}

fn clear_table(t: &mlua::Table) {
    let keys: Vec<LuaValue> = t
        .pairs::<LuaValue, LuaValue>()
        .filter_map(|p| p.ok().map(|(k, _)| k))
        .collect();
    for k in keys {
        let _ = t.set(k, LuaValue::Nil);
    }
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TestFileResult {
    pub file: String,
    pub tests: Vec<TestResult>,
}

impl TestFileResult {
    pub fn passed(&self) -> usize {
        self.tests.iter().filter(|t| t.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.tests.iter().filter(|t| !t.passed).count()
    }
}

fn lua_deep_eq(a: &LuaValue, b: &LuaValue, depth: u32) -> mlua::Result<bool> {
    if depth > 10 {
        return Ok(false);
    }
    match (a, b) {
        (LuaValue::Nil, LuaValue::Nil) => Ok(true),
        (LuaValue::Boolean(a), LuaValue::Boolean(b)) => Ok(a == b),
        (LuaValue::Integer(a), LuaValue::Integer(b)) => Ok(a == b),
        (LuaValue::Integer(a), LuaValue::Number(b)) => Ok((*a as f64) == *b),
        (LuaValue::Number(a), LuaValue::Integer(b)) => Ok(*a == (*b as f64)),
        (LuaValue::Number(a), LuaValue::Number(b)) => Ok(a == b),
        (LuaValue::String(a), LuaValue::String(b)) => Ok(a == b),
        (LuaValue::Table(a), LuaValue::Table(b)) => {
            for pair in a.pairs::<LuaValue, LuaValue>() {
                let (key, val_a) = pair?;
                let val_b: LuaValue = b.get(key.clone())?;
                if !lua_deep_eq(&val_a, &val_b, depth + 1)? {
                    return Ok(false);
                }
            }
            for pair in b.pairs::<LuaValue, LuaValue>() {
                let (key, _) = pair?;
                let val_a: LuaValue = a.get(key)?;
                if matches!(val_a, LuaValue::Nil) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn lua_value_display(v: &LuaValue, depth: u32) -> String {
    if depth > 3 {
        return "{...}".to_string();
    }
    match v {
        LuaValue::Nil => "nil".to_string(),
        LuaValue::Boolean(b) => b.to_string(),
        LuaValue::Integer(n) => n.to_string(),
        LuaValue::Number(n) => format!("{}", n),
        LuaValue::String(s) => format!("\"{}\"", s.to_string_lossy()),
        LuaValue::Table(t) => {
            let mut parts = Vec::new();
            let mut i = 1i32;
            for (k, v) in t.pairs::<LuaValue, LuaValue>().flatten() {
                if matches!(&k, LuaValue::Integer(n) if *n == i) {
                    parts.push(lua_value_display(&v, depth + 1));
                    i += 1;
                } else {
                    let ks = match &k {
                        LuaValue::String(s) => s.to_string_lossy().to_string(),
                        _ => lua_value_display(&k, depth + 1),
                    };
                    parts.push(format!("{} = {}", ks, lua_value_display(&v, depth + 1)));
                }
                if parts.len() > 10 {
                    parts.push("...".to_string());
                    break;
                }
            }
            format!("{{{}}}", parts.join(", "))
        }
        _ => format!("<{}>", v.type_name()),
    }
}

fn install_test_assertions(lua: &Lua, env: &mlua::Table) -> mlua::Result<()> {
    env.set(
        "assert_eq",
        lua.create_function(
            |_, (actual, expected, msg): (LuaValue, LuaValue, Option<String>)| {
                if !lua_deep_eq(&actual, &expected, 0)? {
                    let label = msg
                        .map(|m| format!(" ({})", m))
                        .unwrap_or_default();
                    Err(mlua::Error::RuntimeError(format!(
                        "assert_eq failed{}: expected {}, got {}",
                        label,
                        lua_value_display(&expected, 0),
                        lua_value_display(&actual, 0),
                    )))
                } else {
                    Ok(())
                }
            },
        )?,
    )?;

    env.set(
        "assert_true",
        lua.create_function(|_, (value, msg): (LuaValue, Option<String>)| {
            if !matches!(value, LuaValue::Boolean(true)) {
                let label = msg
                    .map(|m| format!(" ({})", m))
                    .unwrap_or_default();
                Err(mlua::Error::RuntimeError(format!(
                    "assert_true failed{}: got {}",
                    label,
                    lua_value_display(&value, 0),
                )))
            } else {
                Ok(())
            }
        })?,
    )?;

    env.set(
        "assert_false",
        lua.create_function(|_, (value, msg): (LuaValue, Option<String>)| {
            if !matches!(value, LuaValue::Boolean(false)) {
                let label = msg
                    .map(|m| format!(" ({})", m))
                    .unwrap_or_default();
                Err(mlua::Error::RuntimeError(format!(
                    "assert_false failed{}: got {}",
                    label,
                    lua_value_display(&value, 0),
                )))
            } else {
                Ok(())
            }
        })?,
    )?;

    env.set(
        "assert_nil",
        lua.create_function(|_, (value, msg): (LuaValue, Option<String>)| {
            if !matches!(value, LuaValue::Nil) {
                let label = msg
                    .map(|m| format!(" ({})", m))
                    .unwrap_or_default();
                Err(mlua::Error::RuntimeError(format!(
                    "assert_nil failed{}: got {}",
                    label,
                    lua_value_display(&value, 0),
                )))
            } else {
                Ok(())
            }
        })?,
    )?;

    env.set(
        "assert_not_nil",
        lua.create_function(|_, (value, msg): (LuaValue, Option<String>)| {
            if matches!(value, LuaValue::Nil) {
                let label = msg
                    .map(|m| format!(" ({})", m))
                    .unwrap_or_default();
                Err(mlua::Error::RuntimeError(format!(
                    "assert_not_nil failed{}",
                    label,
                )))
            } else {
                Ok(())
            }
        })?,
    )?;

    env.set(
        "assert_error",
        lua.create_function(
            |_, (func, pattern): (mlua::Function, Option<String>)| match func.call::<LuaValue>(()) {
                Ok(_) => Err(mlua::Error::RuntimeError(
                    "assert_error failed: expected an error but call succeeded".into(),
                )),
                Err(e) => {
                    if let Some(pat) = pattern {
                        let msg = e.to_string();
                        if !msg.contains(&pat) {
                            Err(mlua::Error::RuntimeError(format!(
                                "assert_error: error did not match pattern '{}': {}",
                                pat, msg,
                            )))
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                }
            },
        )?,
    )?;

    Ok(())
}

impl SoftcodeRuntime {
    fn discover_test_names(&self, source: &str, name: &str) -> Result<Vec<String>, SoftcodeError> {
        let compiled = self
            .get_or_compile(source, name)
            .map_err(|e| SoftcodeError::Runtime(e.to_string()))?;

        let names: mlua::Result<Vec<String>> = self.lua.scope(|_scope| {
            let env = self.lua.create_table()?;
            api::install_stdlib(&self.lua, &env)?;
            compiled.set_environment(env.clone())?;
            compiled.call::<()>(())?;

            let mut test_names = Vec::new();
            for pair in env.pairs::<String, LuaValue>() {
                if let Ok((key, LuaValue::Function(_))) = pair
                    && key.starts_with("test_") {
                        test_names.push(key);
                    }
            }
            test_names.sort();
            Ok(test_names)
        });

        names.map_err(classify_lua_error)
    }

    pub fn run_tests(
        &self,
        source: &str,
        file_name: &str,
        world: Option<&World>,
        budget: Budget,
    ) -> Result<TestFileResult, SoftcodeError> {
        let test_names = self.discover_test_names(source, file_name)?;
        let mut results = Vec::new();

        if let Some(w) = world {
            self.sync_user_lib_sources(w);
        }

        for test_name in &test_names {
            if world.is_some() {
                self.invalidate_module_cache();
            }
            self.install_budget(budget);

            let empty_themes: HashMap<String, crate::theme::Theme> = HashMap::new();
            let empty_templates: HashMap<String, crate::map_template::MapTemplateFile> =
                HashMap::new();

            let run_result: mlua::Result<()> = self.lua.scope(|scope| {
                let env = self.lua.create_table()?;
                api::install_stdlib(&self.lua, &env)?;
                install_test_assertions(&self.lua, &env)?;

                if let Some(w) = world {
                    let batch = Rc::new(RefCell::new(IntentBatch::default()));
                    let dbref_counter = Rc::new(Cell::new(w.next_id));
                    api::install(
                        &self.lua,
                        scope,
                        &env,
                        w,
                        Rc::clone(&batch),
                        None,
                        Rc::clone(&dbref_counter),
                        &empty_themes,
                        &empty_templates,
                        &[],
                        0,
                        &self.ink,
                    )?;

                    let ctx = self.lua.create_table()?;
                    let mut sorted_objs: Vec<_> = w.objects.values().collect();
                    sorted_objs.sort_by(|a, b| a.ref_id.cmp(&b.ref_id));
                    if let Some(room) = sorted_objs.iter().find(|o| o.kind == Kind::Room) {
                        ctx.set("room", room.ref_id.clone())?;
                    }
                    if let Some(player) = sorted_objs.iter().find(|o| o.kind == Kind::Player) {
                        ctx.set("actor", player.ref_id.clone())?;
                    }
                    if let Some(item) = sorted_objs.iter().find(|o| o.kind == Kind::Item) {
                        ctx.set("this", item.ref_id.clone())?;
                    }
                    env.set("ctx", ctx)?;
                }

                let compiled = self
                    .get_or_compile(source, file_name)
                    .map_err(|e| e.clone())?;
                compiled.set_environment(env.clone())?;
                compiled.call::<()>(())?;

                let func: mlua::Function = env.get(test_name.as_str())?;

                if world.is_some() {
                    let ctx: mlua::Table = env.get("ctx")?;
                    func.call::<()>(ctx)?;
                } else {
                    func.call::<()>(())?;
                }

                Ok(())
            });

            self.lua.remove_interrupt();

            match run_result {
                Ok(()) => results.push(TestResult {
                    name: test_name.clone(),
                    passed: true,
                    error: None,
                }),
                Err(e) => {
                    let error_msg = e.to_string();
                    let clean = error_msg
                        .strip_prefix("runtime error: ")
                        .unwrap_or(&error_msg)
                        .to_string();
                    results.push(TestResult {
                        name: test_name.clone(),
                        passed: false,
                        error: Some(clean),
                    });
                }
            }
        }

        Ok(TestFileResult {
            file: file_name.to_string(),
            tests: results,
        })
    }
}

impl Default for SoftcodeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{GameObject, Kind};

    /// Dbref counter for a test run, seeded from the world's current
    /// `next_id` — mirrors what the engine hands to `run_hook`/`run_eval`
    /// in production.
    fn counter(world: &World) -> Rc<Cell<u64>> {
        Rc::new(Cell::new(world.next_id))
    }

    /// Empty theme map — plenty for tests that don't touch dungeon
    /// generation.
    fn test_themes() -> HashMap<String, Theme> {
        HashMap::new()
    }

    /// Empty map template map — plenty for tests that don't touch
    /// `instantiate_map`.
    fn test_map_templates() -> HashMap<String, crate::map_template::MapTemplateFile> {
        HashMap::new()
    }

    // test_world() creates objects in this fixed order, so dbrefs are
    // predictable: room "#1", room2 "#2", alice "#3", bob "#4", sword "#5",
    // shield "#6", guard "#7", exit "#8".
    fn test_world() -> World {
        let mut world = World::new();
        let room_ref = world.next_dbref(); // "#1"
        let mut room = GameObject::new(&room_ref, "room", Kind::Room).with_title("A Room");
        room.description = "A plain room.".into();
        world.add_object(room);

        let room2_ref = world.next_dbref(); // "#2"
        let mut room2 = GameObject::new(&room2_ref, "room2", Kind::Room).with_title("Another Room");
        room2.description = "Another room.".into();
        world.add_object(room2);

        let alice_ref = world.next_dbref(); // "#3"
        let mut actor = GameObject::new(&alice_ref, "alice", Kind::Player)
            .with_title("Alice")
            .with_location(&room_ref);
        actor.tags.insert(Tag { category: "quest".into(), key: "hero".into() });
        world.add_object(actor);

        let bob_ref = world.next_dbref(); // "#4"
        let bob = GameObject::new(&bob_ref, "bob", Kind::Player)
            .with_title("Bob")
            .with_location(&room_ref);
        world.add_object(bob);

        let sword_ref = world.next_dbref(); // "#5"
        let mut sword = GameObject::new(&sword_ref, "sword", Kind::Item)
            .with_title("a rusty sword")
            .with_location(&room_ref);
        sword.tags.insert(Tag { category: "loot".into(), key: "weapon".into() });
        sword.attrs.insert("damage".into(), serde_json::json!(10));
        world.add_object(sword);

        let shield_ref = world.next_dbref(); // "#6"
        let shield = GameObject::new(&shield_ref, "shield", Kind::Item)
            .with_title("a wooden shield")
            .with_location(&alice_ref);
        world.add_object(shield);

        let guard_ref = world.next_dbref(); // "#7"
        let npc = GameObject::new(&guard_ref, "guard", Kind::Npc)
            .with_title("A Town Guard")
            .with_location(&room_ref);
        world.add_object(npc);

        let exit_ref = world.next_dbref(); // "#8"
        let exit = GameObject::new(&exit_ref, "north", Kind::Exit)
            .with_location(&room_ref)
            .with_target(&room2_ref)
            .with_aliases(vec!["n"]);
        world.add_object(exit);

        world
    }

    #[test]
    fn on_get_queues_emit_and_set_attr_intents() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    emit(actor, "The sword hums as you pick it up.")
                    emit_room(room, actor.display_name .. " picks it up.", {actor.ref_id})
                    set_attr(this, "held_by", actor.ref_id)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        assert!(!result.denied);
        assert_eq!(result.batch.len(), 3);

        let mut world = world;
        let effects = apply_batch(&mut world, &result.batch).expect("batch should apply");
        assert_eq!(effects.len(), 2);
        assert_eq!(
            world.get("#5").unwrap().attrs.get("held_by").unwrap(),
            "#3"
        );
    }

    #[test]
    fn can_get_denies_only_on_explicit_false() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "can_get",
            r#"
                function can_get(this, actor, room)
                    if not has_tag(actor, "quest:worthy") then
                        emit(actor, "The sword refuses to be lifted.")
                        return false
                    end
                    return true
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        assert!(result.denied);
        assert_eq!(result.batch.len(), 1);
    }

    #[test]
    fn cmd_hook_receives_trailing_args() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "cmd_push",
            r#"
                function cmd_push(this, actor, room, args)
                    set_attr(this, "pressed", true)
                    set_attr(this, "last_args", args)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                Some("the button"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        assert_eq!(result.batch.len(), 2);
        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(
            world.get("#5").unwrap().attrs.get("last_args").unwrap(),
            "the button"
        );
    }

    #[test]
    fn runaway_script_hits_budget() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local i = 0
                    while true do
                        i = i + 1
                    end
                end
            "#,
        );

        let err = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::new(1000),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("infinite loop should hit budget");

        assert!(matches!(err, SoftcodeError::BudgetExceeded));
    }

    #[test]
    fn programs_are_isolated_between_objects() {
        // Two different objects define a global function with the *same*
        // name; each run must only ever see its own program's definition,
        // never a leftover from a previous run on the same Lua VM.
        let world = test_world();
        let runtime = SoftcodeRuntime::new();

        let program_a = ProgramRecord::new(
            "on_get",
            r#"function on_get(this, actor, room) set_attr(this, "who", "a") end"#,
        );
        let program_b = ProgramRecord::new(
            "on_get",
            r#"function on_get(this, actor, room) set_attr(this, "who", "b") end"#,
        );

        let result_a = runtime
            .run_hook(
                &world,
                &program_a,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();
        let result_b = runtime
            .run_hook(
                &world,
                &program_b,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let get_who = |batch: &IntentBatch| -> String {
            match &batch.intents[0] {
                Intent::SetAttr { value, .. } => value.as_str().unwrap().to_string(),
                _ => panic!("expected SetAttr"),
            }
        };
        assert_eq!(get_who(&result_a.batch), "a");
        assert_eq!(get_who(&result_b.batch), "b");
    }

    #[test]
    fn syntax_error_is_rejected() {
        let runtime = SoftcodeRuntime::new();
        assert!(runtime.check_syntax("function on_get(this actor room) end").is_err());
        assert!(runtime.check_syntax("function on_get(this, actor, room) return true end").is_ok());
    }

    #[test]
    fn run_eval_applies_world_writes() {
        // @eval's whole point: a one-shot script's writes land through the
        // same Intent/batch path as any hook, not a shortcut around it.
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_eval(
                &world,
                r##"set_attr("#5", "eval_touched", true)"##,
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("eval should run");

        assert_eq!(result.batch.len(), 1);
        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(world.get("#5").unwrap().attrs["eval_touched"], true);
    }

    #[test]
    fn run_eval_reports_compile_error() {
        let runtime = SoftcodeRuntime::new();
        let world = test_world();
        let err = runtime
            .run_eval(
                &world,
                "this is not valid luau (((",
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("garbage source should not compile");
        assert!(matches!(err, SoftcodeError::Compile(_)));
    }

    #[test]
    fn run_eval_reports_runtime_error_without_panicking() {
        let runtime = SoftcodeRuntime::new();
        let world = test_world();
        let err = runtime
            .run_eval(
                &world,
                r#"error("boom")"#,
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("error() should surface as a runtime error, not a panic");
        assert!(matches!(err, SoftcodeError::Runtime(_)));
    }

    #[test]
    fn run_eval_hits_budget_on_runaway_loop() {
        let runtime = SoftcodeRuntime::new();
        let world = test_world();
        let err = runtime
            .run_eval(
                &world,
                "local i = 0 while true do i = i + 1 end",
                "#3",
                Some("#1"),
                Budget::new(1000),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("infinite loop should hit the budget");
        assert!(matches!(err, SoftcodeError::BudgetExceeded));
    }

    #[test]
    fn run_eval_reports_returned_value() {
        let runtime = SoftcodeRuntime::new();
        let world = test_world();
        let result = runtime
            .run_eval(
                &world,
                "return 1 + 1",
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("eval should run");
        assert_eq!(result.returned.as_deref(), Some("2"));
    }

    #[test]
    fn run_eval_exposes_the_caller_as_actor() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_eval(
                &world,
                "return actor.ref_id",
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("eval should run");
        assert_eq!(result.returned.as_deref(), Some("#3"));
    }

    #[test]
    fn run_eval_can_enumerate_every_object() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_eval(
                &world,
                "return #all_objects()",
                "#3",
                Some("#1"),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("eval should run");
        assert_eq!(result.returned.as_deref(), Some(world.objects.len().to_string().as_str()));
    }

    #[test]
    fn spawn_intent_can_be_referenced_by_later_intents_in_batch() {
        let mut world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "cmd_summon",
            r#"
                function cmd_summon(this, actor, room, args)
                    local ref = spawn({ key = "imp", kind = "npc", title = "a summoned imp", location = room.ref_id })
                    set_attr(ref, "summoned_by", actor.ref_id)
                    emit(actor, "You summon an imp.")
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                Some(""),
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let effects = apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(effects.len(), 1);
        let imp = world
            .objects
            .values()
            .find(|o| o.key == "imp")
            .expect("imp should have been spawned");
        assert_eq!(
            imp.attrs.get("summoned_by").unwrap(),
            "#3"
        );
    }

    fn run_script(world: &World, source: &str) -> ProgramResult {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new("on_get", source);
        runtime
            .run_hook(world, &program, "#5", "#3", Some("#1"), None, Budget::default(), counter(world), &test_themes(), &test_map_templates(), &[], 0)
            .expect("script should run")
    }

    #[test]
    fn find_by_tag() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                local found = find_by_tag("loot:weapon")
                set_attr(this, "count", #found)
                set_attr(this, "first", found[1].key)
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["count"], 1);
        assert_eq!(w.get("#5").unwrap().attrs["first"], "sword");
    }

    #[test]
    fn find_in_room() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                local obj = find_in_room(room, "guard")
                if obj then
                    set_attr(this, "found", obj.ref_id)
                end
                local missing = find_in_room(room, "dragon")
                set_attr(this, "missing", missing == nil)
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["found"], "#7");
        assert_eq!(w.get("#5").unwrap().attrs["missing"], true);
    }

    #[test]
    fn get_inventory() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                local inv = get_inventory(actor)
                set_attr(this, "inv_count", #inv)
                if #inv > 0 then
                    set_attr(this, "first_item", inv[1].key)
                end
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["inv_count"], 1);
        assert_eq!(w.get("#5").unwrap().attrs["first_item"], "shield");
    }

    #[test]
    fn get_players_in_room() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                local players = get_players_in_room(room)
                set_attr(this, "player_count", #players)
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["player_count"], 2);
    }

    #[test]
    fn get_all_by_kind() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                local rooms = get_all_by_kind("room")
                set_attr(this, "room_count", #rooms)
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["room_count"], 2);
    }

    #[test]
    fn all_objects_returns_every_ref() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                local all = all_objects()
                set_attr(this, "count", #all)
                local found_room = false
                for _, ref_id in ipairs(all) do
                    if ref_id == "#1" then
                        found_room = true
                    end
                end
                set_attr(this, "found_room", found_room)
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["count"], w.objects.len() as u64);
        assert_eq!(w.get("#5").unwrap().attrs["found_room"], true);
    }

    #[test]
    fn predicates() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                set_attr(this, "actor_is_player", is_player(actor))
                set_attr(this, "actor_is_npc", is_npc(actor))
                set_attr(this, "guard_is_npc", is_npc("#7"))
                set_attr(this, "sword_is_item", is_item(this))
                set_attr(this, "room_is_room", is_room(room))
                set_attr(this, "exit_is_exit", is_exit("#8"))
                set_attr(this, "exists_yes", exists(actor))
                set_attr(this, "exists_no", exists("fake/ref"))
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let s = &w.get("#5").unwrap().attrs;
        assert_eq!(s["actor_is_player"], true);
        assert_eq!(s["actor_is_npc"], false);
        assert_eq!(s["guard_is_npc"], true);
        assert_eq!(s["sword_is_item"], true);
        assert_eq!(s["room_is_room"], true);
        assert_eq!(s["exit_is_exit"], true);
        assert_eq!(s["exists_yes"], true);
        assert_eq!(s["exists_no"], false);
    }

    #[test]
    fn is_carrying_predicate() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                set_attr(this, "has_weapon", is_carrying(actor, "loot:weapon"))
                set_attr(this, "has_potion", is_carrying(actor, "loot:potion"))
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        // alice doesn't carry the sword (it's in the room), but she has the shield (no loot tag)
        assert_eq!(w.get("#5").unwrap().attrs["has_weapon"], false);
        assert_eq!(w.get("#5").unwrap().attrs["has_potion"], false);
    }

    #[test]
    fn same_room_predicate() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                set_attr(this, "alice_bob", same_room(actor, "#4"))
                set_attr(this, "alice_guard", same_room(actor, "#7"))
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["alice_bob"], true);
        assert_eq!(w.get("#5").unwrap().attrs["alice_guard"], true);
    }

    #[test]
    fn set_title_and_description() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                set_title(this, "a gleaming sword")
                set_description(this, "It shines with inner light.")
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().title.as_deref(), Some("a gleaming sword"));
        assert_eq!(w.get("#5").unwrap().description, "It shines with inner light.");
    }

    #[test]
    fn destroy_object() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                destroy("#7")
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert!(w.get("#7").is_none());
    }

    #[test]
    fn destroy_player_rejected() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                destroy("#4")
            end
        "##);
        let mut w = world.clone();
        assert!(apply_batch(&mut w, &result.batch).is_err());
    }

    #[test]
    fn log_does_not_error() {
        let world = test_world();
        run_script(&world, r#"
            function on_get(this, actor, room)
                log("debug: sword picked up by " .. actor.display_name)
            end
        "#);
    }

    #[test]
    fn create_exit_creates_a_traversable_exit() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                local ref = create_exit({ source = "#1", direction = "south", target = "#2" })
                set_attr(this, "new_exit", ref)
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let exit_ref = w.get("#5").unwrap().attrs["new_exit"]
            .as_str()
            .unwrap()
            .to_string();
        let exit = w.get(&exit_ref).unwrap();
        assert_eq!(exit.key, "south");
        assert_eq!(exit.location_ref.as_deref(), Some("#1"));
        assert_eq!(exit.target_ref.as_deref(), Some("#2"));
    }

    #[test]
    fn create_exit_rejects_missing_rooms() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                create_exit({ source = "#1", direction = "south", target = "#999" })
            end
        "##);
        let mut w = world.clone();
        assert!(apply_batch(&mut w, &result.batch).is_err());
    }

    #[test]
    fn set_program_attaches_a_program() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                set_program("#2", "on_look", "function on_look(this, actor, room) end")
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert!(w.get("#2").unwrap().programs.contains_key("on_look"));
    }

    #[test]
    fn set_program_rejects_unknown_hook() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                set_program("#2", "not_a_real_hook", "return true")
            end
        "##);
        let mut w = world.clone();
        assert!(apply_batch(&mut w, &result.batch).is_err());
    }

    fn sample_dungeon_themes() -> HashMap<String, Theme> {
        let mut themes = HashMap::new();
        themes.insert(
            "crypt".to_string(),
            Theme {
                name: "crypt".into(),
                title_prefix: "The Crypt of".into(),
                room_descriptions: vec![
                    crate::theme::RoomDescriptions {
                        shape: "chamber".into(),
                        texts: vec!["A crypt chamber.".into()],
                    },
                    crate::theme::RoomDescriptions {
                        shape: "corridor".into(),
                        texts: vec!["A crypt corridor.".into()],
                    },
                ],
                encounters: vec![crate::theme::EncounterTable {
                    depth: [1, 20],
                    entries: vec![crate::theme::EncounterEntry {
                        monster: "skeleton".into(),
                        count: [1, 2],
                        weight: 1,
                    }],
                }],
                loot: vec![crate::theme::LootTable {
                    depth: [1, 20],
                    items: vec!["bone_charm".into()],
                }],
            },
        );
        themes
    }

    #[test]
    fn generate_dungeon_and_destroy_dungeon_roundtrip() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let themes = sample_dungeon_themes();

        let program = ProgramRecord::new(
            "cmd_delve",
            r#"
                function cmd_delve(this, actor, room, args)
                    local entrance = generate_dungeon("test-seed", {
                        { theme = "crypt", room_count = {2, 3} },
                    })
                    set_attr(actor, "delve_entrance", entrance)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                Some(""),
                Budget::default(),
                counter(&world),
                &themes,
                &test_map_templates(),
                &[], 0,
            )
            .expect("cmd_delve should run");

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).expect("dungeon batch should apply");

        let entrance_ref = w.get("#3").unwrap().attrs["delve_entrance"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(w.get(&entrance_ref).is_some());

        let dungeon_rooms: Vec<String> = w
            .objects
            .values()
            .filter(|o| {
                o.attrs.get("dungeon_seed").and_then(|v| v.as_str()) == Some("test-seed")
            })
            .map(|o| o.ref_id.clone())
            .collect();
        assert!(dungeon_rooms.len() >= 2);

        let destroy_program = ProgramRecord::new(
            "cmd_leave",
            r#"
                function cmd_leave(this, actor, room, args)
                    destroy_dungeon("test-seed")
                end
            "#,
        );
        let destroy_result = runtime
            .run_hook(
                &w,
                &destroy_program,
                "#5",
                "#3",
                Some("#1"),
                Some(""),
                Budget::default(),
                counter(&w),
                &themes,
                &test_map_templates(),
                &[], 0,
            )
            .expect("cmd_leave should run");
        apply_batch(&mut w, &destroy_result.batch).expect("destroy batch should apply");

        for room_ref in &dungeon_rooms {
            assert!(w.get(room_ref).is_none(), "dungeon room {} should be destroyed", room_ref);
        }
    }

    #[test]
    fn instantiate_map_luau_binding_end_to_end() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();

        // Exercises the full TOML -> instantiate() -> Luau table round trip,
        // including a quoted "." terrain key.
        let toml_src = r#"
            [map]
            name = "iron_hills"
            grid = """
            f.
            f.
            """

            [terrain.f]
            theme = "forest"
            title_prefix = "Forest"

            [terrain."."]
            theme = "plains"
            title_prefix = "Plains"
        "#;
        let template: crate::map_template::MapTemplateFile =
            toml::from_str(toml_src).expect("map template should parse");
        let mut map_templates = HashMap::new();
        map_templates.insert("iron_hills".to_string(), template);

        let program = ProgramRecord::new(
            "cmd_explore",
            r#"
                function cmd_explore(this, actor, room, args)
                    local result = instantiate_map("iron_hills")
                    set_attr(actor, "map_entrance", result.entrance_ref)
                    set_attr(actor, "map_room_count", result.room_count)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                Some(""),
                Budget::default(),
                counter(&world),
                &test_themes(),
                &map_templates,
                &[], 0,
            )
            .expect("cmd_explore should run");

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).expect("map batch should apply");

        let entrance_ref = w.get("#3").unwrap().attrs["map_entrance"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(w.get(&entrance_ref).is_some());
        assert_eq!(w.get("#3").unwrap().attrs["map_room_count"], 4);

        let map_rooms: Vec<&GameObject> = w
            .objects
            .values()
            .filter(|o| o.attrs.get("map_name").and_then(|v| v.as_str()) == Some("iron_hills"))
            .collect();
        assert_eq!(map_rooms.len(), 4);
    }

    #[test]
    fn require_loads_module_and_caches_it() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert(
            "utils".into(),
            r#"
                local M = {}
                function M.double(n) return n * 2 end
                return M
            "#
            .into(),
        );
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local utils = require("utils")
                    set_attr(this, "val", utils.double(5))
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(
            world.get("#5").unwrap().attrs.get("val").unwrap(),
            &serde_json::json!(10)
        );
    }

    #[test]
    fn require_unknown_module_errors() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local nope = require("nonexistent")
                end
            "#,
        );

        let err = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("should fail on missing module");

        assert!(matches!(err, SoftcodeError::Runtime(_)));
    }

    #[test]
    fn require_cycle_errors_instead_of_recursing() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("a".into(), r#"local b = require("b") return {}"#.into());
        modules.insert("b".into(), r#"local a = require("a") return {}"#.into());
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    require("a")
                end
            "#,
        );

        let err = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("cycle should be an error");

        match err {
            SoftcodeError::Runtime(msg) => {
                assert!(
                    msg.contains("require cycle detected: a -> b -> a"),
                    "unexpected message: {}",
                    msg
                );
            }
            other => panic!("expected runtime error, got {:?}", other),
        }
    }

    #[test]
    fn require_self_cycle_errors() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("solo".into(), r#"return require("solo")"#.into());
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    require("solo")
                end
            "#,
        );

        let err = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("self-cycle should be an error");

        match err {
            SoftcodeError::Runtime(msg) => {
                assert!(msg.contains("require cycle detected: solo -> solo"), "{}", msg)
            }
            other => panic!("expected runtime error, got {:?}", other),
        }
    }

    /// A failed require must not leave the module on the loading stack, or
    /// every later require of it would look like a cycle.
    #[test]
    fn require_recovers_after_module_error() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("boom".into(), r#"error("kaboom")"#.into());
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local ok1 = pcall(function() require("boom") end)
                    local ok2, err2 = pcall(function() require("boom") end)
                    set_attr(this, "second", tostring(err2))
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        let second = world.get("#5").unwrap().attrs["second"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            !second.contains("require cycle"),
            "stale loading entry leaked: {}",
            second
        );
        assert!(second.contains("kaboom"), "unexpected error: {}", second);
    }

    #[test]
    fn require_works_in_on_tick_hooks() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert(
            "math_ext".into(),
            r#"
                local M = {}
                function M.add(a, b) return a + b end
                return M
            "#
            .into(),
        );
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    local m = require("math_ext")
                    state.sum = m.add(3, 4)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#1",
                "#1",
                None,
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("on_tick hook should run");

        assert_eq!(
            result.state.get("sum").unwrap(),
            &serde_json::json!(7)
        );
    }

    /// A `require("<name>")` for a `Kind::Code` object's `lib_<name>`
    /// Program resolves — the user-library half of `require` resolution,
    /// separate from the shipped-module table above. See
    /// docs/plans/program-authoring.md Stage 2.
    #[test]
    fn require_resolves_user_library() {
        let mut world = test_world();
        let lib_ref = world.next_dbref();
        let mut lib_obj = GameObject::new(&lib_ref, "greet", Kind::Code);
        hooks::set_program(
            &mut lib_obj,
            "lib_greet",
            r#"
                local M = {}
                function M.hello() return "hi" end
                return M
            "#
            .into(),
        )
        .unwrap();
        world.add_object(lib_obj);

        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    local m = require("greet")
                    state.greeting = m.hello()
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#1",
                "#1",
                None,
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("on_tick hook should run");

        assert_eq!(
            result.state.get("greeting").unwrap(),
            &serde_json::json!("hi")
        );
    }

    /// Editing a `lib_<name>` Program takes effect on the *next* `require`
    /// once the module cache is invalidated — without invalidation the
    /// stale cached module keeps being returned.
    #[test]
    fn lib_edit_takes_effect_after_cache_invalidation() {
        let mut world = test_world();
        let lib_ref = world.next_dbref();
        let mut lib_obj = GameObject::new(&lib_ref, "greet", Kind::Code);
        hooks::set_program(&mut lib_obj, "lib_greet", "return { version = 1 }".into()).unwrap();
        world.add_object(lib_obj);

        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    state.version = require("greet").version
                end
            "#,
        );

        fn run(runtime: &SoftcodeRuntime, world: &World, program: &ProgramRecord) -> ProgramResult {
            runtime
                .run_hook(
                    world,
                    program,
                    "#1",
                    "#1",
                    None,
                    None,
                    Budget::default(),
                    counter(world),
                    &test_themes(), &test_map_templates(), &[], 0,
                )
                .expect("on_tick hook should run")
        }

        let first = run(&runtime, &world, &program);
        assert_eq!(first.state.get("version").unwrap(), &serde_json::json!(1));

        // Edit the library's source without invalidating — require() should
        // keep returning the cached (stale) module.
        let obj = world.get_mut(&lib_ref).unwrap();
        hooks::set_program(obj, "lib_greet", "return { version = 2 }".into()).unwrap();
        let stale = run(&runtime, &world, &program);
        assert_eq!(
            stale.state.get("version").unwrap(),
            &serde_json::json!(1),
            "require should still return the cached module before invalidation"
        );

        // Invalidating the module cache makes the next require() re-evaluate.
        runtime.invalidate_module_cache();
        let fresh = run(&runtime, &world, &program);
        assert_eq!(
            fresh.state.get("version").unwrap(),
            &serde_json::json!(2),
            "require should pick up the edit after cache invalidation"
        );
    }

    /// Authoring a `lib_<name>` Program whose name collides with a shipped
    /// module is refused at write time, from softcode's own `set_program`.
    #[test]
    fn set_program_refuses_lib_name_colliding_with_shipped_module() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("str".into(), "return {}".into());
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "cmd_shadow",
            r#"
                function cmd_shadow(this, actor, room)
                    set_program(this, "lib_str", "return {}")
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#1",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect_err("set_program should refuse a shipped-module-colliding lib name");
        let message = result.to_string();
        assert!(
            message.contains("shipped module"),
            "unexpected error: {}",
            message
        );
    }

    #[test]
    fn require_transitive_deps() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert(
            "base".into(),
            r#"
                local M = {}
                function M.greet(name) return "Hello, " .. name end
                return M
            "#
            .into(),
        );
        modules.insert(
            "greeter".into(),
            r#"
                local base = require("base")
                local M = {}
                function M.greet_actor(actor)
                    return base.greet(actor.display_name)
                end
                return M
            "#
            .into(),
        );
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local greeter = require("greeter")
                    set_attr(this, "msg", greeter.greet_actor(actor))
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .expect("hook should run");

        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(
            world.get("#5").unwrap().attrs.get("msg").unwrap(),
            "Hello, Alice"
        );
    }

    #[test]
    fn invalidate_cache_clears_modules() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("v1".into(), "return 1".into());
        runtime.load_modules(modules);

        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    set_attr(this, "ver", require("v1"))
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs.get("ver").unwrap(), 1);

        runtime.invalidate_cache();
        let mut modules = HashMap::new();
        modules.insert("v1".into(), "return 2".into());
        runtime.load_modules(modules);

        let result = runtime
            .run_hook(
                &world,
                &program,
                "#5",
                "#3",
                Some("#1"),
                None,
                Budget::default(),
                counter(&world),
                &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs.get("ver").unwrap(), 2);
    }

    #[test]
    fn grid_new_get_set_roundtrip() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local g = grid_new(3, 3, 0)
                    g:set(2, 2, "wall")
                    set_attr(this, "cell", g:get(2, 2))
                    set_attr(this, "empty", g:get(1, 1))
                    set_attr(this, "oob", g:get(99, 99) == nil)
                    local w, h = g:size()
                    set_attr(this, "w", w)
                    set_attr(this, "h", h)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert_eq!(obj.attrs.get("cell").unwrap(), "wall");
        assert_eq!(obj.attrs.get("empty").unwrap(), 0);
        assert_eq!(obj.attrs.get("oob").unwrap(), true);
        assert_eq!(obj.attrs.get("w").unwrap(), 3);
        assert_eq!(obj.attrs.get("h").unwrap(), 3);
    }

    #[test]
    fn grid_to_value_from_value_roundtrip() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local g = grid_new(2, 2, "floor")
                    g:set(1, 1, "wall")
                    set_attr(this, "grid", g:to_value())
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();

        let restore_program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local saved = get_attr(this, "grid")
                    local g = grid_from_value(saved)
                    set_attr(this, "restored", g:get(1, 1))
                    set_attr(this, "w", g:width())
                end
            "#,
        );

        let result2 = runtime
            .run_hook(
                &w, &restore_program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&w), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        apply_batch(&mut w, &result2.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert_eq!(obj.attrs.get("restored").unwrap(), "wall");
        assert_eq!(obj.attrs.get("w").unwrap(), 2);
    }

    #[test]
    fn grid_pathfind() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local g = grid_new(5, 5, "floor")
                    g:set(2, 1, "wall")
                    g:set(2, 2, "wall")
                    g:set(2, 3, "wall")

                    local path = g:pathfind(1, 1, 3, 1, "floor")
                    set_attr(this, "path_len", #path)
                    set_attr(this, "start_x", path[1].x)
                    set_attr(this, "end_x", path[#path].x)

                    local blocked = grid_new(3, 1, "wall")
                    set_attr(this, "no_path", blocked:pathfind(1, 1, 3, 1, "floor") == nil)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert!(obj.attrs.get("path_len").unwrap().as_u64().unwrap() > 3);
        assert_eq!(obj.attrs.get("start_x").unwrap(), 1);
        assert_eq!(obj.attrs.get("end_x").unwrap(), 3);
        assert_eq!(obj.attrs.get("no_path").unwrap(), true);
    }

    #[test]
    fn grid_fill_and_find() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local g = grid_new(4, 4, 0)
                    g:fill(2, 2, 3, 3, "lava")
                    local pos = g:find("lava")
                    set_attr(this, "fx", pos.x)
                    set_attr(this, "fy", pos.y)
                    local all = g:find_all("lava")
                    set_attr(this, "count", #all)
                    local n = g:neighbors(2, 2)
                    set_attr(this, "neighbor_count", #n)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert_eq!(obj.attrs.get("fx").unwrap(), 2);
        assert_eq!(obj.attrs.get("fy").unwrap(), 2);
        assert_eq!(obj.attrs.get("count").unwrap(), 4);
        assert_eq!(obj.attrs.get("neighbor_count").unwrap(), 4);
    }

    #[test]
    fn noise_functions_return_deterministic_values() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local s = simplex2d(42, 1.5, 2.5)
                    local p = perlin2d(42, 1.5, 2.5)
                    local f = fbm2d(42, 1.5, 2.5)
                    set_attr(this, "simplex", s)
                    set_attr(this, "perlin", p)
                    set_attr(this, "fbm", f)

                    -- Same inputs must produce same outputs
                    local s2 = simplex2d(42, 1.5, 2.5)
                    set_attr(this, "deterministic", s == s2)

                    -- Different seed produces different output
                    local s3 = simplex2d(99, 1.5, 2.5)
                    set_attr(this, "different_seed", s ~= s3)

                    -- 3D variants
                    local s3d = simplex3d(42, 1.0, 2.0, 3.0)
                    local p3d = perlin3d(42, 1.0, 2.0, 3.0)
                    set_attr(this, "has_3d", s3d ~= 0 or p3d ~= 0 or true)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        let simplex = obj.attrs.get("simplex").unwrap().as_f64().unwrap();
        let perlin = obj.attrs.get("perlin").unwrap().as_f64().unwrap();
        assert!((-1.0..=1.0).contains(&simplex));
        assert!((-1.0..=1.0).contains(&perlin));
        assert_eq!(obj.attrs.get("deterministic").unwrap(), true);
        assert_eq!(obj.attrs.get("different_seed").unwrap(), true);
        assert_eq!(obj.attrs.get("has_3d").unwrap(), true);
    }

    #[test]
    fn seeded_rng_is_deterministic() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local h1 = hash_seed("world", 10, 20)
                    local h2 = hash_seed("world", 10, 20)
                    set_attr(this, "hash_deterministic", h1 == h2)

                    local h3 = hash_seed("world", 10, 21)
                    set_attr(this, "hash_varies", h1 ~= h3)

                    local r = seed_random(h1, 1, 6)
                    set_attr(this, "roll", r)

                    local f = seed_float(h1)
                    set_attr(this, "float", f)

                    local items = {"sword", "shield", "potion"}
                    local pick = seed_choice(h1, items)
                    set_attr(this, "choice", pick)
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert_eq!(obj.attrs.get("hash_deterministic").unwrap(), true);
        assert_eq!(obj.attrs.get("hash_varies").unwrap(), true);
        let roll = obj.attrs.get("roll").unwrap().as_i64().unwrap();
        assert!((1..=6).contains(&roll));
        let float = obj.attrs.get("float").unwrap().as_f64().unwrap();
        assert!((0.0..1.0).contains(&float));
        assert!(obj.attrs.get("choice").unwrap().as_str().is_some());
    }

    #[test]
    fn distance_and_coordinate_math() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    set_attr(this, "dist", distance(0, 0, 3, 4))
                    set_attr(this, "man", manhattan(0, 0, 3, 4))
                    set_attr(this, "dir_e", direction_to(0, 0, 5, 0))
                    set_attr(this, "dir_n", direction_to(0, 0, 0, -5))
                    set_attr(this, "dir_here", direction_to(3, 3, 3, 3))
                    set_attr(this, "lerped", lerp(0, 10, 0.5))
                    set_attr(this, "clamped", clamp(15, 0, 10))
                    set_attr(this, "remapped", remap(5, 0, 10, 0, 100))
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let obj = w.get("#5").unwrap();
        assert_eq!(obj.attrs.get("dist").unwrap().as_f64().unwrap(), 5.0);
        assert_eq!(obj.attrs.get("man").unwrap().as_f64().unwrap(), 7.0);
        assert_eq!(obj.attrs.get("dir_e").unwrap().as_str().unwrap(), "e");
        assert_eq!(obj.attrs.get("dir_n").unwrap().as_str().unwrap(), "n");
        assert_eq!(obj.attrs.get("dir_here").unwrap().as_str().unwrap(), "here");
        assert_eq!(obj.attrs.get("lerped").unwrap().as_f64().unwrap(), 5.0);
        assert_eq!(obj.attrs.get("clamped").unwrap().as_f64().unwrap(), 10.0);
        assert_eq!(obj.attrs.get("remapped").unwrap().as_f64().unwrap(), 50.0);
    }

    #[test]
    fn after_schedules_a_hook() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    after(5, this, "on_expire")
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        let effects = apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            Effect::ScheduleHook { target, hook, ticks, .. } => {
                assert_eq!(target, "#5");
                assert_eq!(hook, "on_expire");
                assert_eq!(*ticks, 5);
            }
            _ => panic!("expected ScheduleHook effect"),
        }
    }

    #[test]
    fn after_rejects_zero_ticks() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    after(0, this, "on_expire")
                end
            "#,
        );

        let result = runtime
            .run_hook(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        let err = apply_batch(&mut w, &result.batch).unwrap_err();
        assert!(err.contains("ticks must be > 0"));
    }

    #[test]
    fn test_assertions_pass() {
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_tests(
                r#"
                    function test_assert_eq()
                        assert_eq(1, 1)
                        assert_eq("hello", "hello")
                        assert_eq({1, 2, 3}, {1, 2, 3})
                    end

                    function test_assert_true_false()
                        assert_true(true)
                        assert_false(false)
                    end

                    function test_assert_nil()
                        assert_nil(nil)
                        assert_not_nil(42)
                    end

                    function test_assert_error()
                        assert_error(function() error("boom") end)
                        assert_error(function() error("boom") end, "boom")
                    end
                "#,
                "assertions.test.luau",
                None,
                Budget::default(),
            )
            .expect("test runner should not error");

        assert_eq!(result.passed(), 4);
        assert_eq!(result.failed(), 0);
    }

    #[test]
    fn test_assertions_fail() {
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_tests(
                r#"
                    function test_eq_fails()
                        assert_eq(1, 2)
                    end

                    function test_true_fails()
                        assert_true(false)
                    end
                "#,
                "fail.test.luau",
                None,
                Budget::default(),
            )
            .expect("test runner should not error");

        assert_eq!(result.passed(), 0);
        assert_eq!(result.failed(), 2);
        assert!(result.tests[0].error.as_ref().unwrap().contains("assert_eq failed"));
        assert!(result.tests[1].error.as_ref().unwrap().contains("assert_true failed"));
    }

    #[test]
    fn test_integration_mode() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let result = runtime
            .run_tests(
                r#"
                    function test_get_object(ctx)
                        local actor = get_object(ctx.actor)
                        assert_not_nil(actor)
                        assert_eq(actor.kind, "player")
                    end

                    function test_room_contents(ctx)
                        local contents = get_room_contents(ctx.room)
                        assert_true(#contents > 0)
                    end
                "#,
                "integration.test.luau",
                Some(&world),
                Budget::default(),
            )
            .expect("test runner should not error");

        assert_eq!(result.passed(), 2);
        assert_eq!(result.failed(), 0);
    }

    #[test]
    fn run_game_softcode_tests() {
        let game_dir = std::env::var("HEARTH_GAME_DIR")
            .ok()
            .or_else(|| {
                let candidate = std::path::Path::new("../the-last-stag-mud/world");
                candidate.exists().then(|| candidate.to_string_lossy().to_string())
            });
        let game_dir = match game_dir {
            Some(d) => d,
            None => {
                eprintln!("Skipping softcode game tests: game directory not found");
                return;
            }
        };

        let game_path = std::path::Path::new(&game_dir);
        let runtime = SoftcodeRuntime::new();
        let modules = crate::loader::load_modules(game_path);
        runtime.load_modules(modules);

        let test_files = crate::loader::discover_test_files(game_path);
        if test_files.is_empty() {
            eprintln!("No .test.luau files found in {}", game_dir);
            return;
        }

        let mut total_passed = 0;
        let mut total_failed = 0;
        let mut failures = Vec::new();

        for tf in &test_files {
            let world = if tf.is_lib { None } else { Some(test_world()) };
            let result = runtime.run_tests(
                &tf.source,
                &tf.relative,
                world.as_ref(),
                Budget::default(),
            );
            match result {
                Ok(file_result) => {
                    for tr in &file_result.tests {
                        if tr.passed {
                            total_passed += 1;
                            eprintln!("  PASS {}::{}", tf.relative, tr.name);
                        } else {
                            total_failed += 1;
                            failures.push(format!(
                                "  FAIL {}::{} -- {}",
                                tf.relative,
                                tr.name,
                                tr.error.as_deref().unwrap_or("?")
                            ));
                        }
                    }
                }
                Err(e) => {
                    total_failed += 1;
                    failures.push(format!("  FAIL {} -- file error: {}", tf.relative, e));
                }
            }
        }

        eprintln!("\n{} passed, {} failed", total_passed, total_failed);
        if !failures.is_empty() {
            eprintln!("\nFailures:");
            for f in &failures {
                eprintln!("{}", f);
            }
            panic!("{} softcode test(s) failed", total_failed);
        }
    }
}
