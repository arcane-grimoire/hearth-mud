# Changelog

Notable changes to Hearth. This file starts partway through the project's
history — earlier changes are in the git log.

The format is loosely [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Hearth has no tagged releases yet, so everything so far is unreleased.

## Unreleased

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

- `ListPrograms` left the unauthenticated read set. It had been serving full
  Program source to anyone who could reach `/api`.
- `Examine` left the unauthenticated read set. It had been returning full
  attributes, tags, and locks for any object, bypassing the in-game visibility
  rules (`system:hidden`, `can_see`) that hide them from players.
