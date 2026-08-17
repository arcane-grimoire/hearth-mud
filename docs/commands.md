# Command Reference

## Player commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `look` | `l` | Look at the current room |
| `look <target>` | | Look at a specific object (fires `can_look`/`on_look`) |
| `go <direction>` | or just type the direction (`n`, `north`, etc.) | Move through an exit |
| `say <message>` | `"<message>` | Say something to the room |
| `emote <action>` | `pose`, `:<action>` | Emote (shows "Name action") |
| `get <item>` | `take` | Pick up an item |
| `drop <item>` | | Drop an item you're carrying |
| `use <target>` | | Use an object (fires `can_use`/`on_use`) |
| `inventory` | `inv`, `i` | List what you're carrying |
| `examine <target>` | `ex` | Examine an object (shows ref, attrs, tags) |
| `who` | | List online players |
| `quit` | `q` | Disconnect |
| `help` | `?` | Show available commands |

## Builder commands

Require the `builder` scope. Use `@` prefix (MUSH convention).

### World building

| Command | Description |
|---------|-------------|
| `@dig <key> = <title>` | Create a new room |
| `@open <direction> = <room_ref>` | Create an exit from the current room |
| `@create <key> = <title>` | Create an item in the current room |
| `@destroy <ref>` | Destroy an object (not players, not occupied rooms) |

### Editing

| Command | Description |
|---------|-------------|
| `@describe [<ref> =] <text>` | Set description. Defaults to current room |
| `@name [<ref> =] <name>` | Rename an object. Defaults to current room |
| `@set <ref>/<attr> = <value>` | Set an attribute. Use `here` for current room |
| `@tag <ref> = <tag_spec>` | Add a tag. Use `here` for current room |
| `@untag <ref> = <tag_spec>` | Remove a tag |
| `@teleport <room_ref>` | Teleport to a room |

### Softcode

| Command | Description |
|---------|-------------|
| `@program <ref>/<hook> = <luau>` | Attach a Luau program to a hook |
| `@programs [<ref>]` | List programs on an object (default: current room) |
| `@rmprogram <ref>/<hook>` | Remove a program |

### Global scripts

| Command | Description |
|---------|-------------|
| `@script <name> = <luau>` | Create/update a global script |
| `@scripts` | List all global scripts |
| `@rmscript <name>` | Remove a global script |
| `@script-interval <name> = <N>` | Set tick interval (in ticks, 1 tick = 1 second) |

### Locks

| Command | Description |
|---------|-------------|
| `@lock <ref>/<type> = <expr>` | Set a lock (types: traverse, get, drop, enter, use, look, teleport) |
| `@unlock <ref>/<type>` | Remove a lock |
| `@locks [<ref>]` | View locks on an object or exit (default: current room) |

See [Softcode Guide](softcode-guide.md) for the full scripting and lock DSL reference.

## Admin commands

Require the `admin` scope.

| Command | Description |
|---------|-------------|
| `@grant <user> <scope>` | Grant a scope (`player`, `builder`, `admin`) |
| `@revoke <user> <scope>` | Revoke a scope (can't revoke your own admin) |
| `@scopes [<user>]` | View scopes for a user (default: yourself) |
| `@wall <message>` | Broadcast a message to all online players |
| `@boot <user>` | Disconnect a player |
| `@save` | Save the world and accounts to SQLite |
| `@shutdown` | Graceful server shutdown (saves, notifies, stops) |

## Object refs

Objects are identified by ref strings following the pattern:

```
area/<area>/<kind>/<key>
```

Examples:
- `area/starter/room/town_square`
- `area/built/item/magic_sword`
- `player/sam`

Use `examine` to see an object's ref. Builder commands that create objects
will print the ref of what they created. Use `here` as a shortcut for the
current room in `@set`, `@tag`, `@lock`.

## Access

- **Telnet:** `telnet localhost 4000`
- **Web:** `http://localhost:8000` (browser client at `/` or `/play`, WebSocket at `/ws`)
