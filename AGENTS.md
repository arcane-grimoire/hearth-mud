# Agent Handoff Guide

## What this project is

Hearth MUD is a Rust MUD framework with a Svelte 5 web client. The game
(The Last Stag) is a separate repo at `../the-last-stag-mud/`. Game
content is pure Luau + TOML — no Rust changes needed for game features.

## Before you start

1. Read `CLAUDE.md` for full architecture, source layout, and dev workflow
2. Read `CONTEXT.md` for domain vocabulary (Object, Kind, Hook, Intent, etc.)
3. Check `docs/adr/` for architectural decisions and rationale
4. Check `docs/plans/` for pending work

## Running

```sh
cargo run -- ../the-last-stag-mud/hearth.toml   # run with game (telnet :4000, web :8000)
cargo test                                       # 121 tests, all must pass
```

Delete `*.db` files when schema changes. First account gets admin.

## Web client

The web client lives in `web/` — Svelte 5 + Vite, uses `@kenn-io/kit-ui`
(vendored in `web/kit-ui/`) for UI components and `@lucide/svelte` for icons.

```sh
cd web && npm install         # first time
cd web && npm run dev         # hot-reload dev server on :5173
cd web && npm run build       # production build to web/dist/
```

**Dev server proxies** `/ws` and `/api` to the Rust backend on `:8000`
(configured in `web/vite.config.js`). Open `http://localhost:5173` for
live development.

**Key files:**

| File                          | Purpose                                            |
| ----------------------------- | -------------------------------------------------- |
| `web/src/main.js`             | Svelte mount, imports kit-ui theme + app styles     |
| `web/src/App.svelte`          | Main layout, WebSocket connection, message dispatch |
| `web/src/app.css`             | Theme vars, BBCode markup classes                   |
| `web/src/lib/api.js`          | REST API client (POST /api with Bearer auth)        |
| `web/src/components/Output.svelte`   | Scrolling text output, auto-scroll, clickable cmds |
| `web/src/components/InputBar.svelte` | Command input with history (up/down arrows)        |
| `web/src/components/Sidebar.svelte`  | Structured panels: Who/What's Here, Exits          |
| `web/src/components/Editor.svelte`   | In-browser object editor (title, desc, programs)   |
| `web/src/components/Settings.svelte` | Settings drawer (theme, font, tokens, admin)       |

**WebSocket protocol:** Server sends JSON `{ type, ... }` — types are
`text`, `room`, `auth`, `prompt`, `game`. Client sends plain text commands.

**REST API:** POST `/api` with JSON `{ action, ...params }`, auth via
`Authorization: Bearer <token>`. Actions defined in `src/engine/mod.rs`.

## How softcode works

Programs (Luau scripts) live on objects via named hooks. Scripts cannot
mutate the world directly — they push Intent enum variants into a batch.
The engine applies the batch atomically after the script finishes.

The Luau VM is reused across calls. Compiled bytecode is cached by source
hash. Each call gets a fresh sandboxed environment table.

API functions are available to scripts — see `src/softcode/api.rs` or
`the-last-stag-mud/types/hearth.d.luau` for the full typed reference.

## How the file loader works

`game_dir` in config points to a directory of TOML files. Each file defines
rooms, objects, exits, and programs for an area. Programs can reference
external `.luau` files via `{ file = "script.luau" }`.

Objects created from files are tagged `system:managed`. The loader runs
on every startup — new content is created, managed content is updated,
non-managed content (player-created) is never touched. `@reload-world`
re-reads files at runtime.

**Tiering + locking.** Games split files into `world/*` (content: live,
DB-authoritative) and `std/*` (code: rules + base archetypes,
file-authoritative). Config `locked = [prefixes]` stamps `system:locked` (an
OWN tag, not inherited) on matching managed objects, making their *definition*
read-only to authoring (REST + telnet `@` edits, the builder) while runtime
state via `apply_batch` still applies. `stamp_locked` reconciles the tag at
boot/reload; a reload reports any file change shadowed by an in-game edit
rather than dropping it silently.

## How global commands work

Objects tagged `system:global` have their `cmd_*` hooks checked during
command dispatch regardless of the player's location. The game's rules
object (`area/system/item/rules`) carries hero, troupe, and combat commands.

## Key patterns

- **Hook-aware commands**: Engine commands like `get`, `drop`, `use`, `look`,
  `say`, movement all check DSL locks first, then fire `can_` hooks (veto),
  then perform the action, then fire `on_` hooks (reaction).
- **Visibility**: `system:hidden` tag hides objects. `can_see` hook on the
  hidden object decides per-viewer visibility.
- **Player persistence**: Players get `system:offline` tag on disconnect.
  Object stays in world. Tag removed on reconnect.
- **Ticks**: 1s heartbeat. Objects with `on_tick` program + `tick_interval`
  attr run on the beat. Global scripts (`World.scripts`) also tick.
- **Attribute schemas**: an object/archetype declares `attr_schema = [ … ]`
  (typed descriptors — `src/attr_schema.rs`, closed `AttrType` with an
  `Unknown` fallback). It inherits down the archetype chain
  (`World::resolved_attr_schema`), round-trips through the DB and export/import,
  and `examine` returns it with per-descriptor source, so the builder renders
  typed form widgets. Descriptive only — attr values stay free-form.

## Common tasks

### Adding a new hook
1. Add name to `KNOWN_HOOKS` in `src/softcode/hooks.rs`
2. Add description in `describe_hook()`
3. Wire `fire_hook()` call in the engine where the action happens
4. Update `docs/softcode-guide.md`

### Adding a new Luau API function
1. Add to `install()` in `src/softcode/api.rs` (read functions reference `world`, write functions push `Intent`s)
2. If it's a new mutation, add an `Intent` variant and handle in `apply_batch()`
3. Update `the-last-stag-mud/types/hearth.d.luau`
4. Add a test in `src/softcode/mod.rs`

### Adding a new REST API action
1. Add variant to `ApiRequest` enum in `src/engine/mod.rs`
2. Handle in `handle_api_request()`
3. Both use serde for JSON serialization

### Adding a web client component
1. Create `.svelte` file in `web/src/components/`
2. Import from `@kenn-io/kit-ui` for UI primitives (Button, TextInput, IconButton, etc.)
3. Import icons from `@lucide/svelte/icons/<name>`
4. Use Svelte 5 runes (`$state`, `$derived`, `$effect`, `$props`)
5. Wire into `App.svelte` or parent component
6. Use `api()` from `web/src/lib/api.js` for REST calls
7. For real-time data, use the WebSocket message types in `App.svelte`

### Adding game content (no Rust changes)
1. Create/edit TOML files in `the-last-stag-mud/world/`
2. Create `.luau` files for programs
3. Reference scripts via `{ file = "script.luau" }` in TOML
4. Run server, type `@reload-world` in-game

## Testing conventions

- Tests are `#[cfg(test)] mod tests` at the bottom of each file
- Engine API tests use `test_engine()` helper that creates an in-memory DB
- Softcode tests use `run_script()` helper for quick script evaluation
- Lock tests use plain unit tests with mock objects
- Always run full suite before committing: `cargo test`
- Web client has no automated tests — verify UI changes by running
  the Vite dev server (`cd web && npm run dev`) and testing in browser

## Changelog + releases

**Update `CHANGELOG.md` before pushing to origin, and before cutting a
release.** Anything that lands on origin gets an entry under `## Unreleased`
first — what changed and why it matters to someone using Hearth, not a restated
diff. Cutting a release stamps that section as the version being cut
(`## X.Y.Z — YYYY-MM-DD`).

Keep the entry in its own commit. A `Release vX.Y.Z` commit touches
`Cargo.toml` + `Cargo.lock` and nothing else, then gets an annotated `vX.Y.Z`
tag — pushing that tag is what builds the published binaries and the
`ghcr.io/arcane-grimoire/hearth-mud` image that games pin. The `cut-version`
skill has the full procedure.

## Don't

- Don't push to origin without a `CHANGELOG.md` entry for what you're pushing
- Don't fold the changelog entry (or anything else) into the Release commit
- Don't add features, refactoring, or abstractions beyond what the task requires
- Don't change the intent/batch architecture (ADR 0001)
- Don't make the engine multi-threaded (ADR 0002)
- Don't add a query layer to SQLite during gameplay (ADR 0003)
- Don't add CmdSets or command layers (ADR 0004)
- Don't commit `.db` files
