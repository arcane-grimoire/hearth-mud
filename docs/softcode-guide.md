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
| `can_use` | Before an actor uses this object | `(this, actor, room)` |
| `on_use` | After an actor uses this object | `(this, actor, room)` |
| `can_traverse` | Before an actor enters this room via an exit | `(this, actor, room)` |
| `on_enter` | After an actor enters this room | `(this, actor, room)` |
| `on_leave` | After an actor leaves this room | `(this, actor, room)` |
| `can_look` | Before an actor looks at this object | `(this, actor, room)` |
| `on_look` | After an actor looks at this object | `(this, actor, room)` |
| `can_say` | Before an actor speaks in this room | `(this, actor, room)` |
| `on_say` | After an actor speaks in this room | `(this, actor, room)` |
| `on_tick` | Every N ticks (set `tick_interval` attr) | `(this, state, room)` |
| `cmd_*` | Any name — becomes a player command | `(this, actor, room, args)` |

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

| Function | Returns | Description |
|----------|---------|-------------|
| `get_object(ref)` | table or nil | Get an object by ref |
| `get_attr(ref, key)` | value or nil | Get a single attribute |
| `has_attr(ref, key)` | boolean | Check if an attribute exists |
| `has_tag(ref, spec)` | boolean | Check if a tag exists (e.g., `"quest:worthy"`) |
| `get_tags(ref)` | table | List all tags as spec strings |
| `get_room_contents(ref)` | table | List objects in a room (excludes exits) |
| `get_exits(ref)` | table | List exits from a room |
| `get_location(ref)` | table or nil | Get the object's container |
| `kind_of(ref)` | string or nil | Get the object's kind |
| `find_by_tag(spec)` | table | Find all objects with a tag |
| `find_in_room(room, name)` | table or nil | Fuzzy-match an object by name in a room |
| `get_inventory(ref)` | table | List items carried by an object |
| `get_players_in_room(room)` | table | Online players in a room |
| `get_all_by_kind(kind)` | table | All objects of a kind (`"room"`, `"npc"`, etc.) |

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
| `set_attr(ref, key, value)` | Set an attribute (JSON-compatible values) |
| `unset_attr(ref, key)` | Remove an attribute |
| `emit(ref, message)` | Send a message to a player |
| `emit_room(ref, message, exclude?)` | Send to everyone in a room. `exclude` is an optional list of refs to skip |
| `move_object(ref, destination)` | Move an object to a new location |
| `set_tag(ref, spec)` | Add a tag (e.g., `"quest:completed"`) |
| `unset_tag(ref, spec)` | Remove a tag |
| `set_title(ref, title)` | Change an object's display name |
| `set_description(ref, desc)` | Change an object's description |
| `destroy(ref)` | Remove an object from the world (not players) |
| `trigger(ref, hook)` | Fire a hook on another object (see below) |
| `spawn(opts)` | Create a new object (see below) |

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

The gate's `on_activate` program could then emit to its own room:

```lua
function on_activate(this, actor, room)
  if get_attr(this, "open") then
    emit_room(room, "The iron gate grinds open!")
    set_description(this, "An iron gate, standing open.")
  end
end
```

Triggers don't recurse — if the triggered hook also calls `trigger`, the
second trigger fires after the first finishes. This prevents infinite loops.

## Utility

| Function | Description |
|----------|-------------|
| `log(message)` | Print a debug message to the server console |

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

Modules have stdlib access (string, table, math, require) but **not** the
game write API (emit, set_attr, etc.). They're for pure utility code.

### Bundled modules

| Module | Description |
|--------|-------------|
| `random` | `roll(sides)`, `dice(n, sides)`, `chance(pct)`, `pick(list)`, `weighted(choices)`, `shuffle(list)`, `sample(list, n)` |
| `collections` | `Set` class (add/remove/has/union/intersection/difference), `Array` helpers (map/filter/find/reduce/flat/contains/reverse/slice) |
| `state_machine` | Synchronous FSM: `new({initial, transitions, on_enter, on_exit})`, `:send(event)`, `:can(event)`, `:is(state)` |
| `signal` | Pub/sub: `new()` → Signal with `:fire(...)`, `:connect(fn)` → Connection with `:disconnect()`. Also `newSubject(...)` for replay-last-value signals. |
| `text` | Rich formatting with accessible/visual modes. `for_mode(mode)` returns a formatter with `bar`, `header`, `table`, `box`, `stat`, `divider`, `color`, `bold`, `dim`. |
| `str` | String utilities: `split`, `trim`, `starts_with`, `ends_with`, `title_case`, `pad_right`, `pad_left`, `center`, `truncate`, `pluralize`, `wrap` |
| `Grid3D` | 3D grid data structure (from luau-grids). See also Rust-side Grid2D below. |

### Display modes and accessibility

Players can toggle their display mode:

```
@display visual        -- full formatting (ANSI colors, Unicode box-drawing, progress bars)
@display accessible    -- plain text (screen-reader friendly, no ANSI, descriptive labels)
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
