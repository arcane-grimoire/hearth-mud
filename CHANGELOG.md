# Changelog

Notable changes to Hearth. This file starts partway through the project's
history — earlier changes are in the git log.

The format is loosely [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- **Coordinate exits.** An ordinary exit carrying `_dest_x`/`_dest_y` attributes
  now stamps that arrival cell onto the traversing actor's `_x`/`_y`, so a
  normal room's exit can drop a player onto a specific `(x, y)` of a one-room
  grid/wilderness map — the room→map-cell entry that previously required a
  bespoke softcode command. It's applied in `do_move` after the relocate and
  before `on_enter`, so the destination room's hooks render the cell the player
  actually lands on, and troupe followers land there too. Fully generic (the
  engine only copies the exit's declared coordinates) and authorable wherever an
  exit is — `@open` + `@set <exit>/_dest_x`, area TOML, or the builder — with no
  Luau. A plain exit without those attrs never touches position.
- **Compute-only WASM plugins.** Softcode can now call out to sandboxed
  WebAssembly modules via `wasm_call(module, func, arg?)` — a second extension
  language alongside Luau, for pure data-in → data-out work an author would
  rather write in Rust/AssemblyScript/etc. and ship as a compiled `.wasm`.
  Plugins never touch world state (Luau stays the only thing that emits
  intents), and every call runs under a fuel budget so a runaway plugin traps
  instead of hanging the engine. Modules are code, not editable content: they
  load from `<game_dir>/wasm/*.wasm` on every boot / `@reload-world` and are
  never persisted. A reference plugin — a deterministic, seedable Markov name
  generator — lives in `plugins/names/`. Hosted on `wasmi` (pure-Rust
  interpreter, deterministic by construction). See `src/softcode/wasm.rs`.
- **WASM plugin functions bound as native Luau.** The engine introspects a
  plugin's wasm exports and binds every one matching the plugin ABI as a real
  Luau function under a table named after the module — so a `names.wasm`
  exporting `generate` is callable as `names.generate({ ... })`, no manifest
  required. The wasm is the source of truth for what exists; an optional
  sidecar `<stem>.toml` manifest only annotates (descriptions, a renamed Luau
  name, a different namespace) and can't invent bindings for exports that
  aren't there, so the two can't drift. `wasm_call` remains as the low-level
  escape hatch.
- **WASM instance pooling for hot callers.** A plugin that exports `reset()`
  opts into instance reuse: the host keeps one instance resident and rewinds
  the guest's per-call arena before each call instead of re-instantiating,
  cutting the dominant per-call cost. Plugins without `reset` keep the
  fresh-instance-per-call path, so they can never leak memory across calls; a
  trap evicts a pooled instance so a poisoned one is never reused. The
  `plugins/names` demo ships a reference bump allocator + `reset`.

## 0.1.0-rc.20 — 2026-08-31

### Added

- **Softcode versioning, server-side merge, and edit locks (REST).** Every
  versioned `set_script`/`set_lib` now appends to an append-only history
  (`script_versions`), so softcode edits are no longer last-write-wins with no
  trail. Passing a `base_version` opts into optimistic concurrency: if the script
  moved on the server since you opened it, the engine does a 3-way merge and
  applies it, or returns a `conflict` with all three sides for the client to
  reconcile. New actions `list_script_versions`, `get_script_version`, and
  `revert_script` (rollback re-applies an old version as a new one) expose the
  history; `get_script`/`list_libs`/`list_programs_all` now carry `version`.
  Person-held **edit locks** (`lock_script`/`unlock_script`, 30-minute expiry
  renewed on publish, admin force-unlock) let a builder claim a script so a
  teammate's write is refused rather than silently clobbered — distinct from the
  file-authoritative `system:locked` tier. A new `me` action returns the token's
  account so a client can tell "held by you" from "held by someone else". See
  `docs/plans/softcode-versioning.md`.
- **VS Code extension** (`clients/vscode/`) for editing Hearth softcode against a
  running server over REST: a Programs explorer, open/edit object scripts and lib
  modules with Luau LSP support, explicit Publish with the conflict-reconcile
  flow, version history + diff + revert, and Claim/Release edit-lock commands.
- **`just release` cuts a release; the changelog rule is enforced, not trusted.**
  `just release` takes the next rc (`just release 0.1.0` for anything else) and
  runs the whole procedure: refuse a dirty tree, stamp `## Unreleased` with the
  version and today's date, commit that alone, bump `Cargo.toml`, build, run the
  suite, make a version-only `Release` commit, gate, tag, push. The release
  commit's body *is* the changelog section, so the summary and the changelog
  cannot drift — there's one copy of the prose. `PUSH=0` stops before pushing.
- **A release gate that runs in both places** (`scripts/check-release.sh`, also
  `just release-check vX.Y.Z`): the tag, `Cargo.toml`, and a stamped
  `## X.Y.Z` changelog section must agree, and it warns if the release commit
  touches more than the version files. The `Release` workflow runs it as a
  `guard` job that every publishing job depends on, so a release missing its
  changelog entry ships no binaries, no GitHub release, and no ghcr image —
  the rule now fails closed instead of relying on whoever's cutting it.

### Docs

- **The changelog is now a documented rule, not a habit.** `CLAUDE.md`,
  `AGENTS.md`, and the `cut-version` skill all say it: update `CHANGELOG.md`
  before pushing to origin and before cutting a release, keep the entry in its
  own commit, and leave the `Release vX.Y.Z` commit touching `Cargo.toml` +
  `Cargo.lock` alone. In the skill it's a required numbered step ahead of the
  version bump, not a footnote. Written down because it had already been missed
  twice — rc.8 through rc.16 have no sections at all, and rc.17's entry sat
  under `## Unreleased` until rc.18 was cut.

## 0.1.0-rc.19 — 2026-08-26

### Fixed

- **A managed object now adopts its file's `kind` on reload.** The loader's
  update-existing branch refreshed everything a file owns — title, description,
  location, archetype, attrs, locks, tags, script, `attr_schema` — except
  `kind`, which only `GameObject::new` ever set, in the create branch. So
  editing `kind` in a file and redeploying silently did nothing: the object kept
  the kind it was born with, and converting existing content meant destroying
  and recreating it, or wiping the database. Kind is part of the definition the
  file owns, so it's adopted like the rest. Objects created in-game carry no
  `system:managed` tag and are untouched. One consequence worth knowing:
  `World::objects_in` excludes `Code` and `Exit`, so flipping an object that
  holds contents to `code` stops those contents listing.

### Changed

- **An exit's destination is clickable in the builder.** A room's Exits row
  points at two different objects — the exit and the room it leads to — but the
  whole row opened the exit. The direction chip keeps that; the room name now
  opens the room.

## 0.1.0-rc.18 — 2026-08-26

### Fixed

- **The builder's Help panel no longer drags the editor off-screen.** Opening
  the scripting reference shifted the whole code pane sideways — toolbar, tab
  bar, and panel clipped under the explorer tree. The panel's rows use
  `white-space: nowrap` for their one-line hook docs, so its min-content width
  is the longest doc string (measured 1560px), and the flex default
  `min-width: auto` made that a floor it refused to shrink below: the editor row
  grew to twice its container, and focusing the search box on open made the
  browser scroll the (`overflow: hidden`) workspace pane ~900px sideways to
  reveal it. `min-width: 0` clamps the panel — the nowrap rows ellipsize, as
  they were always styled to — and the dock now clips rather than reflowing its
  neighbour.

- **`game_smoke` had been passing without loading anything.** The test pointed
  at `../the-last-stag-mud/world`, which stopped existing when the game split
  into content and code tiers under `game/`; the missing path took the "sibling
  repo not checked out" branch and returned green, so the one test that loads
  the real game world was a no-op through the whole one-script-per-object
  migration. It now reads `HEARTH_GAME_DIR` or `../the-last-stag-mud/game`, and
  the skip discriminates: an absent sibling repo still skips (for CI), but a
  present repo with a missing game dir fails and says so. Assertions rewritten
  for the current structure — one `system:global` object per command file, the
  multi-file `cmd_combat`, and the `derive_hooks` string-literal guard on
  `onboarding`.

### Docs

- **The global command surface is code, not a hidden item.** The softcode
  guide, the README, and the softcode skill all taught modelling a game-wide
  rules or command object as `kind = "item"` plus `system:hidden` — a fake thing
  on the floor that then has to be hidden. `Kind::Code` is the honest model and
  costs nothing: `World::contents` already filters it out of room contents,
  `look`, inventory, and `get`/`put`, and global dispatch never consults kind
  (the index keys off the `system:global` tag and the hooks a script defines).
  Scoped deliberately to the *global* surface — object-local commands on a real
  physical thing (`cmd_spin` on a roulette wheel, `cmd_talk` on an NPC) belong
  on that object.
- Refreshed the stale `../the-last-stag-mud/world` game-directory paths in
  getting-started and the softcode / softcode-test skills to the current
  `game/` layout (`world/` content, `std/` code).

## 0.1.0-rc.17 — 2026-08-26

### Added

- **Content migrations** (`hearth migrate`). The loader keys a managed object's
  identity off its `_file_key`, so renaming or moving a file-key silently
  orphans the old object and builds a duplicate — a restructure that renames
  every key duplicates the whole world at the next deploy. Migrations fix
  identity in the database *before* the loader reconciles file content against
  it: forward-only, tracked (a `migrations` table records applied revisions, so
  a run is idempotent and safe to redeploy) declarative rename/remove operations
  in `<game_root>/migrations/<revision>_<slug>.toml`. Within one migration,
  `remove`s apply before `rename`s (clear a duplicated key, then rename onto it);
  a rename onto a still-occupied key is a hard error, and planning runs before
  any mutation, so a migration applies fully or leaves the world untouched. Only
  objects carrying a `_file_key` are touched, so player-created content is safe
  by construction. Runs in-process with no server (like `session-test`), as an
  explicit deploy step rather than a silent boot mutation. See
  `docs/migrations.md`.

### Performance

- **Single-writer engine no longer scales spatial queries with world size.**
  The engine processes every message serially in one task, so the cost of one
  command is what every other player waits behind; three of those costs scaled
  with total world size *N* rather than the work at hand, and this removes them
  without touching the single-writer model.

  - **A location (children) index.** `World` now keeps a `location → {refs}` map,
    maintained incrementally through `add_object`/`remove_object` and a single
    `relocate()` funnel (the one correct way to change an in-world object's
    location). Room contents, inventory, container listings, and exit lookups
    (`objects_in`/`exits_from`/`find_exit`) go from full-world scans to
    O(occupants). A `look` in a small room used to touch every object in the
    world; now it touches the room.
  - **A structural epoch decoupled from the save version.** An ordinary move or
    attribute write no longer invalidates the derived indexes (tickables /
    globals-by-hook / troupes). A new `struct_version` bumps only when tags,
    scripts, archetypes, or `tick_interval` change; the derived cache keys on it.
    Previously a single mutation rebuilt the whole index — one `go` forced two
    full rebuilds — because `get_mut` bumped the version unconditionally.
  - **Global command dispatch** (`dispatch_fallback`, `send_commands`) resolves
    through the `globals_by_hook` index instead of scanning every object by
    resolved tags on every custom command.
  - **`format_look`** tallies troupe sizes in a single pass rather than one
    full-world scan per player in the room.

- **Autosave no longer stalls the writer on a per-commit fsync.** The SQLite
  connection now runs `synchronous=NORMAL` under WAL (plus a `busy_timeout`),
  the correct durability point for a checkpoint-only store whose live world is
  in memory: fsync happens at checkpoint rather than on every commit, and a
  save (which runs synchronously in the engine task) no longer blocks it on
  disk.

### Fixed

- The softcode benchmark suite (`benches/softcode.rs`) had drifted out of sync
  with a `run_hook` signature change and no longer compiled; restored.

## 0.1.0-rc.7 — 2026-08-23

### Added

- **Engine-owned softcode API types** — `types/hearth.d.luau` is now the
  canonical, engine-owned LSP type surface, living beside the functions it
  describes and hard-checked against drift (a test builds the registered
  function set from every install site and verifies it against both the
  `.d.luau` and the web editor's completion data). Fills 24 functions the old
  hand-maintained game copy lacked (`all_objects`, `get_contents`,
  `find_by_attr`, `resolve_key`, `set_val`, `transfer_attr`, `apply_template`,
  `json_encode`/`json_decode`, the `ink_*` family, `emit_nearby`/`emit_radius`,
  …), plus the grid surface (`grid_new`, `Grid2D`) and Ink/MapTemplate types.
- **Builder tooltips** — hover tooltips (kit-ui `Tooltip`) on the builder IDE's
  icon-only and keyboard-shortcut buttons.

### Docs

- Refreshed the docs and skills to match the current engine: `#N` dbref refs
  (the old `area/<kind>/<key>` scheme is gone) and title-only `@dig`/`@create`
  syntax; documented boot-time world-content reconciliation (hash-based
  skip-unchanged, DB as source of truth) and `@reload-world`; corrected the
  `prompt(actor, obj, hook)` signature and the softcode skill's API counts.
  Removed design plans that have shipped.

## 0.1.0-rc.6 — 2026-08-23

### Added

- **Unified builder IDE** (web client) — a single builder workspace replacing
  the scatter of loosely-linked tools. One shared selection drives everything:
  a VS Code-style explorer tree (objects grouped by area → their hooks as
  files, plus a Maps folder) on the left, and a tabbed editor in the center
  where objects, hooks, and overviews all open as tabs. An object tab carries
  Properties, Hooks, and Dialogue; a hook tab is the full CodeMirror editor
  (lint, autocomplete, ⌘S, Run). Includes a New-object creator, ⌘K find (across
  objects *and* maps), a hideable sidebar (⌘B), and Table/Map overviews.
- **Native Svelte map builder** — the standalone Mapwright app is ported to
  native Svelte components (grid painter, terrain palette, per-tile room
  inspector, terrain/schema/import-export modals) sharing the map TOML
  parse/serialize logic. It's a real tab in the workspace: kit-ui-themed
  (fonts and colors), no iframe, with per-map deep-linking by name.
- **Dialogue editor** — a full Ink authoring surface in the builder IDE, with a
  Raw mode and a plain-textarea fallback, wired to the `ink_*` API actions that
  previously had no web UI.

### Removed

- **Standalone Mapwright** (`src/net/mapwright.html` and its `GET /builder`
  route) — superseded by the native map builder. Client routes under
  `/builder/*` continue to resolve via the SPA fallback.

## 0.1.0-rc.5 — 2026-08-23

### Added

- **Property-style object access** (issue #19) — hook-facing objects (`this`,
  `actor`, `get_object`/`get_location` results) now support direct field reads
  and writes: `this.hp = 0` is exactly `set_attr(this, "hp", 0)`;
  `this.title = "x"` / `this.description = ...` map to the title/description
  intents; assigning `nil` unsets. Reads are pending-aware, so property syntax
  and `get_attr` always agree within a script. Protected fields (`ref_id`,
  `key`, `kind`, `location_ref`, …) reject writes with errors naming the owning
  API (`move_object` for location). Generalized iteration (`for k, v in this`)
  works over a snapshot view. List results (`all_objects`, `get_contents`, …)
  stay plain snapshot tables — property writes only apply to single-object
  handles.
- **O(1) pending attribute lookups** — `IntentBatch` maintains an index of the
  latest `SetAttr`/`UnsetAttr` per `(target, key)`, replacing the reverse scan.
  This matters because every property read routes through it; it also speeds up
  existing `get_attr`/`has_attr` in write-heavy scripts.
- **Softcode benchmark suite** (#21) — a criterion suite (`benches/softcode.rs`)
  covering VM construction, cold compile, per-hook dispatch overhead,
  budget-interrupt cost, and read/write/mixed hook workloads, plus a library
  target so benches (and external tooling) can link the crate directly.

### Fixed

- clippy `-D warnings` clean across lib + all targets.

## 0.1.0-rc.4 — 2026-08-23

The in-browser builder grows up: a visual room graph, a code editor, a
universal object editor, and a durability layer underneath it all.

### Added

- **Roomwright** — a visual room-graph builder in the web client, backed by a
  scoped slice API, with a table view that toggles against the graph and
  hand-arranged layout positions that persist and restore.

- **Universal object editor.** Full modal editors for rooms (title,
  description, tags, exits, attrs, programs, delete) and for NPCs, items, and
  players, with room-contents editing and exit/alias editing. An object finder
  reaches any object by name or ref, and the table view lists the whole world
  (rooms, npcs, items, players).

- **Code editor** — a CodeMirror workspace at `/builder/code`, with engine
  foundations (`check_program`, `list_programs_all`), hook-name autocomplete,
  inline validation, and a starter example scaffolded the instant a hook is
  picked. The object modal bridges into it for editing hooks.

- **Builder tooling** — a live playtest console, a whole-world "World check"
  problems panel, full-screen mode for all builder panels, and map-builder
  keybindings.

- **Delta saves.** Per-object dirty-tracking so autosave writes only changed
  objects instead of the whole world, with epoch-versioned derived indexes
  that rebuild lazily rather than on every mutation.

- **RBAC security audit** documenting the permission model
  (`docs/audits/2024-rbac-security-audit.md`).

### Changed

- Ink runtime refactored.
- Web client output batches appends, indexes the admin tree, and debounces
  search; the map builder is integrated into the web client.
- Auth re-resolves when the shared session token is stale.

### Fixed

- Hardened the `@export` path from a builder-set `_file_key` (review finding).
- Guard `set_location` and validate exits/aliases (PR #9 review).

## 0.1.0-rc.3 — 2026-08-21

The map builder, and the durability loop that makes it safe to use.

### Added

- **DB-backed map builder.** Map and terrain sources live in a `file_sources`
  table rather than being read from disk on every boot. On-disk `maps/*.toml`
  and `terrain.toml` seed the table if absent; after that the database is
  authoritative and edits are made through the builder, served at `/builder`.
  Writes rebuild the live templates, so a change is visible without a restart.

  Seeding rather than reloading is what makes edits durable in production. A
  deploy can re-copy the game directory over a mounted volume — The Last Stag's
  entrypoint does exactly that — so anything read from `game_dir` on each boot
  is effectively image content, and an edit written there survives restarts and
  then vanishes on deploy. Seed-if-absent is `load_world_files = false`
  semantics applied unconditionally, which is where world content is heading.

- **REST actions** `ListMaps`, `GetMap`, `GetTerrain`, `PutMap`, `PutTerrain`.
  Writes require admin scope, not the generic builder write check, because they
  write to the server's filesystem. Reads require a token and are deliberately
  absent from the unauthenticated read set.

- **`@export` covers maps and terrain**, emitting them in game-directory
  layout alongside world content. That closes the loop — builder, database,
  export, commit — so map work has a path back into git.

- **`@import` covers maps**, resolving each source through the same three-way
  comparison world content uses: the hash recorded at seed or last import,
  the current copy, and the incoming one. Unchanged copies update silently;
  a locally edited one is a conflict.

  Conflicts **keep the local copy and report**, where a Program conflict
  overwrites and preserves the replaced version in the version log. The
  divergence is deliberate: Programs have history to recover from and maps do
  not, so overwriting a map would be data loss. Unifying the two means giving
  maps a version history first, then adopting overwrite-and-preserve — never
  dropping keep-local while maps have no undo.

### Fixed

- **A malformed map or terrain file no longer aborts the entire world load.**
  Every `.toml` under `game_dir` was walked as a world-content area file, so
  `maps/*.toml` and `terrain.toml` were parsed as empty areas — harmless until
  one of them is malformed, at which point the parse error propagates and *no*
  game content loads at all. Boot logs that and continues, so the server comes
  up looking healthy with nothing applied. Map sources are now skipped in the
  area walk. `themes/*.toml` still has the same exposure.

### Security

- **Path traversal is closed at both ends.** Map names accept a bare
  `[A-Za-z0-9_-]{1,64}` only, which makes a traversal path unrepresentable
  rather than blocklisted, and the path is always built as `maps/<name>.toml`.
  `@export` independently refuses to write any source whose stored path is
  absolute or contains a non-normal component — the check belongs at the write
  rather than distributed across everything that can insert a row, especially
  as more writers are planned for that table.

### Documentation

- **`game_dir` is image content and read-only at runtime**; anything a user can
  edit belongs in the database. Not merely because container filesystems are
  ephemeral — a deploy re-copying the directory means even a mounted volume is
  not writable in practice, and the failure is silent in the worst way: writes
  succeed, survive restarts, and disappear on deploy. This is the general form
  of the rule the program-authoring plan works out for Programs.

## 0.1.0-rc.2 — 2026-08-21

Two resource guards and three bug fixes, one of which was introduced by
rc.2's own boot-skip change and caught before release.

### Fixed

- **Editing a program file alone was invisible to change detection.** `.luau`
  files were hashed only for areas whose TOML had already been marked changed,
  so editing a program without touching its area file was never noticed and
  stale code kept running after a restart. `[[scripts]]` program files were
  never hashed at all, since the walk covered rooms and objects only. Hashes
  are now taken for every referenced program file before the skip decision, so
  a changed program de-skips its area — and a skipped area carries its hashes
  forward instead of dropping them, which had been shrinking the persisted set
  on every warm boot. Harmless while boot always reloaded everything; a silent
  correctness bug once boot began honouring the skip, immediately below.

- **Every boot re-read and reinstalled the whole game directory.**
  `load_game_dir` already skips files whose content hash is unchanged, which is
  why `@reload-world` is cheap — but `Engine::new` passed an empty
  previous-hash map, so a restart could never skip anything. Hashes are now
  persisted and restored, so boot skips what has not changed. Hashing moved
  from `DefaultHasher` to blake3 in the process: the former is explicitly not
  stable across Rust versions, so persisting it would have made every file look
  changed after a toolchain bump.

- **`load_world_files = false` broke `spawn_room`.** The `<area>/<key>` → dbref
  map was built only as a by-product of loading the game directory, so with
  boot-time file loading off it stayed empty. `spawn_room` then failed to
  resolve and the engine created a duplicate empty room, landing players in "An
  empty room. Build your world from here." instead of the real spawn. The
  identity already lives on each object in `_file_key` and persists with it, so
  the map is now rebuilt from the loaded world before file loading runs. This
  made the flag unusable, which is the whole point of it. (`resolve_key` and map
  `fixed_room` were unaffected — they resolve by scanning the world rather than
  through the cached map.)

### Security

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

- **Emit limit.** A builder-authored run may emit at most `EMIT_BATCH_LIMIT`
  (50) messages. Scoped to a single batch because that is where a runaway loop
  lives — the instruction budget bounds how long a hook runs but not how much
  it says. The batch is refused whole rather than truncated, so the reason
  stays legible. System authority is exempt, since a server-wide announcement
  legitimately emits once per player. This does not bound a program that emits
  a few every tick forever; that is a content bug rather than a runaway, and it
  is visible in play.

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
