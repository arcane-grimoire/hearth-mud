//! The Lua-facing surface Programs run against: a read API backed directly
//! by [`World`], and a write API that only ever pushes [`Intent`]s into the
//! batch — see ADR 0001. Nothing here ever gets a `&mut World`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, MultiValue, Scope, Table, Value};

use crate::map_template::MapTemplateFile;
use crate::theme::Theme;
use crate::world::{GameObject, Kind, Tag, World};
use crate::softcode::{Intent, IntentBatch, ink};

enum PathSeg {
    Key(String),
    Index(usize),
}

fn set_nested(root: &mut serde_json::Value, path: &[PathSeg], value: serde_json::Value) -> Result<(), String> {
    if path.is_empty() {
        *root = value;
        return Ok(());
    }
    match &path[0] {
        PathSeg::Key(k) => {
            if !root.is_object() {
                *root = serde_json::Value::Object(serde_json::Map::new());
            }
            let obj = root.as_object_mut().unwrap();
            if path.len() == 1 {
                obj.insert(k.clone(), value);
            } else {
                let child = obj.entry(k.clone()).or_insert(serde_json::Value::Object(serde_json::Map::new()));
                set_nested(child, &path[1..], value)?;
            }
        }
        PathSeg::Index(i) => {
            if !root.is_array() {
                *root = serde_json::Value::Array(Vec::new());
            }
            let arr = root.as_array_mut().unwrap();
            while arr.len() <= *i {
                arr.push(serde_json::Value::Null);
            }
            if path.len() == 1 {
                arr[*i] = value;
            } else {
                set_nested(&mut arr[*i], &path[1..], value)?;
            }
        }
    }
    Ok(())
}

/// Give `env` read access to the ambient standard library (string, table,
/// math, pairs/ipairs/ etc.) by falling through to the real globals table on
/// any miss. Luau's baseline globals already exclude `io`/`os.execute`/
/// `require`, so this doesn't need its own allowlist.
///
/// Assignments (e.g. a Program's top-level `function on_get(...)`) land in
/// `env` itself, not the shared globals table, because Lua only consults
/// `__newindex` when the key is absent — and we never set one.
pub fn install_stdlib(lua: &Lua, env: &Table) -> mlua::Result<()> {
    let mt = lua.create_table()?;
    mt.set("__index", lua.globals())?;
    env.set_metatable(Some(mt));
    Ok(())
}

/// Resolve a Lua argument that names an Object: either the object table
/// itself (as produced by [`object_to_value`], read off its `ref_id`
/// field) or a plain ref-string.
fn ref_of(v: &Value) -> mlua::Result<String> {
    match v {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Table(t) => {
            // Proxy tables (issue #19) carry their ref in a hidden raw field —
            // prefer it; a plain get would round-trip through __index.
            if let Ok(Some(r)) = t.raw_get::<Option<String>>("_hearth_ref") {
                return Ok(r);
            }
            let r: Option<String> = t.get("ref_id")?;
            r.ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "expected an object (with a ref_id field) or a ref string".into(),
                )
            })
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "expected an object or a ref string, got {}",
            other.type_name()
        ))),
    }
}

fn parse_tag(spec: &str) -> mlua::Result<Tag> {
    Tag::parse(spec).map_err(mlua::Error::RuntimeError)
}

/// Refuse writing a lib module whose `<name>` collides with a shipped module
/// (embedded stdlib or `<game_dir>/lib`) — loud and early beats a library
/// silently shadowing `str` server-wide.
fn refuse_if_shipped_lib(lua: &Lua, name: &str) -> mlua::Result<()> {
    let sources: Table = lua.named_registry_value(crate::softcode::MODULE_SOURCES_KEY)?;
    if sources.contains_key(name)? {
        return Err(mlua::Error::RuntimeError(format!(
            "set_lib: '{}' is a shipped module — choose a different library name",
            name
        )));
    }
    Ok(())
}

/// Build the table representation of the Object at `ref_id`, or `nil` if it
/// doesn't exist. This is a snapshot taken at call time — mutating it in
/// Lua does not touch the world; only the write API does that.
pub fn object_to_value(
    lua: &Lua,
    world: &World,
    ref_id: &str,
    proxy_mt: Option<&Table>,
) -> mlua::Result<Value> {
    match world.get(ref_id) {
        Some(obj) => Ok(Value::Table(object_to_table(lua, world, obj, proxy_mt)?)),
        None => Ok(Value::Nil),
    }
}

/// The fields resolvable on the hook-facing object proxy — the `__index`
/// allowlist. Proxy tables are EMPTY (metamethods fire only on absent keys,
/// so any pre-populated field would swallow writes into the throwaway
/// snapshot); every one of these names resolves through `__index` instead.
/// Completeness is enforced by `object_member_reference_matches_engine_snapshot`.
const OBJECT_FIELDS: &[&str] = &[
    "ref_id",
    "key",
    "kind",
    "title",
    "display_name",
    "description",
    "location_ref",
    "owner_ref",
    "archetype_ref",
    "attrs",
    "tags",
];

/// Keys that carry engine meaning and must never be written through property
/// assignment. Each error names the API that owns the mutation.
const PROTECTED_FIELDS: &[(&str, &str)] = &[
    ("ref_id", "identity — it cannot be changed"),
    ("key", "authoring key — it cannot be changed"),
    ("kind", "kind — it cannot be changed"),
    ("location_ref", "read-only — use move_object(ref, destination)"),
];

fn object_to_table(
    lua: &Lua,
    world: &World,
    obj: &GameObject,
    proxy_mt: Option<&Table>,
) -> mlua::Result<Table> {
    if let Some(mt) = proxy_mt {
        // Hook-facing proxy: an empty table whose reads/writes resolve through
        // the shared metatable's __index/__newindex against the live batch.
        // See issue #19 — a populated table would let `this.title = "x"`
        // raw-set the snapshot copy and silently evaporate.
        let t = lua.create_table()?;
        t.raw_set("_hearth_ref", obj.ref_id.clone())?;
        t.set_metatable(Some(mt.clone()));
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.set("ref_id", obj.ref_id.clone())?;
    t.set("key", obj.key.clone())?;
    t.set("kind", obj.kind.to_string())?;
    // Instance-first, then up the archetype chain — see
    // docs/plans/archetypes.md. Plain (non-proxy) snapshots are used by list
    // results (all_objects, get_room_contents, ...), so they get the same
    // delegation the hook-facing proxy's __index does.
    t.set("title", world.resolved_title(obj))?;
    t.set("display_name", world.display_name(obj))?;
    t.set("description", world.resolved_description(obj))?;
    t.set("location_ref", obj.location_ref.clone())?;
    t.set("owner_ref", obj.owner_ref.clone())?;
    t.set("archetype_ref", obj.archetype_ref.clone())?;

    let attrs = lua.create_table()?;
    for (k, v) in world.resolved_attrs(obj) {
        attrs.set(k, lua.to_value(&v)?)?;
    }
    t.set("attrs", attrs)?;

    // Union with the archetype chain — tags are additive-only (no per-
    // instance clear in Stage 1, see docs/plans/archetypes.md).
    let tags = lua.create_table()?;
    for (i, tag) in world.resolved_tags(obj).iter().enumerate() {
        tags.set(i + 1, tag.as_spec())?;
    }
    t.set("tags", tags)?;

    Ok(t)
}

fn exits_table(lua: &Lua, world: &World, room_ref: &str) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    for (i, exit) in world.exits_from(room_ref).into_iter().enumerate() {
        let e = lua.create_table()?;
        e.set("ref_id", exit.ref_id.clone())?;
        e.set("key", exit.key.clone())?;
        e.set("target_ref", exit.target_ref.clone())?;
        let aliases = lua.create_table()?;
        for (j, a) in exit.aliases.iter().enumerate() {
            aliases.set(j + 1, a.clone())?;
        }
        e.set("aliases", aliases)?;
        out.set(i + 1, e)?;
    }
    Ok(out)
}

/// Register the read/write API as fields on `env`. Read functions borrow
/// `world` directly; write functions push [`Intent`]s into `batch` instead
/// of touching it.
#[allow(clippy::too_many_arguments)]
/// Resolve an attribute for `target`, consulting the batch's pending writes
/// first (read-your-writes within a script), then the world snapshot. The
/// single source of truth shared by the `get_attr` function and the object
/// proxy's `__index` — property syntax and function syntax cannot drift.
fn resolve_attr<'a>(
    batch: &'a IntentBatch,
    world: &'a World,
    target: &str,
    key: &str,
) -> Option<&'a serde_json::Value> {
    match batch.pending_attr(target, key) {
        Some(Some(v)) => Some(v),
        // Pending unset/clear removes the instance's OWN value, so the
        // effective value becomes whatever the archetype chain provides —
        // matching what a read returns after the batch commits (`get_attr`
        // resolves up the chain, and the own value is gone). Resolve from the
        // archetype up, skipping the instance's own (being unset). Not a hard
        // miss — that's the bug `clear_attr`'s "revert to inheriting" fixes.
        // A non-archetyped object has nothing above, so this is `None` (nil),
        // exactly as before.
        Some(None) => world
            .get(target)
            .and_then(|o| o.archetype_ref.as_deref())
            .and_then(|a| world.get(a))
            .and_then(|anc| world.resolved_attr(anc, key)),
        // Instance-first, then up the archetype chain (World::resolved_attr)
        // — see docs/plans/archetypes.md. A pending write on the instance
        // always wins (handled above); an unwritten attr falls through to
        // whichever object in the chain — instance or ancestor — actually
        // has it.
        None => world.get(target).and_then(|o| world.resolved_attr(o, key)),
    }
}


/// Build the `pairs`/`__iter` state for an object proxy: parallel arrays of
/// keys and values over the snapshot fields (absent optionals skipped).
/// Snapshot-only by design — iteration sees the world as the script entered
/// it, not pending same-script writes.
fn object_pairs_state(lua: &Lua, world: &World, r: &str) -> mlua::Result<Table> {
    let state = lua.create_table()?;
    let mut i = 0i64;
    if let Some(o) = world.get(r) {
        for field in OBJECT_FIELDS {
            let v: Option<Value> = match *field {
                "ref_id" => Some(Value::String(lua.create_string(o.ref_id.as_str())?)),
                "key" => Some(Value::String(lua.create_string(o.key.as_str())?)),
                "kind" => {
                    let s = o.kind.to_string();
                    Some(Value::String(lua.create_string(s.as_str())?))
                }
                // Instance-first, then the archetype chain — see
                // docs/plans/archetypes.md.
                "title" => match world.resolved_title(o) {
                    Some(t) => Some(Value::String(lua.create_string(t.as_str())?)),
                    None => None,
                },
                "display_name" => {
                    Some(Value::String(lua.create_string(world.display_name(o))?))
                }
                "description" => {
                    let d = world.resolved_description(o);
                    Some(Value::String(lua.create_string(d.as_str())?))
                }
                "location_ref" => match &o.location_ref {
                    Some(l) => Some(Value::String(lua.create_string(l.as_str())?)),
                    None => None,
                },
                "owner_ref" => match &o.owner_ref {
                    Some(l) => Some(Value::String(lua.create_string(l.as_str())?)),
                    None => None,
                },
                "archetype_ref" => match &o.archetype_ref {
                    Some(l) => Some(Value::String(lua.create_string(l.as_str())?)),
                    None => None,
                },
                "tags" => {
                    let out = lua.create_table()?;
                    for (j, tag) in world.resolved_tags(o).iter().enumerate() {
                        out.set(j + 1, tag.as_spec())?;
                    }
                    Some(Value::Table(out))
                }
                _ => {
                    // "attrs" — merged with the archetype chain, instance
                    // wins per key (World::resolved_attrs).
                    let out = lua.create_table()?;
                    for (k, av) in world.resolved_attrs(o) {
                        out.set(k, lua.to_value(&av)?)?;
                    }
                    Some(Value::Table(out))
                }
            };
            if let Some(v) = v {
                i += 1;
                state.raw_set(i, *field)?;
                state.raw_set(format!("v{}", i), v)?;
                state
                    .raw_set(format!("p_{}", field), i)
                    .ok();
            }
        }
    }
    state.raw_set("n", i)?;
    Ok(state)
}

/// Same shape for the `attrs` sub-proxy: snapshot attr keys/values, merged
/// with the archetype chain (instance wins per key — see
/// `World::resolved_attrs`).
fn attrs_pairs_state(lua: &Lua, world: &World, r: &str) -> mlua::Result<Table> {
    let state = lua.create_table()?;
    let mut i = 0i64;
    if let Some(o) = world.get(r) {
        for (k, v) in world.resolved_attrs(o) {
            i += 1;
            state.raw_set(i, k.clone())?;
            state.raw_set(format!("v{}", i), lua.to_value(&v)?)?;
            state.raw_set(format!("p_{}", k), i).ok();
        }
    }
    state.raw_set("n", i)?;
    Ok(state)
}

pub fn install<'scope, 'env>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    env: &Table,
    world: &'env World,
    batch: Rc<RefCell<IntentBatch>>,
    default_location: Option<String>,
    dbref_counter: Rc<Cell<u64>>,
    themes: &'env std::collections::HashMap<String, Theme>,
    map_templates: &'env std::collections::HashMap<String, MapTemplateFile>,
    scheduled_hooks: &'env [crate::softcode::ScheduledHook],
    tick_count: u64,
    game_time: Option<serde_json::Value>,
    ink_runtime: &'env RefCell<ink::InkRuntime>,
) -> mlua::Result<Table> {
    // -- Object proxy (issue #19) --
    // Hook-facing objects (`this`, `actor`, `get_object`, …) are EMPTY tables
    // whose reads/writes resolve through these metamethods against the live
    // batch. The table must stay empty: __newindex fires only on absent keys,
    // so any pre-populated field would swallow writes into the throwaway
    // snapshot. Built once per install() — the handlers need `world` ('env),
    // so this is per-script-run, not process-global.

    // Shared pairs() iterator: walks a state table of parallel arrays
    // keys[1..n] / "v<i>"[1..n]. Snapshot-only by design: iteration sees the
    // world as the script entered it, not pending same-script writes.
    // Luau drives generalized iteration by passing the PREVIOUS KEY back as
    // the control value, so the state carries a p_<key> -> position map.
    let pairs_next = scope.create_function(
        move |_lua, (state, ctrl): (Table, Value)| {
            let n: i64 = state.raw_get("n")?;
            let i: i64 = match &ctrl {
                Value::Nil => 1,
                Value::String(s) => {
                    let name = s.to_str()?;
                    // Position AFTER the previous key — otherwise the same row
                    // yields forever.
                    state
                        .raw_get::<Option<i64>>(format!("p_{}", name))?
                        .map_or(n + 1, |prev| prev + 1)
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "invalid iteration control".into(),
                    ))
                }
            };
            if i > n {
                return Ok((Value::Nil, Value::Nil));
            }
            let k: Value = state.raw_get(i)?;
            let v: Value = state.raw_get(format!("v{}", i))?;
            Ok((k, v))
        },
    )?;

    // Sub-proxy for `this.attrs`: pending-aware reads, intent-pushing writes.
    let attrs_mt = {
        let mt = lua.create_table()?;
        let b = Rc::clone(&batch);
        mt.set(
            "__index",
            scope.create_function(move |lua, (t, key): (Table, Value)| {
                let r: String = t.raw_get("_hearth_ref")?;
                let key = match &key {
                    Value::String(s) => s.to_str()?.to_string(),
                    _ => return Ok(Value::Nil),
                };
                match resolve_attr(&b.borrow(), world, &r, &key) {
                    Some(v) => Ok(lua.to_value(v)?),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
        let b = Rc::clone(&batch);
        mt.set(
            "__newindex",
            scope.create_function(move |lua, (t, key, value): (Table, String, Value)| {
                let r: String = t.raw_get("_hearth_ref")?;
                // Same conversion as set_attr — coercion parity by construction.
                let intent = if value == Value::Nil {
                    Intent::UnsetAttr { target: r, key }
                } else {
                    let v: serde_json::Value = lua.from_value(value)?;
                    Intent::SetAttr { target: r, key, value: v }
                };
                b.borrow_mut().push(intent);
                Ok(())
            })?,
        )?;
        // Luau ignores __pairs but honors __iter (`for k,v in t`);
        // register both so either style works.
        for mm in ["__pairs", "__iter"] {
            let next = pairs_next.clone();
            mt.set(
                mm,
                scope.create_function(move |lua, t: Table| {
                    let r: String = t.raw_get("_hearth_ref")?;
                    let state = attrs_pairs_state(lua, world, &r)?;
                    Ok((next.clone(), state, Value::Nil))
                })?,
            )?;
        }
        mt
    };

    // Top-level object proxy.
    let obj_mt = {
        let mt = lua.create_table()?;
        let b = Rc::clone(&batch);
        let am = attrs_mt.clone();
        mt.set(
            "__index",
            scope.create_function(move |lua, (t, key): (Table, Value)| {
                let r: String = t.raw_get("_hearth_ref")?;
                let key = match &key {
                    Value::String(s) => s.to_str()?.to_string(),
                    _ => return Ok(Value::Nil),
                };
                // Attr-shaped keys resolve through the shared lookup first
                // (pending writes, then snapshot attrs) — arbitrary attr
                // names aren't in the allowlist, so this must come before it.
                {
                    let pending = b.borrow();
                    match resolve_attr(&pending, world, &r, &key) {
                        Some(v) => {
                            let out = lua.to_value(v)?;
                            return Ok(out);
                        }
                        None if pending.pending_attr(&r, &key).is_some() => {
                            return Ok(Value::Nil); // pending unset
                        }
                        None => {}
                    }
                }
                match key.as_str() {
                    "ref_id" => return Ok(Value::String(lua.create_string(r.as_str())?)),
                    "attrs" => {
                        let sp = lua.create_table()?;
                        sp.raw_set("_hearth_ref", r)?;
                        sp.set_metatable(Some(am.clone()));
                        return Ok(Value::Table(sp));
                    }
                    "tags" => {
                        let out = lua.create_table()?;
                        if let Some(o) = world.get(&r) {
                            for (i, tag) in world.resolved_tags(o).iter().enumerate() {
                                out.set(i + 1, tag.as_spec())?;
                            }
                        }
                        return Ok(Value::Table(out));
                    }
                    _ => {}
                }
                if !OBJECT_FIELDS.contains(&key.as_str()) {
                    return Ok(Value::Nil);
                }
                // Pending-aware fields first (read-your-writes), then
                // snapshot — falling through the archetype chain
                // (World::resolved_title/resolved_description) when the
                // instance itself has neither pending nor its own value. See
                // docs/plans/archetypes.md.
                match key.as_str() {
                    "title" | "description" => {
                        let pending = b.borrow();
                        let owned = match key.as_str() {
                            "title" => pending.pending_title(&r).map(str::to_string),
                            _ => pending.pending_description(&r).map(str::to_string),
                        };
                        drop(pending);
                        let v = owned.or_else(|| {
                            world.get(&r).and_then(|o| match key.as_str() {
                                "title" => world.resolved_title(o),
                                _ => Some(world.resolved_description(o)),
                            })
                        });
                        Ok(match v {
                            Some(s) => Value::String(lua.create_string(s)?),
                            None => Value::Nil,
                        })
                    }
                    "display_name" => Ok(match world.get(&r) {
                        Some(o) => Value::String(lua.create_string(world.display_name(o))?),
                        None => Value::Nil,
                    }),
                    "location_ref" | "owner_ref" | "archetype_ref" => {
                        let v = world.get(&r).and_then(|o| match key.as_str() {
                            "location_ref" => o.location_ref.clone(),
                            "owner_ref" => o.owner_ref.clone(),
                            _ => o.archetype_ref.clone(),
                        });
                        Ok(match v {
                            Some(s) => Value::String(lua.create_string(s)?),
                            None => Value::Nil,
                        })
                    }
                    // ref_id/key/kind handled above or plain from snapshot:
                    "key" | "kind" => Ok(match world.get(&r) {
                        Some(o) => {
                            let s = match key.as_str() {
                                "key" => o.key.clone(),
                                _ => o.kind.to_string(),
                            };
                            Value::String(lua.create_string(s)?)
                        }
                        None => Value::Nil,
                    }),
                    _ => Ok(Value::Nil),
                }
            })?,
        )?;
        let b = Rc::clone(&batch);
        mt.set(
            "__newindex",
            scope.create_function(move |lua, (_t, key, value): (Table, Value, Value)| {
                let r: String = _t.raw_get("_hearth_ref")?;
                let key: String = match &key {
                    Value::String(s) => s.to_str()?.to_string(),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "object fields are named — use an attr-style string key".into(),
                        ))
                    }
                };
                if let Some((_, hint)) = PROTECTED_FIELDS.iter().find(|(f, _)| *f == key.as_str()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "{} is {}",
                        key, hint
                    )));
                }
                match key.as_str() {
                    "title" | "description" => {
                        if value == Value::Nil {
                            return Err(mlua::Error::RuntimeError(format!(
                                "cannot unset {} — assign an empty string instead",
                                key
                            )));
                        }
                        let s: String = lua.from_value(value)?;
                        let mut btch = b.borrow_mut();
                        if key == "title" {
                            btch.push(Intent::SetTitle { target: r, title: s });
                        } else {
                            btch.push(Intent::SetDescription { target: r, description: s });
                        }
                        Ok(())
                    }
                    "display_name" | "attrs" | "tags" | "_hearth_ref" => {
                        Err(mlua::Error::RuntimeError(format!(
                            "{} is computed/container data — use set_attr/unset_attr or the tag API",
                            key
                        )))
                    }
                    _ => {
                        // Same conversion as set_attr — parity by construction.
                        let intent = if value == Value::Nil {
                            Intent::UnsetAttr { target: r, key }
                        } else {
                            let v: serde_json::Value = lua.from_value(value)?;
                            Intent::SetAttr { target: r, key, value: v }
                        };
                        b.borrow_mut().push(intent);
                        Ok(())
                    }
                }
            })?,
        )?;
        for mm in ["__pairs", "__iter"] {
            let next = pairs_next.clone();
            mt.set(
                mm,
                scope.create_function(move |lua, t: Table| {
                    let r: String = t.raw_get("_hearth_ref")?;
                    let state = object_pairs_state(lua, world, &r)?;
                    Ok((next.clone(), state, Value::Nil))
                })?,
            )?;
        }
        mt
    };

    // -- Read API --

    env.set("get_tick", tick_count)?;

    // get_time(): the in-world clock as a table (minute/hour/day/month/year,
    // is_day, optional weekday/day_name/month_name), or nil when no clock is
    // configured. A FUNCTION (unlike get_tick, a bare value). Builds a fresh
    // table per call so a caller mutating the result can't corrupt later reads.
    env.set(
        "get_time",
        scope.create_function(move |lua, ()| match &game_time {
            Some(v) => lua.to_value(v),
            None => Ok(Value::Nil),
        })?,
    )?;

    let mt = obj_mt.clone();
    env.set(
        "get_object",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            object_to_value(lua, world, &r, Some(&mt))
        })?,
    )?;

    env.set(
        "resolve_key",
        scope.create_function(move |_, file_key: String| {
            Ok(crate::loader::resolve_file_key(world, &file_key))
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "get_attr",
        scope.create_function(move |lua, (r, key): (Value, String)| {
            let r = ref_of(&r)?;
            match resolve_attr(&b.borrow(), world, &r, &key) {
                Some(v) => lua.to_value(v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "has_attr",
        scope.create_function(move |_, (r, key): (Value, String)| {
            let r = ref_of(&r)?;
            if let Some(pending) = b.borrow().pending_attr(&r, &key) {
                return Ok(pending.is_some());
            }
            // Instance-first, then up the archetype chain — see
            // docs/plans/archetypes.md.
            Ok(world
                .get(&r)
                .is_some_and(|o| world.resolved_attr(o, &key).is_some()))
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "pick",
        scope.create_function(move |lua, args: MultiValue| {
            let mut args_vec: Vec<Value> = args.into_vec();
            if args_vec.len() < 2 {
                return Err(mlua::Error::RuntimeError("pick: need at least (ref, attr_key)".into()));
            }
            let r = ref_of(&args_vec[0])?;
            let attr_key = match &args_vec[1] {
                Value::String(s) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::RuntimeError("pick: attr key must be a string".into())),
            };

            let root_val: serde_json::Value;
            if let Some(pending) = b.borrow().pending_attr(&r, &attr_key) {
                match pending {
                    Some(v) => root_val = v.clone(),
                    None => return Ok(Value::Nil),
                }
            } else {
                // Instance-first, then up the archetype chain.
                match world.get(&r).and_then(|o| world.resolved_attr(o, &attr_key)) {
                    Some(v) => root_val = v.clone(),
                    None => return Ok(Value::Nil),
                }
            }

            let path = args_vec.split_off(2);
            let mut current = &root_val;
            for key in &path {
                match key {
                    Value::Integer(i) => {
                        let idx = (*i - 1) as usize;
                        match current.as_array().and_then(|a| a.get(idx)) {
                            Some(v) => current = v,
                            None => return Ok(Value::Nil),
                        }
                    }
                    Value::String(s) => {
                        let s = s.to_str()?;
                        match current.as_object().and_then(|o| o.get(s.as_ref())) {
                            Some(v) => current = v,
                            None => return Ok(Value::Nil),
                        }
                    }
                    _ => return Err(mlua::Error::RuntimeError("pick: path keys must be strings or integers".into())),
                }
            }
            lua.to_value(current)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_val",
        scope.create_function(move |lua, args: MultiValue| {
            let args_vec: Vec<Value> = args.into_vec();
            if args_vec.len() < 4 {
                return Err(mlua::Error::RuntimeError("set_val: need at least (ref, attr_key, path_key, value)".into()));
            }
            let target = ref_of(&args_vec[0])?;
            let attr_key = match &args_vec[1] {
                Value::String(s) => s.to_str()?.to_string(),
                _ => return Err(mlua::Error::RuntimeError("set_val: attr key must be a string".into())),
            };
            let new_value: serde_json::Value = lua.from_value(args_vec[args_vec.len() - 1].clone())?;

            let mut path = Vec::new();
            for key in &args_vec[2..args_vec.len() - 1] {
                match key {
                    Value::Integer(i) => path.push(PathSeg::Index((*i - 1) as usize)),
                    Value::String(s) => path.push(PathSeg::Key(s.to_str()?.to_string())),
                    _ => return Err(mlua::Error::RuntimeError("set_val: path keys must be strings or integers".into())),
                }
            }

            // Copy-on-write: if `attr_key` isn't set on the instance itself
            // but resolves from an archetype, start from the FULL resolved
            // value (not just the leaf being edited) so the write-back below
            // doesn't drop the rest of the inherited object — see
            // docs/plans/archetypes.md. `resolved_attr` already does
            // instance-first-then-chain, so this one call covers both the
            // "own value" and "inherited value" cases.
            let mut root: serde_json::Value = world
                .get(&target)
                .and_then(|o| world.resolved_attr(o, &attr_key).cloned())
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            // Walk to the leaf and set the value using a pointer built from path
            set_nested(&mut root, &path, new_value).map_err(mlua::Error::RuntimeError)?;

            b.borrow_mut().push(Intent::SetAttr {
                target,
                key: attr_key,
                value: root,
            });
            Ok(())
        })?,
    )?;

    env.set(
        "has_tag",
        scope.create_function(move |_, (r, spec): (Value, String)| {
            let r = ref_of(&r)?;
            let tag = parse_tag(&spec)?;
            // Instance-first, then up the archetype chain.
            Ok(world
                .get(&r)
                .is_some_and(|o| world.resolved_tags(o).contains(&tag)))
        })?,
    )?;

    env.set(
        "get_tags",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let out = lua.create_table()?;
            if let Some(obj) = world.get(&r) {
                for (i, tag) in world.resolved_tags(obj).iter().enumerate() {
                    out.set(i + 1, tag.as_spec())?;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "get_room_contents",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let out = lua.create_table()?;
            for (i, obj) in world.objects_in(&r).into_iter().enumerate() {
                out.set(i + 1, object_to_table(lua, world, obj, None)?)?;
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "get_exits",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            exits_table(lua, world, &r)
        })?,
    )?;

    let loc_mt = obj_mt.clone();
    env.set(
        "get_location",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            match world.get(&r).and_then(|o| o.location_ref.as_deref()) {
                Some(loc) => {
                    object_to_value(lua, world, loc, Some(&loc_mt))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    env.set(
        "kind_of",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind.to_string()))
        })?,
    )?;

    // -- Extended read API --

    env.set(
        "find_by_tag",
        scope.create_function(move |lua, spec: String| {
            let tag = parse_tag(&spec)?;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects.values() {
                // Resolve up the archetype chain — an instance that inherits
                // the tag from its archetype is still found, matching has_tag
                // and the object snapshot.
                if world.resolved_tags(obj).contains(&tag) {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "find_by_attr",
        scope.create_function(move |lua, (key, value): (String, Value)| {
            let target: serde_json::Value = lua.from_value(value)?;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects.values() {
                // Instance-first, then up the archetype chain — an instance
                // that inherits the matching attr from its archetype is
                // still found.
                if world.resolved_attr(obj, &key).is_some_and(|v| *v == target) {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    let find_mt = obj_mt.clone();
    env.set(
        "find_in_room",
        scope.create_function(move |lua, (r, name): (Value, String)| {
            let room = ref_of(&r)?;
            let lower = name.to_lowercase();
            for obj in world.objects_in(&room) {
                if obj.key.to_lowercase().contains(&lower)
                    || world.display_name(obj).to_lowercase().contains(&lower)
                {
                    return object_to_value(lua, world, &obj.ref_id, Some(&find_mt));
                }
            }
            Ok(Value::Nil)
        })?,
    )?;

    env.set(
        "get_inventory",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects_in(&r) {
                if obj.kind == Kind::Item {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "get_players_in_room",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let offline = Tag { category: "system".into(), key: "offline".into() };
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects_in(&r) {
                if obj.kind == Kind::Player && !obj.tags.contains(&offline) {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "get_all_by_kind",
        scope.create_function(move |lua, kind_str: String| {
            let kind = Kind::parse(&kind_str).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("unknown kind '{}'", kind_str))
            })?;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects.values() {
                if obj.kind == kind {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "all_objects",
        scope.create_function(move |lua, ()| {
            let out = lua.create_table()?;
            for (i, ref_id) in world.objects.keys().enumerate() {
                out.set(i + 1, ref_id.clone())?;
            }
            Ok(out)
        })?,
    )?;

    // -- Predicates --

    env.set(
        "is_player",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind == Kind::Player).unwrap_or(false))
        })?,
    )?;

    env.set(
        "is_npc",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind == Kind::Npc).unwrap_or(false))
        })?,
    )?;

    env.set(
        "is_item",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind == Kind::Item).unwrap_or(false))
        })?,
    )?;

    env.set(
        "is_room",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind == Kind::Room).unwrap_or(false))
        })?,
    )?;

    env.set(
        "is_exit",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).map(|o| o.kind == Kind::Exit).unwrap_or(false))
        })?,
    )?;

    env.set(
        "exists",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world.get(&r).is_some())
        })?,
    )?;

    env.set(
        "is_carrying",
        scope.create_function(move |_, (actor, item_tag): (Value, String)| {
            let actor_ref = ref_of(&actor)?;
            let tag = parse_tag(&item_tag)?;
            Ok(world
                .objects_in(&actor_ref)
                .iter()
                .any(|o| world.resolved_tags(o).contains(&tag)))
        })?,
    )?;

    env.set(
        "same_room",
        scope.create_function(move |_, (a, b): (Value, Value)| {
            let a_ref = ref_of(&a)?;
            let b_ref = ref_of(&b)?;
            let a_loc = world.get(&a_ref).and_then(|o| o.location_ref.as_deref());
            let b_loc = world.get(&b_ref).and_then(|o| o.location_ref.as_deref());
            Ok(a_loc.is_some() && a_loc == b_loc)
        })?,
    )?;

    env.set(
        "is_container",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            let container_tag = Tag {
                category: "item".into(),
                key: "container".into(),
            };
            Ok(world
                .get(&r)
                .is_some_and(|o| world.resolved_tags(o).contains(&container_tag)))
        })?,
    )?;

    env.set(
        "get_contents",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects_in(&r) {
                if obj.kind == Kind::Item {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "get_owner",
        scope.create_function(move |_, r: Value| {
            let r = ref_of(&r)?;
            Ok(world
                .get(&r)
                .and_then(|o| o.owner_ref.clone()))
        })?,
    )?;

    env.set(
        "get_timers",
        scope.create_function(move |lua, r: Value| {
            let target_ref = ref_of(&r)?;
            let out = lua.create_table()?;
            let mut i = 1;
            for sh in scheduled_hooks {
                if sh.target == target_ref {
                    let entry = lua.create_table()?;
                    entry.set("hook", sh.hook.clone())?;
                    entry.set("fire_at_tick", sh.fire_at_tick)?;
                    if let Some(data) = &sh.data {
                        entry.set("data", lua.to_value(data)?)?;
                    }
                    out.set(i, entry)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "json_decode",
        scope.create_function(move |lua, s: String| {
            let val: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| mlua::Error::RuntimeError(format!("json_decode: {}", e)))?;
            lua.to_value(&val)
        })?,
    )?;

    env.set(
        "json_encode",
        scope.create_function(move |lua, v: Value| {
            let val: serde_json::Value = lua.from_value(v)?;
            let s = serde_json::to_string(&val)
                .map_err(|e| mlua::Error::RuntimeError(format!("json_encode: {}", e)))?;
            Ok(s)
        })?,
    )?;

    env.set(
        "log",
        scope.create_function(move |_, msg: String| {
            tracing::info!(softcode = true, "{}", msg);
            Ok(())
        })?,
    )?;

    // -- Write API (queues Intents; nothing here touches the world) --

    let b = Rc::clone(&batch);
    env.set(
        "set_attr",
        scope.create_function(move |lua, (r, key, value): (Value, String, Value)| {
            let target = ref_of(&r)?;
            if matches!(value, Value::Nil) {
                b.borrow_mut().push(Intent::UnsetAttr { target, key });
            } else {
                let value: serde_json::Value = lua.from_value(value)?;
                b.borrow_mut().push(Intent::SetAttr { target, key, value });
            }
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "unset_attr",
        scope.create_function(move |_, (r, key): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::UnsetAttr { target, key });
            Ok(())
        })?,
    )?;

    // `clear_attr` is the archetype-facing name for the exact same intent as
    // `unset_attr` (MOO `clear_property`): remove this instance's OWN
    // attribute override so `get_attr`/`this.attrs`, which already resolve
    // instance-first-then-up-the-chain (see docs/plans/archetypes.md), fall
    // through to the archetype's value again. "Clear the override so it
    // inherits again" reads differently from "delete this attr" even though
    // the write is identical — hence the alias rather than reusing
    // `unset_attr` in authored code.
    let b = Rc::clone(&batch);
    env.set(
        "clear_attr",
        scope.create_function(move |_, (r, key): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::UnsetAttr { target, key });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "emit",
        scope.create_function(move |_, (r, message): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::EmitActor { target, message });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "emit_room",
        scope.create_function(
            move |_, (r, message, exclude): (Value, String, Option<Table>)| {
                let room = ref_of(&r)?;
                let mut exclude_refs = Vec::new();
                if let Some(t) = exclude {
                    for pair in t.sequence_values::<Value>() {
                        exclude_refs.push(ref_of(&pair?)?);
                    }
                }
                b.borrow_mut().push(Intent::EmitRoom {
                    room,
                    message,
                    exclude: exclude_refs,
                });
                Ok(())
            },
        )?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "emit_data",
        scope.create_function(move |lua, (r, channel, data): (Value, String, Value)| {
            let target = ref_of(&r)?;
            let json: serde_json::Value = lua.from_value(data)?;
            b.borrow_mut().push(Intent::EmitData { target, channel, data: json });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "emit_nearby",
        scope.create_function(
            move |_, (r, x, y, radius, message, exclude): (Value, f64, f64, f64, String, Option<Table>)| {
                let room = ref_of(&r)?;
                let mut exclude_refs = Vec::new();
                if let Some(t) = exclude {
                    for pair in t.sequence_values::<Value>() {
                        exclude_refs.push(ref_of(&pair?)?);
                    }
                }
                b.borrow_mut().push(Intent::EmitNearby {
                    room,
                    x,
                    y,
                    radius,
                    message,
                    exclude: exclude_refs,
                });
                Ok(())
            },
        )?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "emit_radius",
        scope.create_function(
            move |_, (r, radius, messages, exclude): (Value, u32, Table, Option<Table>)| {
                let room = ref_of(&r)?;
                let mut msg_map = HashMap::new();
                for (dist, msg) in messages.pairs::<u32, String>().flatten() {
                    msg_map.insert(dist, msg);
                }
                let mut exclude_refs = Vec::new();
                if let Some(t) = exclude {
                    for pair in t.sequence_values::<Value>() {
                        exclude_refs.push(ref_of(&pair?)?);
                    }
                }
                b.borrow_mut().push(Intent::EmitRadius {
                    room,
                    radius,
                    messages: msg_map,
                    exclude: exclude_refs,
                });
                Ok(())
            },
        )?,
    )?;

    env.set(
        "get_nearby",
        scope.create_function(move |lua, (r, x, y, radius): (Value, f64, f64, f64)| {
            let room = ref_of(&r)?;
            let r2 = radius * radius;
            let out = lua.create_table()?;
            let mut i = 1;
            for obj in world.objects.values() {
                if obj.location_ref.as_deref() != Some(&room) {
                    continue;
                }
                let ox = obj.attrs.get("_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let oy = obj.attrs.get("_y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let dx = ox - x;
                let dy = oy - y;
                if dx * dx + dy * dy <= r2 {
                    out.set(i, object_to_table(lua, world, obj, None)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "transfer_attr",
        scope.create_function(move |_, (from, to, key, amount): (Value, Value, String, f64)| {
            let from = ref_of(&from)?;
            let to = ref_of(&to)?;
            b.borrow_mut().push(Intent::TransferAttr { from, to, key, amount });
            Ok(())
        })?,
    )?;

    env.set(
        "get_rooms_in_radius",
        scope.create_function(move |lua, (r, radius): (Value, u32)| {
            let room = ref_of(&r)?;
            let mut visited: HashMap<String, u32> = HashMap::new();
            let mut queue: std::collections::VecDeque<(String, u32)> = std::collections::VecDeque::new();
            visited.insert(room.clone(), 0);
            queue.push_back((room.clone(), 0));

            while let Some((current, dist)) = queue.pop_front() {
                if dist < radius {
                    for exit in world.exits_from(&current) {
                        if let Some(target_ref) = &exit.target_ref {
                            let muffle = exit.attrs.get("muffle").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let blocked = exit.attrs.get("blocked_sound").and_then(|v| v.as_bool()).unwrap_or(false);
                            if blocked { continue; }
                            let next_dist = dist + 1 + muffle;
                            if next_dist <= radius && !visited.contains_key(target_ref) {
                                visited.insert(target_ref.clone(), next_dist);
                                queue.push_back((target_ref.clone(), next_dist));
                            }
                        }
                    }
                }
            }

            let out = lua.create_table()?;
            for (i, (ref_id, dist)) in visited.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("ref", ref_id.clone())?;
                entry.set("distance", *dist)?;
                if let Some(obj) = world.get(ref_id) {
                    entry.set("name", world.display_name(obj))?;
                }
                out.set(i + 1, entry)?;
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "match_name",
        scope.create_function(move |_, (name, input): (String, String)| {
            let name_lower = name.to_lowercase();
            let input_lower = input.to_lowercase();
            Ok(name_lower == input_lower
                || name_lower.starts_with(&input_lower)
                || name_lower.split_whitespace().any(|w| w.starts_with(&input_lower)))
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "move_object",
        scope.create_function(move |_, (r, dest, opts): (Value, Value, Option<Table>)| {
            let target = ref_of(&r)?;
            let destination = ref_of(&dest)?;
            // Optional third arg: { announce = bool, fire_hooks = bool }.
            // Absent or nil → a bare, silent relocation (the original behavior).
            let (announce, fire_hooks) = match opts {
                Some(t) => (
                    t.get::<Option<bool>>("announce")?.unwrap_or(false),
                    t.get::<Option<bool>>("fire_hooks")?.unwrap_or(false),
                ),
                None => (false, false),
            };
            b.borrow_mut().push(Intent::Move {
                target,
                destination,
                announce,
                fire_hooks,
            });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_aliases",
        scope.create_function(move |_, (r, aliases): (Value, Vec<String>)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::SetAliases { target, aliases });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "update_exit",
        scope.create_function(move |_, (r, opts): (Value, Table)| {
            let target = ref_of(&r)?;
            let direction: Option<String> = opts
                .get::<Option<String>>("direction")?
                .filter(|s| !s.trim().is_empty());
            let destination = match opts.get::<Option<Value>>("destination")? {
                Some(Value::Nil) | None => None,
                Some(v) => Some(ref_of(&v)?),
            };
            b.borrow_mut().push(Intent::UpdateExit {
                target,
                direction,
                destination,
            });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_lock",
        scope.create_function(move |_, (r, hook, expr): (Value, String, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::SetLock { target, hook, expr });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "clear_lock",
        scope.create_function(move |_, (r, hook): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::ClearLock { target, hook });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    let dbref = Rc::clone(&dbref_counter);
    env.set(
        "clone_object",
        scope.create_function(move |_, (src, opts): (Value, Option<Table>)| {
            let source = ref_of(&src)?;
            let ref_id = {
                let id = dbref.get() + 1;
                dbref.set(id);
                format!("#{}", id)
            };
            let (location, owner) = match opts {
                Some(t) => {
                    let location = match t.get::<Option<Value>>("location")? {
                        Some(Value::Nil) | None => None,
                        Some(v) => Some(ref_of(&v)?),
                    };
                    let owner = match t.get::<Option<Value>>("owner")? {
                        Some(Value::Nil) | None => None,
                        Some(v) => Some(ref_of(&v)?),
                    };
                    (location, owner)
                }
                None => (None, None),
            };
            b.borrow_mut().push(Intent::CloneObject {
                ref_id: ref_id.clone(),
                source,
                location,
                owner,
            });
            Ok(ref_id)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "run_command_as",
        scope.create_function(move |_, (actor, command): (Value, String)| {
            let actor = ref_of(&actor)?;
            b.borrow_mut().push(Intent::RunCommandAs { actor, command });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_tag",
        scope.create_function(move |_, (r, spec): (Value, String)| {
            let target = ref_of(&r)?;
            let tag = parse_tag(&spec)?;
            b.borrow_mut().push(Intent::SetTag { target, tag });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "unset_tag",
        scope.create_function(move |_, (r, spec): (Value, String)| {
            let target = ref_of(&r)?;
            let tag = parse_tag(&spec)?;
            b.borrow_mut().push(Intent::UnsetTag { target, tag });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    let dbref = Rc::clone(&dbref_counter);
    let default_loc_for_spawn = default_location.clone();
    env.set(
        "spawn",
        scope.create_function(move |_, opts: Table| {
            let key: String = opts
                .get("key")
                .ok()
                .filter(|s: &String| !s.is_empty())
                .ok_or_else(|| mlua::Error::RuntimeError("spawn: 'key' is required".into()))?;
            let kind_str: String = opts.get("kind").unwrap_or_else(|_| "item".to_string());
            let kind = Kind::parse(&kind_str).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "spawn: unknown kind '{}' (want room, item, or npc)",
                    kind_str
                ))
            })?;
            if kind == Kind::Player {
                return Err(mlua::Error::RuntimeError(
                    "spawn: cannot spawn kind 'player'".into(),
                ));
            }
            if kind == Kind::Code {
                return Err(mlua::Error::RuntimeError(
                    "spawn: cannot spawn kind 'code' — use @script/@lib to author scripts and libraries".into(),
                ));
            }
            let title: Option<String> = opts.get("title").ok();
            let description: Option<String> = opts.get("description").ok();
            let location: Option<Value> = opts.get("location").ok();
            let location = match location {
                Some(v) => ref_of(&v)?,
                None => default_loc_for_spawn.clone().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "spawn: no 'location' given and no default room in context".into(),
                    )
                })?,
            };
            let ref_id: Option<String> = opts.get("ref").ok();
            let ref_id = ref_id.unwrap_or_else(|| {
                let id = dbref.get() + 1;
                dbref.set(id);
                format!("#{}", id)
            });

            let owner: Option<Value> = opts.get("owner").ok();
            let owner = match owner {
                Some(Value::Nil) | None => None,
                Some(v) => Some(ref_of(&v)?),
            };

            // Archetype: a dbref (`"#12"`), an object table, or a file key
            // (`"town/goblin"`) resolved the same way `resolve_key()` does —
            // "both" per docs/plans/archetypes.md's open question.
            let archetype: Option<Value> = opts.get("archetype").ok();
            let archetype = match archetype {
                Some(Value::Nil) | None => None,
                Some(v @ Value::Table(_)) => Some(ref_of(&v)?),
                Some(Value::String(s)) => {
                    let s = s.to_str()?.to_string();
                    if s.starts_with('#') {
                        Some(s)
                    } else {
                        Some(crate::loader::resolve_file_key(world, &s).ok_or_else(|| {
                            mlua::Error::RuntimeError(format!(
                                "spawn: no archetype '{}'",
                                s
                            ))
                        })?)
                    }
                }
                Some(other) => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "spawn: 'archetype' must be a ref, key, or object, got {}",
                        other.type_name()
                    )));
                }
            };
            if let Some(a) = &archetype
                && world.get(a).is_none()
            {
                return Err(mlua::Error::RuntimeError(format!(
                    "spawn: no archetype '{}'",
                    a
                )));
            }

            b.borrow_mut().push(Intent::Spawn {
                ref_id: ref_id.clone(),
                key,
                kind,
                title,
                description,
                location,
                owner,
                archetype,
            });
            Ok(ref_id)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_title",
        scope.create_function(move |_, (r, title): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::SetTitle { target, title });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_description",
        scope.create_function(move |_, (r, description): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut()
                .push(Intent::SetDescription { target, description });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "destroy",
        scope.create_function(move |_, r: Value| {
            let target = ref_of(&r)?;
            // Scripted destroy never cascades — an archetype with live
            // instances refuses (see apply_to's Destroy handling). Cascading
            // deletion is a deliberate, out-of-band admin action in Stage 1,
            // not something a hook can trigger in passing.
            b.borrow_mut().push(Intent::Destroy { target, cascade: false });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "clone",
        scope.create_function(move |_, r: Value| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::Detach { target: target.clone() });
            Ok(target)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_owner",
        scope.create_function(move |_, (r, owner): (Value, Value)| {
            let target = ref_of(&r)?;
            let owner = ref_of(&owner)?;
            b.borrow_mut()
                .push(Intent::SetOwner { target, owner });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_archetype",
        // Point `ref` at an existing archetype, or clear it with nil. Reparents
        // to an existing object — it does not create a new archetype.
        scope.create_function(move |_, (r, arch): (Value, Value)| {
            let target = ref_of(&r)?;
            let archetype = match arch {
                Value::Nil => None,
                other => Some(ref_of(&other)?),
            };
            b.borrow_mut()
                .push(Intent::SetArchetype { target, archetype });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    let dbref = Rc::clone(&dbref_counter);
    env.set(
        "create_exit",
        scope.create_function(move |_, opts: Table| {
            let source: Value = opts.get("source")?;
            let source = ref_of(&source)?;
            let direction: String = opts.get("direction")?;
            let target: Value = opts.get("target")?;
            let target = ref_of(&target)?;
            let aliases: Option<Table> = opts.get("aliases").ok();
            let alias_vec = match aliases {
                Some(tbl) => {
                    let mut v = Vec::new();
                    for pair in tbl.sequence_values::<String>() {
                        v.push(pair?);
                    }
                    v
                }
                None => Vec::new(),
            };

            let ref_id = {
                let id = dbref.get() + 1;
                dbref.set(id);
                format!("#{}", id)
            };

            b.borrow_mut().push(Intent::CreateExit {
                ref_id: ref_id.clone(),
                source,
                direction,
                target,
                aliases: alias_vec,
            });
            Ok(ref_id)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_script",
        scope.create_function(move |_, (r, source): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::SetScript { target, source });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "set_lib",
        scope.create_function(move |lua, (r, name, source): (Value, String, String)| {
            let target = ref_of(&r)?;
            refuse_if_shipped_lib(lua, &name)?;
            b.borrow_mut().push(Intent::SetLib { target, name, source });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "trigger",
        scope.create_function(move |lua, (r, hook, data): (Value, String, Option<Value>)| {
            let target = ref_of(&r)?;
            let data = match data {
                Some(v) if !matches!(v, Value::Nil) => {
                    Some(lua.from_value::<serde_json::Value>(v)?)
                }
                _ => None,
            };
            b.borrow_mut().push(Intent::Trigger { target, hook, data });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "after",
        scope.create_function(
            move |lua, (ticks, r, hook, data): (u64, Value, String, Option<Table>)| {
                let target = ref_of(&r)?;
                let data_map = match data {
                    Some(tbl) => {
                        let mut map = HashMap::new();
                        for pair in tbl.pairs::<String, Value>() {
                            let (k, v) = pair?;
                            let json_val: serde_json::Value = lua.from_value(v)?;
                            map.insert(k, json_val);
                        }
                        Some(map)
                    }
                    None => None,
                };
                b.borrow_mut()
                    .push(Intent::After { target, hook, ticks, data: data_map });
                Ok(())
            },
        )?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "cancel_after",
        scope.create_function(move |_, (r, hook): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut()
                .push(Intent::CancelAfter { target, hook });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "prompt",
        scope.create_function(move |_, (actor, obj, hook): (Value, Value, String)| {
            let actor_ref = ref_of(&actor)?;
            let obj_ref = ref_of(&obj)?;
            b.borrow_mut().push(Intent::SetAttr {
                target: actor_ref.clone(),
                key: "_prompt_object".into(),
                value: serde_json::Value::String(obj_ref),
            });
            b.borrow_mut().push(Intent::SetAttr {
                target: actor_ref,
                key: "_prompt_hook".into(),
                value: serde_json::Value::String(hook),
            });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    let dbref = Rc::clone(&dbref_counter);
    let default_loc = default_location.clone();
    env.set(
        "generate_dungeon",
        scope.create_function(move |_, (seed, config): (String, Option<Table>)| {
            let anchor_ref = default_loc.clone().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "generate_dungeon: no room context to anchor the dungeon (call it from a hook with an actor in a room)".into(),
                )
            })?;

            let zones = match config {
                Some(tbl) => {
                    let mut zones = Vec::new();
                    for pair in tbl.sequence_values::<Table>() {
                        let zt = pair?;
                        let theme_name: String = zt.get("theme").unwrap_or_else(|_| "crypt".to_string());
                        let room_count: Option<Table> = zt.get("room_count").ok();
                        let (min, max) = match room_count {
                            Some(rc) => {
                                let min: u32 = rc.get(1).unwrap_or(4);
                                let max: u32 = rc.get(2).unwrap_or(6);
                                (min, max)
                            }
                            None => (4, 6),
                        };
                        zones.push(crate::dungeon::ZoneConfig {
                            theme_name,
                            room_count_min: min,
                            room_count_max: max,
                            zone_number: zones.len() as u32 + 1,
                        });
                    }
                    zones
                }
                None => {
                    // Default: 3 zones with escalating difficulty
                    vec![
                        crate::dungeon::ZoneConfig { theme_name: "crypt".into(), room_count_min: 4, room_count_max: 6, zone_number: 1 },
                        crate::dungeon::ZoneConfig { theme_name: "crypt".into(), room_count_min: 5, room_count_max: 8, zone_number: 2 },
                        crate::dungeon::ZoneConfig { theme_name: "crypt".into(), room_count_min: 3, room_count_max: 4, zone_number: 3 },
                    ]
                }
            };

            let db_start = dbref.get() + 1;
            let result = crate::dungeon::generate(&seed, &zones, themes, db_start, &anchor_ref)
                .map_err(mlua::Error::RuntimeError)?;

            // Advance the dbref counter past all generated refs.
            let max_ref = result
                .intents
                .iter()
                .filter_map(|i| match i {
                    Intent::Spawn { ref_id, .. } | Intent::CreateExit { ref_id, .. } => {
                        ref_id.trim_start_matches('#').parse::<u64>().ok()
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(dbref.get());
            dbref.set(max_ref);

            // Push all intents into the batch.
            for intent in result.intents {
                b.borrow_mut().push(intent);
            }

            // Store the layout grid on the entrance room.
            b.borrow_mut().push(Intent::SetAttr {
                target: result.entrance_ref.clone(),
                key: "dungeon_layout".into(),
                value: result.layout.to_json(),
            });

            Ok(result.entrance_ref)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "destroy_dungeon",
        scope.create_function(move |_, seed: String| {
            let refs_to_destroy: Vec<String> = world
                .objects
                .values()
                .filter(|obj| {
                    obj.attrs
                        .get("dungeon_seed")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| s == seed)
                })
                .map(|obj| obj.ref_id.clone())
                .collect();

            // Also find exits belonging to these rooms.
            let room_set: std::collections::HashSet<&str> =
                refs_to_destroy.iter().map(|s| s.as_str()).collect();
            let exit_refs: Vec<String> = world
                .objects
                .values()
                .filter(|obj| {
                    obj.kind == Kind::Exit
                        && obj.location_ref.as_deref().is_some_and(|loc| room_set.contains(loc))
                })
                .map(|obj| obj.ref_id.clone())
                .collect();

            for target in refs_to_destroy.into_iter().chain(exit_refs) {
                b.borrow_mut().push(Intent::Destroy { target, cascade: false });
            }
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    let dbref = Rc::clone(&dbref_counter);
    let default_loc = default_location.clone();
    env.set(
        "instantiate_map",
        scope.create_function(move |lua, name: String| {
            let anchor_ref = default_loc.clone().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "instantiate_map: no room context to anchor the map (call it from a hook with an actor in a room)".into(),
                )
            })?;

            let template = map_templates.get(&name).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("instantiate_map: unknown map '{}'", name))
            })?;

            let db_start = dbref.get() + 1;
            let result = crate::map_template::instantiate(template, themes, world, db_start, &anchor_ref)
                .map_err(mlua::Error::RuntimeError)?;

            // Advance the dbref counter past all generated refs.
            let max_ref = result
                .intents
                .iter()
                .filter_map(|i| match i {
                    Intent::Spawn { ref_id, .. } | Intent::CreateExit { ref_id, .. } => {
                        ref_id.trim_start_matches('#').parse::<u64>().ok()
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(dbref.get());
            dbref.set(max_ref.max(dbref.get()));

            for intent in result.intents {
                b.borrow_mut().push(intent);
            }

            let out = lua.create_table()?;
            out.set("entrance_ref", result.entrance_ref)?;
            out.set("room_count", result.room_count)?;
            Ok(out)
        })?,
    )?;

    // 2D-grid movement within a single "grid room" (the wilderness model: a
    // player's position is `_x`/`_y` attrs, terrain comes from a map template —
    // there is NO room per cell). Moves the actor one cell in `dir`, honoring
    // terrain/cell passability, and fires the terrain's `on_leave`/`on_enter`
    // hooks (on its `archetype`, if any) so a game can attach terrain behavior
    // ("lava burns") without a room per square. Returns a table:
    //   { ok=false, reason="no_map" }              -- unknown map
    //   { ok=true, moved=false, reason="blocked" } -- impassable / off-grid
    //   { ok=true, moved=true, x, y, terrain }     -- moved
    let b = Rc::clone(&batch);
    env.set(
        "grid_move",
        scope.create_function(move |lua, (actor, map_name, dir): (Value, String, String)| {
            let actor = ref_of(&actor)?;
            let result = lua.create_table()?;

            let (dx, dy): (i64, i64) = match dir.to_lowercase().as_str() {
                "n" | "north" => (0, -1),
                "s" | "south" => (0, 1),
                "e" | "east" => (1, 0),
                "w" | "west" => (-1, 0),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "grid_move: unknown direction '{}' (use n/s/e/w)",
                        other
                    )));
                }
            };

            let Some(template) = map_templates.get(&map_name) else {
                result.set("ok", false)?;
                result.set("reason", "no_map")?;
                return Ok(result);
            };
            let grid = template.parse_grid();

            let cur = |key: &str| -> i64 {
                world
                    .get(&actor)
                    .and_then(|o| o.attrs.get(key))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            };
            let (x, y) = (cur("_x"), cur("_y"));
            let (nx, ny) = (x + dx, y + dy);

            // Terrain char at a cell (None = off-grid or a gap).
            let terrain_at = |cx: i64, cy: i64| -> Option<String> {
                if cx < 0 || cy < 0 {
                    return None;
                }
                let (cx, cy) = (cx as usize, cy as usize);
                if cy >= grid.height || cx >= grid.width {
                    return None;
                }
                grid.cells[cy][cx].map(|c| c.to_string())
            };

            let Some(new_terrain) = terrain_at(nx, ny) else {
                result.set("ok", true)?;
                result.set("moved", false)?;
                result.set("reason", "blocked")?;
                return Ok(result);
            };
            // Passability: a cell override wins, else the terrain's default,
            // else impassable (unknown terrain never lets you through).
            let passable = template
                .cells
                .get(&format!("{},{}", nx, ny))
                .and_then(|o| o.passable)
                .or_else(|| template.terrain.get(&new_terrain).map(|t| t.passable))
                .unwrap_or(false);
            if !passable {
                result.set("ok", true)?;
                result.set("moved", false)?;
                result.set("reason", "blocked")?;
                return Ok(result);
            }

            // Resolve a terrain's hook host (its `archetype` file key → dbref).
            let arch_of = |tkey: &str| -> Option<String> {
                template
                    .terrain
                    .get(tkey)
                    .and_then(|t| t.archetype.as_deref())
                    .and_then(|k| crate::loader::resolve_file_key(world, k))
            };

            let mut batch = b.borrow_mut();
            batch.push(Intent::SetAttr {
                target: actor.clone(),
                key: "_x".into(),
                value: serde_json::json!(nx),
            });
            batch.push(Intent::SetAttr {
                target: actor.clone(),
                key: "_y".into(),
                value: serde_json::json!(ny),
            });
            // on_leave the old terrain, on_enter the new — fired after the move
            // commits (deferred TriggerHook), with the moving actor as the
            // ambient actor and the cell in the trigger data.
            if let Some(old_terrain) = terrain_at(x, y)
                && let Some(host) = arch_of(&old_terrain)
            {
                batch.push(Intent::Trigger {
                    target: host,
                    hook: "on_leave".into(),
                    data: Some(serde_json::json!({ "x": x, "y": y, "map": map_name, "terrain": old_terrain })),
                });
            }
            if let Some(host) = arch_of(&new_terrain) {
                batch.push(Intent::Trigger {
                    target: host,
                    hook: "on_enter".into(),
                    data: Some(serde_json::json!({ "x": nx, "y": ny, "map": map_name, "terrain": new_terrain })),
                });
            }
            drop(batch);

            result.set("ok", true)?;
            result.set("moved", true)?;
            result.set("x", nx)?;
            result.set("y", ny)?;
            result.set("terrain", new_terrain)?;
            Ok(result)
        })?,
    )?;

    env.set(
        "get_map_template",
        scope.create_function(move |lua, name: String| {
            let template = map_templates.get(&name).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("get_map_template: unknown map '{}'", name))
            })?;

            let grid = template.parse_grid();
            let out = lua.create_table()?;
            out.set("name", template.map.name.clone())?;
            out.set("width", grid.width)?;
            out.set("height", grid.height)?;

            let cells_table = lua.create_table()?;
            for y in 0..grid.height {
                for x in 0..grid.width {
                    if let Some(ch) = grid.cells[y][x] {
                        let key = format!("{},{}", x, y);
                        let cell = lua.create_table()?;
                        cell.set("x", x)?;
                        cell.set("y", y)?;
                        let terrain_key = ch.to_string();
                        cell.set("terrain", terrain_key.clone())?;

                        if let Some(terrain) = template.terrain.get(&terrain_key) {
                            cell.set("theme", terrain.theme.clone())?;
                            cell.set("passable", terrain.passable)?;
                            if let Some(prefix) = &terrain.title_prefix {
                                cell.set("title_prefix", prefix.clone())?;
                            }
                            if let Some(archetype) = &terrain.archetype {
                                cell.set("archetype", archetype.clone())?;
                            }
                            if !terrain.attrs.is_empty() {
                                let attrs = lua.create_table()?;
                                for (k, v) in &terrain.attrs {
                                    attrs.set(k.clone(), lua.to_value(v)?)?;
                                }
                                cell.set("terrain_attrs", attrs)?;
                            }
                        }

                        if let Some(ov) = template.cells.get(&key) {
                            if let Some(title) = &ov.title {
                                cell.set("title", title.clone())?;
                            }
                            if let Some(desc) = &ov.description {
                                cell.set("description", desc.clone())?;
                            }
                            if let Some(fixed) = &ov.fixed_room {
                                cell.set("fixed_room", fixed.clone())?;
                            }
                            if let Some(p) = ov.passable {
                                cell.set("passable", p)?;
                            }
                            if !ov.encounters.is_empty() {
                                let enc = lua.create_table()?;
                                for (i, e) in ov.encounters.iter().enumerate() {
                                    let entry = lua.create_table()?;
                                    entry.set("monster", e.monster.clone())?;
                                    entry.set("count_min", e.count[0])?;
                                    entry.set("count_max", e.count[1])?;
                                    enc.set(i + 1, entry)?;
                                }
                                cell.set("encounters", enc)?;
                            }
                        }

                        cells_table.set(key, cell)?;
                    }
                }
            }
            out.set("cells", cells_table)?;

            let terrain_table = lua.create_table()?;
            for (key, def) in &template.terrain {
                let t = lua.create_table()?;
                t.set("theme", def.theme.clone())?;
                t.set("passable", def.passable)?;
                if let Some(prefix) = &def.title_prefix {
                    t.set("title_prefix", prefix.clone())?;
                }
                if let Some(color) = &def.color {
                    t.set("color", color.clone())?;
                }
                if let Some(archetype) = &def.archetype {
                    t.set("archetype", archetype.clone())?;
                }
                if let Some(image) = &def.tile_image {
                    t.set("tile_image", image.clone())?;
                    t.set("tile_rotation", def.tile_rotation.as_str())?;
                }
                if !def.attrs.is_empty() {
                    let attrs = lua.create_table()?;
                    for (k, v) in &def.attrs {
                        attrs.set(k.clone(), lua.to_value(v)?)?;
                    }
                    t.set("attrs", attrs)?;
                }
                terrain_table.set(key.clone(), t)?;
            }
            out.set("terrain", terrain_table)?;

            Ok(out)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "apply_template",
        scope.create_function(move |_, (r, template): (Value, Value)| {
            let target = ref_of(&r)?;
            // A template is behavior for an object. In the object-script model
            // that is a single Luau chunk defining the hook functions. Accept
            // either the whole source directly, or a table of source fragments
            // (each defining one or more hook functions) that are concatenated
            // — order is irrelevant since they are all top-level definitions.
            let source = match template {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Table(t) => {
                    let mut parts: Vec<String> = Vec::new();
                    for pair in t.pairs::<Value, String>() {
                        let (_, src) = pair?;
                        parts.push(src);
                    }
                    parts.join("\n\n")
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "apply_template: expected a source string or table, got {}",
                        other.type_name()
                    )));
                }
            };
            b.borrow_mut().push(Intent::SetScript { target, source });
            Ok(())
        })?,
    )?;

    // -- Ink dialog API --

    env.set(
        "ink_start",
        scope.create_function(
            move |lua, (actor, npc, opts): (Value, Value, Option<Table>)| {
                let player_ref = ref_of(&actor)?;
                let npc_ref = ref_of(&npc)?;

                let source = if let Some(ref opts) = opts {
                    if let Ok(file_name) = opts.get::<String>("file") {
                        ink_runtime
                            .borrow()
                            .read_ink_file(&file_name)
                            .map_err(mlua::Error::external)?
                    } else {
                        npc_ink_source(world, &npc_ref)?
                    }
                } else {
                    npc_ink_source(world, &npc_ref)?
                };

                let resume = opts
                    .as_ref()
                    .and_then(|o| o.get::<bool>("resume").ok())
                    .unwrap_or(true);
                let state_key = format!("_ink_state_{player_ref}");
                let saved_state = if resume {
                    world
                        .get(&npc_ref)
                        .and_then(|o| o.attrs.get(&state_key))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                } else {
                    None
                };

                let output = ink_runtime
                    .borrow_mut()
                    .start_conversation(
                        &player_ref,
                        &npc_ref,
                        &source,
                        saved_state.as_deref(),
                    )
                    .map_err(mlua::Error::external)?;
                ink_output_to_table(lua, &output)
            },
        )?,
    )?;

    env.set(
        "ink_continue",
        scope.create_function(move |lua, (actor, npc): (Value, Value)| {
            let player_ref = ref_of(&actor)?;
            let npc_ref = ref_of(&npc)?;
            let output = ink_runtime
                .borrow_mut()
                .continue_story(&player_ref, &npc_ref)
                .map_err(mlua::Error::external)?;
            ink_output_to_table(lua, &output)
        })?,
    )?;

    env.set(
        "ink_choose",
        scope.create_function(move |lua, (actor, npc, index): (Value, Value, usize)| {
            let player_ref = ref_of(&actor)?;
            let npc_ref = ref_of(&npc)?;
            let output = ink_runtime
                .borrow_mut()
                .choose(&player_ref, &npc_ref, index)
                .map_err(mlua::Error::external)?;
            ink_output_to_table(lua, &output)
        })?,
    )?;

    env.set(
        "ink_get_var",
        scope.create_function(move |lua, (actor, npc, name): (Value, Value, String)| {
            let player_ref = ref_of(&actor)?;
            let npc_ref = ref_of(&npc)?;
            let val = ink_runtime
                .borrow()
                .get_variable(&player_ref, &npc_ref, &name)
                .map_err(mlua::Error::external)?;
            match val {
                Some(v) => lua.to_value(&v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    env.set(
        "ink_set_var",
        scope.create_function(
            move |lua, (actor, npc, name, val): (Value, Value, String, Value)| {
                let player_ref = ref_of(&actor)?;
                let npc_ref = ref_of(&npc)?;
                let json_val: serde_json::Value = lua.from_value(val)?;
                ink_runtime
                    .borrow_mut()
                    .set_variable(&player_ref, &npc_ref, &name, &json_val)
                    .map_err(mlua::Error::external)?;
                Ok(())
            },
        )?,
    )?;

    {
        let b = Rc::clone(&batch);
        env.set(
            "ink_end",
            scope.create_function(
                move |_, (actor, npc, save): (Value, Value, Option<bool>)| {
                    let player_ref = ref_of(&actor)?;
                    let npc_ref = ref_of(&npc)?;
                    let save = save.unwrap_or(false);
                    let state = ink_runtime
                        .borrow_mut()
                        .end_conversation(&player_ref, &npc_ref, save)
                        .map_err(mlua::Error::external)?;
                    if let Some(state_json) = state {
                        let key = format!("_ink_state_{player_ref}");
                        b.borrow_mut().push(Intent::SetAttr {
                            target: npc_ref,
                            key,
                            value: serde_json::Value::String(state_json),
                        });
                    }
                    Ok(true)
                },
            )?,
        )?;
    }

    env.set(
        "ink_goto",
        scope.create_function(
            move |lua, (actor, npc, path): (Value, Value, String)| {
                let player_ref = ref_of(&actor)?;
                let npc_ref = ref_of(&npc)?;
                let output = ink_runtime
                    .borrow_mut()
                    .goto(&player_ref, &npc_ref, &path)
                    .map_err(mlua::Error::external)?;
                ink_output_to_table(lua, &output)
            },
        )?,
    )?;

    Ok(obj_mt)
}

fn npc_ink_source(world: &World, npc_ref: &str) -> mlua::Result<String> {
    world
        .get(npc_ref)
        .and_then(|o| o.attrs.get("_ink_source"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "object {npc_ref} has no _ink_source attribute"
            ))
        })
}

fn ink_output_to_table(lua: &Lua, output: &ink::InkOutput) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    t.set("text", output.text.clone())?;
    t.set("can_continue", output.can_continue)?;
    t.set("ended", output.ended)?;

    let choices = lua.create_table()?;
    for (i, choice) in output.choices.iter().enumerate() {
        let c = lua.create_table()?;
        c.set("index", choice.index)?;
        c.set("text", choice.text.clone())?;
        let tags = lua.create_table()?;
        for (j, tag) in choice.tags.iter().enumerate() {
            tags.set(j + 1, tag.clone())?;
        }
        c.set("tags", tags)?;
        choices.set(i + 1, c)?;
    }
    t.set("choices", choices)?;

    let tags = lua.create_table()?;
    for (i, tag) in output.tags.iter().enumerate() {
        tags.set(i + 1, tag.clone())?;
    }
    t.set("tags", tags)?;

    Ok(Value::Table(t))
}

#[cfg(test)]
mod tests {
    // (No `use super::*` — this test only needs include_str! + std, and an
    // unused glob dirties the build.)

    /// The softcode API's *name set* lives in three hand-maintained mirrors:
    /// the engine's registrations, the web editor's Help reference
    /// (`hearth-api.js`), and the engine-owned LSP types (`types/hearth.d.luau`,
    /// which downstream games vendor). This test keeps all three in exact
    /// agreement — additions, removals, and renames all fail here (a count
    /// alone would let a swap through silently). Signature/doc text is out of
    /// scope; that needs the future introspection endpoint.
    ///
    /// "Registered" spans every install site: `env.set("name", …)` here (the
    /// per-script sandbox env) and in `mod.rs` (`pass`, installed per-hook by
    /// `run_hook_level` rather than in this file's shared `install`) plus
    /// `lua.globals().set("name", …)` in noise.rs and grid.rs (true globals).
    /// All four files are scanned only up to their `#[cfg(test)]` so test
    /// code can't register phantom names.
    ///
    /// Known fragility: this is a textual scan for literal quoted names after
    /// `env.set(` / `globals().set(`. If a function is ever registered via a
    /// helper, a namespace table, or a non-literal name, the scan will miss
    /// it silently — revisit if the install path grows indirection.
    #[test]
    fn help_panel_api_reference_matches_installed_functions() {
        use std::collections::BTreeSet;
        let api_rs = include_str!("api.rs");
        let softcode_mod_rs = include_str!("mod.rs");
        let noise_rs = include_str!("../noise.rs");
        let grid_rs = include_str!("../grid.rs");
        let js = include_str!("../../web/src/components/code/hearth-api.js");
        let dts = include_str!("../../types/hearth.d.luau");

        // The first quoted string after each `marker`, allowing whitespace or
        // newlines between the marker and the opening quote (registrations are
        // often multiline). Scan stops at `#[cfg(test)]`.
        fn quoted_after<'a>(src: &'a str, marker: &str) -> BTreeSet<&'a str> {
            let src = src.split("#[cfg(test)]").next().unwrap();
            let mut names = BTreeSet::new();
            let mut rest = src;
            while let Some(pos) = rest.find(marker) {
                rest = &rest[pos + marker.len()..];
                let trimmed = rest.trim_start();
                if let Some(after_quote) = trimmed.strip_prefix('"')
                    && let Some(end) = after_quote.find('"')
                {
                    names.insert(&after_quote[..end]);
                }
            }
            names
        }

        // Registered: env functions here, plus the noise/grid globals, plus
        // `pass` — installed per-hook by mod.rs's `run_hook_level` rather
        // than through this file's shared `install` (it needs the resolving
        // ref `install` doesn't have). Scoped to just that function's body,
        // not all of mod.rs: mod.rs also `env.set`s names that are NOT part
        // of the softcode API surface (run_eval's `actor`, run_tests'
        // `ctx`/`assert_*` test-harness globals) — a blanket scan of the
        // whole file would wrongly demand those show up in the Help panel
        // and LSP types too. The globals scan keys off `globals()` then the
        // following `.set("name"`, so unrelated `.set(` calls (e.g. grid.rs
        // result-table builders) that aren't preceded by `globals()` are
        // ignored.
        let mut registered = quoted_after(api_rs, "env.set(");
        let run_hook_level_body = softcode_mod_rs
            .split("fn run_hook_level")
            .nth(1)
            .expect("run_hook_level missing from mod.rs")
            .split("\n    pub fn run_eval")
            .next()
            .expect("run_eval missing from mod.rs (used as run_hook_level's end marker)");
        registered.extend(quoted_after(run_hook_level_body, "env.set("));
        for src in [noise_rs, grid_rs] {
            let body = src.split("#[cfg(test)]").next().unwrap();
            let mut rest = body;
            while let Some(pos) = rest.find("globals()") {
                rest = &rest[pos + "globals()".len()..];
                if let Some(after_set) = rest.trim_start().strip_prefix(".set(")
                    && let Some(after_quote) = after_set.trim_start().strip_prefix('"')
                    && let Some(end) = after_quote.find('"')
                {
                    registered.insert(&after_quote[..end]);
                }
            }
        }
        assert!(
            registered.len() > 80,
            "install-site scan found only {} registrations — the parser is broken",
            registered.len()
        );

        // Referenced (hearth-api.js): every `['name', ...]` row in the
        // API_FUNCTIONS block (API_GLOBALS/OBJECT_MEMBERS are locals and
        // members, not installed functions).
        let referenced: BTreeSet<&str> = {
            let section = js
                .split("export const API_FUNCTIONS")
                .nth(1)
                .expect("API_FUNCTIONS missing from hearth-api.js");
            let section = section.split("export const").next().unwrap();
            let mut names = BTreeSet::new();
            let mut rest = section;
            while let Some(pos) = rest.find("['") {
                rest = &rest[pos + 2..];
                if let Some(end) = rest.find('\'') {
                    names.insert(&rest[..end]);
                    rest = &rest[end..];
                } else {
                    break;
                }
            }
            names
        };

        // Declared (hearth.d.luau): both `declare function name(` functions
        // and bare `declare name:` value globals (e.g. get_tick).
        let declared: BTreeSet<&str> = {
            let mut names = BTreeSet::new();
            let mut rest = dts;
            while let Some(pos) = rest.find("declare ") {
                rest = &rest[pos + "declare ".len()..];
                let after = rest.strip_prefix("function ").unwrap_or(rest);
                let end = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(after.len());
                if end > 0 {
                    names.insert(&after[..end]);
                }
            }
            names
        };

        let report = |label: &str, mirror: &BTreeSet<&str>| {
            let missing: Vec<_> = registered.difference(mirror).collect();
            let stale: Vec<_> = mirror.difference(&registered).collect();
            assert!(
                missing.is_empty() && stale.is_empty(),
                "{label} drifted from the engine:\n  not in the mirror (add it): {missing:?}\n  no longer an engine name (remove it): {stale:?}",
            );
        };
        report("hearth-api.js", &referenced);
        report("types/hearth.d.luau", &declared);
    }

    /// The `Object` shape lives in three hand-maintained places: the engine
    /// snapshot (`object_to_table` here), the editor's `OBJECT_MEMBERS` (drives
    /// `x.` completion and the Help panel), and the engine-owned
    /// `types/hearth.d.luau` (`type Object`, for luau-lsp; downstream games
    /// vendor it). All three are checked hard and in-repo, so the shape can't
    /// drift the way `get_val` did.
    #[test]
    fn object_member_reference_matches_engine_snapshot() {
        use std::collections::BTreeSet;
        let api_rs = include_str!("api.rs");
        let js = include_str!("../../web/src/components/code/hearth-api.js");
        let dts = include_str!("../../types/hearth.d.luau");

        // Fields the engine sets on each object table: `t.set("field", …)` in
        // object_to_table's plain (list-result) branch. The hook-facing proxy
        // resolves its fields through __index instead — checked separately
        // against OBJECT_FIELDS below.
        let snapshot: BTreeSet<String> = {
            let start = api_rs
                .find("fn object_to_table")
                .expect("object_to_table gone from api.rs");
            let body = &api_rs[start..];
            // Stop at the next top-level `fn ` so we scan only this function.
            let end = body[3..].find("\nfn ").map(|p| p + 3).unwrap_or(body.len());
            let body = &body[..end];
            let mut names = BTreeSet::new();
            let mut rest = body;
            while let Some(pos) = rest.find("t.set(\"") {
                rest = &rest[pos + "t.set(\"".len()..];
                if let Some(e) = rest.find('"') {
                    names.insert(rest[..e].to_string());
                }
            }
            names
        };
        assert!(
            snapshot.len() >= 8,
            "object_to_table scan found only {} fields — the parser is broken",
            snapshot.len()
        );

        // The hook-facing proxy's __index allowlist must expose exactly the
        // same field set as the plain snapshot shape.
        let allowlist: BTreeSet<String> = {
            let start = api_rs
                .find("const OBJECT_FIELDS")
                .expect("OBJECT_FIELDS gone from api.rs");
            let body = &api_rs[start..];
            let end = body.find("];").expect("OBJECT_FIELDS unterminated");
            let mut names = BTreeSet::new();
            let mut rest = &body[..end];
            while let Some(pos) = rest.find('"') {
                rest = &rest[pos + 1..];
                if let Some(e) = rest.find('"') {
                    let name = &rest[..e];
                    if !name.trim().is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        names.insert(name.to_string());
                    }
                    rest = &rest[e..];
                } else {
                    break;
                }
            }
            names
        };
        assert_eq!(
            snapshot, allowlist,
            "__index allowlist drifted from the plain snapshot shape"
        );

        // Names in the `OBJECT_MEMBERS = [ ['name', 'doc'], … ]` block.
        let members: BTreeSet<String> = {
            let section = js
                .split("export const OBJECT_MEMBERS")
                .nth(1)
                .expect("OBJECT_MEMBERS missing from hearth-api.js");
            let section = section.split("];").next().unwrap();
            let mut names = BTreeSet::new();
            let mut rest = section;
            while let Some(pos) = rest.find("['") {
                rest = &rest[pos + 2..];
                if let Some(e) = rest.find('\'') {
                    names.insert(rest[..e].to_string());
                    rest = &rest[e..];
                } else {
                    break;
                }
            }
            names
        };

        let missing: Vec<_> = snapshot.difference(&members).collect();
        let stale: Vec<_> = members.difference(&snapshot).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "OBJECT_MEMBERS drifted from the engine object snapshot:\n  missing (add to hearth-api.js): {:?}\n  stale (remove from hearth-api.js): {:?}",
            missing,
            stale
        );

        // The engine-owned LSP types (`type Object`) must describe the same
        // field set. Hard include_str! now that the file lives in this repo.
        {
            let after = dts
                .split("type Object = {")
                .nth(1)
                .expect("`type Object` missing from types/hearth.d.luau");
            // Terminate at the closing brace on its own line, not the inline
            // `{ [string]: any }` braces on the attrs/tags fields.
            let block = after.split("\n}").next().unwrap_or("");
            let typed: BTreeSet<String> = block
                .lines()
                .filter_map(|l| l.trim().split_once(':').map(|(n, _)| n.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            let missing_t: Vec<_> = snapshot.difference(&typed).collect();
            let stale_t: Vec<_> = typed.difference(&snapshot).collect();
            assert!(
                missing_t.is_empty() && stale_t.is_empty(),
                "hearth.d.luau `type Object` drifted from the engine snapshot:\n  missing (add to the .d.luau): {:?}\n  stale (remove from the .d.luau): {:?}",
                missing_t,
                stale_t
            );
        }
    }
}
