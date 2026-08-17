# Hearth MUD

Rust MUD framework with Luau softcode. Not a game — a platform that games
are built on.

## Quick reference

- Language: Rust (edition 2024), Luau for softcode
- Entry: `src/main.rs`
- Default port: Telnet 4000
- Database: SQLite (`hearth.db`), checkpoint-only — world lives in memory
- Softcode: Luau via `mlua` (in-process, single-threaded)

## Running

```sh
cargo run                     # start server
cargo test                    # run tests
RUST_LOG=hearth_mud=debug cargo run  # verbose logging
```

Connect: `telnet localhost 4000`

First account created gets admin/builder/player scopes.

## Architecture

Single-writer engine owns all world state in one tokio task. Telnet
connections send `EngineMessage`s to the engine via an unbounded channel.
The engine processes them sequentially — no locks on world state.

Luau scripts run on the engine's thread with instruction-count budgets.
They cannot mutate the world directly — they push typed `Intent` enum
variants into a batch, which the engine validates and applies atomically.

```
telnet ──► net::telnet ──► EngineMessage ──► Engine (owns World, Lua VM)
                                                │
                                                ├── world/ (objects, exits, tags)
                                                ├── accounts (scopes, auth)
                                                ├── softcode/ (Luau VM, intents, API)
                                                └── db (SQLite checkpoint)
```

## Source layout

```
src/
  main.rs              tokio entrypoint, wires engine + telnet
  accounts.rs          Account, Scope (player/builder/admin), AccountStore
  db.rs                SQLite persistence (save/load world + accounts)
  engine/
    mod.rs             Engine loop, session state machine, all commands
    commands.rs        Gameplay commands (look, go, get, etc.)
  net/
    mod.rs
    telnet.rs          Async telnet with IAC negotiation handling
  softcode/
    mod.rs             Intent enum, IntentBatch, Budget, SoftcodeRuntime
    api.rs             Luau-facing read/write API
    hooks.rs           ProgramRecord, hook names, cmd_ dispatch
  world/
    mod.rs             World struct (object store, exit list, queries)
    object.rs          GameObject, Exit, Kind enum
    tag.rs             Tag (category:key)
```

## Key design decisions

See `docs/adr/` for the full rationale. Summary:

- **ADR 0001** — Luau mutates via typed Intents, not direct access
- **ADR 0002** — Single tokio task owns all world state
- **ADR 0003** — Everything in memory, SQLite is a checkpoint store
- **ADR 0004** — Commands extend via cmd_ hooks on objects, not CmdSets
- **ADR 0005** — Global tick with per-script intervals (not yet implemented)
- **ADR 0006** — Lock DSL for permissions (not yet implemented)

## Domain language

See `CONTEXT.md` for the canonical glossary. Key terms:

- **Object** — universal building block (room, item, npc, player)
- **Kind** — string label on an object (room, item, npc, player)
- **Program** — Luau script attached to an object via a named Hook
- **Hook** — named slot: `can_` (permission), `on_` (reaction), `cmd_` (command)
- **Intent** — typed mutation request from softcode
- **Checkpoint** — save to SQLite

## Testing

```sh
cargo test                    # all tests
cargo test softcode           # softcode tests only
```

## Code conventions

- Commands use `@` prefix for builder/admin (MUSH convention)
- Object refs follow `area/<area>/<kind>/<key>` pattern
- Telnet output uses `\r\n` line endings
- Builder commands are scope-gated in the engine, not the command functions
