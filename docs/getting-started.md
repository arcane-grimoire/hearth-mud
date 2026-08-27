# Getting Started

## Running the server

```sh
cargo run
```

The server starts on port 4000. Connect with any telnet client:

```sh
telnet localhost 4000
```

Or use a MUD client like [Mudlet](https://www.mudlet.org/).

## First login

The first account you create automatically gets `admin`, `builder`, and
`player` scopes. Type `create` at the login prompt, choose a username and
password, and you're in.

```
Welcome to Hearth.

Enter your username, or type 'create' for a new account: create
Choose a username: admin
Choose a password (6+ characters): ******
Confirm password: ******

Account created! Welcome, admin. [admin, builder, player]
```

## The starter world

This is the **framework** repo — it ships no game content. With no `game_dir`
configured, a fresh `cargo run` drops you into a single empty room described
"An empty room. Build your world from here." To play a real world, point
`game_dir` at one in `hearth.toml`, e.g.:

```toml
game_dir = "../the-last-stag-mud/game"
spawn_room = "world/town/crossroads"   # <area>/<key> of the room to spawn in
```

Then `look` shows the loaded world and you can move with `north`/`n`/etc. The
rest of this guide builds a small world from the empty-room default.

## Building

As a builder, you can create new rooms and connect them. Object refs are
`#N` dbrefs — the create commands print the ref they just made, and you use
that ref in later commands:

```
@dig The Dark Dungeon        # prints e.g. "Room created: The Dark Dungeon (#12)"
@open down = #12             # exit "down" from here to the new room
down                         # move into it
@describe A damp, dark dungeon. Water drips from the ceiling.
look
```

Create items (again, note the `#N` the command prints):

```
@create a flickering torch   # prints e.g. "Item created: ... (#13)"
@describe #13 = A crude torch that casts dancing shadows.
```

Set attributes (`here` is the current room):

```
@set #13/lit = true
@set here/mood = "eerie"
```

## Adding scripts

Attach Luau programs to objects to make them interactive:

```
@program #13/cmd_light = function cmd_light(this, actor, room, args) if get_attr(this, "lit") then emit(actor, "It's already lit.") else set_attr(this, "lit", true) emit(actor, "You light the torch. Shadows dance on the walls.") emit_room(room, actor.display_name .. " lights a torch.", {actor.ref_id}) end end
```

Now any player can type `light torch` and it works.

See `@programs` to list scripts on an object, and `@rmprogram` to remove them.
The full scripting reference is in [Softcode Guide](softcode-guide.md).

## Managing players

Grant builder access to another player:

```
@grant sam builder
```

See who has what:

```
@scopes sam
```

Broadcast to everyone:

```
@wall Server will restart in 5 minutes.
```

## Editing softcode from your own editor

`@program` works over telnet, but for real editing — syntax highlighting,
your usual keybindings — use the `hearth` CLI instead of copy-pasting into a
terminal. It's the same binary, run as a client against a server that's
already up:

```
@token create dev-cli
```

prints a token once (also available from the web client's Settings drawer →
API Tokens). Save it:

```sh
export HEARTH_TOKEN=<token>
```

Then, from a normal editor + shell:

```sh
hearth program get #5/on_look > look.luau        # ref is the #N dbref from `examine`
$EDITOR look.luau
hearth program set #5/on_look look.luau
```

The change is live immediately — no restart, no `@reload-world`. `hearth
eval script.luau` runs a one-shot script the same way `@eval` does, and
`hearth program history`/`restore` reach the same version log
`@program/history`/`@program/restore` do. See
[the command-line client section of the Command Reference](commands.md#command-line-client)
for the full flag list — by default it talks to `localhost:8000`; pass
`--addr` or `--config` for anything else.

## World content: files vs. the database

Everything you build in-game — `@dig`, `@create`, `@program`, `@lib` — lives
in the database, not in files. Files (TOML + `.luau`, the format
`game_dir` uses) are how content gets *installed*, not how it's stored once
installed.

By default (`load_world_files = true` in `hearth.toml`, the current
behaviour and the setting new configs get) the server also loads `game_dir`
at every boot, same as it always has — new content from files is created,
content it already loaded before is updated in place, and anything you
authored in-game is left alone. This is how the starter world and any
`the-last-stag-mud`-style `game_dir` show up automatically.

The explicit, on-demand version of that same file→DB direction is `@import`:

```
@import world
```

installs (or upgrades) a TOML+`.luau` bundle into the database — see
[`@import` / `@export`](commands.md#import--export) in the Command
Reference for the full upgrade semantics (it's safe to run repeatedly; a
second import of an unchanged bundle is a no-op, and a local edit is never
silently overwritten). `@export <path>` is the reverse direction: it writes
the whole database (every object except player characters) back out to the
same format — including things like a `@create`d item or a `@dig`ged room
that were never imported in the first place, which get a stable identity
stamped on the moment they're first exported — so `git diff` and disaster
recovery both work even for content that was only ever authored in-game.

`load_world_files = false` turns off the automatic boot-time load, so
startup reads only the database — `@import` becomes the only way file
content ever reaches the DB. This is a deliberate, one-line switch a
maintainer flips when ready, not something this version of Hearth does on
its own.

## Saving

The world lives in memory. To persist it:

```
@save
```

The world also saves automatically on graceful shutdown. On next startup, it
loads from `hearth.db`.

## Next steps

- Read the [Softcode Guide](softcode-guide.md) for the full Luau scripting API
- Read the [Command Reference](commands.md) for all available commands
- Check `docs/adr/` for architectural decisions and rationale
- Check `CONTEXT.md` for the project's domain glossary
