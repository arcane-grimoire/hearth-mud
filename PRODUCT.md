# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Hearth serves two first-class audiences. Both are real products; the **builder
IDE** is the current design focus.

- **Game builders (primary design focus).** Authors who create text worlds on
  the framework — rooms, items, NPCs, exits, hooks, dialogue, maps. Their
  technical range is **mixed and broad**: from veterans of MUSH / MOO / LP /
  Evennia who expect softcode, dbrefs, hooks, and terse power-tools, to
  newcomers meeting the genre for the first time. The IDE must serve both
  without alienating either — power-user density that stays discoverable.
  They work in the browser at `/builder/workspace`.
- **Players.** End users who connect to a game built on Hearth through the web
  client (or telnet). They read output, watch structured room/who/exits panels,
  and type commands.

## Product Purpose

Hearth is a **MUD framework — a platform games are built on, not a game
itself**. It supplies the engine, persistence, softcode runtime, and authoring
tools so an author can build a text-based multiplayer world without writing
Rust. The reference game, The Last Stag, lives in `../the-last-stag-mud/` and is
pure Luau + TOML. Success is an author standing up and evolving a live world
entirely through Luau, TOML, and the web builder.

## Positioning

Mechanisms a neighboring MUD framework could not truthfully copy without
rebuilding to the same choices:

- **Single-writer in-memory engine.** All world state lives in one tokio task
  with no locks; everything else sends `EngineMessage`s. The world lives in
  memory and SQLite is checkpoint-only.
- **Everything is one object.** Rooms, items, NPCs, players, and exits all share
  `GameObject` (`Kind` enum), so tools and API generalize across them.
- **Softcode as typed intent.** Luau (via `mlua`, in-process, bytecode-cached)
  pushes typed `Intent` variants into a batch the engine validates and applies
  atomically after the script finishes.
- **Transport-neutral core.** Telnet, web/WebSocket, and REST all drive the same
  engine; BBCode markup renders to ANSI or HTML per transport.
- **Browser-native, DB-backed authoring.** A unified web builder IDE where
  in-game edits persist to the database and survive reboots.

## Operating Context

- Dev has three moving parts: the Rust backend, the Svelte web client (Vite),
  and softcode/TOML game content — which to run depends on what's being changed.
- Builders work in the browser: a VS Code-style explorer tree plus a tabbed
  editor where objects (Properties/Hooks/Dialogue), hooks (CodeMirror), maps,
  and Table/Map overviews open as tabs. One shared selection drives everything.
- Content is authored as `<area>/*.toml` files plus `.luau` programs; once
  imported, the **database is the source of truth**, reconciled by content hash
  at boot.
- Ports: telnet 4000, web/API 8000. First account created gets
  admin/builder/player scopes.

## Capabilities and Constraints

- Rust (edition 2024) engine; Luau softcode; Svelte 5 + Vite web client.
- 32 hooks, ~78 Luau API functions, a Rust-backed spatial grid (A*/LOS/FOV/
  Dijkstra), containers, a lock DSL, persistent timers, a 1s tick heartbeat, and
  bladeink-powered Ink dialogue.
- A REST API (~40 actions over `POST /api`, Bearer-token auth) powers the web
  builder; a JSON WebSocket protocol drives the player client.
- **Anything a user can edit must persist to the database.** `game_dir` is image
  content and read-only at runtime (a deploy can re-copy it over a mounted
  volume), so any in-game editor — programs, maps, terrain, and future ones —
  stores to the DB, never the filesystem.
- Domain terms future work must use precisely: `GameObject`, `Kind`, `dbref`,
  `hook`, `softcode`, `program`, `intent`, `area/key` ref, `system:managed`.

## Brand Commitments

- The name **Hearth** is committed.
- **Visual identity is NOT yet binding.** The current amber-on-dark theme is a
  working default, not a locked brand — future design work may evolve it.
- Voice: not yet confirmed; leave undecided rather than invent one.

## Evidence on Hand

- **The Last Stag** (`../the-last-stag-mud/`) — real, working content: town /
  forest / dungeon areas with rooms, NPCs, items, and combat commands; shared
  Luau lib modules; Ink dialogue. This is the honest demonstration surface.
- Docs: `docs/adr/` (6 ADRs), `docs/softcode-guide.md`, `docs/commands.md`,
  `docs/getting-started.md`; 276 Rust tests + Luau `.test.luau` suites.
- No marketing site, customers, testimonials, benchmarks, pricing, or press
  exist. Future work must not fabricate any of these.

## Product Principles

1. **Framework, not game.** Never bake a specific game's assumptions into the
   platform; game logic lives in softcode and content, not the engine.
2. **The database owns anything editable.** `game_dir` is read-only image
   content; every in-game editor persists to the DB or the edit silently dies at
   the next deploy.
3. **Serve the whole builder range.** Density and power for veterans,
   discoverability and guardrails for newcomers — in one surface, without
   forcing either into the other's mode.
4. **Transport-neutral.** Core features work across telnet, web, and REST; the
   web is a first-class client, not a bolt-on.
5. **Single-writer integrity.** All mutation flows through the engine as
   validated, atomic intent batches — no side-channel writes to world state.

## Accessibility & Inclusion

The player client ships accessible and visual text modes (`lib/text.luau`). No
formal accessibility standard has been established as a product requirement;
record what exists rather than asserting a target.
