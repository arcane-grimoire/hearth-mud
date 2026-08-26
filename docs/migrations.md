# Content Migrations

Content migrations rename or remove **file-keys** in the world database as a
deliberate deploy step, so a release that restructures game content doesn't
silently orphan and duplicate the world.

## The problem they solve

The loader keys a managed object's *identity* off its `_file_key` attribute
(the `<area>/<key>` path a file-loaded object carries). On boot it matches each
incoming file against the object holding that same `_file_key` and updates it
in place; anything with no match is treated as brand new.

That means **renaming or moving a file-key silently duplicates content.** If a
release moves `town/crossroads` to `world/town/crossroads`, the loader sees no
object with the new key, builds a fresh one, and leaves the old object orphaned
in the database — with its contents and any players still inside it. A
restructure that renames every key duplicates the entire world, and the failure
is invisible until players notice two of everything.

Migrations declare the rename so the loader recognizes the moved content as the
same objects.

## Model

Modeled on Alembic, shaped for content rather than schema:

- A `migrations` database table records every applied revision, so `hearth
  migrate` runs only the pending ones and is safe to re-run and redeploy.
- Revisions are ordered by their string id (zero-padded ordinals sort
  correctly); pending revisions apply in that order.
- **Forward-only** — content renames don't reverse meaningfully, so there is no
  down-migration.
- **Explicit** — applied by the `hearth migrate` deploy step, never as a silent
  boot mutation. This matches how `@import` is deliberate, and the rule that
  user-editable state lives in the database, not on the image.

## File format

Migration files live in `<game_root>/migrations/<revision>_<slug>.toml`, where
the game root is the parent of `game_dir` (the same convention `game_web_dir`
uses).

```toml
revision = "0001"
description = "world content moved under world/"

# `remove` ops apply BEFORE `rename` ops within one migration, so a rename can
# move onto a key a stale duplicate previously occupied.
[[remove]]
prefix = "world/"          # every managed object whose key starts with world/
# or:  key = "system/rules"   # one exact key

[[rename]]
from_prefix = "town/"      # rewrite the leading segment
to_prefix = "world/town/"
# or:  from = "exact/old"     # rewrite one exact key
#      to   = "exact/new"
```

### Operations

- **`[[rename]]`** — either a prefix rewrite (`from_prefix` → `to_prefix`) or an
  exact-key rewrite (`from` → `to`). Set exactly one pair. Renaming restamps the
  object's `_file_key` **in place**: the dbref, attributes, contents, and
  occupants are untouched, so a player standing in a renamed room stays there.
- **`[[remove]]`** — delete the object with an exact `key`, or every object whose
  key starts with `prefix`. Removal deletes the object; a non-managed object
  (a player, a dropped item) located *inside* a removed object is not deleted
  but is left with a dangling `location_ref`, so check a `--dry-run` before
  removing rooms that might have occupants.

### Ordering rules

- Within one migration, **all `remove`s apply before all `rename`s.** This lets a
  rename reclaim a key a stale duplicate held — clear the duplicate, then rename
  the original onto the freed key. Need finer ordering than that? Split into two
  numbered migration files; revisions apply in order.
- A **rename onto a key another object still holds is a hard error** — the
  migration is refused and nothing is written. Declare a `remove` to clear the
  target first.
- Planning (including the collision check) runs *before* any mutation, so a
  migration either applies fully or leaves the world untouched.

### Safety

Only objects that carry a `_file_key` are ever touched. Player-created content
(built with `@create`/`@dig`, dropped items, characters) has no `_file_key`, so
it is untouchable by migrations by construction.

## Running

```sh
hearth migrate [--config PATH] [--db PATH] [--dry-run]
```

Like `session-test`, `migrate` builds the world in-process — **no running
server, no API token** — so it runs as a deploy step *before* the engine comes
up.

- `--config PATH` — game config to read `game_dir` and `db_path` from (default
  `hearth.toml`).
- `--db PATH` — world database to migrate (default: the config's `db_path`).
- `--dry-run` — report exactly what would change, writing nothing.

**Always dry-run first.** It prints every rename and remove it would perform, so
you can check it against the database before anything is written.

Each revision is its own commit: if revision N fails, the revisions before it
stay applied and recorded, N does not, and a re-run resumes at N.

### Typical deploy flow

1. A release renames or moves file-keys.
2. Add `game/migrations/<next>_<slug>.toml` describing the rename/remove.
3. `hearth migrate --config <cfg> --db <prod.db> --dry-run` — review the plan.
4. `hearth migrate --config <cfg> --db <prod.db>` — apply.
5. Redeploy. The loader now recognizes the moved content as the same objects,
   and the applied revision won't run again.

## Worked example: a `world/` restructure

A game moved all content under a `world/` prefix (`town/crossroads` →
`world/town/crossroads`) and shipped it without a migration, so an earlier
deploy built a duplicate copy alongside the originals. This migration adopts the
originals onto the new keys and drops the duplicates:

```toml
revision = "0001"
description = "world/ restructure: adopt originals onto world/*, drop duplicates"

[[remove]]
prefix = "world/"          # the duplicate subtree the un-migrated deploy created
[[remove]]
key = "system/rules"       # superseded by a later split

[[rename]]
from_prefix = "town/"
to_prefix = "world/town/"
[[rename]]
from_prefix = "forest/"
to_prefix = "world/forest/"
[[rename]]
from_prefix = "dungeon/"
to_prefix = "world/dungeon/"
```

The removes drop the duplicate `world/*` objects, freeing those keys; the
renames then restamp the originals (with their contents and any players inside)
onto the new keys. A player in the old `town/crossroads` stays put as the room
is renamed under them.

## Not yet supported

The imperative escape hatch — a Luau script run against the world for the rare
case declarative rename/remove can't express (e.g. "relocate every player out of
a room before removing it") — is a planned follow-up. It will run through the
existing eval path, so it adds no second scripting surface.
