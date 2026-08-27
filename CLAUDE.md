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
cargo test                                   # 282 tests

# Web client (Svelte)
cd web && npm install                        # first time
cd web && npm run dev                        # dev server (port 5173, proxies to backend)
cd web && npm run build                      # production build to web/dist/
```

First account created gets admin/builder/player scopes. For unattended
deploys, set `HEARTH_ADMIN_USER` and `HEARTH_ADMIN_PASSWORD` in the environment:
on a fresh store (no accounts yet) the engine seeds that first admin account at
boot. It's skipped the moment any account exists, so it never touches an
existing deployment.

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
  cli.rs               Command-line client (hearth eval/program/import/export/session-test/migrate)
  migrate.rs           Content migrations — forward-only, tracked rename/remove of file-keys
  session_test.rs      In-process .session E2E runner (drives the real session handler)
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
    telnet.rs          Async telnet with IAC/SGA/ECHO/GMCP negotiation (GMCP → Room.Info + emit_data for Mudlet-style mappers)
    web.rs             Axum HTTP server: WebSocket (JSON protocol), REST API, static file serving
    web_client.html    Fallback HTML client (used when web/dist/ not built)
  softcode/
    mod.rs             Intent enum, IntentBatch, Budget, SoftcodeRuntime, bytecode cache
    api.rs             Luau-facing functions (read, write, predicates, utility, noise, rng, math, scheduling, emit_data, ink)
    hooks.rs           36 known hooks; ObjectScript (one script per object, hooks as functions) + LibModule; derive_hooks (full_moon parser)
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
      Settings.svelte  Settings drawer: theme toggle, font size, API tokens, account, admin actions
      BuilderWorkspace.svelte  Unified builder IDE (route /builder/workspace): explorer
                       tree + tabbed editor. One shared selection (lib/selection.svelte.js)
                       drives everything; objects/hooks/maps all open as tabs.
      builder/         Builder IDE panels — BuilderTree (explorer), ObjectTable,
                       PropertiesPanel, HooksPanel, DialoguePanel, CodeOverlay
      builder/map/     Native Svelte map builder (replaced the Mapwright iframe) —
                       MapBuilder, MapGrid, TerrainPalette, RoomInspector, AttrEditor,
                       Terrain/Schema/ImportExport modals; TOML via lib/mapwright-toml.js
      dialogue/        Ink dialogue authoring — InkEditor, InkCodeEditor, PlaytestPane
      code/            CodeMirror wrapper (CodeEditor), shared by the workspace's
                       CodeOverlay
      room-builder/    Svelte Flow room graph (RoomGraph/RoomNode) + ObjectFinder
                       (⌘K), consumed by the unified workspace
docs/
  adr/                 6 architectural decision records (ADR 0001–0006)
  plans/               Pending/in-progress design docs (archetype traits + live debugger, terrain attr schemas, Mudlet/GMCP terrain-legend + tiles)
  archetypes.md        Archetypes guide (is-a delegation, file-based types, hot-reload)
  commands.md          Command reference
  getting-started.md   Getting started guide
  migrations.md        Content migrations guide (hearth migrate, rename/remove file-keys)
  softcode-guide.md    Softcode programming guide
```

## Key features

- **Everything is an object** — rooms, items, NPCs, players, exits all share `GameObject`
- **36 hooks** — can_get, on_get, can_drop, on_drop, can_put, on_put, can_use, on_use, can_traverse, can_enter, on_enter, on_leave, can_look, on_look, can_say, on_say, can_see, on_move, on_destroy, on_connect, on_disconnect, on_whisper, on_emote, on_receive, on_damage, on_death, on_tick, on_startup, on_shutdown, on_reload, on_save, on_create, on_hour, on_day, on_dawn, on_dusk (the last four are game-clock rollovers, fired on `system:global` objects)
- **Game clock** — optional per-game in-world time (`[clock]` config; absent = off). A monotonic `game_minute` counter advances on the tick heartbeat (`minutes_per_tick`, fractional allowed), persisted in the DB `meta` table. `get_time()` softcode reads `{ total_minutes, minute, hour, day, month, year, is_day, weekday?, month_name? }`; the engine fires `on_hour`/`on_day`/`on_dawn`/`on_dusk` on `system:global` objects on rollover (the hook reads `get_time()`); and the `game_time_between(start, end)` lock predicate uses the game hour (`time_between` stays real UTC). Calendar shape (hours/day, days/month, months/year, dawn/dusk, month/day names, epoch) is all config. See `src/clock.rs`.
- **Object scripts** — one Luau script per object (the Godot/LPMUD model): a single chunk defines the object's hooks as top-level functions sharing one scope (helpers, constants, `require`d modules). The engine derives which hooks a script defines by parsing it with `full_moon` (`derive_hooks`), so a `function on_get(...)` inside a string literal is correctly ignored. `require`able `lib_<name>` modules are a separate concern, stored per-object in `libs` (they `return` a value, so they can't be functions in the shared script scope).
- **Attribute schemas** — an object/archetype declares an `attr_schema` (typed descriptors: type, label, help, default, min/max/enum-values/ref-source; closed `AttrType` set in `src/attr_schema.rs` with a non-fatal `Unknown` fallback). The schema inherits down the archetype chain (`World::resolved_attr_schema`), persists + round-trips through export/import, and `examine` returns it with per-descriptor source, so the web builder renders declared attrs as typed widgets (`AttrField.svelte`: number/checkbox/enum-dropdown/color/`ref`-dropdown via `list_ref_candidates`/`list<T>` rows) instead of raw key/value boxes. Descriptive metadata only — attr values stay free-form.
- **~86 Luau API functions** — read (21, incl. all_objects), predicates (9), write (incl. `set_script`, `set_lib`, emit_data, `set_aliases`, `update_exit`, `set_lock`/`clear_lock`, `clone_object`, `run_command_as`, and `move_object` with `announce`/`fire_hooks` flags), spatial (4), utility (5), noise (5), seeded RNG (4), coordinate math (6), grid (1), `grid_move`/`grid_can_move` (2D-grid wilderness movement), ink (7)
- **2D-grid movement (`grid_move`/`grid_can_move`)** — `grid_move(actor, map, dir)` moves an actor one cell (n/s/e/w) on a map template's grid using the actor's `_x`/`_y` attrs — the wilderness model where the whole grid is ONE room, NO room per cell. Honors terrain/cell passability and fires the terrain's `on_leave`/`on_enter` hooks on that terrain's `archetype` object (a `[terrain.X] archetype = "area/key"`) **as the moving actor**, so terrain behavior ("lava burns") is defined once per terrain, not per square. Returns `{ok,moved,x,y,terrain}`; blocked results carry the attempted `x`/`y` (+`terrain`) with `reason` `"off_grid"`/`"impassable"`; bad requests return `{ok=false, reason="no_map"|"bad_dir"|"no_position"}` (never throws). `grid_can_move(actor, map, dir)` is a pure peek (same passability logic) so an exit list agrees with the step by construction. Pure decision logic (shared `resolve_grid_step`); position + hooks land as `SetAttr`/`Trigger` intents (no new engine surface). The game keeps rendering + encounters. Integration `.test.luau` runs get the game's palette-merged map templates so grid APIs are testable.
- **Luau modules** — `require()` loads shared .luau files from `<game_dir>/lib/`
- **Grid2D userdata** — Rust-backed spatial grid: get/set, A* pathfinding, LOS, FOV, Dijkstra, flood fill
- **Ownership** — `owner_ref` on every object, auto-set on creation, `@chown` admin command, builder permission enforcement
- **Containers** — items can hold other items via `item:container` tag, `put X in Y` / `get X from Y`, capacity limits, nested inventory display, circular containment prevention
- **Lock DSL** — perm(), has_tag(), has_attr(), in_inventory(), is_kind(), is_owner(), time_between(), AND/OR/NOT
- **Timers** — `after(ticks, ref, hook, data?)` with DB persistence, `cancel_after()`, `get_timers()`, data payloads
- **Softcode testing** — `.test.luau` files *or* `test_*` functions co-located in an object's own script (run against itself via `@test #<ref>` / REST `RunTests {ref_id}`, `ctx.this` bound to the object); assert_eq/assert_true/etc., unit mode (lib modules) and integration mode (full API + world); `@test` (no arg) runs every `.test.luau` file (embedded object tests are run per-object, not swept); `cargo test` harness
- **Session testing (E2E)** — `.session` script files drive the REAL telnet session handler in-process (no socket): login/account flow, command dispatch, prompt/dialogue routing, and the renderer — the wire-level paths `.test.luau` can't reach (a green Luau test while a command crashes on spawn, or gets swallowed as dialogue input). A file is alternating `> <input>` lines and `expect:`/`expect-not:` assertions (substring, or `/regex/`) matched against the plain-text a player reads (`markup::to_plain`). Deterministic without sleeps — an `ApiRequest` fence guarantees each input's output is drained before the next (input-driven flows; tick/timer output is a follow-up). Run from the CLI (`hearth session-test <file>... [--config PATH] [--db PATH]`, building an engine in-process — no server needed) or in CI (the `tests/session_test.rs` glob runs `tests/fixtures/*.session` plus any `.session` files under the game's `game_dir`). `src/session_test.rs`.
- **Ticks** — 1s global heartbeat, per-object on_tick with persistent state, global scripts
- **Visibility** — system:hidden tag + can_see hook
- **Global commands** — system:global tag on objects makes their cmd_ hooks available everywhere
- **File loader** — game_dir config, TOML definitions + .luau files, system:managed tag, @reload-world
- **Content migrations** — the loader keys object identity off `_file_key`, so renaming/moving a file-key silently orphans the old object and builds a duplicate. `hearth migrate` fixes identity BEFORE reconciliation: forward-only, tracked (a `migrations` DB table records applied revisions) declarative rename/remove ops in `<game_root>/migrations/<rev>_<slug>.toml`. Within a migration, `remove`s apply before `rename`s (clear a duplicated key, then rename onto it); a rename onto an occupied key is a hard error. Only objects carrying a `_file_key` are touched, so player content is safe. Explicit deploy step (in-process, no server), never a silent boot mutation. `src/migrate.rs`
- **Bytecode cache** — compiled Luau chunks cached by source hash, invalidated on @reload-world
- **Dungeon layout grid** — `generate_dungeon` stores a Grid2D on the entrance room (`dungeon_layout` attr)
- **BBCode markup** — `[b]`, `[red]`, `[cmd=go north]`, `[/]` etc. — transport-neutral styling, converted to ANSI for telnet, HTML for web
- **Clickable commands** — `[cmd=COMMAND]text[/cmd]` renders as clickable text in web client, underlined text in telnet
- **Structured data channel** — `ClientMessage` enum (Text, Prompt, Room, Inventory, Game) sent as JSON to web clients; over telnet, GMCP-enabled clients get the same `Room` payload as a `Room.Info` package and `emit_data` as GMCP (one payload, per-transport rendering)
- **GMCP (telnet option 201)** — negotiated in `net/telnet.rs`; on move the engine's `Room` message (now carrying `num`/`area`/`map`/`environment`/`x`/`y` + exit target dbrefs) is framed as `Room.Info` for Mudlet's built-in mapper. On entering a map-instantiated area the engine also sends a `Terrain.Legend` (via the `Game` channel, once per map per session) — each terrain char → stable `env_id` + color — so the mapper paints rooms by terrain. Text-only clients never enable GMCP, so nothing changes for them. A Mudlet package + install notes live in `clients/mudlet/`. Tile images are the remaining follow-up.
- **Auto room data** — engine sends structured Room messages (exits, contents) on look/movement, powers sidebar panels
- **emit_data()** — softcode can push structured JSON to web clients: `emit_data(player_ref, "channel", data_table)`
- **REST API** — POST /api with ~44 actions (list/examine, create room/object/exit, clone object, set title/desc/attr/tags/aliases, set/clear lock, update exit, force command (admin), script get/set/clear + lib get/set/remove, world_check, ink compile/save/load, maps + terrain, eval, import/export)
- **Web client** — Svelte 5 + Vite, multi-panel layout: scrolling output, structured sidebar, command input with history
- **Unified builder IDE** — one web workspace (`/builder/workspace`) replacing the scatter of tools: a VS Code-style explorer tree (objects → hooks as files, plus Maps and Libraries folders) and a tabbed editor where objects (Properties/Hooks/Dialogue), hooks (CodeMirror), and Table/Map overviews all open as tabs; shared selection, New-object creator, ⌘K find, ⌘B sidebar toggle
- **Builder-authored libraries** — `require()`able lib modules created from the web builder (Libraries folder → New library) or telnet `@lib`, with **no file access**: stored in the DB on a `Kind::Code` host, resolved by `require()` after shipped modules, shipped-name collisions refused, locked-host aware. REST `create_library`/`set_lib`/`remove_lib`; the `std/` file libs stay file-authoritative (locked)
- **Native map builder** — Svelte grid painter, terrain palette, per-tile room inspector, and terrain/schema/import-export modals, sharing map TOML parse/serialize (`lib/mapwright-toml.js`); a real tab in the builder (the old standalone Mapwright `src/net/mapwright.html` + `GET /builder` route were removed)
- **Web dialogue editor** — full Ink authoring surface in the builder IDE (Raw mode + plain-textarea fallback), wired to the `ink_*` REST actions
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
spawn_room = "town/crossroads"  # <area>/<key>
game_dir = "../the-last-stag-mud/world"
game_web_dir = "web/dist"  # optional, relative to game root (parent of game_dir)
load_world_files = true    # boot-time world loading; see below before turning off
locked = ["std"]           # optional: file-key/area prefixes whose managed
                           # objects are file-authoritative and read-only to
                           # in-game authoring (see Tiering + locking below)
# cors_allowed_origins = ["https://play.example.com"]  # optional: restrict
                           # browser CORS to an allow-list in production;
                           # unset = permissive (dev only).

# [clock]                  # optional in-world game clock; omit = no clock
# minutes_per_tick = 1     # 1 tick (1s) → 1 game minute; day = 24 real min
# hours_per_day = 24
# days_per_month = 30
# months_per_year = 12
# dawn_hour = 6            # on_dawn / is_day boundary
# dusk_hour = 20           # on_dusk boundary
# month_names = ["Frostmoon", "Thawmoon"]   # optional
# day_names = ["Sun", "Moon"]               # optional weekday cycle
# start = { year = 1, month = 1, day = 1, hour = 6, minute = 0 }  # epoch
```

**`game_dir` is always required, even with `load_world_files = false`.**
That flag governs *world content* only — `<area>/*.toml` and the program
`.luau` files they reference. Code and narrative assets below still load from
disk on every boot and are never persisted to the database:

| Path | What |
| ---- | ---- |
| `<game_dir>/lib/*.luau` | modules resolved by `require()` |
| `<game_dir>/**/*.ink` | ink narrative scripts |
| `<game_dir>/themes/*.toml` | themes |

Maps and terrain used to load this way too, but are now **DB-owned**: the
`file_sources` table holds `terrain.toml` and each `maps/<name>.toml`, seeded
from those files on `@import` and thereafter edited from the map builder (the DB
is the source of truth once imported).

**Boot-time world-content loading is hash-reconciled, not a blind re-import.**
The world loads from the database first (it is the source of truth); then, when
`load_world_files = true`, each `<area>/*.toml` and the program `.luau` files it
references are compared by **blake3 content hash** against the `file_hashes`
table (see `src/loader.rs` `load_game_dir`, `src/db.rs` `load_file_hashes`):

- **unchanged** files are *skipped* — their DB objects stand, so in-game edits
  (`@program`, the web code editor, `hearth program set`) survive a reboot
  instead of being clobbered by the on-disk copy;
- **changed** files *update* their `system:managed` objects (title, description,
  tags, locks, attrs, programs re-applied from disk; the dbref never changes) or
  *create* new ones with a fresh dbref;
- player-created (non-managed) objects are never touched.

Updated hashes are written back, so the next boot only reconciles what actually
changed (the startup log's `created`/`updated`/`skipped` counts). Setting
`load_world_files = false` skips this reconciliation entirely — the database is
fully authoritative and no `<area>` files are read (the file-key map is still
rebuilt from the DB so path-style refs like `spawn_room` resolve).

A container built without the game directory boots — spawn resolves from the
database, and maps/terrain come from `file_sources` — but libs, ink dialogue,
and themes are all missing, so anything touching them breaks at runtime rather
than at startup. "The database is the source of truth" means world content,
not code and file-loaded assets.

**`game_dir` is image content and read-only at runtime. Anything a user can
edit belongs in the database.** Not merely because a container filesystem is
ephemeral — a deploy can re-copy the game directory over a mounted volume, so
even mounting one does not make it writable in practice. (The Last Stag's Fly
entrypoint does exactly this: `/game/world` → `/data/world` on every deploy.)
The failure is silent in the worst way — writes succeed, the edit looks saved,
and it disappears at the next deploy rather than at the next restart, so it
survives every test that only bounces the process.

This is the general form of the rule Program authoring worked out for Programs
and the map builder applied to maps + terrain; it applies to any future editor —
themes, ink. If a feature lets someone change something from inside the game,
its store is the database.

**Tiering + locking.** A game splits its files into two tiers by convention:
`world/*` areas are **content** — live, DB-authoritative, edited in-game; `std/*`
is **code** — rules + base archetypes, file-authoritative, locked. Locking is the
`system:locked` tag: an OWN tag (never inherited up the archetype chain, so a
locked base does not lock its subtypes/instances) that makes an object's
*definition* read-only to every authoring surface (REST + telnet `@` edits, the
builder) while runtime *state* (softcode intents through `apply_batch`) still
applies — "lock the definition, not the state." The `locked = [prefixes]` config
is the source of truth for which managed objects are locked; `stamp_locked`
reconciles the tag at boot/`@reload-world` (adding and removing as prefixes
change). A reload is vocal: a file change shadowed by an in-game edit is reported,
never silently dropped. The Last Stag's `game/` directory is the reference layout
(`game/world/*` content, `game/std/*` code, `locked = ["std"]`).

## Changelog + releases

**Update `CHANGELOG.md` before pushing to origin, and before cutting a
release.** Anything that lands on origin gets an entry under `## Unreleased`
first — describe the change and why it matters to someone using Hearth, not the
diff. Cutting a release stamps that section as the version being cut
(`## X.Y.Z — YYYY-MM-DD`).

The changelog entry is its **own commit**, never folded into the `Release
vX.Y.Z` commit — a release commit touches `Cargo.toml` + `Cargo.lock` and
nothing else.

`just release` does all of it (next rc; `just release 0.1.0` for anything else)
and refuses to cut one without a stamped entry, as does the `Release` workflow's
`guard` job — so the rule fails closed rather than relying on memory. See the
`cut-version` skill.

## Design decisions

See `docs/adr/` (6 ADRs). See `CONTEXT.md` for domain glossary.

## Testing

282 tests across: softcode (78, incl. derive_hooks), engine (62),
import/export (24), loader (19), db (19), grid (16), accounts (12),
map templates (11), cli (10), ink (9), locks (9), markup (6), dungeon (4),
world (1), game_smoke (1, loads the real Last Stag world).

Softcode tests also discover and run `*.test.luau` files from the game
directory (21 Luau tests across str and collections modules).

The `session_test` integration test drives the real session handler over
`.session` scripts: `tests/fixtures/*.session` always, plus any `.session`
files under the game's `game_dir` when the sibling repo is checked out. The
same runner backs `hearth session-test <file>...` for CLI/local use.

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
