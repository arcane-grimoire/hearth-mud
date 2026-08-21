# Plan: Map builder — DB-backed maps + a live editing surface

## Goal

A visual map builder that edits a running game's maps and terrain palette, with
edits that are durable — they survive restart and redeploy, not just the
current process. This is the same durability the program-authoring work gave
Programs, applied to map templates and the terrain palette.

## What shipped in this branch

- **Map + terrain sources are DB-owned.** A new `map_sources(path, toml)` table
  holds `terrain.toml` and every `maps/<name>.toml` by game-dir-relative path.
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

## The one open question for review — seed-if-absent vs. load_world_files

World content re-installs from files on every boot when `load_world_files` is
true (files win; the DB is truth only when the flag is off). This branch takes a
different stance for maps: **seed-if-absent**, so the DB is authoritative once
seeded regardless of the flag. That was chosen to make "durable in prod by
default" true — The Last Stag's deploy re-copies `world/` from the image every
deploy, so an upsert-on-boot model would clobber builder edits exactly the way
it clobbers hand-edited world files. The cost: hand-editing a `maps/*.toml` in
the repo no longer takes effect once that map is in the DB — you edit through
the builder (or delete the row to reseed). This matches the MUSH/MOO "author
live, export to files" model and the `@import`/`@export` direction, but it *is*
a deliberate divergence from world content's flag semantics, and worth a second
opinion before merge.

## Follow-ons (not in this branch)

- **`@export` should emit maps + terrain**, so the loop is builder → DB →
  `@export` → `.toml` → commit — one durability story, matching world content.
  Until then, edits are durable in the DB and can be committed via the builder's
  Export dialog (copy the TOML into the repo file).
- **Map picker / rename / delete** actions (`delete_map`).
- **Themes and ink** want the same DB-backed treatment; `map_sources` could
  generalize to an authored-source table keyed by path.
- **Token delivery** — the builder currently takes a pasted admin token
  (`@token create`). A session-scoped handoff from a logged-in web client would
  be smoother.

## Files

- `src/db.rs` — `map_sources` table + `seed_map_source` / `save_map_source` /
  `load_map_sources`.
- `src/map_template.rs` — `parse_map_template`, `parse_terrain_palette`,
  `validate_terrain_toml`, `read_map_source_files`, `build_templates_from_sources`.
- `src/engine/mod.rs` — boot seeding + build, `map_sources` field, five actions,
  Admin gate, `rebuild_map_templates`, `valid_map_name`.
- `src/net/web.rs` — `GET /builder`.
- `src/net/mapwright.html` — the builder UI (embedded).
