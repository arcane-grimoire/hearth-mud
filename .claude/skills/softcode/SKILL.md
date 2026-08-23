---
description: Write Luau softcode for Hearth MUD — hooks, commands, game content TOML, and the softcode API. Use when creating or editing .luau scripts, writing TOML area files, or designing game mechanics that run in the Luau VM.
---

# Writing Hearth MUD softcode

Hearth MUD games are built in two layers: **TOML area files** define the world (rooms, NPCs, items, exits) and **Luau scripts** bring it to life (hooks, commands, behaviors). Both live in the game directory (e.g. `../the-last-stag-mud/world/`). The engine is never modified for game logic.

## Architecture

Scripts run on the engine's single thread inside a sandboxed Luau VM. They **cannot mutate the world directly** — every mutation goes through a typed Intent that the engine validates and applies atomically after the script returns. This means:

- Read functions (`get_object`, `get_attr`, `has_tag`, etc.) see a snapshot of the world at call time. **Read-your-writes:** `get_attr`, `has_attr`, and `pick` check the pending batch first, so scripts see their own writes immediately.
- Write functions (`set_attr`, `emit`, `move_object`, `spawn`, etc.) queue Intents into a batch
- If any Intent in the batch fails validation, the entire batch is rolled back
- The script's return value matters only for `can_` hooks (return `false` to veto)

## TOML area files

Each `.toml` file defines one area. The file loader assigns dbrefs (`#1`, `#2`, ...) automatically.

```toml
area = "town"

[[rooms]]
key = "crossroads"
title = "The Crossroads"
description = "A weathered crossroads..."

[[rooms]]
key = "tavern"
title = "The Stag and Crown"
description = "A low-beamed tavern..."

[[objects]]
key = "barkeep"
kind = "npc"
title = "Aldric the Barkeep"
description = "A heavyset man..."
location = "tavern"
tags = ["quest:giver"]

[objects.attrs]
dialogue_state = "idle"

[objects.programs]
on_look = { file = "barkeep_look.luau" }
cmd_talk = { file = "barkeep_talk.luau" }

[[objects]]
key = "torch"
kind = "item"
title = "a sputtering torch"
location = "crossroads"

[[exits]]
from = "crossroads"
direction = "north"
to = "tavern"
aliases = ["n"]

[[exits]]
from = "tavern"
direction = "south"
to = "crossroads"
aliases = ["s"]

# Cross-area exits use "area/key" format:
[[exits]]
from = "crossroads"
direction = "east"
to = "forest/edge"
aliases = ["e"]

[[scripts]]
name = "weather"
entry = "on_tick"
interval = 60
file = "weather.luau"
```

### Key rules

- `key` is the human-readable identifier, not unique — two swords can both have `key = "sword"`
- `location` references another object's key within the same area, or `"area/key"` for cross-area
- `kind` is one of: `room`, `item`, `npc`, `exit` (never `player` — those are created by the engine)
- Programs can be inline (`source = "..."`) or file references (`file = "script.luau"`)
- Objects loaded from files are tagged `system:managed` and updated on `@reload-world`

## Hooks

Three families, each with a different signature:

### `can_` hooks — permission gates

```lua
function can_get(this, actor, room)
  if not has_tag(actor, "quest:worthy") then
    emit(actor, "The sword refuses to be lifted.")
    return false  -- veto the action
  end
  return true  -- allow (also: returning nil or nothing allows)
end
```

Only an explicit `return false` vetoes. Returning `true`, `nil`, or nothing allows.

Hooks: `can_get`, `can_drop`, `can_traverse`, `can_enter`, `can_look`, `can_say`, `can_use`, `can_see`

### Lifecycle hooks — engine events

```lua
function on_startup(this, state, room)
  log("World is ready, initializing " .. this.display_name)
end
```

Same signature as `on_tick` (`this`, `state`, `room`). State is persistent.

| Hook | Fires when |
|------|------------|
| `on_startup` | Engine starts, after world + game files load |
| `on_shutdown` | Engine stopping, before final save |
| `on_reload` | After `@reload-world` completes |
| `on_save` | Before each world save (autosave or `@save`) |
| `on_create` | Object first created at runtime (not from DB/files) |

### `on_` hooks — reactive behavior

```lua
function on_get(this, actor, room)
  emit(actor, "The sword hums as you pick it up.")
  emit_room(room, actor.display_name .. " picks up a glowing sword.", {actor.ref_id})
  set_attr(this, "held_by", actor.ref_id)
end
```

Return value is ignored. Runs after the action has already happened.

Hooks: `on_get`, `on_drop`, `on_enter`, `on_leave`, `on_look`, `on_say`, `on_use`, `on_move`, `on_destroy`, `on_connect`, `on_disconnect`, `on_whisper`, `on_emote`, `on_receive`, `on_damage`, `on_death`, `on_tick`, `on_startup`, `on_shutdown`, `on_reload`, `on_save`, `on_create`

**Room hook overrides:** When `on_look` is attached to a room, it suppresses the default look rendering — the hook handles all output (enables personalized views, e.g. shared wilderness rooms). When `on_say` is attached to a room, it suppresses the default say broadcast — the hook handles message distribution. The message text is available via `get_attr(room, "_say_message")`.

### `cmd_` hooks — custom commands

```lua
function cmd_talk(this, actor, room, args)
  emit(actor, this.display_name .. ' says, "Welcome, traveler."')
  emit_room(room, this.display_name .. " speaks to " .. actor.display_name .. ".", {actor.ref_id})
end
```

The player types `talk` and the engine finds a `cmd_talk` program on an object in the room, the actor's inventory, or a `system:global` tagged object. The `args` parameter receives everything after the command name.

Resolution order: room itself → objects in room → actor's inventory → global objects.

**Global lifecycle hooks:** Objects tagged `system:global` now receive lifecycle hooks (`on_enter`, `on_leave`, `on_connect`, `on_disconnect`) from all rooms, not just `cmd_*` hooks. This lets a single global object handle cross-cutting concerns (e.g. map data updates on every room enter) without repeating hooks on every room.

### `on_tick` — special signature

```lua
function on_tick(this, state, room)
  state.count = (state.count or 0) + 1
  if state.count % 10 == 0 then
    emit_room(room, "The torch flickers.")
  end
end
```

`state` is a persistent table — values survive between ticks. Set `tick_interval` attr on the object to control frequency (default: every tick, i.e. every second).

## Modules (require)

Shared Luau code in `<game_dir>/lib/*.luau` is loaded with `require("name")`. Modules return a table, can require other modules, and are cached until `@reload-world`. Modules have stdlib + require but **not** the write API.

Bundled modules (embedded in the binary, overridable by game): `random` (dice, weighted, shuffle), `collections` (Set, Array helpers), `state_machine` (sync FSM), `signal` (pub/sub), `Grid3D` (3D grid), `text` (rich BBCode formatting with accessible mode), `str` (string utilities), `grids` (re-export of Grid3D).

### text module — accessible-aware formatting

```lua
local text = require("text")
local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")

emit(actor, fmt.header("Combat Status"))
emit(actor, fmt.bar(hp, max_hp, 20, "HP"))
emit(actor, fmt.stat("Defense", 12, "cyan"))
emit(actor, fmt.table(rows, {"Name", "HP", "Class"}))
emit(actor, fmt.divider(40))
emit(actor, fmt.box({"Line 1", "Line 2"}))
emit(actor, fmt.bold("Important!"))
emit(actor, fmt.color("danger", "red"))
```

Visual mode uses BBCode markup (box-drawing, Unicode bars, colors) that works on both telnet and web. Accessible mode uses plain text (screen-reader friendly). Players toggle with `@display accessible` / `@display visual`.

### str module — string utilities

`split`, `trim`, `starts_with`, `ends_with`, `title_case`, `pad_right`, `pad_left`, `center`, `truncate`, `pluralize`, `wrap`.

## Grid2D (Rust-side globals)

Rust-backed spatial grid — operations don't consume instruction budget.

```lua
local g = grid_new(10, 10, "floor")       -- constructor
local g = grid_from_value(saved_table)    -- deserialize from attr

g:get(x, y)                              -- read cell (1-indexed, nil if OOB)
g:set(x, y, value)                       -- write cell
g:width() / g:height() / g:size()        -- dimensions
g:fill(x1, y1, x2, y2, value)           -- rectangle fill
g:to_value()                             -- serialize for set_attr()
g:find(value) / g:find_all(value)        -- spatial search
g:neighbors(x, y)                        -- 4 cardinal {{x, y, value}, ...}
g:pathfind(x1, y1, x2, y2, walkable)    -- A* → {{x,y}, ...} or nil
g:has_los(x1, y1, x2, y2, blocking)     -- Bresenham LOS (bool)
g:fov(x, y, radius, blocking)           -- shadowcast FOV {{x,y}, ...}
g:distance_map(x, y, walkable)          -- Dijkstra → new Grid2D of distances
g:flood_fill(x, y, old, new)            -- fill connected region → count
```

`generate_dungeon` stores a layout grid on the entrance room (`dungeon_layout` attr) mapping grid positions to room refs.

## Read API (22 functions + 1 value)

All accept either an object table or a ref string (`"#5"`). `get_attr`, `has_attr`, and `pick` see pending writes from the current script (read-your-writes).

| Function | Returns | Description |
|---|---|---|
| `get_object(ref)` | table or nil | Full object snapshot |
| `get_attr(ref, key)` | value or nil | Single attribute. Sees pending writes. |
| `has_attr(ref, key)` | bool | Whether attr exists. Sees pending writes. |
| `pick(ref, attr, ...)` | value or nil | Walk into nested attrs: `pick(ref, "combat", 1, "hp")`. Sees pending writes. |
| `has_tag(ref, "cat:key")` | bool | Whether tag exists |
| `get_tags(ref)` | table | List of tag specs |
| `get_room_contents(ref)` | table | Non-exit objects in room |
| `get_contents(ref)` | table | Item objects located in ref (a container's contents) |
| `get_exits(ref)` | table | Exits from room |
| `get_location(ref)` | table or nil | Object's location |
| `kind_of(ref)` | string or nil | "room", "item", "npc", "player", "exit" |
| `find_by_tag("cat:key")` | table | All objects with tag |
| `find_by_attr(key, value)` | table | All objects where `attrs[key] == value` |
| `find_in_room(room, "name")` | table or nil | Fuzzy name match in room |
| `match_name(name, input)` | bool | Partial name matching — `match_name("iron sword", "ir")` → true. Matches start of full name or any word. |
| `get_inventory(ref)` | table | Items located in ref |
| `get_players_in_room(room)` | table | Online players in room |
| `get_all_by_kind("npc")` | table | All objects of kind |
| `all_objects()` | table | Every object's ref_id in the world (flat list of ref strings) |
| `get_tick` | number | Current engine tick count (a value, not a function). Use for cooldowns, time-based logic. |
| `resolve_key(file_key)` | string or nil | Returns ref_id for a TOML-defined object by file key (e.g. `"town/crossroads"`) |
| `get_owner(ref)` | string or nil | Owner's ref_id |
| `get_timers(ref)` | table | Active timers on this object |

## Predicates (9 functions)

| Function | Description |
|---|---|
| `is_player(ref)` | Kind == player |
| `is_npc(ref)` | Kind == npc |
| `is_item(ref)` | Kind == item |
| `is_room(ref)` | Kind == room |
| `is_exit(ref)` | Kind == exit |
| `exists(ref)` | Object exists |
| `is_carrying(actor, "cat:key")` | Actor has item with tag |
| `is_container(ref)` | Has `item:container` tag |
| `same_room(a, b)` | Both in same location |

## Spatial Queries

| Function | Returns | Description |
|---|---|---|
| `get_nearby(room, x, y, radius)` | table | All objects in `room` whose `_x`/`_y` attrs are within `radius` of the given coords |
| `get_rooms_in_radius(room, distance)` | table | BFS walk through exits, returns `{ {ref, distance, name}, ... }`. Respects `muffle` and `blocked_sound` exit attrs. |

## Write API (24 functions)

All queue Intents — nothing happens until the script returns and the batch applies.

| Function | Description |
|---|---|
| `set_attr(ref, key, value)` | Set attribute. Pass `nil` to remove it (same as `unset_attr`). |
| `set_val(ref, attr, ..., value)` | Set a value deep inside a nested attr: `set_val(ref, "combat", 1, "hp", 5)` |
| `unset_attr(ref, key)` | Remove attribute |
| `transfer_attr(from, to, key, amount)` | Atomic numeric transfer. Validates sufficient balance; rolls back on failure. |
| `emit(ref, message)` | Send text message to a player |
| `emit_room(room, message, {exclude...})` | Send text to all in room |
| `emit_nearby(room, x, y, radius, message, exclude?)` | Send text to players in `room` whose `_x`/`_y` attrs are within `radius` |
| `emit_radius(room, distance, messages, exclude?)` | BFS walk through exits. `messages` is a `{[0]="...", [1]="...", ...}` table keyed by distance. Exit attrs `muffle` (adds distance) and `blocked_sound` (blocks traversal) control propagation. |
| `emit_data(ref, channel, data)` | Send structured JSON to player's web client |
| `move_object(ref, destination)` | Move object to new location |
| `set_tag(ref, "cat:key")` | Add tag |
| `unset_tag(ref, "cat:key")` | Remove tag |
| `spawn({key, kind, title, description, location})` | Create new object, returns dbref |
| `create_exit({source, direction, target, aliases?})` | Create an exit from the source room to the target, returns dbref |
| `set_title(ref, title)` | Change title |
| `set_description(ref, desc)` | Change description |
| `set_owner(ref, owner_ref)` | Set the object's owner |
| `destroy(ref)` | Remove object (not players) |
| `trigger(ref, hook, data?)` | Fire a hook on another object. Optional `data` (table) is available as `_trigger_data` attr during execution. |
| `set_program(ref, hook, source)` | Attach a Luau program to an object |
| `apply_template(ref, table)` | Install multiple programs from a `{ hook = source }` table. Works with `require()`: `apply_template(ref, require("template_foo"))` |
| `prompt(actor, obj, hook)` | Set up interactive prompt |
| `after(ticks, ref, hook, data?)` | Schedule a hook to fire after N engine ticks, with optional data payload |
| `cancel_after(ref, hook)` | Cancel a pending timer |

## BBCode markup

Text output uses BBCode-style tags that render differently per transport:
- **Telnet**: converted to ANSI escape sequences
- **Web**: converted to HTML with CSS classes

### Formatting tags

| Tag | Telnet | Web |
|-----|--------|-----|
| `[b]...[/b]` | Bold | `<span class="b">` |
| `[dim]...[/dim]` | Dim | `<span class="dim">` |
| `[i]...[/i]` | Italic | `<span class="i">` |
| `[u]...[/u]` | Underline | `<span class="u">` |
| `[red]...[/red]` | Red text | `<span class="c-red">` |
| `[green]`, `[yellow]`, `[blue]`, `[magenta]`, `[cyan]`, `[white]` | Corresponding color | CSS class |
| `[bright_red]`, `[bright_green]`, `[bright_yellow]`, etc. | Bright variant | CSS class (e.g. `c-bright-red`) |
| `[/]` | Reset all | Close all open spans |
| `[cmd=COMMAND]...[/cmd]` | Underlined text | Clickable — sends COMMAND on click |

### Clickable commands

Use `[cmd=...]` to make text interactive in the web client:

```lua
emit(actor, "[cmd=look sword]Rusty Sword[/cmd]")
emit(actor, "[cmd=attack]Attack[/cmd]  [cmd=defend]Defend[/cmd]  [cmd=flee]Flee[/cmd]")
```

On telnet, these render as underlined text. On web, they're clickable elements that send the command.

### Structured data and game widgets (emit_data)

Push structured JSON to the web client. If the data includes a `widget` field, the default web client renders it as a sidebar panel automatically — no custom client needed.

```lua
-- Custom sidebar widget (renders automatically)
emit_data(actor, "map", {
    widget = "map",
    title = "Map",
    current = room_ref,
    rooms = { { ref = "town/square", name = "Square", short = "Sq", x = 0, y = 0 } },
    edges = { { from = room_ref, to = "town/square", dir = "north" } },
})

-- Raw data (game-specific client handles rendering)
emit_data(actor, "combat", { target = "Goblin", hp = 15, max_hp = 30 })
```

Built-in widget types:

| Widget | Data shape |
|--------|-----------|
| `map` | `{ current, rooms: [{ref, name, short, x, y}], edges: [{from, to, dir}] }` |
| `list` | `{ items: [{label, value?, command?, color?}], empty? }` |
| `meter` | `{ bars: [{label, value, max, color?}] }` |
| `text` | `{ text }` or `{ lines: [{text, color?}] }` or `{ html }` |

## Noise (procedural generation)

Rust-backed, deterministic noise functions. Same seed + coordinates = same value.

| Function | Returns | Description |
|---|---|---|
| `simplex2d(seed, x, y)` | [-1, 1] | 2D simplex noise |
| `simplex3d(seed, x, y, z)` | [-1, 1] | 3D simplex noise |
| `perlin2d(seed, x, y)` | [-1, 1] | 2D Perlin noise |
| `perlin3d(seed, x, y, z)` | [-1, 1] | 3D Perlin noise |
| `fbm2d(seed, x, y, octaves?, freq?, lac?, persist?)` | ~[-1, 1] | Fractal Brownian Motion |

## Seeded RNG (deterministic randomness)

| Function | Description |
|---|---|
| `hash_seed(...)` | Deterministic hash from strings/numbers/booleans |
| `seed_random(seed, min, max)` | Deterministic integer in [min, max] |
| `seed_float(seed)` | Deterministic float in [0, 1) |
| `seed_choice(seed, list)` | Deterministic pick from a table |

## Coordinate math

| Function | Description |
|---|---|
| `distance(x1, y1, x2, y2)` | Euclidean distance |
| `manhattan(x1, y1, x2, y2)` | Manhattan distance |
| `direction_to(x1, y1, x2, y2)` | Compass string (n/s/e/w/ne/nw/se/sw/here) |
| `lerp(a, b, t)` | Linear interpolation |
| `clamp(value, min, max)` | Clamp to range |
| `remap(v, inMin, inMax, outMin, outMax)` | Remap between ranges |

## Maps & Dungeons

| Function | Returns | Description |
|---|---|---|
| `get_map_template(name)` | table | Parsed map template data (grid, terrain, cells with overrides). Read-only. |
| `instantiate_map(name)` | table | Spawns rooms/exits from a map template, returns `{entrance_ref, room_count}` |
| `generate_dungeon(opts)` | string | Procedurally generates a dungeon layout, returns entrance ref |
| `destroy_dungeon(seed)` | — | Destroys all rooms generated by a dungeon seed |

## Utility

| Function | Description |
|---|---|
| `log(message)` | Write to server log (tracing::info) |
| `json_encode(value)` | Convert Lua value to JSON string |
| `json_decode(string)` | Parse JSON string into Lua value |

## Object table fields

When you receive an object (from `get_object`, or as `this`/`actor`/`room` parameters):

```lua
obj.ref_id        -- "#5" (the dbref)
obj.key           -- "crossroads" (human-readable)
obj.kind          -- "room", "item", "npc", "player", "exit"
obj.title         -- "The Crossroads" or nil
obj.display_name  -- title if set, otherwise key
obj.description   -- string
obj.location_ref  -- "#3" or nil
obj.attrs         -- table of attributes
obj.tags          -- array of tag specs {"zone:castle", "system:managed"}
```

These are snapshots — mutating the table does nothing. Use the write API.

## Tags

Tags are `category:key` pairs. Common conventions:

- `system:managed` — loaded from files, updated on reload
- `system:hidden` — hidden from room contents
- `system:offline` — disconnected player
- `system:global` — cmd_ hooks available everywhere + lifecycle hooks (on_enter, on_leave, on_connect, on_disconnect) fire from all rooms
- `owner:<ref>` — owned by a player (e.g. heroes)
- `troupe:<ref>` — in a player's active party

## Patterns

### Global command (available everywhere)

Create an object with `system:global` tag and attach `cmd_` hooks:

```toml
[[objects]]
key = "rules"
kind = "item"
title = "Game Rules"
tags = ["system:global"]

[objects.programs]
cmd_help = { source = 'function cmd_help(this, actor, room, args) emit(actor, "...") end' }
```

### Persistent state on objects

Use attrs for state that persists across restarts:

```lua
function cmd_push(this, actor, room, args)
  local count = get_attr(this, "push_count") or 0
  count = count + 1
  set_attr(this, "push_count", count)
  if count >= 3 then
    emit(actor, "The wall slides open!")
    -- reveal a hidden exit, etc.
  else
    emit(actor, "The stone shifts slightly. (" .. count .. "/3)")
  end
end
```

### Spawning and tracking

```lua
local ref = spawn({
  key = "imp",
  kind = "npc",
  title = "a summoned imp",
  description = "A small fiery creature.",
  location = room.ref_id,
})
set_attr(ref, "summoned_by", actor.ref_id)
set_tag(ref, "summon:" .. actor.ref_id)
```

`spawn` returns the new dbref immediately — you can pass it to `set_attr`, `set_tag`, etc. in the same script.

### Combat state on player attrs

The Last Stag stores combat state as attrs on the player:

```lua
set_attr(actor, "combat_active", true)
set_attr(actor, "combat_round", 1)
set_attr(actor, "combat_monsters", { {name="Goblin", hp=2, alive=true}, ... })
```

This pattern works for any complex state — attrs can hold tables, not just scalars.

## Exit propagation attrs

Exits can carry attributes that control how `emit_radius` and `get_rooms_in_radius` propagate:

| Attr | Type | Effect |
|------|------|--------|
| `muffle` | integer | Adds to perceived distance when propagating through this exit |
| `blocked_sound` | boolean | Prevents propagation through this exit entirely |

## Testing softcode

- `@program <ref>/<hook> = <luau>` — install inline (one-liners)
- `@programs [<ref>]` — list programs on an object
- `@rmprogram <ref>/<hook>` — remove a program
- `@reload-world` — reload changed files only (hash-based incremental, shows what changed). Clears bytecode cache.
- Syntax errors are caught at install time and rejected
- Runtime errors disable the program; `@reload <ref>/<hook>` re-enables

## File organization

```
world/
  lib/                   — shared modules (loaded via require())
    random.luau          — dice, weighted choice, shuffle
    collections.luau     — Set, Array helpers
    state_machine.luau   — synchronous FSM
    signal.luau          — pub/sub signals
  town/
    town.toml            — area definition
    barkeep_talk.luau    — referenced by town.toml programs
  forest/
    forest.toml
  system/
    system.toml          — global objects (system:global tag)
    cmd_fight.luau       — game-wide commands
```

Keep `.luau` files next to the `.toml` that references them. The `file` path in programs is relative to the TOML file's directory. Shared utility code goes in `lib/` and is loaded with `require("name")`.
