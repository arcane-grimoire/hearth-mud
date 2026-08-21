# Plan: Map builder — DB-backed maps + a live editing surface

## Goal

A visual map builder that edits a running game's maps and terrain palette, with
edits that are durable — they survive restart and redeploy, not just the
current process. This is the same durability the program-authoring work gave
Programs, applied to map templates and the terrain palette.

## What shipped in this branch

- **Map + terrain sources are DB-owned.** A new `file_sources(path, toml)` table
  holds `terrain.toml` and every `maps/<name>.toml` by game-dir-relative path.
  (Named generically on purpose — themes and ink can share it later without a
  `map_`-prefixed table holding non-map content.)
- **Files seed, the DB owns.** At boot, on-disk `maps/*.toml` + `terrain.toml`
  are seeded into the DB with `INSERT OR IGNORE` — a fresh database gets the
  image's content, but files never overwrite what the DB already holds. The
  runtime `map_templates` (grid + palette folded) are then built from the DB
  sources (`build_templates_from_sources`), not the filesystem.
- **Five REST actions** on the existing `POST /api` envelope (not new routes —
  one auth path, one dispatch):
  - `list_maps` → `{ maps, terrain }` — Builder-gated
  - `get_map { name }` → `{ name, toml }` — Builder
  - `get_terrain` → `{ toml }` — Builder
  - `put_map { name, toml }` — Admin; validates name + parses TOML, writes the
    DB, rebuilds live templates
  - `put_terrain { toml }` — Admin; validates + writes + rebuilds
- **Live reload.** A write calls `rebuild_map_templates`, so
  `get_map_template`/`instantiate_map` see the change with no restart.
- **The builder UI** (`src/net/mapwright.html`) is served at `GET /builder`,
  embedded in the binary. It talks to `POST /api` with a Bearer admin token.

## Security decisions (reviewed)

- **Writes are Admin, not Builder.** A `put_*` that installs content the loader
  turns into rooms is closer to `Eval`/`Import` than to a normal builder write,
  so it sits in the same Admin gate. Reads are Builder-gated and deliberately
  **not** in the unauthenticated `is_read` set.
- **Path traversal is closed at the name.** `valid_map_name` accepts a bare
  `[A-Za-z0-9_-]{1,64}` filename only — no separators, `..`, or absolutes — and
  the path is always built as `maps/<name>.toml`, never trusted from input.
- **Bad input can't corrupt state.** `put_map` parses the TOML and `put_terrain`
  validates the palette before touching the DB; a parse error is returned, not
  written.

## seed-if-absent, and the update gap it leaves

World content re-installs from files on every boot when `load_world_files` is
true (files win; the DB is truth only when the flag is off). Maps take the
**seed-if-absent** stance: the DB is authoritative once seeded, regardless of
the flag. This isn't a third model — it's `load_world_files = false` semantics
applied unconditionally, which is where world content is heading now that the
flag can actually be flipped. So maps aren't diverging; they're standing where
world content is about to stand. It's also what makes "durable in prod by
default" true — The Last Stag's deploy re-copies `world/` from the image every
deploy, so an upsert-on-boot model would clobber builder edits the way it
clobbers hand-edited world files.

The gap is narrower than "files are dead." `INSERT OR IGNORE` is keyed on path,
so a genuinely **new** map in a later image still seeds — shipping new content
works. What's blocked is **updating a map already in the DB**: a map fix shipped
in an image won't apply, and there's no supported path from files into the DB
short of deleting the row by hand. World content has an answer for exactly this
(`@import`); maps don't yet. That's why the export/import work below is the
thing that makes seed-if-absent *complete* rather than a nice-to-have — see
next.

## Follow-ons (not in this branch)

- **`@import`/`@export` should cover maps + terrain** — this is what closes the
  update gap above, not just a convenience. `@export` emits the DB sources to
  `.toml` (builder → DB → export → commit); `@import` brings a changed file back
  into the DB, reusing the recorded/current/incoming three-way hash `@import`
  already applies to Programs' conffile problem — a map whose DB copy still
  matches what was seeded updates silently, one edited in the builder is a
  conflict to report rather than clobber. With it, maps get world content's full
  story and `load_world_files` stops needing to mean anything for maps. Until
  then, builder edits are durable in the DB and can be committed via the
  builder's Export dialog (copy the TOML into the repo file).
- **Map picker / rename / delete** actions (`delete_map`).
- **Themes and ink** want the same DB-backed treatment; `file_sources` is
  already the generic authored-source table (keyed by path) they'd share.
- **Token delivery** — the builder currently takes a pasted admin token
  (`@token create`). A session-scoped handoff from a logged-in web client would
  be smoother.

## Files

- `src/db.rs` — `file_sources` table + `seed_file_source` / `save_file_source` /
  `load_file_sources`.
- `src/map_template.rs` — `parse_map_template`, `parse_terrain_palette`,
  `validate_terrain_toml`, `read_map_source_files`, `build_templates_from_sources`.
- `src/engine/mod.rs` — boot seeding + build, `file_sources` field, five actions,
  Admin gate, `rebuild_map_templates`, `valid_map_name`.
- `src/net/web.rs` — `GET /builder`.
- `src/net/mapwright.html` — the builder UI (embedded).
