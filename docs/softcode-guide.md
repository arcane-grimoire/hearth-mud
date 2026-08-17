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

## Known hooks

| Hook | Fires when... |
|------|---------------|
| `can_get` | Before an actor picks up this object |
| `on_get` | After an actor picks up this object |
| `can_drop` | Before an actor drops this object |
| `on_drop` | After an actor drops this object |
| `can_traverse` | Before an actor uses this exit |
| `on_enter` | After an actor enters this room |
| `on_leave` | After an actor leaves this room |
| `can_look` | Before an actor looks at this object |
| `on_look` | After an actor looks at this object |
| `can_say` | Before an actor speaks in this room |
| `on_say` | After an actor speaks in this room |
| `cmd_*` | Any name — becomes a player command |

## Hook parameters

Every hook function receives these parameters:

| Parameter | Description |
|-----------|-------------|
| `this` | The object the program is attached to |
| `actor` | The player performing the action |
| `room` | The room the action is happening in |
| `args` | (`cmd_` only) The text after the command name |

Each of these is a table with the object's data:

```lua
-- actor is a table like:
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
| `get_room_contents(ref)` | table | List objects in a room |
| `get_exits(ref)` | table | List exits from a room |
| `get_location(ref)` | table or nil | Get the object's container |
| `kind_of(ref)` | string or nil | Get the object's kind |

The `ref` argument can be either a ref string (`"area/starter/item/sword"`) or
an object table (like `this` or `actor`). Both work everywhere.

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

## Builder commands

All `@` commands require the `builder` scope.

### @program — attach a script

```
@program <ref>/<hook> = <luau source>
```

Examples:

```
@program area/starter/item/rusty_sword/on_get = function on_get(this, actor, room) emit(actor, "The sword hums!") end
```

```
@program area/starter/room/town_square/cmd_pray = function cmd_pray(this, actor, room, args) emit(actor, "You kneel and pray. A warm feeling washes over you.") emit_room(room, actor.display_name .. " kneels in prayer.", {actor.ref_id}) end
```

The source is syntax-checked before being installed. If there's a compile
error, the program is rejected and the error is shown.

### @programs — list scripts

```
@programs                     -- programs on the current room
@programs <ref>               -- programs on a specific object
```

### @rmprogram — remove a script

```
@rmprogram <ref>/<hook>
```

## Sandboxing

Programs run in an isolated environment per call:

- **No file/network access.** Luau's baseline globals already exclude `io`,
  `os.execute`, `require`, etc.
- **Instruction budget.** A runaway script (infinite loop, excessive recursion)
  is killed after a fixed instruction count. The engine stays responsive.
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
