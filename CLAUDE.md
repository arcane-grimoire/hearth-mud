# Hearth MUD

Rust MUD framework with Luau softcode. Not a game — a platform that games
are built on. The game (The Last Stag) lives in `../the-last-stag-mud/`.

## Quick reference

- Language: Rust (edition 2024), Luau for softcode
- Entry: `src/main.rs`
- Config: `hearth.toml` (or pass path as CLI arg)
- Ports: Telnet 4000, Web/API 8000
- Database: SQLite (checkpoint-only — world lives in memory)
- Softcode: Luau via `mlua` (in-process, single-threaded, bytecode cached)

## Running

```sh
cargo run                                    # default config (hearth.toml)
cargo run -- ../the-last-stag-mud/hearth.toml  # game-specific config
cargo test                                   # 55 tests
```

First account created gets admin/builder/player scopes.

## Architecture

Single-writer engine owns all world state in one tokio task. Everything
else sends `EngineMessage`s via channels. No locks on world state.

```
telnet ──► net::telnet ──► EngineMessage ──► Engine
web/ws ──► net::web    ──►                    │
REST   ──► POST /api   ──►                    ├── World (objects, scripts)
                                              ├── Accounts (scopes, auth)
                                              ├── SoftcodeRuntime (Lua VM)
                                              └── Database (SQLite)
```

Luau scripts push typed `Intent` enum variants into a batch. The engine
validates and applies the batch atomically after the script finishes.

## Source layout

```
src/
  main.rs              tokio entrypoint, wires engine + telnet + web
  accounts.rs          Account, Scope (player/builder/admin), AccountStore
  ansi.rs              ANSI color helpers
  config.rs            hearth.toml deserialization
  db.rs                SQLite persistence (save/load world + accounts)
  grid.rs              Grid2D userdata — spatial grid with A*, LOS, FOV, Dijkstra
  loader.rs            Game file loader (TOML + .luau from game_dir, lib/ modules)
  locks.rs             Lock DSL parser and evaluator
  engine/
    mod.rs             Engine loop, session state, all commands, API handler
    commands.rs        Gameplay commands (look, go, get, etc.)
  net/
    mod.rs
    telnet.rs          Async telnet with IAC/SGA/ECHO negotiation
    web.rs             Axum HTTP server: web client, WebSocket, REST API
    web_client.html    Browser-based MUD client
  softcode/
    mod.rs             Intent enum, IntentBatch, Budget, SoftcodeRuntime, bytecode cache
    api.rs             54 Luau-facing functions (read, write, predicates, utility, noise, rng, math, scheduling)
    hooks.rs           30 known hooks, ProgramRecord with persistent state
  world/
    mod.rs             World struct (objects HashMap, scripts, queries)
    object.rs          GameObject, Kind enum (Room/Item/Npc/Player/Exit)
    script.rs          Script (global tick scripts)
    tag.rs             Tag (category:key)
```

## Key features

- **Everything is an object** — rooms, items, NPCs, players, exits all share `GameObject`
- **32 hooks** — can_get, on_get, can_drop, on_drop, can_put, on_put, can_use, on_use, can_traverse, can_enter, on_enter, on_leave, can_look, on_look, can_say, on_say, can_see, on_move, on_destroy, on_connect, on_disconnect, on_whisper, on_emote, on_receive, on_damage, on_death, on_tick, on_startup, on_shutdown, on_reload, on_save, on_create
- **63 Luau API functions** — read (18), predicates (9), write (18), utility (3), noise (5), seeded RNG (4), coordinate math (6), grid (1)
- **Luau modules** — `require()` loads shared .luau files from `<game_dir>/lib/`
- **Grid2D userdata** — Rust-backed spatial grid: get/set, A* pathfinding, LOS, FOV, Dijkstra, flood fill
- **Ownership** — `owner_ref` on every object, auto-set on creation, `@chown` admin command, builder permission enforcement
- **Containers** — items can hold other items via `item:container` tag, `put X in Y` / `get X from Y`, capacity limits, nested inventory display, circular containment prevention
- **Lock DSL** — perm(), has_tag(), has_attr(), in_inventory(), is_kind(), is_owner(), time_between(), AND/OR/NOT
- **Timers** — `after(ticks, ref, hook, data?)` with DB persistence, `cancel_after()`, `get_timers()`, data payloads
- **Softcode testing** — `.test.luau` files with assert_eq/assert_true/etc., unit mode (lib modules) and integration mode (full API + world), `@test` command, `cargo test` harness
- **Ticks** — 1s global heartbeat, per-object on_tick with persistent state, global scripts
- **Visibility** — system:hidden tag + can_see hook
- **Global commands** — system:global tag on objects makes their cmd_ hooks available everywhere
- **File loader** — game_dir config, TOML definitions + .luau files, system:managed tag, @reload-world
- **Bytecode cache** — compiled Luau chunks cached by source hash, invalidated on @reload-world
- **Dungeon layout grid** — `generate_dungeon` stores a Grid2D on the entrance room (`dungeon_layout` attr)
- **REST API** — POST /api with 17 actions (list, create, examine, set, delete, etc.)
- **Web client** — browser-based at /play with WebSocket
- **Player persistence** — players marked offline on disconnect, restored on reconnect
- **Autosave** — configurable interval (default 5 min)
- **Accounts** — argon2 password hashing, scoped roles, optional email

## Config (hearth.toml)

```toml
telnet_addr = "0.0.0.0:4000"
web_addr = "0.0.0.0:8000"
db_path = "hearth.db"
autosave_secs = 300
tick_secs = 1
spawn_room = "area/town/room/crossroads"
game_dir = "../the-last-stag-mud/world"
```

## Design decisions

See `docs/adr/` (6 ADRs). See `CONTEXT.md` for domain glossary.

## Testing

115 tests across: accounts (12), db round-trips (7), engine API (8),
locks DSL (9), softcode (37), grid (14), loader (6), dungeon (4),
theme (1), lock validator (7), map templates (9), game softcode (1 harness).

Softcode tests also discover and run `*.test.luau` files from the game
directory (21 Luau tests across str and collections modules).

```sh
cargo test                    # all
cargo test softcode           # softcode only
cargo test grid               # grid + pathfinding
cargo test engine             # API tests
cargo test locks              # lock DSL
```

## The Last Stag (game)

Lives in `../the-last-stag-mud/`. Pure Luau + TOML, no Rust.

```
the-last-stag-mud/
  hearth.toml                 — game config
  types/hearth.d.luau         — Luau LSP type definitions
  world/
    town/town.toml            — 5 rooms, 3 NPCs
    forest/forest.toml        — 3 rooms
    dungeon/dungeon.toml      — 2 rooms, 1 item
    system/
      system.toml             — global rules object
      cmd_hero.luau            — hero create/list (4 classes)
      cmd_troupe.luau          — troupe add/remove/list (up to 6)
      cmd_fight.luau           — start combat (goblin/orc/skeleton)
      cmd_attack.luau          — troupe member attacks
      cmd_endturn.luau         — monster phase resolution
      cmd_status.luau          — combat status display
    lib/                      — shared Luau modules (loaded via require())
      text.luau               — rich formatting with accessible/visual modes
      str.luau                — string utilities (split, trim, wrap, etc.)
      collections.luau        — Set, Array helpers
      random.luau             — dice, weighted choice, shuffle
      state_machine.luau      — synchronous FSM
      signal.luau             — pub/sub signals (adapted from luau-signal)
      grids.luau              — Grid3D entry point (Grid2D is Rust-side)
      Grid3D.luau             — 3D grid (from luau-grids)
  docs/                       — game design docs from original prototype
```
