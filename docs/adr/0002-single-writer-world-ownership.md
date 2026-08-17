# Single task owns all world state

All world state lives in a single tokio task (the "engine"). Telnet connections, web sockets, the tick scheduler, and MCP endpoints send typed messages to the engine — they never hold a reference to the world. The engine processes messages sequentially.

The previous Python implementation enforced this by convention ("portal never mutates world"). Rust enforces it at compile time because `World` is owned by one task and is not `Send + Sync` shared.

Luau scripts run on the engine's thread, gated by instruction-count Budgets. This avoids the complexity of a script thread pool and channel-based intent delivery. If tick load grows (many NPCs with scripts), the architecture supports moving Luau to a pool without changing the Intent API.
