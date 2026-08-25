# Command Reference

## Player commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `look` | `l` | Look at the current room |
| `look <target>` | | Look at a specific object (fires `can_look`/`on_look`) |
| `go <direction>` | or just type the direction (`n`, `north`, etc.) | Move through an exit |
| `say <message>` | `"<message>` | Say something to the room |
| `emote <action>` | `pose`, `:<action>` | Emote (shows "Name action") |
| `whisper <player> <message>` | | Send a private message to one player (fires `on_whisper`) |
| `get <item>` | `take` | Pick up an item |
| `drop <item>` | | Drop an item you're carrying |
| `put <item> in <container>` | `place` | Put an item into a container |
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
| `@dig <title>` | Create a new room (its key is derived from the title) |
| `@open <direction> = <room_ref>` | Create an exit from the current room |
| `@create <title>` | Create an item in the current room (key derived from title) |
| `@clone <ref>` | Deep-copy an object into a new one you own (refuses a locked source) |
| `@destroy <ref>` | Destroy an object (not players, not occupied rooms) |

### Editing

| Command | Description |
|---------|-------------|
| `@describe [<ref> =] <text>` | Set description. Defaults to current room |
| `@name [<ref> =] <name>` | Rename an object. Defaults to current room |
| `@alias <ref> = <a> <b> …` | Replace an object's alias keywords. Use `here` for current room |
| `@set <ref>/<attr> = <value>` | Set an attribute. Use `here` for current room |
| `@tag <ref> = <tag_spec>` | Add a tag. Use `here` for current room |
| `@untag <ref> = <tag_spec>` | Remove a tag |
| `@teleport <room_ref>` | Teleport to a room |

### Softcode

| Command | Description |
|---------|-------------|
| `@program <ref> = <luau>` | Set the object's **whole** script (hooks are functions defined in it) |
| `@program <ref>` | With nothing after the ref, opens a multi-line editor seeded with the current script (see below) |
| `@programs [<ref>]` | Show an object's script and the hooks derived from it (default: current room) |
| `@rmprogram <ref>` | Remove the object's script entirely |
| `@reload <ref>` | Re-validate and re-enable the object's script (e.g. after fixing a syntax error that disabled it) |
| `@test #<ref>` | Run the `test_*` functions embedded in that object's own script (`ctx.this` is the object) |
| `@test <file>` | Run one `.test.luau` file (relative to `game_dir`) |
| `@test` | Run every `.test.luau` file plus every object with embedded `test_*` functions |

An object has one script — a single Luau chunk that defines its hooks as
top-level functions (`function on_get(this, actor, room) ... end`,
`function cmd_talk(...) ... end`), all sharing one file-level scope. `@program
<ref> = <source>` sets that whole script, so it's one write, not one hook.
The source is syntax-checked before install; a compile error rejects it and
shows the error. `@programs <ref>` reports the script and the hooks the engine
detected in it.

`@program <ref>` with nothing after the ref opens the same multi-line editor
`@eval` uses, seeded with the object's current script: type source across as
many lines as needed, then a bare `.` on its own line to install it, or
`@abort` to cancel without writing anything. Needed because `@program`
otherwise reads a single line, so without it no multi-line Luau could be
authored from telnet at all — only the web Editor could write a real
multi-line script.

### Global scripts

| Command | Description |
|---------|-------------|
| `@script <name> = <luau>` | Create/update a global script |
| `@scripts` | List all global scripts |
| `@rmscript <name>` | Remove a global script |
| `@script-interval <name> = <N>` | Set tick interval (in ticks, 1 tick = 1 second) |

A global script and a library are both backed by a `Kind::Code` object —
code with no physical presence, never shown in room contents, `look`,
inventory, `get`, or a container. `@script` sets a Code object's script (whose
source defines `function on_tick(this, state, room) ... end`); `@lib` below
sets a `require()`able lib module. `@script-interval` only changes the
`tick_interval` attr, not the script's source.

### Libraries

| Command | Description |
|---------|-------------|
| `@lib <name> = <luau>` | Create/update a library, loadable as `require("<name>")` |
| `@libs` | List all libraries |
| `@rmlib <name>` | Remove a library |

`@lib <name>` is refused if `<name>` collides with a shipped module (from
`<game_dir>/lib/` or the embedded stdlib) — pick a different name. Editing a
library's source takes effect on the *next* `require()` call anywhere, not
retroactively for callers that already required it. See
[Modules (require)](softcode-guide.md#modules-require) in the softcode guide.

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
| `@force <player> = <command>` | Run a command as an online player (charm/puppet). `@`-commands and quit are refused; the command runs under the target's own scopes |
| `@boot <user>` | Disconnect a player |
| `@save` | Save the world and accounts to SQLite |
| `@shutdown` | Graceful server shutdown (saves, notifies, stops) |
| `@eval <luau>` | Run a one-shot Luau script against the live world (see below) |
| `@import <path> [--dry-run]` | Install a TOML+`.luau` bundle into the database (see below) |
| `@export <path>` | Write DB-owned content back to files in the same format (see below) |
| `@reload-world` | Hot-reload the game directory: re-read libs + ink, invalidate the bytecode cache, and re-run the same hash-reconciled world load as boot — changed `<area>`/script files update their `system:managed` objects (dbrefs preserved), unchanged files are skipped, player-created objects untouched (see below) |
| `@reload <ref>` | Re-validate and re-enable an object's script (e.g. after fixing a syntax error that disabled it) |

### `@reload-world`

`@reload-world` applies edits to the game directory *without a restart*. It runs
the identical reconciliation the engine does at boot (`loader::load_game_dir`):
each `<area>/*.toml` and the `.luau` script files it references are hashed
(blake3) against the stored `file_hashes`, and only files whose content changed
are re-applied. Changed managed objects get their
title/description/tags/locks/attrs/script refreshed from disk while keeping
their `#N` dbref; new content is created;
player-created (non-`system:managed`) objects are never touched. It also reloads
`lib/*.luau` modules and `.ink` files and clears the compiled-bytecode cache.
`spawn_room` is re-resolved afterward. Maps/terrain are DB-owned — use `@import`
to bring changed `maps/*.toml` / `terrain.toml` into the database.

### `@eval`

`@eval` runs arbitrary Luau against the live world outside of any hook — the
mechanism for fixing up existing objects when code changes (Evennia calls
this `@batchcode`). It has the full read/write API (including `all_objects()`
for world enumeration), runs under the same instruction budget as any hook,
and every world write still goes through the normal Intent batch — `@eval`
gets no shortcut around it.

```
@eval <luau>
```

runs one line immediately. With no arguments, `@eval` opens the same
multi-line editor `@dialogue edit` uses: type source across as many lines as
needed, then a bare `.` on its own line to run it, or `@abort` to cancel
without running anything. The buffer is stored on your player object, so it
survives a disconnect mid-edit.

The script's top-level `return` value (if any) is echoed back, along with how
many writes it applied. Every `@eval` run is logged (actor ref + source) via
`tracing::info!` — the server log is the audit trail for `@eval`. It has the
full write API, including `set_script()` for attaching behavior to
procedurally generated objects.

```
@eval for _, ref in ipairs(all_objects()) do if has_tag(ref, "loot:weapon") then set_attr(ref, "durability", 100) end end
```

For anything longer than fits comfortably on one line, use the editor
(`@eval` with no arguments) instead.

### `@import` / `@export`

Boot only ever reads the database (see
[Getting Started](getting-started.md#world-content-files-vs-the-database) for
the `load_world_files` config option that controls file loading at boot).
`@import` and `@export` are the explicit, admin-only crossing of that
boundary in each direction — installing a TOML+`.luau` bundle into the
database, and writing DB-owned content back out to the same format.

```
@import <path> [--dry-run]
@export <path>
```

`<path>` is resolved on the **server's own filesystem** — relative to the
server process's working directory, or absolute — the same convention
`@test`'s path argument and `game_dir` itself already use. This is not a
file-upload mechanism; run it against a bundle the server can already read
(most naturally, something under `game_dir` or a checkout next to it).

**Import is idempotent by identity.** Every object gets a `"<area>/<key>"`
identity (the same mechanism the boot loader has always used). Importing the
same bundle twice does not create duplicates — a second import is an
*upgrade*, reconciled per object:

- In the bundle, not in the DB — **created**.
- In both, and the object's script was **not** edited in-game since the last
  import — **overwritten** with the incoming source (a no-op if it's
  identical).
- In both, but the object's script was **edited in-game** — the local edit is
  **kept as-is** and the import declines to overwrite it, reporting it. Because
  a script is a single database-owned unit, an in-game edit always wins over
  the bundle; import is non-destructive by construction, so it never has to
  block on a prompt.
- In the DB (under one of the bundle's areas) but no longer in the bundle —
  **reported, never removed**. Auto-deleting on a file mismatch is exactly
  the bug this whole mechanism exists to prevent.

**Two things refuse the whole import before anything is written:** the same
identity declared twice within one bundle, and the same `cmd_` hook name
defined on two different objects within one bundle. Both are "which one
wins?" situations with no safe silent answer, so both are a hard error
instead of last-write-wins.

```
@import world --dry-run
Import (dry run) of world:
  1 created, 2 updated, 5 unchanged
    + town/well (item)
    ~ town/crossroads (room)
    ~ town/barkeep (npc)
  1 in-game script edit(s) kept as-is (import did not overwrite them):
    = #9
  1 object(s) in the database but missing from this bundle (NOT removed):
    ? town/old_sign
```

`--dry-run` computes and prints the exact same report without writing
anything, so "will this eat my work?" is always checkable before a real
import.

`@export <path>` writes the database back to `<path>` as one
`<area>/<area>.toml` per area plus sibling `.luau` files, one per object
script (and one per lib module) — the same format `@import` reads and the same
format game authors already hand-write. It covers **every object except `Kind::Player`** — player
characters are account-linked runtime state, not world content, and are
never written regardless of anything else. Everything else is in scope,
including objects that were never imported and have no file identity yet:
anything created ad hoc with `@create`, `@dig`, `@script`, `@lib`, and so on.

Because of that, `@export` is not purely read-only: an object with no
`"<area>/<key>"` identity gets one stamped on before it's written, so a
second `@export` (or a re-import of the first one) sees the same identity
rather than minting a new one. The key is a slug of the object's title (its
plain `key` if it has no title), disambiguated with a `-2`, `-3`, ...
suffix on collision; the object's own in-game `key` (what `get`/`drop`/
`examine` match against) is never touched, so exporting something can never
change what a player types to refer to it. The area comes from the
containing room — found by walking up through any number of nested
containers or a carrying player — or, if that resolves to no room at all (a
freshly `@dig`ged room, a `@script`/`@lib` object, which have no location,
or a broken chain), from a catch-all `unfiled` area rather than being
dropped silently. An item nested inside another item exports and re-imports
correctly — its `location` in the TOML names the containing object's key,
not a room.

The one case `@export` can't represent, and reports under "skipped" rather
than writing: an item currently being carried by a player (its true
location is "in someone's inventory," not any area or container the file
format can express).

Export→import is a no-op by construction — this is what makes `@export`
disaster recovery as well as the git story once boot stops reading files.

## Command-line client

`hearth eval`, `hearth program ...`, `hearth import`, and `hearth export` —
the `hearth-mud` binary doubles as a thin CLI client over the REST API, so
softcode (and whole bundles) can be authored in a normal editor and pushed to
an already-running server without a restart or `@reload-world`. This is a
*client* — it does not start a server, and it only works against one that's
already up. `hearth import` from the shell is the primary dev loop: a CLI
push, not a boot-time overwrite mode.

```sh
hearth eval [FILE]                # run one-shot Luau; stdin if FILE is '-' or omitted
hearth program get <ref>          # print the object's whole script to stdout
hearth program set <ref> [FILE]   # set the object's whole script; stdin if FILE is '-' or omitted
hearth import <path> [--dry-run]  # install a bundle into the DB (path is on the SERVER)
hearth export <path>              # write DB-owned content back to files (path is on the SERVER)
```

`hearth import`/`export`'s `<path>` is resolved on the server's filesystem,
not the machine running the CLI — same as `@import`/`@export` above. This
works cleanly for the common case (running the CLI against your own dev
server, where the two share a filesystem); it is not a way to upload a local
bundle to a remote server.

The subcommand comes first — `hearth eval ...` / `hearth program ...` /
`hearth import ...` / `hearth export ...` — so that back-compat with
`cargo run -- <config-path>` is exact: only these four literal first words
are ever treated as a subcommand, anything else (a `.toml` path, nothing at
all) starts the server exactly as before. Connection flags go *after* the
subcommand and may appear in any order:

| Flag | Meaning |
|------|---------|
| `--addr HOST:PORT` | Server address (default: `localhost:8000`) |
| `--config PATH` | Read the address from a `hearth.toml`-style config's `web_addr` (swapping an unspecified bind host like `0.0.0.0` for `localhost`) |
| `--token TOKEN` | API token (default: the `HEARTH_TOKEN` environment variable) |

Every one of these commands needs a token: `program get/set`
need at least builder scope, `eval`/`import`/`export` need admin — same
gates as the telnet commands they wrap. With no token at all, or a rejected
one, the CLI fails fast with an actionable message rather than a bare HTTP
error.

**Minting a token:** `@token create <label>` in-game (telnet, or the web
client's command bar) prints the token once — save it. The web client's
Settings drawer has an "API Tokens" panel with the same create/list flow.
Then either export it:

```sh
export HEARTH_TOKEN=<token>
hearth eval script.luau
```

or pass it per-invocation with `--token`. `HEARTH_TOKEN` is a convention this
CLI introduces — unrelated to `HEARTH_GAME_DIR`, which is test-only.

```sh
# From an editor save hook: push the file straight to the live object.
# The ref is the object's #N dbref (from `examine`); the file is its whole script.
hearth program set #5 crossroads.luau --addr localhost:8000

# Pull the current script down to edit locally.
hearth program get #5 > crossroads.luau

# One-shot data fixups, same job as @eval:
hearth eval fixup.luau
```

`hearth program get`/`set` go through the same `get_script`/`set_script`
REST actions `@program` and the web Editor use, so writes are
syntax-checked identically.

## Object refs

Every object — rooms, items, NPCs, exits, players, file-loaded content — is
identified by an auto-assigned integer **dbref** of the form `#N`:

```
#1
#42
#357
```

Use `examine` to see an object's ref (it prints `Ref: #N`); builder commands
that create objects print the ref of what they created. Use `here` as a
shortcut for the current room in `@set`, `@tag`, `@lock`.

File-loaded content also carries an area/key file identity (stored as a
`_file_key` attr, used for export and reload), and softcode can resolve a
path-style key to a dbref with `resolve_key("area/key")` — but the ref you
pass to commands is always the `#N` dbref.

## Access

- **Telnet:** `telnet localhost 4000`
- **Web:** `http://localhost:8000` (browser client at `/` or `/play`, WebSocket at `/ws`)
