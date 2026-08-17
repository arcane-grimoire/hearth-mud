# Luau programs mutate the world via typed Intents, not direct access

Luau scripts cannot mutate world state directly. Instead, they enqueue typed Intent enum variants into a batch during execution. After the script finishes, the engine validates the entire batch and applies it atomically — rolling back on any failure.

We considered giving Luau direct mutable access to the world via a `WorldMut` trait/handle, but Rust's ownership model makes this impractical: the Luau VM holds references during script execution, so handing out `&mut World` to callbacks would require interior mutability (`RefCell`/`Mutex`) everywhere, trading compile-time safety for runtime panics. A mutation buffer is effectively forced by the borrow checker — Intents are that buffer, with validation, dry-run, and audit built in.

## Consequences

- Programs get dry-run for free (validate the batch without applying it) — valuable for builders and MCP tooling.
- Atomic rollback means a partially-failed script never leaves the world in an inconsistent state.
- The Intent enum is the exhaustive list of mutations softcode can perform — easy to audit and extend.
