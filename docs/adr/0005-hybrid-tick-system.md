# Hybrid tick system: global heartbeat with per-script intervals

The engine fires a global tick at a fixed rate (1 second). Scripts declare how many ticks between runs — combat NPCs every 1 tick, weather every 60. Everything aligns to the global beat with no scheduling drift.

When tick scripts exceed the tick window, a hard time cap applies: the engine runs as many scripts as fit within the budget and defers the rest to the next tick. This prevents script load from starving player command processing.

Tick order is deterministic (sorted by stable_ref) but otherwise unspecified — no priority system yet.

Each tick script's intents are validated and applied before the next script runs (sequential). Scripts see the world as modified by all previously-run scripts in the same tick.
