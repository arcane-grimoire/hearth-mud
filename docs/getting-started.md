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

You start in Town Square with exits to the Market (north) and the Rusty
Flagon tavern (east). There's a rusty sword on the ground and a mug of ale
in the tavern.

Try: `look`, `north`, `south`, `east`, `get sword`, `inventory`, `examine sword`.

## Building

As a builder, you can create new rooms and connect them:

```
@dig dungeon = The Dark Dungeon
@open down = area/built/room/dungeon
down
@describe A damp, dark dungeon. Water drips from the ceiling.
look
```

Create items:

```
@create torch = a flickering torch
@describe area/built/item/torch = A crude torch that casts dancing shadows.
```

Set attributes:

```
@set area/built/item/torch/lit = true
@set here/mood = "eerie"
```

## Adding scripts

Attach Luau programs to objects to make them interactive:

```
@program area/built/item/torch/cmd_light = function cmd_light(this, actor, room, args) if get_attr(this, "lit") then emit(actor, "It's already lit.") else set_attr(this, "lit", true) emit(actor, "You light the torch. Shadows dance on the walls.") emit_room(room, actor.display_name .. " lights a torch.", {actor.ref_id}) end end
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
