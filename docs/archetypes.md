# Archetypes

An **archetype** is an object other objects delegate to. Define "what a goblin
is" once; spawn many goblin **instances** that share its behavior and defaults
while keeping their own state and per-field overrides. It's the blueprint/clone
idea from LPMUD and the parent/child prototype model from MOO: *program shared,
object-variables per-clone*.

This solves a problem the one-script-per-object model creates: a script lives on
the object, so without archetypes 50 goblins would mean 50 copies of the goblin
script to maintain. With archetypes there is one goblin, and 50 references to it.

## The model

- **Single-parent delegation (`is-a`).** An object has at most one
  `archetype_ref`. The chain may be deep (`goblin_chief → goblin → monster`) —
  resolution walks up a single parent line, so it's never ambiguous. There is
  **no multiple inheritance**.
- **Copy-on-write per field.** Reading `title`, `description`, or any attribute
  resolves *instance-first, then up the chain* — the instance's own value wins,
  otherwise the archetype's is used.
- **Tags union.** An instance's effective tags are its own **plus** every
  ancestor's (a tag has no single value to override).
- **`state` is never inherited.** Per-object persistent `state` (an NPC's memory,
  an `on_tick` counter) always belongs to the instance, never the archetype.
- **Hooks resolve per hook.** Firing `on_death` on an instance runs the nearest
  script up the chain that defines `on_death`, **bound to the instance** — its
  `state`, attrs, and `ref_id` are the instance's; only the *code* comes from the
  archetype.

## Declaring an archetype in files

An archetype is just an object — no special "archetype" kind or flag. Any object
another object points at becomes one. Declare instances with `archetype =
"area/key"`:

```toml
# world/bestiary/bestiary.toml
area = "bestiary"

# The archetype — what a goblin *is*. No `location`: it's a template, not a
# goblin standing in a room.
[[objects]]
key = "goblin"
kind = "npc"
title = "Goblin"
description = "A wiry, green-skinned goblin clutching a rusty blade."
[objects.attrs]
hp = 5
attack = 2
[objects.script]
source = """
function on_look(this, actor, room)
  emit(actor, this.title .. " snarls. [dim](HP " .. tostring(get_attr(this, "hp")) .. ")[/dim]")
end
"""

# A pure delegate — declares nothing but its archetype, inherits everything.
[[objects]]
key = "grunt"
kind = "npc"
location = "bestiary/pit"
archetype = "bestiary/goblin"

# Overrides title + hp; still inherits attack and the on_look hook.
[[objects]]
key = "scout"
kind = "npc"
location = "bestiary/pit"
archetype = "bestiary/goblin"
title = "Goblin Scout"
[objects.attrs]
hp = 3
```

With that loaded:

| | `title` | `hp` | `attack` | `on_look` |
| --- | --- | --- | --- | --- |
| `goblin` (archetype) | Goblin | 5 | 2 | defines it |
| `grunt` (delegate) | *Goblin* | *5* | *2* | *inherited* |
| `scout` (override) | Goblin Scout | 3 | *2* | *inherited* |

*(italic = inherited from the archetype)*. Looking at `grunt` runs the goblin's
`on_look`, bound to grunt — it prints "Goblin snarls. (HP 5)". Looking at
`scout` runs the same one hook, printing "Goblin Scout snarls. (HP 3)".

`archetype` resolves at load time (both boot and `@import`), and `@export`
round-trips it. A cyclic declaration is broken safely (the delegation is dropped
with a warning) rather than hanging or failing the load.

## Editing an archetype: hot-reload

Because instances *delegate* rather than *copy*, editing an archetype updates
every live instance at once — no re-spawn, nothing to migrate. Edit the goblin's
`hp` to `9` and its `on_look`, then:

```
@reload-world
```

and every goblin instance in the world immediately reflects the change. A
`scout` that overrode `hp` keeps its own value but still picks up the new
`attack` and the new hook. This is LP's `update`-a-blueprint and MOO's
edit-the-parent, together — and it falls out of delegation for free (nothing is
copied into instances to go stale). `state` survives the reload; it was never
delegated.

## Runtime

From softcode:

- `spawn({ archetype = "bestiary/goblin", location = room, key = "g1" })` — a new
  instance delegating to the archetype. Any extra fields become overrides. The
  archetype's `on_create` fires on the new instance (a constructor seam).
- `clone(ref)` — flatten an instance: copy its *resolved* title/description/
  attrs/tags/script down onto it and clear `archetype_ref`, so it stops
  delegating and stands alone. The escape hatch for "I want this one to diverge."

Deleting an archetype that still has live instances is refused unless you pass
`--cascade`, which **flattens** every instance first (so none silently lose
their behavior) before removing the archetype.

## Where archetypes live: the mudlib idea

Archetypes are **file-first** — the shared type vocabulary a game is built on
belongs in version-controlled files, reviewed and diffable, the way LPMUD kept a
curated *mudlib*. Runtime *creation* of brand-new archetype types is deliberately
out for now; you build instances live, but the type layer is authored in files.

The tiers are not "types in the mudlib, instances in the world" — they are:

- **`std/` (mudlib)** — *reusable base* types: `monster`, `weapon`, `container`.
- **`world/` (game content)** — *game-specific* types (`bestiary/goblin`) **and**
  all instances.

Types live in *both* tiers. A game's bestiary is game-specific types that stand
on mudlib bases, and instances stand on those. Chains are single-parent but may
be deep, so all three layers compose:

```toml
# world/std/monster.toml — the mudlib base (reusable across games)
[[objects]]
key = "monster"
kind = "npc"
[objects.attrs]
hp = 1
armor = 0
xp = 10
[objects.script]
source = "function on_death(this, a, r) emit_room(r, this.title .. ' collapses.') end"
```

```toml
# world/bestiary/bestiary.toml — the game's goblin TYPE, on the mudlib base
[[objects]]
key = "goblin"
kind = "npc"
archetype = "std/monster"   # inherits armor, xp, on_death
title = "Goblin"
[objects.attrs]
hp = 9
attack = 4
[objects.script]
source = "function on_look(this, a, r) ... end"   # its own hook
```

An instance (`grunt`, `archetype = "bestiary/goblin"`) then resolves the whole
chain: `hp`/`attack` and `on_look` from `goblin`, and `armor`/`xp`/`on_death`
from `std/monster` two hops up — `grunt → Goblin → creature`. Per hook and per
field, the nearest definer up the single parent line wins.

See `docs/plans/archetypes.md` for the design rationale — LP vs MOO, the
Smalltalk-image framing, and Stage 2: traits, `pass()`, `clear_attr`, and the
builder authoring surface.

## Known limitations

Both are narrow and don't affect a fresh `archetype =` declaration:

- **Converting an existing managed object to delegate doesn't clear its old
  own-fields.** If you edit a TOML object to add `archetype =` and *drop* fields
  it used to define (a title, some attrs), the stale own-values remain and shadow
  the inherited defaults. Remove them explicitly, or start the instance fresh.
- **A room instance can't inherit its title through `@export`.** `RoomDef.title`
  is required, so an archetyped room with no own title round-trips as an empty
  title. (Rooms rarely delegate; items/NPCs are the common case.)
