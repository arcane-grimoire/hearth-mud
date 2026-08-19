# Hearth MUD

A MUD framework in Rust with Luau softcode. Not a game — a platform that games are built on.

Hearth handles the engine (networking, persistence, object model, command dispatch) while game content lives entirely in Luau scripts and TOML data files in a separate directory. Everything is an object — rooms, items, NPCs, players, exits all share the same `GameObject` type and can carry programs (hooks) written in Luau.

## Quick start

```sh
cargo run                        # starts with default hearth.toml
cargo run -- path/to/hearth.toml # game-specific config
cargo test                       # run tests
```

First account created gets admin/builder/player scopes. Connect via telnet on port 4000 or open the web client at http://localhost:8000.

### Docker

```sh
docker run -p 4000:4000 -p 8000:8000 \
  -v ./my-game:/data \
  ghcr.io/arcane-grimoire/hearth-mud hearth.toml
```

## Configuration

```toml
telnet_addr = "0.0.0.0:4000"
web_addr = "0.0.0.0:8000"
db_path = "hearth.db"
autosave_secs = 300
tick_secs = 1
spawn_room = "area/room_key"
game_dir = "path/to/world"
game_web_dir = "web/dist"  # optional, overrides the built-in web client
```

## Architecture

Single-writer engine owns all world state in one tokio task. Everything else communicates via message channels. No locks on world state.

```
telnet ──► net::telnet ──► EngineMessage ──► Engine
web/ws ──► net::web    ──►                    │
REST   ──► POST /api   ──►                    ├── World (objects, scripts)
                                              ├── Accounts (scopes, auth)
                                              ├── SoftcodeRuntime (Lua VM)
                                              └── Database (SQLite)
```

Luau scripts push typed `Intent` enum variants into a batch. The engine validates and applies the batch atomically after the script finishes.

## Hooks

Programs are Luau functions attached to objects. The engine calls them at specific lifecycle points. Hook functions receive `(this, actor, room)` as arguments (`on_tick` receives `(this, state, room)`). `can_*` hooks return `false` to deny the action.

| Hook | Fires when |
|------|-----------|
| `can_get` | Actor tries to pick up this object |
| `can_drop` | Actor tries to drop this object |
| `can_put` | Actor tries to put an item into this container |
| `can_use` | Actor tries to use this object |
| `can_traverse` | Actor tries to use this exit |
| `can_enter` | Actor tries to enter this room |
| `can_look` | Actor tries to look at this object |
| `can_say` | Actor tries to speak in this room |
| `can_see` | Controls visibility of this hidden object |
| `on_get` | After actor picks up this object |
| `on_drop` | After actor drops this object |
| `on_put` | After actor puts an item into this container |
| `on_use` | After actor uses this object |
| `on_enter` | After actor enters this room |
| `on_leave` | After actor leaves this room |
| `on_look` | After actor looks at this object |
| `on_say` | After actor speaks in this room |
| `on_move` | When this object is moved to a new location |
| `on_receive` | When an item is placed in this object |
| `on_damage` | When this object takes damage |
| `on_death` | When this object dies |
| `on_create` | When this object is first created at runtime |
| `on_destroy` | Before this object is destroyed |
| `on_connect` | When a player connects (fires on player and room) |
| `on_disconnect` | When a player disconnects (fires on player and room) |
| `on_whisper` | When an actor whispers in this room |
| `on_emote` | When an actor emotes in this room |
| `on_tick` | Every N ticks (set `tick_interval` attr) |
| `on_startup` | Once when the engine starts |
| `on_shutdown` | Once when the engine is shutting down |
| `on_reload` | After `@reload-world` completes |
| `on_save` | Before each world save |
| `cmd_<name>` | Custom command — `cmd_talk` handles the `talk` command |

Objects tagged `system:global` receive `cmd_*` hooks from anywhere and lifecycle hooks (`on_enter`, `on_leave`, `on_connect`, `on_disconnect`) from all rooms.

## Softcode API

### Read

| Function | Description |
|----------|-------------|
| `get_object(ref)` | Returns a snapshot table of the object (`ref_id`, `key`, `kind`, `title`, `description`, `attrs`, `tags`, etc.) |
| `get_attr(ref, key)` | Returns the value of one attribute, or `nil` |
| `has_attr(ref, key)` | Returns `true` if the attribute exists |
| `pick(ref, attr, ...)` | Walk into nested attrs: `pick(ref, "combat", 1, "hp")` returns the deep value |
| `has_tag(ref, "category:key")` | Returns `true` if the object has this tag |
| `get_tags(ref)` | Returns a list of tag specs (`"category:key"`) |
| `get_room_contents(ref)` | Returns all objects located in a room |
| `get_exits(ref)` | Returns exit objects from a room (`ref_id`, `key`, `target_ref`, `aliases`) |
| `get_location(ref)` | Returns the object's location as an object table |
| `kind_of(ref)` | Returns the kind string (`"room"`, `"item"`, `"npc"`, `"player"`, `"exit"`) |
| `get_inventory(ref)` | Returns objects carried by this object |
| `get_contents(ref)` | Returns objects inside a container |
| `get_owner(ref)` | Returns the owner's ref_id |
| `get_players_in_room(ref)` | Returns player objects in a room |
| `get_timers(ref)` | Returns active timers on this object |
| `get_all_by_kind(kind)` | Returns all objects of a given kind |

### Search

| Function | Description |
|----------|-------------|
| `find_by_tag("category:key")` | Returns all objects with this tag |
| `find_by_attr(key, value)` | Returns all objects where `attrs[key] == value` |
| `find_in_room(room_ref, name)` | Finds an object in a room by name/key match |

### Predicates

| Function | Description |
|----------|-------------|
| `is_player(ref)` | Returns `true` if the object is a player |
| `is_npc(ref)` | Returns `true` if the object is an NPC |
| `is_item(ref)` | Returns `true` if the object is an item |
| `is_room(ref)` | Returns `true` if the object is a room |
| `is_exit(ref)` | Returns `true` if the object is an exit |
| `exists(ref)` | Returns `true` if the object exists |
| `is_carrying(actor, item)` | Returns `true` if actor has item in inventory |
| `is_container(ref)` | Returns `true` if object has the `item:container` tag |
| `same_room(ref_a, ref_b)` | Returns `true` if both objects share a location |

### Write

| Function | Description |
|----------|-------------|
| `set_attr(ref, key, value)` | Sets an attribute. Pass `nil` to remove it. |
| `set_val(ref, attr, ..., value)` | Sets a value deep inside a nested attr: `set_val(ref, "combat", 1, "hp", 5)` |
| `unset_attr(ref, key)` | Removes an attribute |
| `set_tag(ref, "category:key")` | Adds a tag |
| `unset_tag(ref, "category:key")` | Removes a tag |
| `set_title(ref, title)` | Sets the object's title |
| `set_description(ref, desc)` | Sets the object's description |
| `set_owner(ref, owner_ref)` | Sets the object's owner |
| `set_program(ref, hook, source)` | Installs a Luau program on the object |
| `apply_template(ref, table)` | Installs multiple programs from a `{ hook = source }` table |
| `move_object(ref, dest_ref)` | Moves an object to a new location |
| `spawn(opts)` | Creates a new object (`{ key, kind, title, description, location }`) |
| `create_exit(opts)` | Creates a new exit (`{ source, direction, target, aliases }`) |
| `destroy(ref)` | Destroys an object |

### Communication

| Function | Description |
|----------|-------------|
| `emit(ref, message)` | Sends a message to one player |
| `emit_room(room_ref, message, exclude?)` | Sends a message to all players in a room |
| `emit_data(ref, channel, data)` | Sends structured JSON to a player's web client |
| `prompt(ref, message)` | Prompts a player for input (fires `on_reply` with their response) |
| `log(message)` | Writes to the server log |

### Timers

| Function | Description |
|----------|-------------|
| `after(ticks, ref, hook, data?)` | Schedules a hook to fire after N ticks, with optional data payload |
| `cancel_after(ref, hook)` | Cancels a pending timer |

### Maps & Dungeons

| Function | Description |
|----------|-------------|
| `get_map_template(name)` | Returns parsed map template data (grid, terrain, cells) as a read-only table |
| `instantiate_map(name)` | Spawns rooms and exits from a map template, returns `{ entrance_ref, room_count }` |
| `generate_dungeon(opts)` | Procedurally generates a dungeon layout |
| `destroy_dungeon(seed)` | Destroys all rooms generated by a dungeon seed |

### Utility

| Function | Description |
|----------|-------------|
| `json_encode(value)` | Converts a Lua value to a JSON string |
| `json_decode(string)` | Parses a JSON string into a Lua value |
| `trigger(ref, hook)` | Manually fires a hook on an object |

## Standard Library

These modules are bundled in the binary and available via `require()`. Games can override any module by placing a same-named `.luau` file in their `lib/` directory.

| Module | Description |
|--------|-------------|
| `str` | String utilities: `split`, `trim`, `pad_left`, `pad_right`, `wrap`, `truncate`, `pluralize`, `title_case` |
| `collections` | Ordered `Set` and `Array` helpers: `map`, `filter`, `find`, `reduce`, `flat`, `slice`, `every`, `some` |
| `random` | Dice rolls (`roll("2d6")`), `weighted_choice`, `shuffle`, `sample`, `chance` |
| `text` | Rich text formatting with accessible/visual modes: `bar`, `header`, `divider`, `table`, `box`, `stat`, `for_mode` |
| `signal` | Pub/sub event system: `Signal` (fire-and-forget) and `Subject` (replay last value) |
| `state_machine` | Synchronous FSM with guards, actions, enter/exit callbacks |
| `Grid3D` | 3D spatial grid with get/set/update/iterate (complements the Rust-side Grid2D) |
| `grids` | Re-export: `{ Grid3D = require("Grid3D") }` |

## Web Client

The built-in web client is a Svelte 5 app with:

- Scrolling output pane with BBCode rendering and clickable `[cmd=...]` elements
- Structured sidebar: Who's Here, What's Here, Exits (clickable)
- Command input with history
- In-browser object editor (for builders)
- Settings drawer with theme toggle

### Game Widgets

Games can push custom UI panels to the sidebar without forking the web client. Use `emit_data()` with a `widget` field and the framework renders it:

```lua
emit_data(actor, "map", {
    widget = "map",
    title = "Map",
    current = room_ref,
    rooms = { { ref = "town/square", name = "Square", short = "Square", x = 0, y = 0 } },
    edges = { { from = "town/crossroads", to = "town/square", dir = "north" } },
})
```

Built-in widget types:

| Widget | Data shape |
|--------|-----------|
| `map` | `{ current, rooms: [{ ref, name, short, x, y }], edges: [{ from, to, dir }] }` |
| `list` | `{ items: [{ label, value?, command?, color? }], empty? }` |
| `meter` | `{ bars: [{ label, value, max, color? }] }` |
| `text` | `{ text }` or `{ lines: [{ text, color? }] }` or `{ html }` |

## BBCode Markup

Transport-neutral styling — converted to ANSI for telnet, HTML for web:

`[b]bold[/b]`, `[u]underline[/u]`, `[dim]dim[/dim]`, `[red]color[/red]`, `[cmd=go north]clickable[/cmd]`

## License

MIT
