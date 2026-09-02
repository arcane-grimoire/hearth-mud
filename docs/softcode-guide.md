# Softcode Guide

Hearth's softcode system lets builders attach a Luau script to an object. A
script runs in a sandbox — it can read the world freely but can only change it
through a controlled set of functions that queue up changes for the engine
to apply.

This is the reference. For complete, pasteable implementations — a bulletin
board, a vendor, a scheduler, a weather system, vehicles, a job tracker — see
the [MUSH](mush-cookbook.md), [Diku](diku-cookbook.md) and
[LPMUD](lpmud-cookbook.md) cookbooks. Each opens with a concept map for builders
arriving from that tradition — PennMUSH/TinyMUX, Diku/ROM/Circle/Smaug, or LPC.

## One script per object

An object has **one script** — a single Luau chunk that defines its hooks as
top-level functions:

```lua
local GREETING = "Welcome, traveler."   -- shared by every hook below

function on_enter(this, actor, room)
  emit(actor, GREETING)
end

function cmd_talk(this, actor, room, args)
  emit(actor, this.display_name .. ' says, "' .. GREETING .. '"')
end
```

This is the Godot/LPMUD "object is the unit, hooks are its methods" model. The
file-level scope is the object's shared "class body": every hook in the script
sees the same top-level `local` helpers and constants, so behavior that spans
several hooks can share code without globals or `require()`. The engine parses
the script to learn which hooks it defines.

A hook must be **statically present at the top level** of the chunk for the
engine to detect it. Two forms count:

```lua
function on_get(this, actor, room) ... end     -- declaration
on_get = function(this, actor, room) ... end   -- assignment
```

Anything else is invisible to the parser: functions inside string literals,
inside a loop or `if`, under a dotted or `:method` name, or built by
metaprogramming (`_G["cmd_" .. verb] = ...`). Those fail silently — no hook is
registered and nothing reports an error — so build hooks by writing them out,
even when a table drives their bodies.

Persistent `state` (see `on_tick` below) is per-object — shared across all of
that object's hooks, like a Godot node's member variables.

## Hooks

A hook is a named function the engine calls at a defined moment. There are
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
| `on_startup` | Engine starts, after world loads | `(this, this, room)` — **no state table**, see below |
| `on_shutdown` | Engine is shutting down, before final save | `(this, this, room)` — **no state table**, see below |
| `on_reload` | After `@reload-world` completes | `(this, this, room)` — **no state table**, see below |
| `on_save` | Before each world save (autosave or `@save`) | `(this, this, room)` — **no state table**, see below |
| `on_create` | When this object is first created at runtime | `(this, this, room)` — **no state table**, see below |
| `on_destroy` | Before this object is destroyed | `(this, actor, room)` |
| `on_hour` | Game clock: the hour rolled over (on `system:global` objects) | `(this)` — read `get_time()` |
| `on_day` | Game clock: the day rolled over | `(this)` — read `get_time()` |
| `on_dawn` | Game clock: the hour reached `dawn_hour` | `(this)` — read `get_time()` |
| `on_dusk` | Game clock: the hour reached `dusk_hour` | `(this)` — read `get_time()` |
| `cmd_*` | Any name — becomes a player command | `(this, actor, room, args)` |

**`can_traverse` vs `can_enter`.** These gate the same movement but belong to
different objects, and the pairing matches the DSL locks: `can_traverse` is the
**exit's** hook (`this` is the exit), alongside the exit's `traverse` lock;
`can_enter` is the **destination room's** hook (`this` is the room), alongside
the room's `enter` lock. Use the exit's when the passage is what's gated (a
locked door, a toll bridge); use the room's when the place is (a ward, a private
chamber) and you want it to hold no matter which exit leads in. Both receive the
room the actor is leaving as `room`.


**Only `on_tick` gets a `state` table.** The lifecycle hooks above are fired
with no actor, so their second parameter is the object itself — `this` twice.
Naming it `state` and assigning to it is actively harmful, because object tables
support property-assignment sugar:

```lua
-- WRONG. There is no state table here; `state` IS `this`, and this line is
-- `set_attr(this, "boot_count", 42)` — a persisted attribute, written silently.
function on_startup(this, state, room)
  state.boot_count = 42
end
```

Write these as `(this, _actor, room)` and keep persistent data in attrs
deliberately:

```lua
function on_startup(this, _actor, room)
  set_attr(this, "boot_count", (get_attr(this, "boot_count") or 0) + 1)
end
```

### Global hooks

Objects tagged `system:global` receive lifecycle hooks from all rooms, not just
the room they're in. This includes `on_enter`, `on_leave`, `on_connect`, and
`on_disconnect`, in addition to `cmd_*` hooks (which were always global).

This lets a single global rules object handle game-wide events:

```toml
[[objects]]
key = "rules"
kind = "code"
tags = ["system:global", "system:hidden"]
script = "rules.luau"   # one script defining on_enter, on_connect, etc.
```

The single `rules.luau` defines both hooks as top-level functions, so they can
share helpers:

```lua
local function refresh_map(actor) ... end

function on_enter(this, actor, room) refresh_map(actor) end
function on_connect(this, actor, room) refresh_map(actor) end
```

**Give the global surface `kind = "code"`, not a hidden item.** `Kind::Code`
means "code with no physical presence": the engine already leaves those objects
out of room contents, `look`, inventory, and `get`/`put`, so a global rules or
command object stops pretending to be a thing lying on the floor. Dispatch
doesn't care — the global index keys off the `system:global` tag and the hooks a
script defines, not the kind. (`system:hidden` on top is belt-and-braces, and
harmless.)

This is about the *global* surface only. A command that belongs to a real,
physical object — `cmd_spin` on a roulette wheel, `cmd_pull` on a lever,
`cmd_talk` on an NPC — stays on that item or NPC, where it reads as the thing's
own behavior. The smell is a typed-anywhere command surface wearing an item's
clothes, not commands on items.

## Hook parameters

Every hook function receives these parameters:

| Parameter | Description |
|-----------|-------------|
| `this` | The object the script is attached to |
| `actor` | The player performing the action |
| `room` | The room the action is happening in |
| `state` | (`on_tick` only) Persistent state table |
| `args` | (`cmd_` only) The text after the command name |

Each object parameter is a table with the object's data:

```lua
{
  ref_id = "#5",
  key = "sam",
  kind = "player",
  title = "Sam",
  display_name = "Sam",
  description = "A traveler.",
  location_ref = "#3",
  attrs = { hp = 100, class = "warrior" },
  tags = { "quest:worthy", "faction:guild" }
}
```

These tables are snapshots — changing them in Lua has no effect. Use the write
API functions to actually change the world.

## Property syntax

Hook-facing objects (`this`, `actor`, results of `get_object`/`get_location`)
support reading and writing fields and attributes directly:

```lua
function on_get(this, actor, room)
  this.picked_up_count = (this.picked_up_count or 0) + 1   -- attrs
  this.title = "The Shiny Sword"                            -- title
  this.description = "It gleams."                           -- description
  emit(actor, this.description)                             -- reads are pending-aware
end
```

Rules:

- **Writes push intents** — `this.hp = 0` is exactly `set_attr(this, "hp", 0)`;
  `this.title = ...` is `set_title`; `this.description = ...` is
  `set_description`. Nothing touches the world until the batch applies.
- **Reads see your own writes** — `this.hp` resolves through the same path as
  `get_attr(this, "hp")`: pending writes first, then the snapshot. Both syntaxes
  always agree.
- **`this.key = nil` unsets** — it's `unset_attr(this, "key")`, not a null set.
- **Protected fields reject writes** with an error: `ref_id`, `key`, `kind`,
  `location_ref` (use `move_object(ref, destination)`), plus computed/container
  fields (`display_name`, `attrs`, `tags`).
- **Iteration** works via generalized iteration (`for k, v in this do`) and sees
  the snapshot as the script entered it, not same-script writes. Each access to
  `this.attrs` returns a fresh proxy table, so proxies shouldn't be compared or
  used as table keys.
- **List results stay plain tables** — `all_objects()`, `get_contents()`, etc.
  return ordinary snapshot tables (writes to them evaporate silently, as before);
  only single-object handles carry the live property behavior.

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
| `get_room_contents(ref)` | table | Everything located in ref of any kind except exits and code objects — the one to use for a vehicle's occupants as well as a room's |
| `get_exits(ref)` | table | List exits from a room |
| `get_location(ref)` | table or nil | Get the object's container |
| `kind_of(ref)` | string or nil | Get the object's kind |
| `get_owner(ref)` | string or nil | Get the owner's ref_id |
| `get_contents(ref)` | table | **Items** located in ref — a container's contents. Filtered to `kind == "item"`, so it does **not** see a player or NPC inside (a vehicle's passengers); use `get_room_contents` for those. |
| `get_timers(ref)` | table | Active timers on this object |
| `get_tick` | number | Current engine tick count (a value, not a function). Use for cooldowns, time-based logic. |
| `resolve_key(file_key)` | string or nil | Returns the ref_id for a TOML-defined object by file key (e.g., `"town/crossroads"`) |

### Search

| Function | Returns | Description |
|----------|---------|-------------|
| `find_by_tag(spec)` | table | Find all objects with a tag |
| `find_by_attr(key, value)` | table | Find all objects where `attrs[key] == value` |
| `find_in_room(room, name)` | table or nil | Fuzzy-match an object by name in a room |
| `find_in_inventory(ref, name)` | table or nil | Same, among what `ref` carries (or a container holds) |
| `find_player(name)` | table or nil | Resolve a player by name, online or off; prefers a connected one |
| `find_exit(room, name)` | table or nil | Match a direction **or alias**, the way movement does. `find_in_room` can never return an exit — `objects_in` excludes them. |
| `responds_to(ref, hook)` | bool | Does this object handle that hook (including via its archetype)? For a dispatcher that delegates only when the target implements the behavior. |
| `get_time()` | table or nil | The in-world clock: `{ total_minutes, minute, hour, day, month, year, is_day, weekday?, month_name? }`. `nil` when no `[clock]` is configured. |
| `get_inventory(ref)` | table | List items carried by an object |
| `get_players_in_room(room)` | table | Online players in a room |
| `get_all_by_kind(kind)` | table | All objects of a kind (`"room"`, `"npc"`, etc.) |
| `all_objects()` | table | Every object ref in the world, regardless of kind. World enumeration for batch fixups — see `@eval` in [Command Reference](commands.md) |
| `match_name(name, input)` | boolean | Partial name matching — `match_name("iron sword", "ir")` returns `true`. Matches start of full name or any word. |

### Spatial queries

| Function | Returns | Description |
|----------|---------|-------------|
| `get_nearby(room, x, y, radius)` | table | All objects in `room` whose `_x`/`_y` attrs are within `radius` |
| `get_rooms_in_radius(room, distance)` | table | BFS walk through exits, returns `{ {ref, distance, name}, ... }`. Respects `muffle` and `blocked_sound` exit attrs. |

The `ref` argument can be either a dbref string (`"#5"`) or an object table
(like `this` or `actor`). Both work everywhere.

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
| `set_script(ref, source)` | Set the object's **whole** script (source defines its hook functions). Replaces any existing script. |
| `set_lib(ref, name, source)` | Set a `require()`able library module (bare `name`, no prefix) on a `Kind::Code` object |
| `apply_template(ref, source_or_table)` | Install a script from a whole source string, or from a table of source fragments concatenated into one script (see below) |
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
  -- resolve the gate by its file key (a #N dbref would be brittle in an example)
  local gate = resolve_key("dungeon/gate")
  set_attr(gate, "open", true)
  trigger(gate, "on_activate")
end
```

The optional third argument passes a data table to the triggered hook — it
arrives as the hook's **4th parameter**, a real table (no magic attr). An
optional fourth argument fires the hook *as* a chosen actor (default: the
ambient actor — whoever caused the current batch):

```lua
-- Alert an NPC about combat, firing the hook as the attacker.
trigger(npc, "on_alert", { threat = actor.ref_id, type = "combat" }, actor)

-- In the NPC's on_alert hook — data is the 4th arg:
function on_alert(this, actor, room, data)
  if data and data.type == "combat" then
    emit_room(room, this.display_name .. " rushes toward the sound of fighting!")
  end
end
```

The triggered hook still runs *after* the current script's batch commits
(deferred, like all effects) — it can't return a value to the caller.

Triggers **do** nest: a triggered hook's own `trigger` fires from inside the
first one's delivery, so a chain runs lever → gate → alarm depth-first. That
recursion is bounded by `MAX_TRIGGER_DEPTH` (8). Past it the remaining triggers
are refused and logged rather than fired — the world stays consistent, because
every batch up to that point has committed.

Do not write a hook that triggers itself unconditionally. It is not an infinite
loop that eventually stops; without the cap it recurses on the engine's stack
until the process aborts. The cap exists precisely because two lines of softcode
could otherwise kill the server.

### set_script / set_lib

`set_script(ref, source)` sets an object's **entire** script — the source is
one Luau chunk that defines the object's hooks as top-level functions. To give
a spawned object several hooks, put all their `function <hook>(...) end`
definitions in **one** source string and call `set_script` once:

```lua
local imp = spawn({ key = "imp", kind = "npc", title = "an imp", location = room })
set_script(imp, [[
  function on_look(this, actor, room)
    emit(actor, "The imp grins at you.")
  end
  function cmd_poke(this, actor, room, args)
    emit(actor, "The imp cackles.")
  end
]])
```

A second `set_script` **replaces** the first — it does not merge. Build the
whole script in one string.

`set_lib(ref, name, source)` sets a `require()`able library module (a bare
`name` — no `lib_` prefix) on a `Kind::Code` object. See
[Libraries](#libraries) below.

### apply_template

Install a script from a template. `apply_template` accepts either a whole
source string, or a table of source fragments — each fragment defining one or
more hook functions — that get concatenated into a single script (so an
object's methods can be assembled from reusable pieces that still share one
scope). Works naturally with `require()` for reusable behavior bundles:

```lua
-- lib/template_wilderness.luau
return {
  'function on_enter(this, actor, room) ... end',
  'function can_look(this, actor, room) ... end',
}

-- in a hook:
local tmpl = require("template_wilderness")
apply_template(room_ref, tmpl)   -- fragments concatenated into one script
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

The target object's script needs to define the specified hook for anything to
happen. Timers are persisted to the database and survive server restarts. Use
`cancel_after(ref, hook)` to cancel a pending timer.

## Communication

| Function | Description |
|----------|-------------|
| `emit(ref, message)` | Send a message to one player |
| `emit_room(room, message, exclude?)` | Send to everyone in a room. `exclude` is an optional list of refs to skip. |
| `emit_nearby(room, x, y, radius, message, exclude?)` | Send to players in `room` whose `_x`/`_y` attrs are within `radius`. For coordinate-based shared rooms. |
| `emit_radius(room, distance, messages, exclude?)` | BFS walk through exits, delivers distance-keyed messages to players in reached rooms. |
| `emit_data(ref, channel, data)` | Send structured JSON to a player's web client (see Widgets below). |
| `prompt(actor, obj, hook)` | Arm a one-shot input prompt: the actor's next line fires `hook` on `obj` instead of running as a command. (Stores `_prompt_object`/`_prompt_hook` on the actor.) |
| `log(message)` | Write to the server log. |

### Multi-line text

`emit` and `emit_room` accept embedded newlines and deliver each line properly
— interior `\n` is normalized to CRLF on the way out. So a wrapped paragraph
goes in one call:

```lua
local str = require("str")
emit(actor, str.wrap(long_description, 72))   -- one call, many lines
```

### `movement_blocked` — holding an actor in place

Setting the `movement_blocked` attr on an actor stops them moving and gives the
refusal message:

```lua
set_attr(actor, "movement_blocked", "You can't move while in combat!")
-- ...later
set_attr(actor, "movement_blocked", nil)
```

It's honored by ordinary exit movement (`do_move`) and by `grid_move`, so a hold
covers both walking through a door and crossing a wilderness map. It is *not*
consulted by `grid_can_move`, which takes coordinates rather than an actor and
answers "is that cell passable", not "may this actor move" — check the attr
yourself when building an exit list for a held actor. Admin teleport bypasses it
deliberately.

Use it for a blanket hold. For gating one particular passage, use the exit's
`can_traverse` hook or a `traverse` lock instead.

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

A global script is a Luau script not conceptually attached to any *physical*
object — for game systems like weather, combat, economy, respawns. Under the
hood it's the script on a `Kind::Code` object whose source defines `on_tick`: a
kind of object that exists purely to hold code, and that is guaranteed to never
show up in room contents, `look`, inventory, `get`, or a container — see
[Libraries](#libraries) below, which uses the same kind for the other
kind of code that isn't tied to a physical thing.

The script's `on_tick` has the same signature as any other object's tick hook —
`this` is the Code object itself:

```lua
function on_tick(this, state, room)
  state.counter = (state.counter or 0) + 1
  if state.counter % 60 == 0 then
    local weathers = {"clear", "cloudy", "rainy", "stormy"}
    state.weather = weathers[math.random(#weathers)]
  end
end
```

`room` is `nil` for a global script — it has no location. `state` persists
between ticks exactly like a per-object `on_tick`'s state does.

### Builder commands for global scripts

```
@script weather = function on_tick(this, state, room) ... end
@script-interval weather = 60
@scripts
@rmscript weather
```

`[[scripts]]` blocks in area TOML files define global scripts the same way —
see `docs/getting-started.md`/loader docs for the TOML shape. A script's
source and tick interval reconcile on `@reload-world` like any other
file-owned script; an in-game edit via `@script` shadows the file version,
same as `@program` does everywhere else. The source you give `@script` must
define `function on_tick(this, state, room) ... end`; `@script-interval` only
changes the object's `tick_interval` attr.

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
| `put` | containers | Before an item is put into it |

### Lock functions

| Function | Description |
|----------|-------------|
| `perm(scope)` | Actor's account has this scope (admin implies all) |
| `has_tag(spec)` | Actor has this tag |
| `has_attr(key)` | Actor has this attribute |
| `has_attr(key, value)` | Actor has this attribute with this value |
| `in_inventory(tag_spec)` | Actor is carrying an object with this tag |
| `is_kind(kind)` | Actor is this kind (player, npc, item) |
| `time_between(start, end)` | Current UTC (real-world) hour is in range |
| `game_time_between(start, end)` | Current in-world clock hour is in range (false if no `[clock]`) |
| `true` / `false` | Always allow / always deny |

Combinators: `AND`, `OR`, `NOT`, parentheses.

### Builder commands for locks

```
@lock #8/traverse = has_tag(vip) OR perm(builder)
@lock #12/get = in_inventory(quest:key)
@lock here/enter = NOT is_kind(npc)
@unlock #8/traverse
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
@program <ref> = <luau source>    -- set the object's WHOLE script (hooks are functions in it)
@program <ref>                    -- open a multi-line editor, seeded with the current script
@programs [<ref>]                 -- show the object's script + its derived hooks (default: room)
@rmprogram <ref>                  -- remove the object's script entirely
@reload <ref>                     -- re-validate and re-enable the object's script
```

`@program <ref> = <source>` sets the object's **whole** script. The source
defines the object's hooks as top-level functions — one `@program` write is
the whole script, not a single hook. It's syntax-checked before being
installed; on a compile error the script is rejected and the error is shown.
`@programs <ref>` reports the object's script and the hooks the engine derived
from it. `@reload <ref>` re-validates and re-enables a script that a runtime
error disabled.

`@program <ref>` with nothing after the ref (no `=`) opens a multi-line editor
seeded with the object's current script — the same editor `@eval` uses: type
source across as many lines as you need, then a bare `.` on its own line to
install it, or `@abort` to cancel without writing anything. The buffer lives
on your player object's attrs, so it survives a disconnect mid-edit.

### Ticks (per-object)

```
@set <ref>/tick_interval = 5             -- run on_tick every 5 seconds
@program <ref> = function on_tick(this, state, room) ... end   -- script defines on_tick
```

### `@eval` (admin only)

Unlike the rest of this section, `@eval` requires the `admin` scope, not
`builder` — it runs arbitrary Luau against the live world with no attached
object, hook, or lock check standing between the script and the write API.

```
@eval <luau>       -- run one line immediately
@eval               -- open a multi-line editor; '.' alone on a line runs it, '@abort' cancels
```

It has the same read/write API as any hook (see above), runs under the same
instruction [Budget](#sandboxing) as any hook, and world writes still go
through the normal Intent batch. Its `return` value, if any, is echoed back
to you, along with a count of the writes it applied. This is the mechanism
for fixing up existing objects when a Program changes — pair it with
`all_objects()` to sweep the whole world:

```lua
@eval for _, ref in ipairs(all_objects()) do if is_npc(ref) then set_attr(ref, "hp", 100) end end
```

### `@import` / `@export` (admin only)

`@import`/`@export` cross the file/database boundary explicitly, in each
direction, instead of the boot loader's automatic (and, by default, still
running) reconcile. They're not softcode APIs — there's no Luau involved —
but they're what makes the scripts this section describes portable between
a git checkout and a running database: `@import` installs `.luau` sources
(and the TOML that attaches them as object scripts and libs) the same way
`@program`/`@lib` would. Use

```
@import <path> [--dry-run]
@export <path>
```

Full semantics — the upgrade reconciliation, the three-way comparison that
resolves a conflicting edit, and why a conflict is always non-destructive —
are in [the Command Reference](commands.md#import--export). The short version
for softcode authors: a script you write by hand in `@import`'s bundle and a
script you author with `@program`/`@lib` end up indistinguishable once
installed. Re-importing a bundle after editing one of its scripts in-game
never silently discards that edit; at worst it's reported as a conflict, with
the edit preserved.

## Modules (require)

Shared Luau code comes from two places, resolved the same way through
`require("name")` — a hook, global script, or `@eval` doesn't need to know
which kind it got:

- **Shipped modules** — files under `<game_dir>/lib/*.luau` (plus a small
  embedded stdlib). File-owned: edit the file and `@reload-world`.
- **User libraries** — authored in-game with `@lib` (or `set_lib`), stored in
  the database. See [Libraries](#libraries) below.

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
cached — the source only runs once per engine lifetime (or, for a user
library, until it is next edited — see below). Cache clears
wholesale on `@reload-world`.

Circular requires are rejected rather than looping forever. If `a` requires
`b` and `b` requires `a`, the inner call errors with
`require cycle detected: a -> b -> a`. Break the cycle by extracting the
shared piece into a third module.

A module's free names resolve against the **calling hook's environment**, so a
module function can use whatever API that hook has — `emit`, `set_attr`,
`grid_move`, `ink_*`, all of it. That is deliberate (see `install_require` in
`src/softcode/mod.rs`): it is what lets a shared `combat.luau` do the emitting
rather than returning strings for every caller to emit themselves.

Two consequences worth knowing:

- **A module is not a sandbox boundary.** It is shared code, running with the
  caller's authority. Keep a module's side effects obvious from its name, the
  same way you would in any language.
- **With no hook running, a module falls back to plain globals.** That is the
  unit-mode `.test.luau` case, so a pure compute module tests exactly as it
  behaves in a hook — but one that emits has nothing to emit *through*, which is
  a good reason to keep computation and output in separate functions.

### Libraries

A library is a user-authored module — the in-game, database-backed
counterpart to a shipped `lib/*.luau` file. Like a global script, it lives on
a `Kind::Code` object, but it's stored as a named lib module (a bare `<name>`,
no prefix) rather than as one of the object's hook functions; a single Code
object can carry both a script and one or more libs if that's genuinely useful,
though that's unusual. Libs are a separate concept from hooks — `lib_` is not a
hook prefix.

```
@lib combat_utils = local M = {} function M.roll_damage(base, bonus) return base + math.random(1, bonus) end return M
@libs
@rmlib combat_utils
```

Softcode can set one at runtime with `set_lib(ref, name, source)`. Once
created, `require("combat_utils")` resolves it exactly like a shipped module —
from any hook, global script, or `@eval`, anywhere in the game.

**Name collisions with a shipped module are refused at write time** —
`@lib str = ...` when `str` is already a shipped module (embedded stdlib or
`<game_dir>/lib/str.luau`) fails immediately with an error, rather than
silently shadowing it. This applies on every path that can set a lib:
`@lib`, the REST API's `set_lib` action, and softcode's own `set_lib()`.

**Editing a library takes effect on the next `require`, not the current
one.** `require()`'s result is cached for the lifetime of the module (like
any module) — editing `combat_utils` invalidates that cache, so the *next*
`require("combat_utils")` anywhere re-evaluates the new source. Anything that
already called `require("combat_utils")` and holds the old table keeps using
it; there's no live-patching of in-flight references, same as shipped modules
on `@reload-world`.

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

## WASM plugins

A second extension language, for pure data-in → data-out work you would rather
write in Rust (or AssemblyScript, or anything targeting WebAssembly) than in
Luau — a name generator, a pathfinder, a parser.

Plugins are **compute only**. They never touch world state: Luau stays the only
thing that emits intents, so a plugin can compute an answer but not act on it.
Every call runs under a fuel budget, so a runaway plugin traps instead of
hanging the engine.

Modules load from `<game_dir>/wasm/*.wasm` on every boot and `@reload-world`.
They are **code, not content**: never persisted to the database, exactly like
`lib/*.luau` and `.ink` files.

Two ways to call one:

```lua
-- Bound automatically: the engine introspects a module's exports and binds
-- each one matching the plugin ABI under a table named after the module, so
-- names.wasm exporting `generate` is simply:
local name = names.generate({ seed = 42, syllables = 3 })

-- The low-level escape hatch, for anything the binding doesn't cover:
local result = wasm_call("names", "generate", { seed = 42 })
```

The wasm file is the source of truth for what exists. An optional sidecar
`<stem>.toml` manifest only *annotates* — a description, a different Luau name
or namespace — and cannot invent a binding for an export that isn't there, so
the two can't drift.

A plugin that exports `reset()` opts into instance pooling: the host keeps one
instance resident and rewinds its per-call arena instead of re-instantiating.
Without `reset` each call gets a fresh instance, so it can never leak across
calls. A trap evicts a pooled instance rather than reusing a poisoned one.

The reference plugin — a deterministic, seedable Markov name generator with a
bump allocator and `reset` — is in `plugins/names/`. Implementation:
`src/softcode/wasm.rs`, hosted on `wasmi` (a pure-Rust interpreter, so behavior
is deterministic by construction).

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
- **Isolation between invocations.** Each hook invocation gets its own
  environment table. Globals set in one invocation are not visible to another —
  share state within an object through top-level `local`s in its script (visible
  to every hook it defines) or persistent `state`, not through globals.

## Tips

- Use `emit()` for messages only the actor sees. Use `emit_room()` for
  messages everyone in the room sees. Pass `{actor.ref_id}` as the third
  argument to exclude the actor from the room message.
- Attributes can hold any JSON-compatible value: strings, numbers, booleans,
  tables (arrays/objects), or nil.
- Tags are great for boolean state (`quest:completed`, `faction:guild`).
  Use attributes for values (`hp = 100`).
- The `this` parameter is the object the script is attached to — use it to
  read and write the object's own state without hardcoding ref strings.
- Persistent `state` (`state` in `on_tick` and the lifecycle hooks) is private
  to the object and shared across its hooks. It doesn't show up in `examine` or
  `get_attr`. Use it for internal working memory.
- Locks and hooks stack: DSL lock is checked first, then the `can_` hook.
  You can use one or both.
