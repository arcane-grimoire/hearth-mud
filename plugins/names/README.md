# `names` — demo WASM plugin

A deterministic, seedable order-2 character Markov name generator, and the
reference example of a **compute-only WASM plugin** for Hearth.

Plugins are a second sandboxed language alongside Luau: an author writes pure
data-in → data-out logic in any language that targets WebAssembly, compiles it
to a `.wasm`, and calls it from softcode. Plugins **never touch world state** —
they take a JSON payload and return a JSON payload — so the single-writer
engine's invariants are untouched. Each call runs under a fuel budget, so a
runaway plugin traps instead of hanging the engine.

## Calling it from Luau

The engine introspects the wasm's exports and binds every one matching the
plugin ABI as a real Luau function, under a table named after the module. No
manifest needed:

```lua
local out = names.generate({ seed = 42, kind = "elf" })
print(out.name)   -- e.g. "Luthiel"  (same seed+kind → same name, always)
```

There's also a low-level escape hatch that needs no binding — `wasm_call(module,
func, arg?)` — which marshals `arg` (any JSON-able Lua value) in and the
plugin's JSON result out:

```lua
local out = wasm_call("names", "generate", { seed = 42, kind = "elf" })
```

### Optional manifest (`names.toml`)

The wasm's exports are the source of truth for *what exists*, so a manifest is
**optional** — it only annotates: human descriptions, a renamed Luau function,
or a different table namespace. A sidecar `<stem>.toml` beside `<stem>.wasm`:

```toml
description = "Fantasy name generator."

[[functions]]
export = "generate"       # the wasm export
lua = "generate"          # Luau name (defaults to export); optional
description = "Roll a name from { seed, kind }."
```

A manifest entry whose `export` isn't actually in the module is ignored, so the
manifest and the binary can't drift.

## Building

```sh
rustup target add wasm32-unknown-unknown      # once
cd plugins/names
cargo build --release --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/release/names.wasm
```

## Deploying to a game

Plugins are **code**, not editable content — like `lib/*.luau` and `*.ink`,
they load from disk on every boot / `@reload-world` and are never persisted to
the database. Drop the built module into the game's `wasm/` directory:

```sh
cp target/wasm32-unknown-unknown/release/names.wasm \
   ../../the-last-stag-mud/world/wasm/names.wasm
```

The engine loads every `<game_dir>/wasm/*.wasm` at startup.

## The guest ABI

Core WebAssembly only — no WASI, no Component Model. A guest module exports:

| Export | Signature | Role |
| ------ | --------- | ---- |
| `memory` | (linear memory) | automatic for a `cdylib` wasm target |
| `alloc` | `(len: u32) -> u32` | reserve `len` bytes, return a pointer |
| `<func>` | `(ptr: u32, len: u32) -> u64` | read input JSON from `[ptr, ptr+len)`, return the result packed as `(out_ptr << 32) \| out_len` |

The host allocates, writes the input, calls the function, and reads the result
back out of linear memory. See `src/softcode/wasm.rs` for the host side.

### Instance pooling (`reset`)

This plugin also exports `reset()` (no params, no results), which opts into
**instance pooling**: the host keeps one instance resident and calls `reset`
before each `generate` instead of re-instantiating — the fast path for hot
callers. It's safe because every call's allocations (input, the Markov model,
the output) come from a per-call bump arena that `reset` rewinds, so memory
never grows across calls. A plugin that omits `reset` simply gets a fresh
instance per call. See the arena allocator in `src/lib.rs`.
