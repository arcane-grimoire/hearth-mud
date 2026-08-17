# Agent Handoff Guide

## What this project is

Hearth MUD is a Rust MUD framework. The game (The Last Stag) is a
separate repo at `../the-last-stag-mud/`. Game content is pure
Luau + TOML — no Rust changes needed for game features.

## Before you start

1. Read `CLAUDE.md` for full architecture and source layout
2. Read `CONTEXT.md` for domain vocabulary (Object, Kind, Hook, Intent, etc.)
3. Check `docs/adr/` for architectural decisions and rationale
4. Check `docs/plans/` for pending work

## Running

```sh
cargo run -- ../the-last-stag-mud/hearth.toml   # run with game
cargo test                                       # 55 tests, all must pass
```

Delete `*.db` files when schema changes. First account gets admin.

## How softcode works

Programs (Luau scripts) live on objects via named hooks. Scripts cannot
mutate the world directly — they push Intent enum variants into a batch.
The engine applies the batch atomically after the script finishes.

The Luau VM is reused across calls. Compiled bytecode is cached by source
hash. Each call gets a fresh sandboxed environment table.

35 API functions are available to scripts — see `src/softcode/api.rs` or
`the-last-stag-mud/types/hearth.d.luau` for the full typed reference.

## How the file loader works

`game_dir` in config points to a directory of TOML files. Each file defines
rooms, objects, exits, and programs for an area. Programs can reference
external `.luau` files via `{ file = "script.luau" }`.

Objects created from files are tagged `system:managed`. The loader runs
on every startup — new content is created, managed content is updated,
non-managed content (player-created) is never touched. `@reload-world`
re-reads files at runtime.

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

## Pending: dbref migration

See `docs/plans/dbref-migration.md`. This is the next major change:
- Replace all string-path refs with auto-incrementing integer dbrefs (#1, #2)
- File loader assigns dbrefs at load time (two-pass: create objects, then resolve refs)
- Breaking change — requires fresh DB
- Touches every file and every test
- ~300-500 lines changed

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

## Don't

- Don't add features, refactoring, or abstractions beyond what the task requires
- Don't change the intent/batch architecture (ADR 0001)
- Don't make the engine multi-threaded (ADR 0002)
- Don't add a query layer to SQLite during gameplay (ADR 0003)
- Don't add CmdSets or command layers (ADR 0004)
- Don't commit `.db` files
