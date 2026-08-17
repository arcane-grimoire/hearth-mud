# Plan: Migrate to dbrefs

## Goal

Replace all string-path refs (`area/town/room/crossroads`) with auto-incrementing integer dbrefs (`#1`, `#2`, `#3`). Everything is a dbref — rooms, exits, items, NPCs, players. File-loaded content gets dbrefs assigned at load time.

## Current state

- Object refs are strings like `area/town/room/crossroads`
- `GameObject.ref_id` is a `String`
- Exits, location_ref, target_ref all use string refs
- File loader creates refs from area/kind/key patterns
- Builder commands (`@dig`, `@create`) generate slug-based refs
- Tests and softcode reference string refs

## Target state

- Object refs are `#N` where N is an auto-incrementing integer
- `World` owns a `next_id: u64` counter, persisted in DB
- `GameObject.ref_id` is still a `String` (formatted as `#N`) — avoids changing every API signature
- File-loaded content gets dbrefs at load time; the TOML key is stored as `GameObject.key` for display/search, not as the ref
- Builder commands auto-assign dbrefs
- Player objects get dbrefs too (`#42` not `player/sam`)
- `spawn()` in Luau returns `#N`

## File format changes

Files use human-readable keys for cross-references within the same file. The loader resolves them to dbrefs.

```toml
area = "town"

[[rooms]]
key = "crossroads"           # human-readable, used for intra-file refs
title = "The Crossroads"
description = "..."

[[rooms]]
key = "square"
title = "Town Square"

[[exits]]
from = "crossroads"          # resolved to dbref of the room with key "crossroads"
direction = "north"
to = "square"                # resolved to dbref of the room with key "square"

[[exits]]
from = "crossroads"
direction = "south"
to = "forest/edge"           # cross-area: "area_name/key" format
```

The loader does two passes:
1. Create all objects, assign dbrefs, build a `key → dbref` mapping
2. Resolve references (exits, locations) using the mapping

## Implementation steps

### 1. Add dbref counter to World

```rust
pub struct World {
    pub objects: HashMap<String, GameObject>,
    pub scripts: HashMap<String, Script>,
    pub next_id: u64,
}

impl World {
    pub fn next_dbref(&mut self) -> String {
        self.next_id += 1;
        format!("#{}", self.next_id)
    }
}
```

### 2. Update DB schema

- Add `next_id INTEGER` to a new `meta` table (or store on the objects table as a max)
- Save/load `next_id` with the world

### 3. Update file loader (two-pass)

Pass 1: create objects, assign dbrefs, store `area/key → dbref` mapping
```
town/crossroads → #1
town/square → #2
town/tavern → #3
```

Pass 2: resolve exit from/to, object locations using the mapping

Cross-area refs use `area_name/key` format:
```
from = "crossroads"       → look up "town/crossroads" → #1
to = "forest/edge"        → look up "forest/edge" → #8
```

### 4. Update builder commands

- `@dig The Dark Cellar` → creates `#34` with title "The Dark Cellar"
- `@create magic sword` → creates `#35`
- `@open north = #12` → creates exit `#36` from current room to `#12`
- `@examine #12` → shows object details
- `@set #12/hp = 100` → set attribute
- `@teleport #5` → teleport to room #5
- `@destroy #35` → destroy object

### 5. Update engine

- `enter_world` — player gets a dbref, stored on account as `character_ref`
- `handle_disconnect` — player ref is a dbref
- `cmd_look`, `cmd_go`, `cmd_get`, etc. — all work with dbrefs internally
- `dispatch_fallback` — unchanged (searches by hook name, not ref)
- `do_move` — exit ref and target ref are dbrefs
- `check_lock` — unchanged (works on refs generically)

### 6. Update softcode API

- `get_object("#12")` — works
- `spawn()` — returns `#N`
- All functions accept dbrefs
- Object snapshots include `ref_id = "#12"` instead of path

### 7. Update REST API

- All ref_id fields use dbrefs
- `CreateRoom` returns `{"ref_id": "#5"}`
- `Examine` takes `{"ref_id": "#5"}`

### 8. Update tests

- All tests use dbrefs instead of string paths
- Test helper creates objects and captures returned dbrefs
- `test_engine()` starts with a world that has assigned dbrefs

### 9. Update display

- `examine` shows `Ref: #12` 
- `@dig` shows `Room created: The Dark Cellar (#34)`
- Room contents show dbrefs in builder mode (maybe `@examine` only)
- Players see names, not dbrefs

### 10. Migration for existing DBs

- On load, if objects have path-based refs, migrate them to dbrefs
- Or: just require a fresh DB (breaking change, acceptable at this stage)

## Files to modify

- `src/world/mod.rs` — next_id counter, next_dbref()
- `src/world/object.rs` — no struct changes (ref_id stays String)
- `src/db.rs` — save/load next_id, update load_world
- `src/loader.rs` — two-pass loading, key→dbref mapping
- `src/engine/mod.rs` — builder commands, enter_world, player refs
- `src/engine/commands.rs` — display changes
- `src/softcode/api.rs` — no changes (already uses String refs)
- `src/softcode/mod.rs` — spawn intent uses dbref
- `src/locks.rs` — no changes
- `src/config.rs` — spawn_room becomes a key reference, not a dbref
- `src/net/web.rs` — no changes
- Tests in engine, db, softcode modules

## Estimated scope

~300-500 lines changed across 8-10 files. No new dependencies. Breaking change for existing DBs (require fresh DB).

## Risks

- File loader becomes more complex (two-pass, cross-area resolution)
- Config `spawn_room` can't be a dbref (it's not assigned yet at config load time) — use a key like `town/crossroads` and resolve on first load
- Player character_ref on Account needs updating — assign dbref on first login, store on account
