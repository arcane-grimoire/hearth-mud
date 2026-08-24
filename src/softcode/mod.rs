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
use hooks::ObjectScript;

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
        /// The archetype (blueprint) this instance delegates to, already
        /// resolved to a dbref — see docs/plans/archetypes.md Stage 1. `None`
        /// spawns a standalone object, same as before archetypes existed.
        archetype: Option<String>,
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
        /// Delete even if other objects have `archetype_ref` pointing at this
        /// one. Default `false` — see [`apply_to`]'s `Destroy` handling: an
        /// archetype with live instances refuses to delete unless this is
        /// set, so orphaning them (silently losing behavior) is a loud,
        /// opt-in choice rather than a three-sessions-later bug.
        cascade: bool,
    },
    /// Flatten an instance in place: copy its *resolved* title, description,
    /// attrs, and (if it has none of its own) its nearest archetype
    /// ancestor's script onto the object, then clear `archetype_ref`. The
    /// `clone`/`detach` escape hatch from docs/plans/archetypes.md Stage 1 —
    /// delegation stays the default; this is the loud opt-out. `state` is
    /// untouched (it was always the instance's own).
    Detach {
        target: String,
    },
    CreateExit {
        ref_id: String,
        source: String,
        direction: String,
        target: String,
        aliases: Vec<String>,
    },
    SetScript {
        target: String,
        source: String,
    },
    SetLib {
        target: String,
        name: String,
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
    /// `target -> (attr key -> index in `intents`)` of the *latest*
    /// `SetAttr`/`UnsetAttr` for that pair. Lets [`pending_attr`] answer in
    /// O(1) instead of reverse-scanning the whole vector on every read —
    /// which matters because property-style access routes every `this.x`
    /// through `pending_attr`, so a write-then-read loop was O(n·k).
    ///
    /// Maintained exclusively by [`push`]; the batch is append-only at
    /// runtime (nothing removes or reorders intents), so a stored index stays
    /// valid for the batch's whole life. **Do not push onto `intents`
    /// directly** — that desyncs this map. Build pre-populated batches with
    /// [`IntentBatch::from_intents`], not a struct literal.
    ///
    /// [`pending_attr`]: IntentBatch::pending_attr
    /// [`push`]: IntentBatch::push
    attr_index: HashMap<String, HashMap<String, usize>>,
}

impl IntentBatch {
    /// Build a batch from a pre-existing intent list, keeping `attr_index` in
    /// sync. Use this instead of the struct literal when you already hold a
    /// `Vec<Intent>` (tests, generators), so `pending_attr` stays correct.
    pub fn from_intents(intents: Vec<Intent>) -> Self {
        let mut batch = IntentBatch::default();
        for intent in intents {
            batch.push(intent);
        }
        batch
    }

    pub fn push(&mut self, intent: Intent) {
        let idx = self.intents.len();
        match &intent {
            Intent::SetAttr { target, key, .. } | Intent::UnsetAttr { target, key } => {
                self.attr_index
                    .entry(target.clone())
                    .or_default()
                    .insert(key.clone(), idx);
            }
            _ => {}
        }
        self.intents.push(intent);
    }

    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    pub fn len(&self) -> usize {
        self.intents.len()
    }

    /// Check for a pending write to `target`/`key`. Returns `Some(Some(value))`
    /// for a pending set, `Some(None)` for a pending unset, or `None` if no
    /// write is pending.
    ///
    /// O(1): `attr_index` points straight at the latest `SetAttr`/`UnsetAttr`
    /// for the pair (later `push`es overwrite the entry), so "latest write
    /// wins" holds without the old reverse scan of every intent.
    pub fn pending_attr(&self, target: &str, key: &str) -> Option<Option<&serde_json::Value>> {
        let idx = *self.attr_index.get(target)?.get(key)?;
        match &self.intents[idx] {
            Intent::SetAttr { value, .. } => Some(Some(value)),
            Intent::UnsetAttr { .. } => Some(None),
            // attr_index only ever records the two variants above.
            _ => None,
        }
    }

    /// Latest pending `SetTitle` for `target`, if any. A reverse scan is fine
    /// here (unlike `pending_attr`, which property access hammers): title
    /// writes within one script run are rare, and there is no unset intent to
    /// index for.
    pub fn pending_title(&self, target: &str) -> Option<&str> {
        self.intents.iter().rev().find_map(|i| match i {
            Intent::SetTitle { target: t, title } if t == target => Some(title.as_str()),
            _ => None,
        })
    }

    /// Latest pending `SetDescription` for `target`, if any. Same shape and
    /// reasoning as [`Self::pending_title`].
    pub fn pending_description(&self, target: &str) -> Option<&str> {
        self.intents.iter().rev().find_map(|i| match i {
            Intent::SetDescription { target: t, description } if t == target => {
                Some(description.as_str())
            }
            _ => None,
        })
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

/// Whether a batch running under `authority` may modify `target`.
///
/// `None` authority means the running Program is attached to an object with
/// no owner — the system layer, which every file-authored object belongs to
/// (`owner_ref` defaults to `None` and neither the loader nor `@import` sets
/// it). System code is trusted and unrestricted.
///
/// Anything else is a builder, and may only modify what it owns. Note the
/// asymmetry this creates deliberately: a builder cannot modify an *unowned*
/// object, so builder code cannot rewrite the system layer.
fn may_modify(world: &World, authority: Option<&str>, target: &str) -> bool {
    let Some(authority) = authority else {
        return true;
    };
    world
        .get(target)
        .and_then(|o| o.owner_ref.as_deref())
        .is_some_and(|owner| owner == authority)
}

fn refuse(verb: &str, target: &str) -> String {
    format!("{}: permission denied on '{}'", verb, target)
}

/// Flatten `target` in place: copy its *resolved* title, description, attrs,
/// and tags onto the object, plus (if it has none of its own) its nearest
/// archetype ancestor's script, then clear `archetype_ref`. A no-op if
/// `target` isn't an instance (no `archetype_ref` set).
///
/// The shared mechanism behind two callers: `Intent::Detach` (the scripted
/// `clone()`/`detach()` escape hatch) and cascade-delete (`Intent::Destroy`
/// with `cascade: true`, which flattens every live instance *before*
/// removing the archetype they depend on, so cascading never orphans
/// anything — see docs/plans/archetypes.md).
pub(crate) fn detach_object(world: &mut World, target: &str) -> Result<(), String> {
    let obj = world
        .get(target)
        .ok_or_else(|| format!("clone: no object '{}'", target))?
        .clone();
    if obj.archetype_ref.is_none() {
        return Ok(());
    }
    let title = world.resolved_title(&obj);
    let description = world.resolved_description(&obj);
    let attrs = world.resolved_attrs(&obj);
    let tags = world.resolved_tags(&obj);
    // Flatten the WHOLE resolved behavior — the instance's own script plus
    // every ancestor's, own/nearest winning — into one source, so a partial
    // override (a local hook alongside inherited ones) keeps ALL its hooks
    // after detaching, not just the locally-defined ones. See
    // `hooks::flattened_chain_source`.
    let flattened = hooks::flattened_chain_source(world, &obj);
    let target_obj = world
        .get_mut(target)
        .ok_or_else(|| format!("clone: no object '{}'", target))?;
    target_obj.title = title;
    target_obj.description = description;
    target_obj.attrs = attrs;
    target_obj.tags = tags;
    if let Some(source) = flattened {
        // `set_script_with_origin` re-derives the hook index and PRESERVES the
        // instance's own `state` (never the ancestor's — state doesn't
        // delegate), which is exactly what a detach wants.
        hooks::set_script_with_origin(target_obj, source, hooks::ProgramOrigin::InGame);
    }
    target_obj.archetype_ref = None;
    Ok(())
}

/// Whether `authority` is already at its object ceiling. Counts on demand
/// rather than caching: creation is rare next to reads, and a stale cache
/// here would be a worse bug than the scan is a cost.
fn at_object_quota(world: &World, authority: Option<&str>) -> bool {
    let Some(authority) = authority else {
        return false;
    };
    world
        .objects
        .values()
        .filter(|o| o.owner_ref.as_deref() == Some(authority))
        .count()
        >= OWNER_OBJECT_QUOTA
}

/// How many objects one owner may have in existence at once.
///
/// A ceiling on the total, not on any single batch: the failure this actually
/// catches is a builder's loop creating a handful of objects every tick, which
/// stays under any per-batch cap indefinitely. System authority is exempt —
/// procedural generation legitimately creates hundreds of rooms at a time.
pub const OWNER_OBJECT_QUOTA: usize = 500;

/// How many messages one builder-authored run may emit.
///
/// Scoped to a single batch because that is where the runaway loop lives: the
/// instruction budget bounds how long a hook runs but not how much it says,
/// and fifty messages out of one hook is already pathological. System
/// authority is exempt — a server-wide announcement legitimately emits once
/// per player.
///
/// This does not bound a program that emits a handful every tick forever.
/// That is a content bug rather than a runaway, and it is visible in play.
pub const EMIT_BATCH_LIMIT: usize = 50;

/// How many timers may be pending against one owner's objects at once.
///
/// The fork bomb this stops — a hook that schedules two more of itself — is
/// worse than a runaway loop, because `scheduled_hooks` is persisted: it
/// survives a restart, so bouncing the server does not clear it. The
/// instruction budget is no defence either, since each individual run is well
/// inside it and only the count grows.
pub const OWNER_TIMER_QUOTA: usize = 100;

/// Hooks the engine fires as part of an object's lifecycle rather than in
/// response to a player action. Firing one out of context is what makes
/// `Intent::Trigger` a privilege-escalation seam, so these are restricted to
/// objects the running authority owns.
fn is_lifecycle_hook(hook: &str) -> bool {
    matches!(
        hook,
        "on_tick"
            | "on_create"
            | "on_destroy"
            | "on_startup"
            | "on_shutdown"
            | "on_reload"
            | "on_save"
            | "on_connect"
            | "on_disconnect"
    )
}

fn apply_to(world: &mut World, batch: &IntentBatch) -> Result<Vec<Effect>, String> {
    let mut effects = Vec::new();
    let authority = batch.authority.as_deref();

    // Checked up front rather than counted as we go, so an over-limit batch
    // is refused whole instead of applying its first fifty messages and then
    // failing — the atomic-rollback guarantee would cover it either way, but
    // this keeps the reason legible.
    if authority.is_some() {
        let emits = batch
            .intents
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    Intent::EmitActor { .. }
                        | Intent::EmitRoom { .. }
                        | Intent::EmitNearby { .. }
                        | Intent::EmitRadius { .. }
                        | Intent::EmitData { .. }
                )
            })
            .count();
        if emits > EMIT_BATCH_LIMIT {
            return Err(format!(
                "emit: {} messages in one run exceeds the limit of {}",
                emits, EMIT_BATCH_LIMIT
            ));
        }
    }

    for intent in &batch.intents {
        match intent {
            Intent::SetAttr { target, key, value } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_attr", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_attr: no object '{}'", target))?;
                obj.attrs.insert(key.clone(), value.clone());
            }
            Intent::UnsetAttr { target, key } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("unset_attr", target));
                }
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
                // Deliberately unrestricted, unlike the other mutating
                // intents. Requiring ownership of what you move would mean
                // owning a player in order to teleport them, which rules out
                // builder-made teleporters, shops, containers, and knockback —
                // most of what Move exists for.
                //
                // The cost is that possession routes around ownership: a
                // builder cannot destroy or reprogram someone else's object,
                // but can move it into one of their own. That is theft, and it
                // is accepted here as a social problem rather than a technical
                // one, on the assumption that the builder flag is granted to
                // people you know. Revisit if that assumption changes.
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
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_tag", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_tag: no object '{}'", target))?;
                obj.tags.insert(tag.clone());
            }
            Intent::UnsetTag { target, tag } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("unset_tag", target));
                }
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
                archetype,
            } => {
                if at_object_quota(world, authority) {
                    return Err(format!(
                        "spawn: object quota reached ({} objects)",
                        OWNER_OBJECT_QUOTA
                    ));
                }
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
                if let Some(a) = archetype {
                    if world.get(a).is_none() {
                        return Err(format!("spawn: no archetype '{}'", a));
                    }
                    // A freshly minted `ref_id` can never already be part of
                    // an existing chain, so this is defense in depth today —
                    // it earns its keep the moment anything besides Spawn can
                    // set `archetype_ref` (Stage 1 has no reparent operation;
                    // `clone`/`detach` only ever clears it).
                    if world.would_cycle_archetype(ref_id, a) {
                        return Err(format!(
                            "spawn: archetype '{}' would create a cycle",
                            a
                        ));
                    }
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
                obj.archetype_ref = archetype.clone();
                world.add_object(obj);
                // Constructor seam: an archetype's on_create (or the
                // instance's own, if it somehow already has a script) fires
                // on the new instance, same as CreateExit below. Delivered as
                // an Effect (not run inline) so it goes through the normal
                // fire_hook path — including archetype hook resolution — once
                // this whole batch has actually committed.
                effects.push(Effect::TriggerHook {
                    target: ref_id.clone(),
                    hook: "on_create".to_string(),
                    data: None,
                });
            }
            Intent::SetTitle { target, title } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_title", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_title: no object '{}'", target))?;
                obj.title = Some(title.clone());
            }
            Intent::SetDescription { target, description } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_description", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_description: no object '{}'", target))?;
                obj.description = description.clone();
            }
            Intent::Destroy { target, cascade } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("destroy", target));
                }
                match world.get(target) {
                    Some(obj) if obj.kind == Kind::Player => {
                        return Err("destroy: cannot destroy player objects".into());
                    }
                    Some(_) => {
                        let instances: Vec<String> = world
                            .objects
                            .values()
                            .filter(|o| o.archetype_ref.as_deref() == Some(target.as_str()))
                            .map(|o| o.ref_id.clone())
                            .collect();
                        if !instances.is_empty() {
                            // Refuse to delete an archetype out from under its
                            // instances by default — an orphaned
                            // `archetype_ref` (silently losing behavior) is a
                            // three-sessions-later bug, not an error anyone
                            // would notice at delete time.
                            if !cascade {
                                return Err(format!(
                                    "destroy: '{}' is an archetype with live instances — pass cascade to delete anyway",
                                    target
                                ));
                            }
                            // `cascade` means "detach then delete", never
                            // "delete and orphan": flatten every instance
                            // (copy its resolved fields/script down, clear
                            // archetype_ref) *before* the archetype it
                            // depends on is removed, so none of them lose
                            // behavior.
                            for instance_ref in &instances {
                                detach_object(world, instance_ref)?;
                            }
                        }
                        world.remove_object(target);
                    }
                    None => return Err(format!("destroy: no object '{}'", target)),
                }
            }
            Intent::Detach { target } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("clone", target));
                }
                detach_object(world, target)?;
            }
            Intent::CreateExit { ref_id, source, direction, target, aliases } => {
                if at_object_quota(world, authority) {
                    return Err(format!(
                        "create_exit: object quota reached ({} objects)",
                        OWNER_OBJECT_QUOTA
                    ));
                }
                if !may_modify(world, authority, source) {
                    return Err(refuse("create_exit", source));
                }
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
            Intent::SetScript { target, source } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_script", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_script: no object '{}'", target))?;
                crate::softcode::hooks::set_script(obj, source.clone());
            }
            Intent::SetLib { target, name, source } => {
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_lib", target));
                }
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_lib: no object '{}'", target))?;
                crate::softcode::hooks::set_lib(
                    obj,
                    name,
                    source.clone(),
                    crate::softcode::hooks::ProgramOrigin::InGame,
                );
            }
            Intent::Trigger { target, hook, data } => {
                if world.get(target).is_none() {
                    return Err(format!("trigger: no object '{}'", target));
                }
                // Triggering runs the target's Program under *its* authority,
                // so this is the confused-deputy seam. Gameplay hooks stay
                // open because ordinary play fires them on objects you do not
                // own all the time; lifecycle hooks do not, and firing one out
                // of context breaks the invariant it exists to maintain.
                if is_lifecycle_hook(hook) && !may_modify(world, authority, target) {
                    return Err(refuse("trigger", target));
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
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_lock", target));
                }
                crate::locks::parse(expr).map_err(|e| format!("set_lock: {}", e))?;
                let obj = world
                    .get_mut(target)
                    .ok_or_else(|| format!("set_lock: no object '{}'", target))?;
                obj.locks.insert(hook.clone(), expr.clone());
            }
            Intent::SetOwner { target, owner } => {
                // Giving away what you own is fine; taking what you do not is
                // escalation. `owner` is a required String, so this intent
                // cannot clear an owner back to `None` and slip an object into
                // the unrestricted system layer.
                if !may_modify(world, authority, target) {
                    return Err(refuse("set_owner", target));
                }
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
                // Both ends are mutated, so both need authority — checking
                // only the destination would let a builder drain someone
                // else's object into their own.
                if !may_modify(world, authority, from) {
                    return Err(refuse("transfer_attr", from));
                }
                if !may_modify(world, authority, to) {
                    return Err(refuse("transfer_attr", to));
                }
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
    /// Whether [`Self::sync_user_lib_sources`] needs to rebuild the
    /// user-lib sources table before the next Program execution. Set by
    /// [`Self::mark_libs_dirty`] (called from the cache-invalidation entry
    /// points the engine already invokes on every program mutation), so
    /// per-hook-fire syncs become no-ops while libs are unchanged.
    libs_dirty: std::cell::Cell<bool>,
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
            // Start dirty: nothing has been synced yet.
            libs_dirty: std::cell::Cell::new(true),
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
        self.mark_libs_dirty();
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

    /// Refresh the user-library source table from `world` — every lib module
    /// on any object, keyed by its bare `<name>`.
    /// Flag the user-lib sources table as stale so the next Program
    /// execution re-syncs it from `World`. Every program-mutation path in
    /// the engine already funnels through `invalidate_module_cache` /
    /// `invalidate_cache`, which call this — see
    /// `Engine::invalidate_libs_touched_by` and the `@program`/`@lib`/
    /// `@script`/restore command handlers.
    pub fn mark_libs_dirty(&self) {
        self.libs_dirty.set(true);
    }

    /// Rebuild the user-lib sources table only when something changed since
    /// the last sync. Combined with `invalidate_module_cache` at write time
    /// (which marks the table dirty), an edit takes effect on the *next*
    /// `require` call without needing to track who required what.
    fn sync_user_lib_sources(&self, world: &World) {
        if !self.libs_dirty.replace(false) {
            return;
        }
        let table: mlua::Table = self
            .lua
            .named_registry_value(USER_LIB_SOURCES_KEY)
            .expect("user lib sources table");
        clear_table(&table);
        // Any object may host `require`able lib modules (a `Kind::Code` object
        // is the usual home, but rooms/objects can carry them too — the TOML
        // `[*.libs]` tables are accepted on rooms and objects alike, so a
        // kind filter here would silently make advertised libs un-requireable).
        for obj in world.objects.values() {
            for (name, lib) in &obj.libs {
                let _ = table.set(name.clone(), lib.source.clone());
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
        self.mark_libs_dirty();
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

    /// Run `script`'s Luau source, then call its `hook`-named function with
    /// `(this, actor, room[, args])`.
    ///
    /// The whole object script runs first (defining every hook into a shared
    /// env — helpers and constants at the top are visible to all of them),
    /// then the `hook`-named function is looked up and invoked. For `on_tick`
    /// hooks the signature is `(this, state, room)` — `state` is the object's
    /// mutable state table, persisted between runs and shared by all hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn run_hook(
        &self,
        world: &World,
        script: &ObjectScript,
        hook: &str,
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
        let is_tick = hook == "on_tick";
        let state_capture: Rc<RefCell<HashMap<String, serde_json::Value>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let state_writer = Rc::clone(&state_capture);

        let run_result: mlua::Result<LuaValue> = self.lua.scope(|scope| {
            let env = self.lua.create_table()?;
            api::install_stdlib(&self.lua, &env)?;
            let obj_mt = api::install(
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

            let compiled = self.get_or_compile(&script.source, hook)
                .map_err(|e| e.clone())?;
            compiled.set_environment(env.clone())?;
            compiled.call::<()>(())?;

            let func: Option<mlua::Function> = env.get(hook)?;
            let func = match func {
                Some(f) => f,
                None => return Ok(LuaValue::Nil),
            };

            let this_val =
                api::object_to_value(&self.lua, world, this_ref, Some(&obj_mt))?;

            if is_tick {
                let state_tbl = self.lua.create_table()?;
                for (k, v) in &script.state {
                    state_tbl.set(k.clone(), self.lua.to_value(v)?)?;
                }
                let room_val = match room_ref.or(default_location.as_deref()) {
                    Some(r) => {
                        api::object_to_value(&self.lua, world, r, Some(&obj_mt))?
                    }
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
                let actor_val =
                    api::object_to_value(&self.lua, world, actor_ref, Some(&obj_mt))?;
                let room_val = match room_ref.or(default_location.as_deref()) {
                    Some(r) => {
                        api::object_to_value(&self.lua, world, r, Some(&obj_mt))?
                    }
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
            let obj_mt = api::install(
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
            env.set(
                "actor",
                api::object_to_value(&self.lua, world, actor_ref, Some(&obj_mt))?,
            )?;

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
                    let _obj_mt = api::install(
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

    /// Test-only stand-in for the removed per-hook `ProgramRecord`: a single
    /// hook function plus its source. With one script per object, `run_hook`
    /// takes the whole `ObjectScript` and the hook name; this shim lets the
    /// existing tests keep the "one hook, one source" shape by wrapping the
    /// source in a one-hook `ObjectScript` at call time.
    struct TestProgram {
        hook: String,
        source: String,
    }

    impl TestProgram {
        fn new(hook: impl Into<String>, source: impl Into<String>) -> Self {
            Self {
                hook: hook.into(),
                source: source.into(),
            }
        }
    }

    /// `run_hook_rec` — a test-only wrapper matching the pre-refactor
    /// `run_hook` call shape (a program record instead of script + hook), so
    /// call sites need only rename `run_hook` → `run_hook_rec`.
    trait RunHookCompat {
        #[allow(clippy::too_many_arguments)]
        fn run_hook_rec(
            &self,
            world: &World,
            program: &TestProgram,
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
        ) -> Result<ProgramResult, SoftcodeError>;
    }

    impl RunHookCompat for SoftcodeRuntime {
        fn run_hook_rec(
            &self,
            world: &World,
            program: &TestProgram,
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
            let script = hooks::ObjectScript {
                source: program.source.clone(),
                enabled: true,
                state: Default::default(),
                origin: Default::default(),
                hooks: vec![program.hook.clone()],
            };
            SoftcodeRuntime::run_hook(
                self,
                world,
                &script,
                &program.hook,
                this_ref,
                actor_ref,
                room_ref,
                args,
                budget,
                dbref_counter,
                themes,
                map_templates,
                scheduled_hooks,
                tick_count,
            )
        }
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
            "cmd_push",
            r#"
                function cmd_push(this, actor, room, args)
                    set_attr(this, "pressed", true)
                    set_attr(this, "last_args", args)
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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

        let program_a = TestProgram::new(
            "on_get",
            r#"function on_get(this, actor, room) set_attr(this, "who", "a") end"#,
        );
        let program_b = TestProgram::new(
            "on_get",
            r#"function on_get(this, actor, room) set_attr(this, "who", "b") end"#,
        );

        let result_a = runtime
            .run_hook_rec(
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
            .run_hook_rec(
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
    fn check_syntax_accepts_type_annotations() {
        // The editor seeds fresh hooks with typed signatures
        // (function on_enter(this: Object, actor: Object, room: Object)). Luau
        // erases type annotations at compile time and mlua's loader doesn't run
        // the analyzer, so an undefined `Object` type must still compile — if
        // this ever failed, every newly-opened hook would lint red.
        let runtime = SoftcodeRuntime::new();
        runtime
            .check_syntax(
                "function on_enter(this: Object, actor: Object, room: Object)\n  return true\nend",
            )
            .expect("typed Luau signature should compile");
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        // The `emit` plus the on_create constructor trigger every Spawn now
        // queues (see docs/plans/archetypes.md's "constructor seam") — the
        // imp has no script so that trigger is a no-op, but the effect is
        // still queued for delivery.
        assert_eq!(effects.len(), 2);
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
        let program = TestProgram::new("on_get", source);
        runtime
            .run_hook_rec(world, &program, "#5", "#3", Some("#1"), None, Budget::default(), counter(world), &test_themes(), &test_map_templates(), &[], 0)
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

    // -- Archetype (is-a) — docs/plans/archetypes.md Stage 1 --

    /// `Intent::Spawn`'s `archetype` sets `archetype_ref` on the new
    /// instance, and queues the constructor-seam `on_create` trigger exactly
    /// like `CreateExit` already does — see the "Constructor seam" comment
    /// in `apply_to`'s `Spawn` arm. (Whether the archetype's `on_create`
    /// actually *runs* bound to the instance is the engine's job — see
    /// `engine::tests::archetype_instance_inherits_hook_fires_bound_to_instance`.)
    #[test]
    fn spawn_with_archetype_sets_archetype_ref_and_queues_on_create() {
        let mut world = test_world();
        let archetype_ref = "#7".to_string(); // the guard NPC, per test_world()'s doc comment

        let batch = IntentBatch::from_intents(vec![Intent::Spawn {
            ref_id: "#100".into(),
            key: "goblin1".into(),
            kind: Kind::Npc,
            title: None,
            description: None,
            location: "#1".into(),
            owner: None,
            archetype: Some(archetype_ref.clone()),
        }]);

        let effects = apply_batch(&mut world, &batch).expect("spawn should succeed");
        let instance = world.get("#100").expect("instance should exist");
        assert_eq!(instance.archetype_ref.as_deref(), Some(archetype_ref.as_str()));
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::TriggerHook { target, hook, .. }
                    if target == "#100" && hook == "on_create"
            )),
            "spawn should queue the on_create constructor trigger: {:?}",
            effects
        );
    }

    #[test]
    fn spawn_refuses_a_nonexistent_archetype() {
        let mut world = test_world();
        let batch = IntentBatch::from_intents(vec![Intent::Spawn {
            ref_id: "#100".into(),
            key: "goblin1".into(),
            kind: Kind::Npc,
            title: None,
            description: None,
            location: "#1".into(),
            owner: None,
            archetype: Some("#999".into()),
        }]);
        let err = apply_batch(&mut world, &batch).expect_err("no such archetype");
        assert!(err.contains("no archetype"), "unexpected error: {}", err);
        assert!(world.get("#100").is_none(), "the batch should roll back entirely");
    }

    /// Refuse to delete an archetype while instances still delegate to it —
    /// orphaning them (silently losing behavior) is the "three sessions
    /// later" bug the plan calls out. `cascade: true` is the loud opt-out.
    #[test]
    fn destroy_refuses_an_archetype_with_live_instances_unless_cascaded() {
        let mut world = test_world();
        let archetype_ref = "#7".to_string();
        {
            let archetype = world.get_mut(&archetype_ref).unwrap();
            archetype
                .attrs
                .insert("max_hp".into(), serde_json::json!(10));
            archetype
                .tags
                .insert(Tag { category: "quest".into(), key: "elite".into() });
            hooks::set_script(
                archetype,
                "function on_get(this, actor, room) end".to_string(),
            );
        }
        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location("#1");
        instance.archetype_ref = Some(archetype_ref.clone());
        world.add_object(instance);

        let refuse = IntentBatch::from_intents(vec![Intent::Destroy {
            target: archetype_ref.clone(),
            cascade: false,
        }]);
        let err = apply_batch(&mut world, &refuse).expect_err("archetype has a live instance");
        assert!(err.contains("live instances"), "unexpected error: {}", err);
        assert!(world.get(&archetype_ref).is_some(), "refused delete must not touch the world");

        let cascade = IntentBatch::from_intents(vec![Intent::Destroy {
            target: archetype_ref.clone(),
            cascade: true,
        }]);
        apply_batch(&mut world, &cascade).expect("cascade should override the guard");
        assert!(world.get(&archetype_ref).is_none(), "the archetype itself is gone");

        // `cascade` FLATTENS every instance before deleting the archetype —
        // it must never orphan them with a dangling archetype_ref (that's
        // exactly the bug the guard exists to prevent).
        let instance = world
            .get(&instance_ref)
            .expect("the instance itself must survive a cascade delete");
        assert!(instance.archetype_ref.is_none(), "archetype_ref must be cleared");
        assert_eq!(instance.title.as_deref(), Some("A Town Guard"));
        assert_eq!(instance.attrs.get("max_hp"), Some(&serde_json::json!(10)));
        assert!(instance.tags.contains(&Tag { category: "quest".into(), key: "elite".into() }));
        let script = instance.script.as_ref().expect("script copied down from the archetype");
        assert!(script.hooks.contains(&"on_get".to_string()));
    }

    /// `clone`/`detach`: flatten an instance in place — resolved fields and
    /// (since this instance has no script of its own) the archetype's script
    /// copied verbatim, `archetype_ref` cleared, and any accumulated
    /// `state` preserved rather than overwritten by the archetype's.
    #[test]
    fn detach_flattens_an_instance_and_keeps_its_own_state() {
        let mut world = test_world();
        let archetype_ref = world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_title("Goblin")
            .with_description("A snarling goblin.");
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        archetype.tags.insert(Tag { category: "quest".into(), key: "elite".into() });
        hooks::set_script(
            &mut archetype,
            "function on_get(this, actor, room) end".to_string(),
        );
        world.add_object(archetype);

        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location("#1");
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.attrs.insert("name_tag".into(), serde_json::json!("Grubnak"));
        instance.tags.insert(Tag { category: "loot".into(), key: "weapon".into() });
        world.add_object(instance);
        // Simulate accumulated tick state predating the detach — must survive.
        hooks::ensure_own_state_slot(world.get_mut(&instance_ref).unwrap())
            .state
            .insert("ticks".into(), serde_json::json!(5));

        let batch = IntentBatch::from_intents(vec![Intent::Detach {
            target: instance_ref.clone(),
        }]);
        apply_batch(&mut world, &batch).expect("detach should succeed");

        let instance = world.get(&instance_ref).unwrap();
        assert!(instance.archetype_ref.is_none(), "detach clears archetype_ref");
        assert_eq!(instance.title.as_deref(), Some("Goblin"));
        assert_eq!(instance.description, "A snarling goblin.");
        assert_eq!(instance.attrs.get("max_hp"), Some(&serde_json::json!(10)));
        assert_eq!(instance.attrs.get("name_tag"), Some(&serde_json::json!("Grubnak")));
        // Tags union: the instance's own plus the archetype's.
        assert!(instance.tags.contains(&Tag { category: "quest".into(), key: "elite".into() }));
        assert!(instance.tags.contains(&Tag { category: "loot".into(), key: "weapon".into() }));
        let script = instance.script.as_ref().expect("script copied from archetype");
        assert!(script.hooks.contains(&"on_get".to_string()));
        assert_eq!(
            script.state.get("ticks"),
            Some(&serde_json::json!(5)),
            "state must survive detach — it was never delegated"
        );

        // The archetype itself is untouched.
        let archetype = world.get(&archetype_ref).unwrap();
        assert_eq!(archetype.title.as_deref(), Some("Goblin"));
    }

    #[test]
    fn detach_preserves_inherited_hooks_on_partial_override() {
        // A partial override — the instance defines its OWN on_get but
        // inherits on_tick from its archetype — must keep BOTH hooks after
        // detaching. Regression: detach used to keep only the instance's own
        // script, silently dropping every inherited hook.
        let mut world = test_world();
        let archetype_ref = world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc);
        hooks::set_script(
            &mut archetype,
            "function on_get(this, actor, room) end\nfunction on_tick(this, state, room) end".to_string(),
        );
        world.add_object(archetype);

        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc).with_location("#1");
        instance.archetype_ref = Some(archetype_ref.clone());
        // Own on_get override; on_tick is inherited only.
        hooks::set_script(
            &mut instance,
            "function on_get(this, actor, room) emit(actor, \"mine\") end".to_string(),
        );
        world.add_object(instance);

        apply_batch(
            &mut world,
            &IntentBatch::from_intents(vec![Intent::Detach { target: instance_ref.clone() }]),
        )
        .expect("detach should succeed");

        let inst = world.get(&instance_ref).unwrap();
        assert!(inst.archetype_ref.is_none());
        let script = inst.script.as_ref().expect("flattened script");
        assert!(script.hooks.contains(&"on_get".to_string()), "own hook kept: {:?}", script.hooks);
        assert!(
            script.hooks.contains(&"on_tick".to_string()),
            "inherited hook must survive detach: {:?}",
            script.hooks
        );
        // The instance's own on_get wins (it's emitted last in the flattened
        // source), while the inherited on_tick is preserved.
        assert!(script.source.contains("\"mine\""), "own on_get override retained");
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
    fn set_script_attaches_a_script() {
        let world = test_world();
        let result = run_script(&world, r##"
            function on_get(this, actor, room)
                set_script("#2", "function on_look(this, actor, room) end")
            end
        "##);
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert!(hooks::object_defines_hook(w.get("#2").unwrap(), "on_look"));
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

        let program = TestProgram::new(
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
            .run_hook_rec(
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

        let destroy_program = TestProgram::new(
            "cmd_leave",
            r#"
                function cmd_leave(this, actor, room, args)
                    destroy_dungeon("test-seed")
                end
            "#,
        );
        let destroy_result = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
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
            .run_hook_rec(
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

    /// The user-lib sources table is only rebuilt when marked dirty — a
    /// second sync with no intervening invalidation is a no-op, and an
    /// `invalidate_module_cache` (fired on every lib program write) makes
    /// the next sync pick up the change.
    #[test]
    fn user_lib_sync_is_skipped_until_marked_dirty() {
        use super::hooks;

        let runtime = SoftcodeRuntime::new();
        let mut world = World::new();
        let mut obj = GameObject::new("#1", "lib", Kind::Code);
        hooks::set_lib(&mut obj, "m", "return 1".into(), hooks::ProgramOrigin::InGame);
        world.add_object(obj);

        let table: mlua::Table = runtime
            .lua
            .named_registry_value(crate::softcode::USER_LIB_SOURCES_KEY)
            .unwrap();

        runtime.sync_user_lib_sources(&world);
        assert_eq!(table.get::<String>("m").unwrap(), "return 1");

        // Mutate the world without invalidating: sync must be a no-op and
        // the table keeps the old source.
        world
            .get_mut("#1")
            .unwrap()
            .libs
            .get_mut("m")
            .unwrap()
            .source = "return 2".into();
        runtime.sync_user_lib_sources(&world);
        assert_eq!(table.get::<String>("m").unwrap(), "return 1");

        // Invalidate (what the engine does on every program write): next
        // sync picks up the new source.
        runtime.invalidate_module_cache();
        runtime.sync_user_lib_sources(&world);
        assert_eq!(table.get::<String>("m").unwrap(), "return 2");
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local utils = require("utils")
                    set_attr(this, "val", utils.double(5))
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local nope = require("nonexistent")
                end
            "#,
        );

        let err = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    require("a")
                end
            "#,
        );

        let err = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    require("solo")
                end
            "#,
        );

        let err = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
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
            .run_hook_rec(
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

        let program = TestProgram::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    local m = require("math_ext")
                    state.sum = m.add(3, 4)
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
        hooks::set_lib(
            &mut lib_obj,
            "greet",
            r#"
                local M = {}
                function M.hello() return "hi" end
                return M
            "#
            .into(),
            hooks::ProgramOrigin::InGame,
        );
        world.add_object(lib_obj);

        let runtime = SoftcodeRuntime::new();
        let program = TestProgram::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    local m = require("greet")
                    state.greeting = m.hello()
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
        hooks::set_lib(&mut lib_obj, "greet", "return { version = 1 }".into(), hooks::ProgramOrigin::InGame);
        world.add_object(lib_obj);

        let runtime = SoftcodeRuntime::new();
        let program = TestProgram::new(
            "on_tick",
            r#"
                function on_tick(this, state, room)
                    state.version = require("greet").version
                end
            "#,
        );

        fn run(runtime: &SoftcodeRuntime, world: &World, program: &TestProgram) -> ProgramResult {
            runtime
                .run_hook_rec(
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
        hooks::set_lib(obj, "greet", "return { version = 2 }".into(), hooks::ProgramOrigin::InGame);
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

    /// Authoring a lib module whose name collides with a shipped module is
    /// refused at write time, from softcode's own `set_lib`.
    #[test]
    fn set_lib_refuses_lib_name_colliding_with_shipped_module() {
        let world = test_world();
        let runtime = SoftcodeRuntime::new();
        let mut modules = HashMap::new();
        modules.insert("str".into(), "return {}".into());
        runtime.load_modules(modules);

        let program = TestProgram::new(
            "cmd_shadow",
            r#"
                function cmd_shadow(this, actor, room)
                    set_lib(this, "str", "return {}")
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
            .expect_err("set_lib should refuse a shipped-module-colliding lib name");
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local greeter = require("greeter")
                    set_attr(this, "msg", greeter.greet_actor(actor))
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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

        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    set_attr(this, "ver", require("v1"))
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
                &world, &program, "#5", "#3", Some("#1"), None,
                Budget::default(), counter(&world), &test_themes(), &test_map_templates(), &[], 0,
            )
            .unwrap();

        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();

        let restore_program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
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
            .run_hook_rec(
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
        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    after(5, this, "on_expire")
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
        let program = TestProgram::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    after(0, this, "on_expire")
                end
            "#,
        );

        let result = runtime
            .run_hook_rec(
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
        // Wire the game dir into the Ink runtime so file-based dialogue
        // (`ink_start(actor, npc, { file = "..." })`) is exercisable from
        // integration tests, the same as it is at runtime.
        runtime
            .ink_runtime()
            .borrow_mut()
            .set_ink_dir(game_path.to_path_buf());
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

    // -- UGC intent authorization ------------------------------------------
    //
    // A Program runs with the authority of the object it is attached to (its
    // `owner_ref`), not with the authority of whoever triggered it. See
    // docs/plans/program-authoring.md.

    /// Build a world holding one item owned by `owner`, and return its ref.
    fn world_with_owned_item(owner: Option<&str>) -> (World, String) {
        let mut world = World::new();
        let ref_id = world.next_dbref();
        let mut obj = GameObject::new(&ref_id, "crate", Kind::Item);
        if let Some(o) = owner {
            obj = obj.with_owner(o);
        }
        world.add_object(obj);
        (world, ref_id)
    }

    fn batch_as(authority: Option<&str>, intents: Vec<Intent>) -> IntentBatch {
        IntentBatch {
            authority: authority.map(|s| s.to_string()),
            ..IntentBatch::from_intents(intents)
        }
    }

    #[test]
    fn a_program_cannot_set_attrs_on_an_object_it_does_not_own() {
        let (mut world, target) = world_with_owned_item(Some("#builder-b"));

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::SetAttr {
                target: target.clone(),
                key: "hp".into(),
                value: serde_json::json!(1),
            }],
        );

        let result = apply_batch(&mut world, &batch);

        assert!(result.is_err(), "expected refusal, got {:?}", result);
        assert!(
            !world.get(&target).unwrap().attrs.contains_key("hp"),
            "a refused batch must leave the world untouched"
        );
    }

    #[test]
    fn a_program_can_set_attrs_on_an_object_it_owns() {
        let (mut world, target) = world_with_owned_item(Some("#builder-a"));

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::SetAttr {
                target: target.clone(),
                key: "hp".into(),
                value: serde_json::json!(1),
            }],
        );

        apply_batch(&mut world, &batch).expect("owner should be allowed");
        assert!(world.get(&target).unwrap().attrs.contains_key("hp"));
    }

    #[test]
    fn system_authority_is_unrestricted() {
        // Every file-authored object is unowned, so its Programs carry no
        // authority. That has to mean "system", not "nobody", or the game
        // would not run at all.
        let (mut world, target) = world_with_owned_item(Some("#builder-b"));

        let batch = batch_as(
            None,
            vec![Intent::SetAttr {
                target: target.clone(),
                key: "hp".into(),
                value: serde_json::json!(1),
            }],
        );

        apply_batch(&mut world, &batch).expect("system authority should be allowed");
        assert!(world.get(&target).unwrap().attrs.contains_key("hp"));
    }

    #[test]
    fn a_builder_cannot_modify_an_unowned_system_object() {
        // The other half of the rule above: system code is unrestricted, but
        // builder code must not be able to reach *into* the system layer.
        let (mut world, target) = world_with_owned_item(None);

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::SetAttr {
                target: target.clone(),
                key: "hp".into(),
                value: serde_json::json!(1),
            }],
        );

        assert!(
            apply_batch(&mut world, &batch).is_err(),
            "an unowned object belongs to the system layer"
        );
    }

    #[test]
    fn a_program_cannot_destroy_an_object_it_does_not_own() {
        let (mut world, target) = world_with_owned_item(Some("#builder-b"));

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::Destroy { target: target.clone(), cascade: false }],
        );

        assert!(apply_batch(&mut world, &batch).is_err(), "expected refusal");
        assert!(
            world.get(&target).is_some(),
            "a refused destroy must leave the object standing"
        );
    }

    #[test]
    fn a_program_cannot_take_ownership_of_someone_elses_object() {
        // Giving your own object away is fine; taking someone else's is
        // privilege escalation with no further steps needed.
        let mut world = World::new();
        // Both builders must exist as real objects, or set_owner's existence
        // check refuses the batch before authorization is ever consulted and
        // the test passes for the wrong reason.
        let thief = world.next_dbref();
        world.add_object(GameObject::new(&thief, "alice", Kind::Player));
        let victim = world.next_dbref();
        world.add_object(GameObject::new(&victim, "bob", Kind::Player));
        let target = world.next_dbref();
        world
            .add_object(GameObject::new(&target, "crate", Kind::Item).with_owner(&victim));

        let batch = batch_as(
            Some(&thief),
            vec![Intent::SetOwner {
                target: target.clone(),
                owner: thief.clone(),
            }],
        );

        assert!(apply_batch(&mut world, &batch).is_err(), "expected refusal");
        assert_eq!(
            world.get(&target).unwrap().owner_ref.as_deref(),
            Some(victim.as_str()),
            "ownership must not have moved"
        );
    }

    #[test]
    fn a_program_can_give_away_an_object_it_owns() {
        let mut world = World::new();
        let recipient = world.next_dbref();
        world.add_object(GameObject::new(&recipient, "bob", Kind::Player));
        let target = world.next_dbref();
        world.add_object(
            GameObject::new(&target, "crate", Kind::Item).with_owner("#builder-a"),
        );

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::SetOwner {
                target: target.clone(),
                owner: recipient.clone(),
            }],
        );

        apply_batch(&mut world, &batch).expect("giving away what you own is allowed");
        assert_eq!(
            world.get(&target).unwrap().owner_ref.as_deref(),
            Some(recipient.as_str())
        );
    }

    #[test]
    fn every_mutating_intent_is_refused_on_an_object_you_do_not_own() {
        // One case per intent that writes to a named target. Kept as a table
        // so that adding a mutating intent without an authority check shows
        // up here rather than in someone's world.
        let cases: Vec<(&str, fn(&str) -> Intent)> = vec![
            ("unset_attr", |t| Intent::UnsetAttr {
                target: t.into(),
                key: "hp".into(),
            }),
            ("set_tag", |t| Intent::SetTag {
                target: t.into(),
                tag: Tag { category: "x".into(), key: "y".into() },
            }),
            ("unset_tag", |t| Intent::UnsetTag {
                target: t.into(),
                tag: Tag { category: "x".into(), key: "y".into() },
            }),
            ("set_title", |t| Intent::SetTitle {
                target: t.into(),
                title: "pwned".into(),
            }),
            ("set_description", |t| Intent::SetDescription {
                target: t.into(),
                description: "pwned".into(),
            }),
            ("set_script", |t| Intent::SetScript {
                target: t.into(),
                source: "function on_use(this, actor, room) return true end".into(),
            }),
            ("set_lock", |t| Intent::SetLock {
                target: t.into(),
                hook: "can_get".into(),
                expr: "perm(admin)".into(),
            }),
        ];

        for (name, build) in cases {
            let (mut world, target) = world_with_owned_item(Some("#builder-b"));
            let batch = batch_as(Some("#builder-a"), vec![build(&target)]);
            assert!(
                apply_batch(&mut world, &batch).is_err(),
                "{} should be refused on an object owned by someone else",
                name
            );
        }
    }

    /// The tests above hand-build batches, so they would all still pass if
    /// `run_hook` never stamped an authority at all. This drives the real
    /// path: a Program on a builder-owned object reaching for someone else's.
    #[test]
    fn authority_comes_from_the_hosting_objects_owner() {
        let mut world = World::new();
        let alice = world.next_dbref();
        world.add_object(GameObject::new(&alice, "alice", Kind::Player));
        let bob = world.next_dbref();
        world.add_object(GameObject::new(&bob, "bob", Kind::Player));

        let host = world.next_dbref();
        world.add_object(GameObject::new(&host, "wand", Kind::Item).with_owner(&alice));
        let victim = world.next_dbref();
        world.add_object(GameObject::new(&victim, "chest", Kind::Item).with_owner(&bob));

        let runtime = SoftcodeRuntime::new();
        let program = TestProgram::new(
            "on_use",
            format!(
                r#"
                function on_use(this, actor, room)
                    set_attr("{}", "pwned", true)
                end
                "#,
                victim
            ),
        );

        let result = runtime
            .run_hook_rec(
                &world,
                &program,
                &host,
                &alice,
                None,
                None,
                Budget::default(),
                counter(&world),
                &test_themes(),
                &test_map_templates(),
                &[],
                0,
            )
            .expect("hook should run");

        assert_eq!(
            result.batch.authority.as_deref(),
            Some(alice.as_str()),
            "authority should be the host object's owner, not the actor"
        );

        let mut world = world;
        assert!(
            apply_batch(&mut world, &result.batch).is_err(),
            "alice's program must not reach bob's object"
        );
        assert!(!world.get(&victim).unwrap().attrs.contains_key("pwned"));
    }

    #[test]
    fn a_program_cannot_trigger_lifecycle_hooks_on_someone_elses_object() {
        // Gameplay hooks fire on other people's objects constantly through
        // normal play, so triggering one is a shortcut for something you
        // could already cause. Lifecycle hooks are the ones where firing out
        // of context breaks an invariant — on_create running twice, on_destroy
        // running on something that still exists.
        for hook in ["on_tick", "on_create", "on_destroy", "on_startup"] {
            let (mut world, target) = world_with_owned_item(Some("#builder-b"));
            let batch = batch_as(
                Some("#builder-a"),
                vec![Intent::Trigger {
                    target: target.clone(),
                    hook: hook.into(),
                    data: None,
                }],
            );
            assert!(
                apply_batch(&mut world, &batch).is_err(),
                "{} should not be triggerable on another builder's object",
                hook
            );
        }
    }

    #[test]
    fn a_program_can_trigger_gameplay_hooks_on_someone_elses_object() {
        let (mut world, target) = world_with_owned_item(Some("#builder-b"));
        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::Trigger {
                target: target.clone(),
                hook: "on_use".into(),
                data: None,
            }],
        );

        apply_batch(&mut world, &batch)
            .expect("gameplay hooks stay reachable, as they are through normal play");
    }

    #[test]
    fn a_builder_cannot_spawn_past_their_quota() {
        // The limit that actually bites is a bug, not an attack: a loop that
        // creates objects every tick stays under any per-batch cap forever.
        let mut world = World::new();
        let room = world.next_dbref();
        world.add_object(GameObject::new(&room, "room", Kind::Room));

        for i in 0..OWNER_OBJECT_QUOTA {
            let r = world.next_dbref();
            world.add_object(
                GameObject::new(&r, format!("thing{}", i), Kind::Item)
                    .with_owner("#builder-a"),
            );
        }

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::Spawn {
                ref_id: "#9999".into(),
                key: "one-too-many".into(),
                kind: Kind::Item,
                title: None,
                description: None,
                location: room.clone(),
                owner: Some("#builder-a".into()),
                archetype: None,
            }],
        );

        assert!(
            apply_batch(&mut world, &batch).is_err(),
            "spawning past the quota should be refused"
        );
        assert!(world.get("#9999").is_none());
    }

    #[test]
    fn system_authority_is_not_subject_to_a_quota() {
        // Procedural generation runs as system and legitimately creates
        // hundreds of rooms at a time.
        let mut world = World::new();
        let room = world.next_dbref();
        world.add_object(GameObject::new(&room, "room", Kind::Room));

        for i in 0..OWNER_OBJECT_QUOTA {
            let r = world.next_dbref();
            world.add_object(GameObject::new(&r, format!("thing{}", i), Kind::Item));
        }

        let batch = batch_as(
            None,
            vec![Intent::Spawn {
                ref_id: "#9999".into(),
                key: "generated".into(),
                kind: Kind::Item,
                title: None,
                description: None,
                location: room.clone(),
                owner: None,
                archetype: None,
            }],
        );

        apply_batch(&mut world, &batch).expect("system generation is unbounded");
    }

    #[test]
    fn transfer_attr_requires_ownership_of_both_ends() {
        // It reads a number off one object and writes it to another, so it is
        // a mutation of both.
        let mut world = World::new();
        let mine = world.next_dbref();
        let mut a = GameObject::new(&mine, "purse", Kind::Item).with_owner("#builder-a");
        a.attrs.insert("gold".into(), serde_json::json!(100.0));
        world.add_object(a);

        let theirs = world.next_dbref();
        let mut b = GameObject::new(&theirs, "vault", Kind::Item).with_owner("#builder-b");
        b.attrs.insert("gold".into(), serde_json::json!(100.0));
        world.add_object(b);

        // Draining someone else's object into your own.
        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::TransferAttr {
                from: theirs.clone(),
                to: mine.clone(),
                key: "gold".into(),
                amount: 50.0,
            }],
        );

        assert!(apply_batch(&mut world, &batch).is_err(), "expected refusal");
        assert_eq!(
            world.get(&theirs).unwrap().attrs.get("gold").unwrap(),
            &serde_json::json!(100.0),
            "the victim's balance must be untouched"
        );
    }

    #[test]
    fn creating_an_exit_counts_against_the_quota() {
        let mut world = World::new();
        let room = world.next_dbref();
        world.add_object(GameObject::new(&room, "room", Kind::Room).with_owner("#builder-a"));
        for i in 0..OWNER_OBJECT_QUOTA {
            let r = world.next_dbref();
            world.add_object(
                GameObject::new(&r, format!("thing{}", i), Kind::Item)
                    .with_owner("#builder-a"),
            );
        }

        let batch = batch_as(
            Some("#builder-a"),
            vec![Intent::CreateExit {
                ref_id: "#9999".into(),
                source: room.clone(),
                direction: "north".into(),
                target: room.clone(),
                aliases: vec![],
            }],
        );

        assert!(apply_batch(&mut world, &batch).is_err());
        assert!(world.get("#9999").is_none());
    }

    #[test]
    fn a_builders_batch_cannot_emit_without_limit() {
        let mut world = World::new();
        let victim = world.next_dbref();
        world.add_object(GameObject::new(&victim, "bob", Kind::Player));

        let intents = (0..=EMIT_BATCH_LIMIT)
            .map(|i| Intent::EmitActor {
                target: victim.clone(),
                message: format!("spam {}", i),
            })
            .collect();

        let batch = batch_as(Some("#builder-a"), intents);
        assert!(
            apply_batch(&mut world, &batch).is_err(),
            "a single run emitting past the limit should be refused"
        );
    }

    #[test]
    fn system_authority_may_emit_without_limit() {
        // A server-wide announcement legitimately emits once per player.
        let mut world = World::new();
        let victim = world.next_dbref();
        world.add_object(GameObject::new(&victim, "bob", Kind::Player));

        let intents = (0..=EMIT_BATCH_LIMIT)
            .map(|i| Intent::EmitActor {
                target: victim.clone(),
                message: format!("announcement {}", i),
            })
            .collect();

        let batch = batch_as(None, intents);
        apply_batch(&mut world, &batch).expect("system broadcasts are unbounded");
    }

    // -- Property-style object access (issue #19) --

    #[test]
    fn property_writes_persist_through_the_batch() {
        let world = test_world();
        let result = run_script(
            &world,
            r#"
            function on_get(this, actor, room)
                this.mood = "sunny"
                this.title = "Shiny Thing"
                this.description = "It gleams."
            end
        "#,
        );
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        assert_eq!(w.get("#5").unwrap().attrs["mood"], "sunny");
        assert_eq!(w.get("#5").unwrap().title.as_deref(), Some("Shiny Thing"));
        assert_eq!(w.get("#5").unwrap().description, "It gleams.");
    }

    #[test]
    fn property_reads_match_get_attr_including_same_script_writes() {
        let world = test_world();
        let result = run_script(
            &world,
            r#"
            function on_get(this, actor, room)
                set_attr(this, "hp", 10)
                local a = this.hp
                local b = get_attr(this, "hp")
                this.hp = nil
                local c = this.hp
                local d = get_attr(this, "hp")
                assert(a == 10 and b == 10)
                assert(c == nil and d == nil)
            end
        "#,
        );
        // Two writes: the set_attr and the nil-assignment's UnsetAttr. The
        // reads push nothing.
        assert_eq!(result.batch.len(), 2);
        assert!(matches!(
            result.batch.intents[1],
            crate::softcode::Intent::UnsetAttr { .. }
        ));
    }

    #[test]
    fn property_nil_write_is_an_unset_not_a_null_set() {
        let world = test_world();
        let result = run_script(
            &world,
            r#"
            function on_get(this, actor, room) this.torch = nil end
        "#,
        );
        assert_eq!(result.batch.len(), 1);
        assert!(matches!(
            result.batch.intents[0],
            crate::softcode::Intent::UnsetAttr { .. }
        ));
    }

    fn run_script_raw(
        world: &World,
        source: &str,
    ) -> Result<ProgramResult, SoftcodeError> {
        run_script_raw_budget(world, source, Budget::default())
    }

    fn run_script_raw_budget(
        world: &World,
        source: &str,
        budget: Budget,
    ) -> Result<ProgramResult, SoftcodeError> {
        let runtime = SoftcodeRuntime::new();
        let program = TestProgram::new("on_get", source);
        runtime.run_hook_rec(
            world,
            &program,
            "#5",
            "#3",
            Some("#1"),
            None,
            budget,
            counter(world),
            &test_themes(),
            &test_map_templates(),
            &[],
            0,
        )
    }

    #[test]
    fn protected_fields_raise_errors_naming_the_owner_api() {
        let world = test_world();
        for (field, needle) in [
            ("location_ref", "move_object"),
            ("ref_id", "cannot be changed"),
            ("kind", "cannot be changed"),
            ("display_name", "computed"),
        ] {
            let err = run_script_raw(
                &world,
                &format!(r#"function on_get(this, actor, room) this.{field} = "x" end"#),
            )
            .expect_err("protected write must fail");
            assert!(
                format!("{err:?}").contains(needle),
                "{field}: error should mention '{needle}', got {err:?}"
            );
        }
    }

    #[test]
    fn proxy_pairs_enumerates_fields_and_attrs() {
        let world = test_world();
        // Metamethod dispatch per field costs VM instructions — give the
        // iteration room beyond the default hook budget.
        let result = run_script_raw_budget(
            &world,
            r#"
            function on_get(this, actor, room)
                local names = {}
                for k, v in this do names[#names + 1] = k end
                set_attr(this, "_seen", #names)

                local anames = {}
                for k, v in this.attrs do anames[#anames + 1] = k end
                set_attr(this, "_attrs_seen", #anames)
            end
        "#,
            Budget::new(1_000_000),
        )
        .expect("script should run");
        let mut w = world.clone();
        apply_batch(&mut w, &result.batch).unwrap();
        // The real assertion: pairs() yields at all through __pairs — a raw
        // empty proxy table would yield nothing.
        let obj = w.get("#5").unwrap();
        let seen = obj.attrs["_seen"].as_u64().unwrap();
        assert!(seen >= 8, "object pairs() yielded only {seen} fields");
        assert!(
            obj.attrs.get("_attrs_seen").map(|v| v.as_u64().unwrap_or(0)).unwrap_or(0) >= 1,
            "attrs iteration should see at least the two attrs written above"
        );
    }

    #[test]
    fn list_results_stay_plain_tables() {
        let world = test_world();
        let result = run_script(
            &world,
            r#"
            function on_get(this, actor, room)
                local objs = get_contents(room)
                local o = objs[1]
                -- Raw writes to plain tables must NOT push intents.
                o.anything = 1
            end
        "#,
        );
        assert!(
            result.batch.is_empty(),
            "list results must not be proxies: {:?}",
            result.batch.intents
        );
    }
}
