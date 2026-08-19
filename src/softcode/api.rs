//! The Lua-facing surface Programs run against: a read API backed directly
//! by [`World`], and a write API that only ever pushes [`Intent`]s into the
//! batch — see ADR 0001. Nothing here ever gets a `&mut World`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, Scope, Table, Value};

use crate::map_template::MapTemplateFile;
use crate::theme::Theme;
use crate::world::{GameObject, Kind, Tag, World};
use crate::softcode::{Intent, IntentBatch};

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

/// Build the table representation of the Object at `ref_id`, or `nil` if it
/// doesn't exist. This is a snapshot taken at call time — mutating it in
/// Lua does not touch the world; only the write API does that.
pub fn object_to_value(lua: &Lua, world: &World, ref_id: &str) -> mlua::Result<Value> {
    match world.get(ref_id) {
        Some(obj) => Ok(Value::Table(object_to_table(lua, obj)?)),
        None => Ok(Value::Nil),
    }
}

fn object_to_table(lua: &Lua, obj: &GameObject) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("ref_id", obj.ref_id.clone())?;
    t.set("key", obj.key.clone())?;
    t.set("kind", obj.kind.to_string())?;
    t.set("title", obj.title.clone())?;
    t.set("display_name", obj.display_name().to_string())?;
    t.set("description", obj.description.clone())?;
    t.set("location_ref", obj.location_ref.clone())?;
    t.set("owner_ref", obj.owner_ref.clone())?;

    let attrs = lua.create_table()?;
    for (k, v) in &obj.attrs {
        attrs.set(k.clone(), lua.to_value(v)?)?;
    }
    t.set("attrs", attrs)?;

    let tags = lua.create_table()?;
    for (i, tag) in obj.tags.iter().enumerate() {
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
pub fn install<'scope, 'env>(
    _lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    env: &Table,
    world: &'env World,
    batch: Rc<RefCell<IntentBatch>>,
    default_location: Option<String>,
    dbref_counter: Rc<Cell<u64>>,
    themes: &'env std::collections::HashMap<String, Theme>,
    map_templates: &'env std::collections::HashMap<String, MapTemplateFile>,
    scheduled_hooks: &'env [crate::softcode::ScheduledHook],
) -> mlua::Result<()> {
    // -- Read API --

    env.set(
        "get_object",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            object_to_value(lua, world, &r)
        })?,
    )?;

    env.set(
        "get_attr",
        scope.create_function(move |lua, (r, key): (Value, String)| {
            let r = ref_of(&r)?;
            match world.get(&r).and_then(|o| o.attrs.get(&key)) {
                Some(v) => lua.to_value(v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    env.set(
        "has_attr",
        scope.create_function(move |_, (r, key): (Value, String)| {
            let r = ref_of(&r)?;
            Ok(world
                .get(&r)
                .map(|o| o.attrs.contains_key(&key))
                .unwrap_or(false))
        })?,
    )?;

    env.set(
        "has_tag",
        scope.create_function(move |_, (r, spec): (Value, String)| {
            let r = ref_of(&r)?;
            let tag = parse_tag(&spec)?;
            Ok(world.get(&r).map(|o| o.tags.contains(&tag)).unwrap_or(false))
        })?,
    )?;

    env.set(
        "get_tags",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            let out = lua.create_table()?;
            if let Some(obj) = world.get(&r) {
                for (i, tag) in obj.tags.iter().enumerate() {
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
                out.set(i + 1, object_to_table(lua, obj)?)?;
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

    env.set(
        "get_location",
        scope.create_function(move |lua, r: Value| {
            let r = ref_of(&r)?;
            match world.get(&r).and_then(|o| o.location_ref.as_deref()) {
                Some(loc) => object_to_value(lua, world, loc),
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
                if obj.tags.contains(&tag) {
                    out.set(i, object_to_table(lua, obj)?)?;
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
                if obj.attrs.get(&key).is_some_and(|v| *v == target) {
                    out.set(i, object_to_table(lua, obj)?)?;
                    i += 1;
                }
            }
            Ok(out)
        })?,
    )?;

    env.set(
        "find_in_room",
        scope.create_function(move |lua, (r, name): (Value, String)| {
            let room = ref_of(&r)?;
            let lower = name.to_lowercase();
            for obj in world.objects_in(&room) {
                if obj.key.to_lowercase().contains(&lower)
                    || obj.display_name().to_lowercase().contains(&lower)
                {
                    return object_to_value(lua, world, &obj.ref_id);
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
                    out.set(i, object_to_table(lua, obj)?)?;
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
                    out.set(i, object_to_table(lua, obj)?)?;
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
                    out.set(i, object_to_table(lua, obj)?)?;
                    i += 1;
                }
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
            Ok(world.objects_in(&actor_ref).iter().any(|o| o.tags.contains(&tag)))
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
                .map(|o| o.tags.contains(&container_tag))
                .unwrap_or(false))
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
                    out.set(i, object_to_table(lua, obj)?)?;
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
        "move_object",
        scope.create_function(move |_, (r, dest): (Value, Value)| {
            let target = ref_of(&r)?;
            let destination = ref_of(&dest)?;
            b.borrow_mut().push(Intent::Move { target, destination });
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

            b.borrow_mut().push(Intent::Spawn {
                ref_id: ref_id.clone(),
                key,
                kind,
                title,
                description,
                location,
                owner,
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
            b.borrow_mut().push(Intent::Destroy { target });
            Ok(())
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
        "set_program",
        scope.create_function(move |_, (r, hook, source): (Value, String, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::SetProgram { target, hook, source });
            Ok(())
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "trigger",
        scope.create_function(move |_, (r, hook): (Value, String)| {
            let target = ref_of(&r)?;
            b.borrow_mut().push(Intent::Trigger { target, hook });
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
                b.borrow_mut().push(Intent::Destroy { target });
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
                terrain_table.set(key.clone(), t)?;
            }
            out.set("terrain", terrain_table)?;

            Ok(out)
        })?,
    )?;

    let b = Rc::clone(&batch);
    env.set(
        "apply_template",
        scope.create_function(move |_, (r, template): (Value, Table)| {
            let target = ref_of(&r)?;
            for pair in template.pairs::<String, String>() {
                if let Ok((hook, source)) = pair {
                    b.borrow_mut().push(Intent::SetProgram {
                        target: target.clone(),
                        hook,
                        source,
                    });
                }
            }
            Ok(())
        })?,
    )?;

    Ok(())
}
