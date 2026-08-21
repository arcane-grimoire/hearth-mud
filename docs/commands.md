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
| `@program <ref>/<hook> =` | With nothing after `=`, opens a multi-line editor (see below) |
| `@programs [<ref>]` | List programs on an object (default: current room) |
| `@rmprogram <ref>/<hook>` | Remove a program |
| `@program/history <ref>/<hook>` | List a program's version history: number, timestamp, author |
| `@program/restore <ref>/<hook> <n>` | Restore version `<n>`'s source as a **new** version |
| `@program/diff <ref>/<hook> <n> [<m>]` | Diff version `<n>` against version `<m>`, or the current live source if `<m>` is omitted |

`@program <ref>/<hook> =` with nothing after the `=` opens the same
multi-line editor `@eval` uses: type source across as many lines as needed,
then a bare `.` on its own line to install it, or `@abort` to cancel without
writing anything. Needed because `@program` otherwise reads a single line, so
without it no multi-line Luau could be authored from telnet at all — only the
web Editor could write a real multi-line program.

Every `@program`/`@rmprogram` write is recorded as a version — see
[Program version history](softcode-guide.md#program-version-history) in the
softcode guide.

### Global scripts

| Command | Description |
|---------|-------------|
| `@script <name> = <luau>` | Create/update a global script |
| `@scripts` | List all global scripts |
| `@rmscript <name>` | Remove a global script |
| `@script-interval <name> = <N>` | Set tick interval (in ticks, 1 tick = 1 second) |

A global script and a library are both backed by a `Kind::Code` object —
code with no physical presence, never shown in room contents, `look`,
inventory, `get`, or a container. `@script` is the `on_tick` half; `@lib`
below is the `require()` half.

Like `@program`/`@lib`, every `@script`/`@rmscript` write is versioned — a
global script is exactly `@program <ref>/on_tick = ...` with friendlier
ergonomics, so a human writing one is authoring the same as any other
Program. Use `@program/history <ref>/on_tick` (the script's object ref, from
`@scripts` or `examine`) to see its history — see
[Program version history](softcode-guide.md#program-version-history).
`@script-interval` only changes the `tick_interval` attr, not the program's
source, so it does not create a version.

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
Like `@program`, every `@lib`/`@rmlib` write is versioned — see
[Program version history](softcode-guide.md#program-version-history).

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
| `@eval <luau>` | Run a one-shot Luau script against the live world (see below) |
| `@import <path> [--dry-run]` | Install a TOML+`.luau` bundle into the database (see below) |
| `@export <path>` | Write DB-owned content back to files in the same format (see below) |

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
`tracing::info!` — the server log is the only audit trail for `@eval` itself.
This is deliberate: `@eval` is a one-shot batch job against the world, not an
authored Program attached to a hook, so it has nothing to keep a version
history *of*. Note that this also covers `set_program()` called *from* an
`@eval` script: that's the same softcode write path a hook uses, so — like
any softcode `set_program()` call — it does **not** get a program version
either. Only `@program`/`@lib` (and the loader's file installs) do. See
[Program version history](softcode-guide.md#program-version-history) for why.

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
database, and writing DB-owned content back out to the same format. See
[Program version history](softcode-guide.md#program-version-history) and
`docs/plans/program-authoring.md` Stage 4 for the full design.

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
- In both, unchanged since the last import — **overwritten** with the
  incoming source (a no-op if it's identical).
- In both, edited locally since the last import *and* changed upstream too —
  a genuine **conflict**: overwritten with the incoming source, but the local
  edit is preserved as a version first and the import reports it loudly —
  see `@program/history <ref>/<hook>` to recover it. Non-destructive by
  construction, so import never has to block on a prompt.
- In both, edited locally but upstream **hasn't** changed since the last
  import — the local edit is **kept**, nothing is overwritten.
- In the DB (under one of the bundle's areas) but no longer in the bundle —
  **reported, never removed**. Auto-deleting on a file mismatch is exactly
  the bug this whole mechanism exists to prevent (see
  `docs/plans/program-authoring.md`'s "Superseded" section).

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
  1 local edit(s) kept as-is (upstream unchanged since the last import):
    = #14/on_reply
  WARNING: 1 local edit(s) were overwritten by this import. Nothing was lost — your edits are preserved in the version log:
    ! #9/cmd_talk — see @program/history #9/cmd_talk
  1 object(s) in the database but missing from this bundle (NOT removed):
    ? town/old_sign
```

`--dry-run` computes and prints the exact same report without writing
anything — to either the world or the version log — so "will this eat my
work?" is always checkable before a real import.

`@export <path>` writes the database back to `<path>` as one
`<area>/<area>.toml` per area plus sibling `.luau` files, one per Program —
the same format `@import` reads and the same format game authors already
hand-write. It covers **every object except `Kind::Player`** — player
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
already up. `hearth import` from the shell is the primary dev loop this
plan is built around — see `docs/plans/program-authoring.md` Stage 4's "The
dev loop: a CLI, not an overwrite mode."

```sh
hearth eval [FILE]                       # run one-shot Luau; stdin if FILE is '-' or omitted
hearth program get <ref>/<hook>          # print current source to stdout
hearth program set <ref>/<hook> [FILE]   # set source; stdin if FILE is '-' or omitted
hearth program history <ref>/<hook>      # list version history
hearth program restore <ref>/<hook> <n>  # restore version <n> as a new version
hearth import <path> [--dry-run]         # install a bundle into the DB (path is on the SERVER)
hearth export <path>                     # write DB-owned content back to files (path is on the SERVER)
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

Every one of these commands needs a token: `program get/set/history/restore`
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
hearth program set area/town/room/crossroads/on_look crossroads_look.luau --addr localhost:8000

# Pull current source down to edit locally.
hearth program get area/town/room/crossroads/on_look > crossroads_look.luau

# One-shot data fixups, same job as @eval:
hearth eval fixup.luau
```

`hearth program get`/`set` go through the same `SetProgram`/`ListPrograms`
REST actions `@program` and the web Editor use, so writes are versioned and
syntax-checked identically — see
[Program version history](softcode-guide.md#program-version-history).

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
