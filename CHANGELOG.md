# Changelog

Notable changes to Hearth. This file starts partway through the project's
history — earlier changes are in the git log.

The format is loosely [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

Nothing yet.

## 0.1.0-rc.1 — 2026-08-21

First tagged release. Everything below is the initial cut rather than a
delta against a previous version.

### Added

- **`@eval`** — admin command running a one-shot Luau script against the live
  world, for migrating world data when code changes. Multi-line editor, its own
  instruction budget (`Budget::for_eval`, far larger than a hook's, since
  sweeping every object is the point), and every run logged with actor and
  source.
- **Command-line client.** `hearth eval` and `hearth program get|set|history|
  restore` talk to a running server over the REST API, so softcode can be
  authored in a normal editor and pushed live without a restart. Server address
  from `--addr`/`--config`, token from `--token` or `HEARTH_TOKEN`.
- **`Kind::Code`** — a new object kind for code with no physical presence.
  Global scripts and libraries are both Code objects; they are excluded from
  room contents, `look`, inventory, `get`/`put`, and containers.
- **User libraries.** `@lib`/`@libs`/`@rmlib` create Code objects carrying
  `lib_<name>` Programs, requireable from softcode via `require("<name>")`.
  Editing one invalidates the module cache. Shipped modules (the embedded
  stdlib and `<game_dir>/lib/`) continue to load from disk into the runtime
  registry and are never stored in the database; authoring a user library whose
  name collides with one is refused at write time.
- **Program version history.** Every authoring write records a content-addressed
  version (blake3, deduped, capped at 50 per program). `@program/history`,
  `@program/diff`, and `@program/restore` read them; restore is
  non-destructive, writing the old source back as a new version rather than
  rewinding. Deletions are recorded as tombstones. Softcode's `set_program()` is
  deliberately not versioned — that path is instantiation, not authoring.
- **Multi-line authoring from telnet.** `@program` previously read a single
  line, so multi-line Luau could only be authored through the web editor.
- **`all_objects()`** softcode function, returning every object ref.
- **REST actions** `Eval`, `ProgramHistory`, and `ProgramRestore`. `Eval`
  requires admin scope, not the generic builder write check.
- **`@import` and `@export`.** Bundles of TOML + `.luau` install into the
  database and emit back out again, available over telnet, REST, and the CLI.
  Import is idempotent by object identity, refuses key collisions before
  writing anything, and supports `--dry-run`. Re-importing a changed bundle is
  an upgrade: a three-way comparison of the hash the last import recorded, the
  current source, and the incoming source decides each program. Where both
  changed, the local version is written to the version log before being
  replaced and the import names it in its report, so a conflict is always
  recoverable through `@program/history` and an import never blocks on a
  prompt. A program in the database but absent from the bundle is reported, not
  deleted. Bundle paths resolve on the server's filesystem, matching `@test`
  and `game_dir`.
- **`load_world_files`** config option controlling boot-time file loading,
  defaulting to the current behaviour. Turning it off makes the database the
  sole source of truth at boot, with `@import` as the explicit way in.
- **`@export` now covers in-game-created content.** Previously it only wrote
  back objects that already carried a `"<area>/<key>"` file identity, so
  anything built live with `@create`/`@dig`/`@script`/`@lib` had no way back
  to files — a real gap now that `load_world_files` can turn boot-time
  loading off and the database becomes the only copy of anything authored
  in-game. `@export` now stamps a stable identity (a disambiguated slug of
  the object's title, never its in-game `key`) onto any object that lacks
  one, derives its area from the containing room — walking through nested
  containers and a carrying player — and falls back to a catch-all `unfiled`
  area rather than dropping anything that resolves to no area at all (an
  ad-hoc room, a script/library object). `Kind::Player` is excluded
  explicitly at the export filter rather than incidentally via lacking an
  identity. Nested containment (an item inside another item) round-trips
  correctly. `export_bundle` now takes `&mut World` to support the stamping.
- **Require cycle detection.** A module requiring itself directly or
  transitively now errors with the chain rendered (`a -> b -> a`) instead of
  recursing until the stack or instruction budget blew.
- **Shared terrain palette.** A game defines default terrain "square types"
  once in `<game_dir>/terrain.toml`; every map inherits them, and a map's own
  `[terrain.X]` block overrides that character wholesale. The palette folds
  into each template at load, so maps can drop redundant `[terrain]` blocks
  with no change to instantiation or `get_map_template`.
- **Custom terrain attributes.** Any key on a `[terrain.X]` block beyond the
  known fields is captured and stamped onto every room of that terrain as a
  `terrain_<key>` attribute, so game softcode can reference it at runtime — the
  `terrain_` namespace mirrors the existing `map_*` room stamps.
- **Terrain presentation metadata** — optional `color`, `tile_image`, and
  `tile_rotation` (cardinal) fields on a terrain, carried through
  `get_map_template` for a builder or graphical client. The engine never reads
  them. A plan for delivering room/terrain data to map-aware clients (GMCP,
  Mudlet) lives in `docs/plans/mudlet-client-integration.md`.

### Changed

- **Breaking:** global scripts are now Code objects. Their hook signature moved
  from `on_tick(state)` to `on_tick(this, state, room)`, and the `[[scripts]]`
  `entry` field is gone — the generic per-object tick loop only fires `on_tick`.
  Existing `scripts` rows migrate to Code objects automatically on load.
- **Breaking:** `save_world` no longer writes the legacy `scripts` table, so an
  older binary will not see migrated scripts.
- `@script`, `@rmscript`, `@lib`, and `@rmlib` all record program versions.
  `@script-interval` does not — it writes an attribute, not program source.
- `IntentBatch` now carries the acting reference and the owning authority.
  Nothing enforces the authority yet; it exists so that permission checks can
  later be added inside `apply_to` rather than retrofitted through every call
  site.

### Fixed

- **`on_tick` programs silently discarded their `state` writes.** `run_hook`
  used `Rc::try_unwrap(...).unwrap_or_default()` where the reference count is
  always at least two, so every tick's accumulated state was thrown away
  instead of persisted. Longstanding, and untested until per-object ticking got
  its own coverage.
- **Export could silently clobber one object's Program with another's.**
  `.luau` filenames were derived from the in-game `key`, which `@create` builds
  from a lowercased title with no uniqueness check — unlike imported objects,
  where collisions are refused. Two ad-hoc objects both called "Crate", each
  with a Program, would overwrite each other's file. Filenames now use the
  disambiguated export identity.

### Security

- **Intent authorization.** `apply_to` previously validated only that an
  intent's target existed, so any Program could write to, reprogram, or destroy
  any object. Programs now run with the authority of the object they are
  attached to — its `owner_ref` — rather than with the authority of whoever
  triggered them, which is the property MUSH's permission model rests on.
  A Program may only modify what its authority owns.

  An object with no owner belongs to the system layer and its Programs are
  unrestricted. That is not a default but a load-bearing invariant: `owner_ref`
  defaults to `None` and neither the loader nor `@import` sets it, so every
  file-authored object is unowned. The converse matters as much — a builder
  cannot modify an *unowned* object, so builder code cannot reach into the
  system layer.

  `SetOwner` follows from the same rule: you may give away what you own and
  may not take what you do not.

  `Trigger` is restricted for lifecycle hooks (`on_tick`, `on_create`,
  `on_destroy`, `on_startup`, `on_shutdown`, `on_reload`, `on_save`,
  `on_connect`, `on_disconnect`) but left open for gameplay hooks, which
  ordinary play fires on objects you do not own constantly. Triggering runs the
  target's Program under *its* authority, so this is the confused-deputy seam;
  firing a lifecycle hook out of context is what breaks the invariant it exists
  to maintain.

  `Move` is deliberately left unrestricted. Requiring ownership of what you
  move would mean owning a player in order to teleport them, ruling out
  builder-made teleporters, shops, containers, and knockback. The cost is that
  possession routes around ownership — a builder cannot destroy or reprogram
  someone else's object but can move it into one of their own. That is accepted
  as a social problem rather than a technical one, on the assumption that the
  builder flag goes to people you know.

- **Fork-bomb guard on timers.** At most `OWNER_TIMER_QUOTA` (100) timers may
  be pending against one owner's objects. A hook that schedules two more of
  itself is worse than a runaway loop, because `scheduled_hooks` is persisted
  and so the bomb survives a restart — bouncing the server does not clear it,
  and the instruction budget is no defence when every individual run is well
  inside it and only the count grows. Counted against the target's owner rather
  than whoever scheduled it, so the cap holds however the timer was created;
  unowned targets are the system layer and exempt. A refused timer is dropped
  with a warning rather than failing the batch, since effects are delivered
  after the batch has already been applied.

- **Creation quota.** One owner may hold at most `OWNER_OBJECT_QUOTA` (500)
  objects; `Spawn` and `CreateExit` are refused past it. A ceiling on the total
  rather than on a batch, because the failure this catches is a bug rather than
  an attack — a loop creating a few objects per tick stays under any per-batch
  cap indefinitely. System authority is exempt, since procedural generation
  legitimately creates hundreds of rooms at once.

- `ListPrograms` left the unauthenticated read set. It had been serving full
  Program source to anyone who could reach `/api`.
- `Examine` left the unauthenticated read set. It had been returning full
  attributes, tags, and locks for any object, bypassing the in-game visibility
  rules (`system:hidden`, `can_see`) that hide them from players.
