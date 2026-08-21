# Softcode Guide

Hearth's softcode system lets builders attach Luau scripts to objects. Scripts
run in a sandbox — they can read the world freely but can only change it
through a controlled set of functions that queue up changes for the engine
to apply.

## Hooks

A hook is a named slot on an object where a program can be attached. There are
three kinds:

### `can_` hooks — permission gates

Run before an action happens. Return `false` to block it. Anything else
(including `nil` / no return) allows it.

```lua
function can_get(this, actor, room)
  if not has_tag(actor, "quest:worthy") then
    emit(actor, "The sword refuses to be lifted.")
    return false
  end
  return true
end
```

### `on_` hooks — reactions

Run after an action succeeds. Used for flavor text, side effects, updating
attributes, etc.

```lua
function on_get(this, actor, room)
  emit(actor, "The sword hums as you pick it up.")
  emit_room(room, actor.display_name .. " picks up the sword and it hums with energy.", {actor.ref_id})
  set_attr(this, "picked_up_count", (get_attr(this, "picked_up_count") or 0) + 1)
end
```

### `cmd_` hooks — custom commands

Define new player-typeable commands. When a player types something that doesn't
match a builtin command, the engine searches objects in the room and the
player's inventory for a matching `cmd_<name>` hook.

```lua
function cmd_push(this, actor, room, args)
  emit(actor, "You push the button. Something clicks.")
  emit_room(room, actor.display_name .. " pushes the button.", {actor.ref_id})
  set_attr(this, "pressed", true)
end
```

If the player types `push button`, the engine finds the button object in the
room, sees it has `cmd_push`, and runs it. The `args` parameter contains
everything after the command name (e.g., `push button hard` would have
`args = "button hard"`).

### `on_tick` — timed behavior

Runs on the global tick (1-second heartbeat). Use `tick_interval` attribute
to control frequency. Receives a persistent `state` table instead of `actor`.

```lua
function on_tick(this, state, room)
  state.count = (state.count or 0) + 1
  if state.count % 10 == 0 then
    emit_room(room, "The torch flickers and dims.")
  end
end
```

State survives between runs and persists through server restarts. Use it for
internal working memory that shouldn't be visible as object attributes.

## Known hooks

| Hook | Fires when... | Signature |
|------|---------------|-----------|
| `can_get` | Before an actor picks up this object | `(this, actor, room)` |
| `on_get` | After an actor picks up this object | `(this, actor, room)` |
| `can_drop` | Before an actor drops this object | `(this, actor, room)` |
| `on_drop` | After an actor drops this object | `(this, actor, room)` |
| `can_put` | Before an actor puts an item into this container | `(this, actor, room)` |
| `on_put` | After an actor puts an item into this container | `(this, actor, room)` |
| `can_use` | Before an actor uses this object | `(this, actor, room)` |
| `on_use` | After an actor uses this object | `(this, actor, room)` |
| `can_traverse` | Before an actor uses this exit | `(this, actor, room)` |
| `can_enter` | Before an actor enters this room | `(this, actor, room)` |
| `on_enter` | After an actor enters this room | `(this, actor, room)` |
| `on_leave` | After an actor leaves this room | `(this, actor, room)` |
| `can_look` | Before an actor looks at this object | `(this, actor, room)` |
| `on_look` | After an actor looks at this object. **On rooms: suppresses default look output** — the hook handles all rendering. | `(this, actor, room)` |
| `can_say` | Before an actor speaks in this room | `(this, actor, room)` |
| `on_say` | After an actor speaks in this room. **On rooms: suppresses default say broadcast** — the hook handles distribution. Message available via `_say_message` attr. | `(this, actor, room)` |
| `can_see` | Controls visibility of this hidden object | `(this, actor, room)` |
| `on_move` | When this object is moved to a new location | `(this, actor, room)` |
| `on_receive` | When an item is placed in this object | `(this, actor, room)` |
| `on_damage` | When this object takes damage | `(this, actor, room)` |
| `on_death` | When this object dies | `(this, actor, room)` |
| `on_connect` | When a player connects (fires on player and room) | `(this, actor, room)` |
| `on_disconnect` | When a player disconnects (fires on player and room) | `(this, actor, room)` |
| `on_whisper` | When an actor whispers in this room | `(this, actor, room)` |
| `on_emote` | When an actor emotes in this room | `(this, actor, room)` |
| `on_tick` | Every N ticks (set `tick_interval` attr) | `(this, state, room)` |
| `on_startup` | Engine starts, after world loads | `(this, state, room)` |
| `on_shutdown` | Engine is shutting down, before final save | `(this, state, room)` |
| `on_reload` | After `@reload-world` completes | `(this, state, room)` |
| `on_save` | Before each world save (autosave or `@save`) | `(this, state, room)` |
| `on_create` | When this object is first created at runtime | `(this, state, room)` |
| `on_destroy` | Before this object is destroyed | `(this, actor, room)` |
| `cmd_*` | Any name — becomes a player command | `(this, actor, room, args)` |

### Global hooks

Objects tagged `system:global` receive lifecycle hooks from all rooms, not just
the room they're in. This includes `on_enter`, `on_leave`, `on_connect`, and
`on_disconnect`, in addition to `cmd_*` hooks (which were always global).

This lets a single global rules object handle game-wide events:

```toml
[[objects]]
key = "rules"
kind = "item"
tags = ["system:global", "system:hidden"]

[objects.programs]
on_enter = { file = "on_enter_map.luau" }
on_connect = { file = "on_enter_map.luau" }
```

## Hook parameters

Every hook function receives these parameters:

| Parameter | Description |
|-----------|-------------|
| `this` | The object the program is attached to |
| `actor` | The player performing the action |
| `room` | The room the action is happening in |
| `state` | (`on_tick` only) Persistent state table |
| `args` | (`cmd_` only) The text after the command name |

Each object parameter is a table with the object's data:

```lua
{
  ref_id = "player/sam",
  key = "sam",
  kind = "player",
  title = "Sam",
  display_name = "Sam",
  description = "A traveler.",
  location_ref = "area/starter/room/town_square",
  attrs = { hp = 100, class = "warrior" },
  tags = { "quest:worthy", "faction:guild" }
}
```

These tables are snapshots — changing them in Lua has no effect. Use the write
API functions to actually change the world.

## Read API

These functions read from the world. They're safe to call anytime.
`get_attr`, `has_attr`, and `pick` support **read-your-writes** — they
check the pending intent batch first, so you see your own `set_attr`/`set_val`
changes immediately within the same script.

| Function | Returns | Description |
|----------|---------|-------------|
| `get_object(ref)` | table or nil | Get an object by ref |
| `get_attr(ref, key)` | value or nil | Get a single attribute. Sees pending writes. |
| `has_attr(ref, key)` | boolean | Check if an attribute exists. Sees pending writes. |
| `pick(ref, attr, ...)` | value or nil | Walk into nested attrs: `pick(ref, "combat", 1, "hp")`. Sees pending writes. |
| `has_tag(ref, spec)` | boolean | Check if a tag exists (e.g., `"quest:worthy"`) |
| `get_tags(ref)` | table | List all tags as spec strings |
| `get_room_contents(ref)` | table | List objects in a room (excludes exits) |
| `get_exits(ref)` | table | List exits from a room |
| `get_location(ref)` | table or nil | Get the object's container |
| `kind_of(ref)` | string or nil | Get the object's kind |
| `get_owner(ref)` | string or nil | Get the owner's ref_id |
| `get_contents(ref)` | table | Objects inside a container |
| `get_timers(ref)` | table | Active timers on this object |
| `get_tick` | number | Current engine tick count (a value, not a function). Use for cooldowns, time-based logic. |
| `resolve_key(file_key)` | string or nil | Returns the ref_id for a TOML-defined object by file key (e.g., `"town/crossroads"`) |

### Search

| Function | Returns | Description |
|----------|---------|-------------|
| `find_by_tag(spec)` | table | Find all objects with a tag |
| `find_by_attr(key, value)` | table | Find all objects where `attrs[key] == value` |
| `find_in_room(room, name)` | table or nil | Fuzzy-match an object by name in a room |
| `get_inventory(ref)` | table | List items carried by an object |
| `get_players_in_room(room)` | table | Online players in a room |
| `get_all_by_kind(kind)` | table | All objects of a kind (`"room"`, `"npc"`, etc.) |
| `match_name(name, input)` | boolean | Partial name matching — `match_name("iron sword", "ir")` returns `true`. Matches start of full name or any word. |

### Spatial queries

| Function | Returns | Description |
|----------|---------|-------------|
| `get_nearby(room, x, y, radius)` | table | All objects in `room` whose `_x`/`_y` attrs are within `radius` |
| `get_rooms_in_radius(room, distance)` | table | BFS walk through exits, returns `{ {ref, distance, name}, ... }`. Respects `muffle` and `blocked_sound` exit attrs. |

The `ref` argument can be either a ref string (`"area/starter/item/sword"`) or
an object table (like `this` or `actor`). Both work everywhere.

## Predicates

Boolean checks for common conditions.

| Function | Returns | Description |
|----------|---------|-------------|
| `is_player(ref)` | boolean | Is this a player? |
| `is_npc(ref)` | boolean | Is this an NPC? |
| `is_item(ref)` | boolean | Is this an item? |
| `is_room(ref)` | boolean | Is this a room? |
| `is_exit(ref)` | boolean | Is this an exit? |
| `exists(ref)` | boolean | Does this ref point to a real object? |
| `is_carrying(actor, tag)` | boolean | Is the actor carrying an item with this tag? |
| `is_container(ref)` | boolean | Object has the `item:container` tag? |
| `same_room(a, b)` | boolean | Are two objects in the same room? |

Example:

```lua
function can_get(this, actor, room)
  if is_npc(actor) then
    return false  -- NPCs can't pick things up
  end
  if not is_carrying(actor, "quest:key") then
    emit(actor, "You need the key to take this.")
    return false
  end
  return true
end
```

## Write API

These functions don't change the world immediately. They queue up intents that
the engine applies atomically after the script finishes. If any intent is
invalid, the entire batch is rolled back.

| Function | Description |
|----------|-------------|
| `set_attr(ref, key, value)` | Set an attribute (JSON-compatible values). Pass `nil` to remove it. |
| `set_val(ref, attr, ..., value)` | Set a value deep inside a nested attr: `set_val(ref, "combat", 1, "hp", 5)` |
| `unset_attr(ref, key)` | Remove an attribute |
| `transfer_attr(from, to, key, amount)` | Atomic numeric transfer between objects. Validates sufficient balance; rolls back on failure. |
| `set_tag(ref, spec)` | Add a tag (e.g., `"quest:completed"`) |
| `unset_tag(ref, spec)` | Remove a tag |
| `set_title(ref, title)` | Change an object's display name |
| `set_description(ref, desc)` | Change an object's description |
| `set_owner(ref, owner_ref)` | Set the object's owner |
| `set_program(ref, hook, source)` | Attach a Luau program to an object |
| `apply_template(ref, table)` | Install multiple programs from a `{ hook = source }` table |
| `move_object(ref, destination)` | Move an object to a new location |
| `spawn(opts)` | Create a new object (see below) |
| `create_exit(opts)` | Create a new exit (`{ source, direction, target, aliases }`) |
| `destroy(ref)` | Remove an object from the world (not players) |
| `trigger(ref, hook, data?)` | Fire a hook on another object (see below) |
| `after(ticks, ref, hook, data?)` | Schedule a hook to fire after N ticks (see below) |
| `cancel_after(ref, hook)` | Cancel a pending timer |

### spawn

Creates a new object in the world:

```lua
local ref = spawn({
  key = "gold_coin",
  kind = "item",              -- "item", "npc", or "room" (not "player")
  title = "a gold coin",
  description = "A shiny gold coin.",
  location = room,            -- ref or object table
})
-- ref is the new object's ref_id, usable immediately in this script
emit(actor, "A gold coin materializes!")
```

### trigger

Fires a hook on another object. The triggered hook runs after the current
script's intents are applied, so it sees the updated world state. This is
how you build connected machinery, puzzles, and chain reactions.

```lua
-- A lever that opens a gate in another room
function cmd_pull(this, actor, room, args)
  emit(actor, "You pull the lever. A grinding sound echoes.")
  emit_room(room, actor.display_name .. " pulls the lever.", {actor.ref_id})
  set_attr("area/dungeon/item/gate", "open", true)
  trigger("area/dungeon/item/gate", "on_activate")
end
```

The optional third argument passes data to the triggered hook. The data is
available as the `_trigger_data` attr on the target during execution:

```lua
-- Alert nearby NPCs about combat
trigger(npc, "on_alert", { threat = actor.ref_id, type = "combat" })

-- In the NPC's on_alert hook:
function on_alert(this, actor, room)
  local data = get_attr(this, "_trigger_data")
  if data and data.type == "combat" then
    emit_room(room, this.display_name .. " rushes toward the sound of fighting!")
  end
end
```

Triggers don't recurse — if the triggered hook also calls `trigger`, the
second trigger fires after the first finishes. This prevents infinite loops.

### apply_template

Install multiple programs on an object from a table. Works naturally with
`require()` for reusable behavior bundles:

```lua
-- lib/template_wilderness.luau
return {
  on_enter = 'function on_enter(this, actor, room) ... end',
  can_look = 'function can_look(this, actor, room) ... end',
}

-- in a hook:
local tmpl = require("template_wilderness")
apply_template(room_ref, tmpl)
```

### after

Schedules a hook to fire on an object after a delay (in engine ticks, default
1 tick = 1 second). The hook fires once, not repeatedly.

```lua
-- A poison that wears off after 30 seconds
function cmd_poison(this, actor, room, args)
  set_attr(actor, "poisoned", true)
  emit(actor, "You feel a burning sensation...")
  after(30, actor, "on_cure")
end
```

The target object needs a program on the specified hook for anything to happen.
Timers are persisted to the database and survive server restarts. Use
`cancel_after(ref, hook)` to cancel a pending timer.

## Communication

| Function | Description |
|----------|-------------|
| `emit(ref, message)` | Send a message to one player |
| `emit_room(room, message, exclude?)` | Send to everyone in a room. `exclude` is an optional list of refs to skip. |
| `emit_nearby(room, x, y, radius, message, exclude?)` | Send to players in `room` whose `_x`/`_y` attrs are within `radius`. For coordinate-based shared rooms. |
| `emit_radius(room, distance, messages, exclude?)` | BFS walk through exits, delivers distance-keyed messages to players in reached rooms. |
| `emit_data(ref, channel, data)` | Send structured JSON to a player's web client (see Widgets below). |
| `prompt(ref, message)` | Prompt a player for input (fires `on_reply` with their response). |
| `log(message)` | Write to the server log. |

### emit_radius — multi-room propagation

Walk the exit graph from a source room and deliver different messages at
each distance:

```lua
emit_radius(room, 3, {
  [0] = 'You shout, "Guards!"\r\n',
  [1] = 'Someone shouts, "Guards!" nearby.\r\n',
  [2] = "You hear a distant shout.\r\n",
  [3] = "You hear a faint commotion.\r\n",
})
```

Exit attrs control propagation:

| Attr | Type | Effect |
|------|------|--------|
| `muffle` | integer | Adds to perceived distance when crossing this exit |
| `blocked_sound` | boolean | Prevents propagation through this exit entirely |

A heavy door with `muffle = 1` makes a shout from the next room sound
two rooms away. A sealed vault with `blocked_sound = true` blocks all
propagation. These same attrs are respected by `get_rooms_in_radius`.

## Widgets (web client)

Games can push custom UI panels to the web client sidebar without forking
the client. Use `emit_data()` with a `widget` field:

```lua
emit_data(actor, "map", {
  widget = "map",
  title = "Map",
  current = room_ref,
  rooms = { { ref = "town/square", name = "Square", short = "Sq", x = 0, y = 0 } },
  edges = { { from = "town/crossroads", to = "town/square", dir = "north" } },
})
```

Built-in widget types:

| Widget | Data shape |
|--------|-----------|
| `map` | `{ current, rooms: [{ref, name, short, x, y}], edges: [{from, to, dir}] }` |
| `list` | `{ items: [{label, value?, command?, color?}], empty? }` |
| `meter` | `{ bars: [{label, value, max, color?}] }` |
| `text` | `{ text }` or `{ lines: [{text, color?}] }` or `{ html }` |

Panels appear at the top of the sidebar, above the built-in Who's Here /
What's Here / Exits sections.

## Maps & Dungeons

| Function | Returns | Description |
|----------|---------|-------------|
| `get_map_template(name)` | table | Parsed map template data (grid, terrain, cells) as a read-only table |
| `instantiate_map(name)` | table | Spawn rooms/exits from a map template, returns `{ entrance_ref, room_count }` |
| `generate_dungeon(opts)` | string | Procedurally generate a dungeon layout, returns entrance ref |
| `destroy_dungeon(seed)` | nil | Destroy all rooms generated by a dungeon seed |

### get_map_template

Read the parsed TOML map data without instantiating rooms:

```lua
local map = get_map_template("iron_hills")
-- map.name, map.width, map.height
-- map.cells["3,2"].terrain, .theme, .passable, .title, .description
-- map.terrain["f"].theme, .passable, .title_prefix
```

## Noise (procedural generation)

Rust-backed noise functions for terrain generation, biomes, weather, etc.
All are deterministic — same seed and coordinates always produce the same value.

| Function | Returns | Description |
|----------|---------|-------------|
| `simplex2d(seed, x, y)` | [-1, 1] | 2D simplex noise |
| `simplex3d(seed, x, y, z)` | [-1, 1] | 3D simplex noise |
| `perlin2d(seed, x, y)` | [-1, 1] | 2D Perlin noise |
| `perlin3d(seed, x, y, z)` | [-1, 1] | 3D Perlin noise |
| `fbm2d(seed, x, y, ...)` | ~[-1, 1] | Fractal Brownian Motion (layered Perlin) |

`fbm2d` accepts optional parameters after y: `octaves` (default 6), `frequency`
(default 1.0), `lacunarity` (default 2.0), `persistence` (default 0.5).

```lua
-- Generate terrain type from coordinates
function get_terrain(seed, x, y)
  local elevation = fbm2d(seed, x * 0.01, y * 0.01, 4, 1.0, 2.0, 0.5)
  local moisture = simplex2d(seed + 1000, x * 0.02, y * 0.02)
  if elevation > 0.5 then return "mountain"
  elseif elevation > 0.0 and moisture > 0.3 then return "forest"
  elseif elevation > 0.0 then return "plains"
  else return "water" end
end
```

## Seeded RNG (deterministic randomness)

For procedural content that must be reproducible from a seed — same inputs
always produce the same outputs, no stored state.

| Function | Returns | Description |
|----------|---------|-------------|
| `hash_seed(...)` | integer | Hash any mix of strings, numbers, booleans |
| `seed_random(seed, min, max)` | integer | Random integer in [min, max] |
| `seed_float(seed)` | float | Random float in [0, 1) |
| `seed_choice(seed, list)` | element | Pick from a list |

```lua
-- Deterministic room description from coordinates
local h = hash_seed("room_desc", x, y)
local adj = seed_choice(h, {"dusty", "mossy", "damp", "dim"})
local noun = seed_choice(hash_seed("noun", x, y), {"chamber", "tunnel", "cavern"})
set_description(room, "A " .. adj .. " " .. noun .. ".")
```

## Coordinate math

Utility functions for spatial calculations — distance, direction, interpolation.

| Function | Returns | Description |
|----------|---------|-------------|
| `distance(x1, y1, x2, y2)` | number | Euclidean distance |
| `manhattan(x1, y1, x2, y2)` | number | Manhattan distance |
| `direction_to(x1, y1, x2, y2)` | string | Compass direction (n/s/e/w/ne/nw/se/sw/here) |
| `lerp(a, b, t)` | number | Linear interpolation (t=0→a, t=1→b) |
| `clamp(value, min, max)` | number | Clamp to range |
| `remap(v, inMin, inMax, outMin, outMax)` | number | Remap between ranges |

## Utility

| Function | Description |
|----------|-------------|
| `log(message)` | Print a debug message to the server console |
| `json_encode(value)` | Convert a Lua value to a JSON string |
| `json_decode(string)` | Parse a JSON string into a Lua value |

Use `log()` to debug scripts during development. Messages appear in the
server's log output with a `softcode=true` marker.

```lua
function on_tick(this, state, room)
  state.count = (state.count or 0) + 1
  log("tick #" .. state.count .. " on " .. this.ref_id)
end
```

## Global scripts

Global scripts are standalone Luau programs not attached to any object. They're
for game systems — weather, combat, economy, respawns — that don't belong
on a specific object.

```lua
function on_tick(state)
  state.counter = (state.counter or 0) + 1
  if state.counter % 60 == 0 then
    local weathers = {"clear", "cloudy", "rainy", "stormy"}
    state.weather = weathers[math.random(#weathers)]
  end
end
```

Global scripts have their own persistent state table, their own tick interval,
and full access to the read/write API. They don't receive `this`, `actor`, or
`room` — just `state`.

### Builder commands for global scripts

```
@script weather = function on_tick(state) ... end
@script-interval weather = 60
@scripts
@rmscript weather
```

## Locks

Locks are DSL expressions on objects and exits that gate actions without Luau.
They're simpler than `can_` hooks for common permission patterns.

### Lock types

| Type | Checked on | When |
|------|-----------|------|
| `traverse` | exits | Before movement |
| `enter` | rooms | Before entering |
| `get` | items | Before picking up |
| `drop` | items | Before dropping |
| `use` | objects | Before using |
| `look` | objects | Before examining |
| `teleport` | rooms | Before teleporting |

### Lock functions

| Function | Description |
|----------|-------------|
| `perm(scope)` | Actor's account has this scope (admin implies all) |
| `has_tag(spec)` | Actor has this tag |
| `has_attr(key)` | Actor has this attribute |
| `has_attr(key, value)` | Actor has this attribute with this value |
| `in_inventory(tag_spec)` | Actor is carrying an object with this tag |
| `is_kind(kind)` | Actor is this kind (player, npc, item) |
| `time_between(start, end)` | Current UTC hour is in range |
| `true` / `false` | Always allow / always deny |

Combinators: `AND`, `OR`, `NOT`, parentheses.

### Builder commands for locks

```
@lock area/starter/exit/square_to_tavern/traverse = has_tag(vip) OR perm(builder)
@lock area/starter/item/rusty_sword/get = in_inventory(quest:key)
@lock here/enter = NOT is_kind(npc)
@unlock area/starter/exit/square_to_tavern/traverse
@locks                        -- locks on current room
@locks <ref>                  -- locks on a specific object/exit
```

### Locks vs hooks

Locks and hooks can coexist. The engine checks DSL locks first, then fires
the `can_` hook. If either denies, the action is blocked.

- **Use locks** for simple, data-driven rules (has a tag, has a permission,
  is carrying a key item).
- **Use `can_` hooks** for complex logic (check multiple conditions, emit
  custom denial messages, modify state).

## Builder commands

All `@` commands require the `builder` scope.

### Programs

```
@program <ref>/<hook> = <luau source>    -- attach a program
@programs [<ref>]                        -- list programs (default: room)
@rmprogram <ref>/<hook>                  -- remove a program
```

The source is syntax-checked before being installed. If there's a compile
error, the program is rejected and the error is shown.

### Ticks (per-object)

```
@set <ref>/tick_interval = 5             -- run on_tick every 5 seconds
@program <ref>/on_tick = function on_tick(this, state, room) ... end
```

## Modules (require)

Shared Luau code lives in `<game_dir>/lib/*.luau`. Any hook or global script
can load a module with `require("name")`:

```lua
local random = require("random")
local roll = random.roll(20)
```

Modules return a table (standard Lua module pattern):

```lua
-- lib/combat_utils.luau
local M = {}
function M.roll_damage(base, bonus) return base + math.random(1, bonus) end
return M
```

Modules can require other modules (transitive deps). Module return values are
cached — the source only runs once per engine lifetime. Cache clears on
`@reload-world`.

Circular requires are rejected rather than looping forever. If `a` requires
`b` and `b` requires `a`, the inner call errors with
`require cycle detected: a -> b -> a`. Break the cycle by extracting the
shared piece into a third module.

Modules have stdlib access (string, table, math, require) but **not** the
game write API (emit, set_attr, etc.). They're for pure utility code.

### Bundled modules

| Module | Description |
|--------|-------------|
| `random` | `roll(sides)`, `dice(n, sides)`, `chance(pct)`, `pick(list)`, `weighted(choices)`, `shuffle(list)`, `sample(list, n)` |
| `collections` | `Set` class (add/remove/has/union/intersection/difference), `Array` helpers (map/filter/find/reduce/flat/contains/reverse/slice) |
| `state_machine` | Synchronous FSM: `new({initial, transitions, on_enter, on_exit})`, `:send(event)`, `:can(event)`, `:is(state)` |
| `signal` | Pub/sub: `new()` → Signal with `:fire(...)`, `:connect(fn)` → Connection with `:disconnect()`. Also `newSubject(...)` for replay-last-value signals. |
| `text` | Rich formatting with accessible/visual modes using BBCode markup (works on both telnet and web). `for_mode(mode)` returns a formatter with `bar`, `header`, `table`, `box`, `stat`, `divider`, `color`, `bold`, `dim`. |
| `str` | String utilities: `split`, `trim`, `starts_with`, `ends_with`, `title_case`, `pad_right`, `pad_left`, `center`, `truncate`, `pluralize`, `wrap` |
| `Grid3D` | 3D grid data structure (from luau-grids). See also Rust-side Grid2D below. |

### Display modes and accessibility

Players can toggle their display mode:

```
@display visual        -- full formatting (BBCode colors, Unicode box-drawing, progress bars)
@display accessible    -- plain text (screen-reader friendly, no formatting, descriptive labels)
```

The mode is stored as the `_display_mode` attr on the player object. Scripts
read it to choose formatting:

```lua
local text = require("text")
local mode = get_attr(actor, "_display_mode") or "visual"
local fmt = text.for_mode(mode)

-- Visual:     [████████░░░░] 8/12
-- Accessible: HP: 8/12 (67%)
emit(actor, fmt.bar(hp, max_hp, 20, "HP"))

-- Visual:     ═══ Combat Status ═══
-- Accessible: --- Combat Status ---
emit(actor, fmt.header("Combat Status"))
```

## Grid2D (Rust-side)

Grid2D is a Rust-backed spatial grid registered as a Lua global. Grid operations
run in Rust and do **not** consume the instruction budget.

### Creating grids

```lua
local g = grid_new(10, 10, "floor")       -- 10x10, all cells = "floor"
local g = grid_from_value(get_attr(obj, "my_grid"))  -- from saved attr
```

### Methods

| Method | Description |
|--------|-------------|
| `g:get(x, y)` | Cell value (1-indexed, nil if out of bounds) |
| `g:set(x, y, value)` | Set cell (errors if out of bounds) |
| `g:width()`, `g:height()`, `g:size()` | Dimensions |
| `g:fill(x1, y1, x2, y2, value)` | Fill rectangular region |
| `g:to_value()` | Serialize to table for `set_attr()` |
| `g:find(value)` | First `{x, y}` matching value, or nil |
| `g:find_all(value)` | List of `{x, y}` matching value |
| `g:neighbors(x, y)` | 4 cardinal neighbors `{{x, y, value}, ...}` |
| `g:pathfind(x1, y1, x2, y2, walkable)` | A* path as `{{x,y}, ...}` or nil |
| `g:has_los(x1, y1, x2, y2, blocking)` | Bresenham line-of-sight (bool) |
| `g:fov(x, y, radius, blocking)` | Shadowcast field of view `{{x,y}, ...}` |
| `g:distance_map(x, y, walkable)` | Dijkstra flood → new Grid2D of distances (-1 = unreachable) |
| `g:flood_fill(x, y, old, new)` | Fill connected region, returns count |

### Persisting grids

```lua
-- Save
set_attr(system_obj, "dungeon_map", g:to_value())

-- Load
local g = grid_from_value(get_attr(system_obj, "dungeon_map"))
```

### Dungeon layout

`generate_dungeon` automatically stores a layout grid on the entrance room as
the `dungeon_layout` attribute. Each cell is either `null` (empty) or a room
ref string:

```lua
local entrance = generate_dungeon("my-seed")
local layout = grid_from_value(get_attr(entrance, "dungeon_layout"))
local room_ref = layout:get(3, 2)  -- "#42" or nil
```

## Sandboxing

Programs run in an isolated environment per call:

- **No file/network access.** Luau's baseline globals exclude `io` and
  `os.execute`. `require` is overridden to load from `lib/` only.
- **Instruction budget.** A runaway script (infinite loop, excessive recursion)
  is killed after a fixed instruction count. The engine stays responsive.
  Grid2D operations are exempt (they run in Rust).
- **No direct mutation.** The write API only queues intents. The engine
  validates and applies them after the script finishes, rolling back the
  entire batch on any error.
- **Isolation between programs.** Each hook invocation gets its own environment
  table. Global variables set in one program are not visible to another.

## Tips

- Use `emit()` for messages only the actor sees. Use `emit_room()` for
  messages everyone in the room sees. Pass `{actor.ref_id}` as the third
  argument to exclude the actor from the room message.
- Attributes can hold any JSON-compatible value: strings, numbers, booleans,
  tables (arrays/objects), or nil.
- Tags are great for boolean state (`quest:completed`, `faction:guild`).
  Use attributes for values (`hp = 100`).
- The `this` parameter is the object the program is attached to — use it to
  read and write the object's own state without hardcoding ref strings.
- Script state (`state` in `on_tick`) is private to the program. It doesn't
  show up in `examine` or `get_attr`. Use it for internal working memory.
- Locks and hooks stack: DSL lock is checked first, then the `can_` hook.
  You can use one or both.
