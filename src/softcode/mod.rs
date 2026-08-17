//! The Luau softcode system.
//!
//! Programs (Luau scripts, see [`hooks`]) run on the engine's single thread,
//! gated by an instruction [`Budget`]. They cannot touch [`crate::world::World`]
//! directly — see ADR 0001. Instead every mutating call in the API
//! ([`api`]) pushes a typed [`Intent`] into an [`IntentBatch`]; the engine
//! validates and applies the whole batch atomically once the script returns.

pub mod api;
pub mod hooks;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{Lua, LuaSerdeExt, Value as LuaValue, VmState};

use crate::world::{GameObject, Kind, Tag, World};
use hooks::ProgramRecord;

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
    Trigger {
        target: String,
        hook: String,
    },
}

/// The intents a Program has queued during a single run. Collected while the
/// script executes, applied all-at-once (or not at all) afterward.
#[derive(Debug, Clone, Default)]
pub struct IntentBatch {
    pub intents: Vec<Intent>,
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
    TriggerHook {
        target: String,
        hook: String,
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
            } => {
                if world.get(ref_id).is_some() {
                    return Err(format!("spawn: ref '{}' already exists", ref_id));
                }
                if world.get(location).is_none() {
                    return Err(format!("spawn: no location '{}'", location));
                }
                let mut obj = GameObject::new(ref_id.clone(), key.clone(), kind.clone())
                    .with_location(location.clone());
                if let Some(t) = title {
                    obj = obj.with_title(t.clone());
                }
                if let Some(d) = description {
                    obj = obj.with_description(d.clone());
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
                if target.starts_with("player/") {
                    return Err("destroy: cannot destroy player objects".into());
                }
                if world.objects.remove(target).is_none() {
                    return Err(format!("destroy: no object '{}'", target));
                }
            }
            Intent::Trigger { target, hook } => {
                if world.get(target).is_none() {
                    return Err(format!("trigger: no object '{}'", target));
                }
                effects.push(Effect::TriggerHook {
                    target: target.clone(),
                    hook: hook.clone(),
                });
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

/// Owns the Luau VM. One `SoftcodeRuntime` is enough for the whole engine —
/// each [`SoftcodeRuntime::run_hook`] call isolates a Program in its own
/// environment table so unrelated Programs never see each other's globals.
///
/// Compiled chunks are cached by source hash — same source skips
/// recompilation on subsequent calls.
pub struct SoftcodeRuntime {
    lua: Lua,
    chunk_cache: std::cell::RefCell<HashMap<u64, mlua::RegistryKey>>,
}

impl SoftcodeRuntime {
    pub fn new() -> Self {
        Self {
            lua: Lua::new(),
            chunk_cache: std::cell::RefCell::new(HashMap::new()),
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
        if let Some(key) = cache.get(&hash) {
            if let Ok(func) = self.lua.registry_value::<mlua::Function>(key) {
                return Ok(func);
            }
        }
        drop(cache);

        let func = self.lua.load(source).set_name(name).into_function()?;
        let key = self.lua.create_registry_value(func.clone())?;
        self.chunk_cache.borrow_mut().insert(hash, key);
        Ok(func)
    }

    pub fn invalidate_cache(&self) {
        self.chunk_cache.borrow_mut().clear();
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
    ) -> Result<ProgramResult, SoftcodeError> {
        self.install_budget(budget);
        let batch = Rc::new(RefCell::new(IntentBatch::default()));
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
                    if let Ok((k, v)) = pair {
                        if let Ok(json_val) = self.lua.from_value::<serde_json::Value>(v) {
                            map.insert(k, json_val);
                        }
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

        let state = Rc::try_unwrap(state_capture)
            .map(|cell| cell.into_inner())
            .unwrap_or_default();

        Ok(ProgramResult {
            batch,
            denied: matches!(ret, LuaValue::Boolean(false)),
            state,
        })
    }

    /// Run a global script — no `this` object, just `(state)`.
    pub fn run_global_script(
        &self,
        world: &World,
        source: &str,
        entry: &str,
        state: &HashMap<String, serde_json::Value>,
        budget: Budget,
    ) -> Result<ProgramResult, SoftcodeError> {
        self.install_budget(budget);
        let batch = Rc::new(RefCell::new(IntentBatch::default()));
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
                None,
            )?;

            let compiled = self.get_or_compile(source, entry)
                .map_err(|e| e.clone())?;
            compiled.set_environment(env.clone())?;
            compiled.call::<()>(())?;

            let func: Option<mlua::Function> = env.get(entry)?;
            let func = match func {
                Some(f) => f,
                None => return Ok(LuaValue::Nil),
            };

            let state_tbl = self.lua.create_table()?;
            for (k, v) in state {
                state_tbl.set(k.clone(), self.lua.to_value(v)?)?;
            }
            let ret = func.call::<LuaValue>(state_tbl.clone())?;
            // Read state back before scope ends
            let mut map = state_writer.borrow_mut();
            for pair in state_tbl.pairs::<String, LuaValue>() {
                if let Ok((k, v)) = pair {
                    if let Ok(json_val) = self.lua.from_value::<serde_json::Value>(v) {
                        map.insert(k, json_val);
                    }
                }
            }
            Ok(ret)
        });

        self.lua.remove_interrupt();

        let ret = match run_result {
            Ok(v) => v,
            Err(e) => return Err(classify_lua_error(e)),
        };

        let batch = Rc::try_unwrap(batch)
            .map(|cell| cell.into_inner())
            .unwrap_or_else(|rc| rc.borrow().clone());

        let new_state = Rc::try_unwrap(state_capture)
            .map(|cell| cell.into_inner())
            .unwrap_or_default();

        Ok(ProgramResult {
            batch,
            denied: matches!(ret, LuaValue::Boolean(false)),
            state: new_state,
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

    fn test_world() -> World {
        let mut world = World::new();
        let mut room = GameObject::new("room/1", "room", Kind::Room).with_title("A Room");
        room.description = "A plain room.".into();
        world.add_object(room);

        let mut room2 = GameObject::new("room/2", "room2", Kind::Room).with_title("Another Room");
        room2.description = "Another room.".into();
        world.add_object(room2);

        let mut actor = GameObject::new("player/alice", "alice", Kind::Player)
            .with_title("Alice")
            .with_location("room/1");
        actor.tags.insert(Tag { category: "quest".into(), key: "hero".into() });
        world.add_object(actor);

        let bob = GameObject::new("player/bob", "bob", Kind::Player)
            .with_title("Bob")
            .with_location("room/1");
        world.add_object(bob);

        let mut sword = GameObject::new("item/sword", "sword", Kind::Item)
            .with_title("a rusty sword")
            .with_location("room/1");
        sword.tags.insert(Tag { category: "loot".into(), key: "weapon".into() });
        sword.attrs.insert("damage".into(), serde_json::json!(10));
        world.add_object(sword);

        let shield = GameObject::new("item/shield", "shield", Kind::Item)
            .with_title("a wooden shield")
            .with_location("player/alice");
        world.add_object(shield);

        let npc = GameObject::new("npc/guard", "guard", Kind::Npc)
            .with_title("A Town Guard")
            .with_location("room/1");
        world.add_object(npc);

        let exit = GameObject::new("exit/1_to_2", "north", Kind::Exit)
            .with_location("room/1")
            .with_target("room/2")
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                None,
                Budget::default(),
            )
            .expect("hook should run");

        assert!(!result.denied);
        assert_eq!(result.batch.len(), 3);

        let mut world = world;
        let effects = apply_batch(&mut world, &result.batch).expect("batch should apply");
        assert_eq!(effects.len(), 2);
        assert_eq!(
            world.get("item/sword").unwrap().attrs.get("held_by").unwrap(),
            "player/alice"
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                None,
                Budget::default(),
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                Some("the button"),
                Budget::default(),
            )
            .expect("hook should run");

        assert_eq!(result.batch.len(), 2);
        let mut world = world;
        apply_batch(&mut world, &result.batch).unwrap();
        assert_eq!(
            world.get("item/sword").unwrap().attrs.get("last_args").unwrap(),
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                None,
                Budget::new(1000),
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                None,
                Budget::default(),
            )
            .unwrap();
        let result_b = runtime
            .run_hook(
                &world,
                &program_b,
                "item/sword",
                "player/alice",
                Some("room/1"),
                None,
                Budget::default(),
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
                "item/sword",
                "player/alice",
                Some("room/1"),
                Some(""),
                Budget::default(),
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
            "player/alice"
        );
    }

    fn run_script(world: &World, source: &str) -> ProgramResult {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new("on_get", source);
        runtime
            .run_hook(world, &program, "item/sword", "player/alice", Some("room/1"), None, Budget::default())
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
        assert_eq!(w.get("item/sword").unwrap().attrs["count"], 1);
        assert_eq!(w.get("item/sword").unwrap().attrs["first"], "sword");
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
        assert_eq!(w.get("item/sword").unwrap().attrs["found"], "npc/guard");
        assert_eq!(w.get("item/sword").unwrap().attrs["missing"], true);
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
        assert_eq!(w.get("item/sword").unwrap().attrs["inv_count"], 1);
        assert_eq!(w.get("item/sword").unwrap().attrs["first_item"], "shield");
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
        assert_eq!(w.get("item/sword").unwrap().attrs["player_count"], 2);
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
        assert_eq!(w.get("item/sword").unwrap().attrs["room_count"], 2);
    }

    #[test]
    fn predicates() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                set_attr(this, "actor_is_player", is_player(actor))
                set_attr(this, "actor_is_npc", is_npc(actor))
                set_attr(this, "guard_is_npc", is_npc("npc/guard"))
                set_attr(this, "sword_is_item", is_item(this))
                set_attr(this, "room_is_room", is_room(room))
                set_attr(this, "exit_is_exit", is_exit("exit/1_to_2"))
                set_attr(this, "exists_yes", exists(actor))
                set_attr(this, "exists_no", exists("fake/ref"))
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        let s = &w.get("item/sword").unwrap().attrs;
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
        assert_eq!(w.get("item/sword").unwrap().attrs["has_weapon"], false);
        assert_eq!(w.get("item/sword").unwrap().attrs["has_potion"], false);
    }

    #[test]
    fn same_room_predicate() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                set_attr(this, "alice_bob", same_room(actor, "player/bob"))
                set_attr(this, "alice_guard", same_room(actor, "npc/guard"))
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("item/sword").unwrap().attrs["alice_bob"], true);
        assert_eq!(w.get("item/sword").unwrap().attrs["alice_guard"], true);
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
        assert_eq!(w.get("item/sword").unwrap().title.as_deref(), Some("a gleaming sword"));
        assert_eq!(w.get("item/sword").unwrap().description, "It shines with inner light.");
    }

    #[test]
    fn destroy_object() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                destroy("npc/guard")
            end
        "#);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert!(w.get("npc/guard").is_none());
    }

    #[test]
    fn destroy_player_rejected() {
        let world = test_world();
        let result = run_script(&world, r#"
            function on_get(this, actor, room)
                destroy("player/bob")
            end
        "#);
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
}
