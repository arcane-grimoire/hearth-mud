# Hearth MUD

Rust MUD framework with Luau softcode. Not a game — a platform that games
are built on. The game (The Last Stag) lives in `../the-last-stag-mud/`.

## Quick reference

- Language: Rust (edition 2024), Luau for softcode, Svelte 5 for web client
- Entry: `src/main.rs`
- Config: `hearth.toml` (or pass path as CLI arg)
- Ports: Telnet 4000, Web/API 8000
- Database: SQLite (checkpoint-only — world lives in memory)
- Softcode: Luau via `mlua` (in-process, single-threaded, bytecode cached)
- Web client: Vite + Svelte 5, lives in `web/`, built to `web/dist/`

## Running

```sh
cargo run                                    # default config (hearth.toml)
cargo run -- ../the-last-stag-mud/hearth.toml  # game-specific config
cargo test                                   # 236 tests

# Web client (Svelte)
cd web && npm install                        # first time
cd web && npm run dev                        # dev server (port 5173, proxies to backend)
cd web && npm run build                      # production build to web/dist/
```

First account created gets admin/builder/player scopes.

## Dev workflow

Hearth has three moving parts during development. Which ones to run
depends on what you're changing:

| Changing           | Run                                                  |
| ------------------ | ---------------------------------------------------- |
| Rust backend       | `cargo run -- ../the-last-stag-mud/hearth.toml`      |
| Web client only    | Backend + `cd web && npm run dev` (hot reload on :5173) |
| Softcode (Luau)    | Backend running, edit .luau files, `hearth program set` (or `@reload-world`) |
| Game content (TOML)| Backend running, edit .toml files, `hearth import` (or `@reload-world`) |

**Web client dev:** The Vite dev server on port 5173 proxies `/ws` and
`/api` to the Rust backend on port 8000. Open `http://localhost:5173`.
Changes to `.svelte` and `.css` files hot-reload instantly. The web
client uses `@kenn-io/kit-ui` (vendored in `web/kit-ui/`) for UI
components and `@lucide/svelte` for icons.

**Backend dev:** `cargo run` serves the built web client from
`web/dist/` (if it exists) or falls back to `src/net/web_client.html`.
For rapid frontend iteration, use the Vite dev server instead.

**WebSocket protocol:** The web client connects to `/ws`. Server sends
JSON messages with `type` field: `text` (BBCode HTML), `room` (structured
room data for sidebar), `auth` (token + scopes), `prompt`, `game`.
Client sends plain text commands.

**REST API:** POST to `/api` with JSON `{ action, ...params }`.
Auth via `Authorization: Bearer <token>` header. See `src/net/web.rs`
and `src/engine/mod.rs` for the full action list.

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
  ansi.rs              BBCode-style markup helpers (room_title, exit_list, etc.)
  cli.rs               Command-line client (hearth eval/program/import/export)
  import_export.rs     Bundle install/emit — @import, @export, three-way upgrade
  markup.rs            BBCode → ANSI (telnet) and BBCode → HTML (web) converters
  config.rs            hearth.toml deserialization
  db.rs                SQLite persistence (save/load world + accounts)
  dungeon.rs           Procedural dungeon generation
  grid.rs              Grid2D userdata — spatial grid with A*, LOS, FOV, Dijkstra
  loader.rs            Game file loader (TOML + .luau from game_dir, lib/ modules)
  locks.rs             Lock DSL parser and evaluator
  map_template.rs      Map template definitions and generation
  noise.rs             Noise functions exposed to Luau (perlin, simplex, etc.)
  theme.rs             Theme system for styled output
  engine/
    mod.rs             Engine loop, session state, all commands, API handler
    commands.rs        Gameplay commands (look, go, get, etc.)
  net/
    mod.rs
    telnet.rs          Async telnet with IAC/SGA/ECHO negotiation
    web.rs             Axum HTTP server: WebSocket (JSON protocol), REST API, static file serving
    web_client.html    Fallback HTML client (used when web/dist/ not built)
  softcode/
    mod.rs             Intent enum, IntentBatch, Budget, SoftcodeRuntime, bytecode cache
    api.rs             Luau-facing functions (read, write, predicates, utility, noise, rng, math, scheduling, emit_data, ink)
    hooks.rs           32 known hooks, ProgramRecord with persistent state
    ink.rs             Ink narrative runtime (bladeink), compile cache, conversation state
  world/
    mod.rs             World struct (objects HashMap, queries)
    object.rs          GameObject, Kind enum (Room/Item/Npc/Player/Exit/Code)
    tag.rs             Tag (category:key)
web/                   Svelte 5 + Vite web client
  package.json         Dependencies: @kenn-io/kit-ui (local), @lucide/svelte, svelte 5, vite 6
  vite.config.js       Dev server config (proxies /ws and /api to backend on :8000)
  index.html           Vite entry HTML
  kit-ui/              Local UI component library (subtree)
  src/
    main.js            Svelte mount, imports kit-ui theme + app styles
    App.svelte         Main layout: TopBar, output pane, sidebar/editor, input bar, settings drawer
    app.css            Theme variables, BBCode markup classes, scrollbar styles
    lib/
      api.js           REST API client (POST /api with Bearer token auth)
    components/
      Output.svelte    Scrolling text output with auto-scroll, clickable [cmd] elements
      InputBar.svelte  Command input with history (up/down, prev/next buttons)
      Sidebar.svelte   Structured panels: Who's Here, What's Here, Exits (clickable chips)
      Editor.svelte    In-browser object editor: title, description, program CRUD via REST API
      Settings.svelte  Settings drawer: theme toggle, font size, API tokens, account, admin actions
docs/
  adr/                 6 architectural decision records (ADR 0001–0006)
  plans/               Pending work (dbref-migration.md)
  commands.md          Command reference
  getting-started.md   Getting started guide
  softcode-guide.md    Softcode programming guide
```

## Key features

- **Everything is an object** — rooms, items, NPCs, players, exits all share `GameObject`
- **32 hooks** — can_get, on_get, can_drop, on_drop, can_put, on_put, can_use, on_use, can_traverse, can_enter, on_enter, on_leave, can_look, on_look, can_say, on_say, can_see, on_move, on_destroy, on_connect, on_disconnect, on_whisper, on_emote, on_receive, on_damage, on_death, on_tick, on_startup, on_shutdown, on_reload, on_save, on_create
- **78 Luau API functions** — read (21, incl. all_objects), predicates (9), write (20 incl. emit_data), spatial (4), utility (5), noise (5), seeded RNG (4), coordinate math (6), grid (1), ink (7)
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
- **BBCode markup** — `[b]`, `[red]`, `[cmd=go north]`, `[/]` etc. — transport-neutral styling, converted to ANSI for telnet, HTML for web
- **Clickable commands** — `[cmd=COMMAND]text[/cmd]` renders as clickable text in web client, underlined text in telnet
- **Structured data channel** — `ClientMessage` enum (Text, Prompt, Room, Inventory, Game) sent as JSON to web clients
- **Auto room data** — engine sends structured Room messages (exits, contents) on look/movement, powers sidebar panels
- **emit_data()** — softcode can push structured JSON to web clients: `emit_data(player_ref, "channel", data_table)`
- **REST API** — POST /api with 17 actions (list, create, examine, set, delete, etc.)
- **Web client** — Svelte 5 + Vite, multi-panel layout: scrolling output, structured sidebar, command input with history
- **Game web override** — `game_web_dir` config lets games provide their own web client, framework falls back to default
- **Ink dialog** — bladeink-powered narrative scripting, compile-on-demand with source hash caching, 7 Luau API functions (`ink_start`, `ink_continue`, `ink_choose`, `ink_get_var`, `ink_set_var`, `ink_end`, `ink_goto`), `@dialogue` builder command with multi-line editor, state persistence via attrs
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
game_web_dir = "web/dist"  # optional, relative to game root (parent of game_dir)
```

## Design decisions

See `docs/adr/` (6 ADRs). See `CONTEXT.md` for domain glossary.

## Testing

236 tests across: softcode (57), engine (54), db (18), import/export (17),
grid (16), loader (14), accounts (12), cli (10), ink (9),
map templates (9), locks (9), markup (6), dungeon (4), world (1).

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
      dialog.luau             — ink dialog wrapper (start, render choices, prompt loop)
      random.luau             — dice, weighted choice, shuffle
      state_machine.luau      — synchronous FSM
      signal.luau             — pub/sub signals (adapted from luau-signal)
      grids.luau              — Grid3D entry point (Grid2D is Rust-side)
      Grid3D.luau             — 3D grid (from luau-grids)
  docs/                       — game design docs from original prototype
```
