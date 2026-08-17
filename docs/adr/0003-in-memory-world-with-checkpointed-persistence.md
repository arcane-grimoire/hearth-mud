# Entire world loads into memory; SQLite is a checkpoint store

All objects, rooms, exits, and attributes load into memory at startup. SQLite is not queried during gameplay — it is a persistence layer only. Checkpoints are triggered by autosave timer, explicit save command, or graceful shutdown. Areas are the unit of save/load.

We considered demand-loading (LP MUD style) but it adds significant complexity: blocking I/O in the world task on area transitions, unloaded NPCs that stop ticking, cross-area references to objects not yet in memory, and queries against the DB during gameplay contradicting the checkpoint-only model. A Rust world with tens of thousands of objects fits comfortably in 100-200MB of RAM.

Demand-loading can be added later behind the area boundary without changing the object model — areas are already the save/load unit.
