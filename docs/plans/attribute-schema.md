# Plan: Attribute schemas — terrain consumer (remaining)

> **Status: the mechanism shipped for objects/archetypes.** The kind-agnostic
> core (`src/attr_schema.rs`: `AttrType` closed enum + `Unknown` fallback,
> `AttrDescriptor`) is built and wired for **objects and rooms**: declare
> `attr_schema = [ … ]` on a `[[objects]]`/`[[rooms]]` element; it persists (DB
> `attr_schema_json`), round-trips through `@export`/`@import`, inherits down the
> archetype chain (`World::resolved_attr_schema`, nearest-wins), `examine`
> returns it with per-descriptor `source`, the builder renders typed widgets
> (`AttrField.svelte`), and the warn-only load-time value validation is done.
> **Terrain is the one remaining consumer.**

## Remaining goal

Fold `[[terrain_attr]]` onto the same `AttrType`/`AttrDescriptor` so the map
builder's terrain form gets typed inputs instead of free-form key/value boxes.
Custom terrain attributes are today a free-form `HashMap<String, toml::Value>`
on `TerrainDef` (serde-flatten): the engine stamps each onto a room as
`terrain_<key>` and never interprets it, so an editor has no idea which keys
exist, their types, or valid values. This plan closes that gap for terrain,
reusing the shipped mechanism — no new type system.

## Descriptor shape (reuse the object/room one)

```toml
[[terrain_attr]]
key      = "movement_cost"   # stamped as terrain_movement_cost
type     = "int"
label    = "Movement cost"
help     = "AP to enter this tile"
default  = 1
min      = 0
max      = 10

[[terrain_attr]]
key    = "biome"
type   = "enum"
values = ["arid", "temperate", "alpine"]
default = "temperate"

[[terrain_attr]]
key  = "boss"
type = "ref"
ref  = "monster"             # source of the dropdown options
```

Lives inline in the game-level `terrain.toml`, alongside the palette it
describes. A game with no `[[terrain_attr]]` blocks keeps working as today —
schema is purely additive.

## Implementation stages

1. **Load** the `[[terrain_attr]]` array in `load_terrain_palette` (or a
   sibling), deserializing into the existing `AttrDescriptor`. Same non-fatal
   loader contract.
2. **Expose** it — `get_terrain_schema()` and/or a `schema` field on the
   `get_map_template` return — so the map builder (over REST/WS) and softcode
   can read it.
3. **Do not gate** on it. Terrain attrs stay free-form at load; the schema is
   descriptive metadata for tooling, same as the object/room path.
4. **(Editor)** the map builder's terrain form renders per the schema, reusing
   `AttrField.svelte`.

## Open questions

- **`ref` source vocabulary** — which entity kinds a terrain `ref` may point at
  (`monster`, `item`, `theme`, `area`, `room`, …) and how the map builder
  enumerates each.
