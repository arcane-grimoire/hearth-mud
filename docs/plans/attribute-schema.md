# Plan: Attribute schemas for game-defined data

> **Status: objects/archetypes shipped.** The kind-agnostic mechanism
> (`src/attr_schema.rs`: `AttrType` closed enum + `Unknown` fallback,
> `AttrDescriptor`) is built and wired for **objects and rooms**: declare
> `attr_schema = [ … ]` (inline array of descriptors) on a `[[objects]]`/
> `[[rooms]]` element; it persists (DB `attr_schema_json`), round-trips through
> `@export`/`@import`, inherits down the archetype chain
> (`World::resolved_attr_schema`, nearest-wins), and `examine` returns it with
> per-descriptor `source` (own/ancestor). The web builder's `PropertiesPanel`
> renders declared attrs as typed widgets (`AttrField.svelte`) — text/number/
> checkbox/enum-dropdown/color/`ref`-dropdown (populated by the
> `list_ref_candidates` REST action)/repeatable `list<T>` rows — with
> own-vs-inherited affordances; undeclared attrs keep the raw field.
> **Terrain** is the remaining consumer: fold `[[terrain_attr]]` onto the same
> `AttrType`/`AttrDescriptor` (Stages 1–3 below) when the terrain form is built.
> The optional warn-only load-time value validation is still open.

## Goal

Let a game *declare* the custom attributes it uses — starting with terrain
attributes — so a web builder can render a real form (typed inputs, defaults,
validation, discoverability) instead of untyped key/value text boxes. The
schema is game-authored, engine-exposed, and consumed by tooling. Build it
with the editor, not ahead of it.

## Current state

Custom terrain attributes are a free-form `HashMap<String, toml::Value>` on
`TerrainDef` (serde-flatten). The engine stamps each onto a room as
`terrain_<key>` and never interprets it. Powerful, but it gives an editor
nothing: no idea which keys exist, what types they take, what's valid, or how
to label them. The *built-in* fields (`theme`, `passable`, `color`,
`tile_image`, `tile_rotation`) already have an implicit schema — they're typed
Rust fields the editor can hardcode. This plan is only about the open-ended
`attrs` bag.

## Standard types

A small, closed set of attribute types, each mapping cleanly to a TOML value, a
JSON Schema fragment (the attrs serialize to `serde_json::Value`, so JSON
Schema is the natural validation/form-gen target), and an editor widget:

| type      | TOML example                      | JSON Schema                              | Editor widget            |
|-----------|-----------------------------------|------------------------------------------|--------------------------|
| `string`  | `label = "Iron Pass"`             | `{"type":"string"}`                      | single-line text         |
| `text`    | `ambient = "wind howls"`          | `{"type":"string"}` (+`"format":"text"`) | multi-line textarea      |
| `int`     | `movement_cost = 3`               | `{"type":"integer"}`                     | number spinner (step 1)  |
| `float`   | `spawn_rate = 0.25`               | `{"type":"number"}`                      | number input             |
| `bool`    | `is_high_ground = true`           | `{"type":"boolean"}`                     | checkbox / toggle        |
| `enum`    | `biome = "alpine"`                | `{"enum":[...]}`                          | dropdown                 |
| `color`   | `minimap = "#3a6a2e"`             | `{"type":"string","format":"color"}`     | color picker             |
| `ref`     | `boss = "orc_chief"`              | `{"type":"string"}` (+ ref source)       | entity dropdown          |
| `list<T>` | `tags = ["cold","windy"]`         | `{"type":"array","items":{T}}`           | repeatable rows of T     |

Notes:

- **`ref`** is the high-value one for a builder — it points at another game
  entity (a monster key, item key, theme name, area/room). The descriptor
  names the source (`ref = "monster"`), and the editor populates the dropdown
  from live game data rather than a free-text key the author can typo.
- **`color`** overlaps the first-class `color` field on terrain, but a game
  may want *additional* colors (minimap vs. lighting vs. faction tint) as
  custom attrs — hence a reusable type.
- **`list<T>`** takes a base scalar type as its item type; no nested lists to
  start (keeps the form generator simple).
- Types are a **closed set**. An unknown `type` in a descriptor is a warn (the
  editor falls back to a raw JSON field), never a hard error — same non-fatal
  contract as the other loaders.

## Descriptor shape

One entry per attribute, declared as an array in TOML:

```toml
[[terrain_attr]]
key      = "movement_cost"   # the attr name (stamped as terrain_movement_cost)
type     = "int"
label    = "Movement cost"   # human label for the form field
help     = "AP to enter this tile"
default  = 1
min      = 0                 # int/float only
max      = 10
required = false

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

Common descriptor fields: `key`, `type`, `label`, `help`, `default`,
`required`. Per-type extras: `min`/`max`/`step` (int/float), `values` (enum),
`ref` (ref), `pattern` (string), `items`/`item_type` (list). The engine
carries these through opaquely; the editor and validator interpret them.

## Where it lives

In the game-level `<game_dir>/terrain.toml`, alongside the palette it
describes. A game with no `[[terrain_attr]]` blocks keeps working exactly as
today — schema is purely additive.

## Engine role: expose, don't enforce

- **Load** the `[[terrain_attr]]` array with `terrain.toml` (same non-fatal
  loader contract).
- **Expose** it as data — a `get_terrain_schema()` softcode read and/or a
  `schema` field on the `get_map_template` return — so the builder (over the
  REST/WS API) and softcode can read it.
- **Do not gate** on it. Terrain attrs stay free-form at load; the schema is
  descriptive metadata for tooling. This matches how presentation fields are
  "carried but never read."
- **Optional, later:** a warn-only validation pass at load ("terrain `m` attr
  `movement_cost` = \"three\", expected int; terrain `f` has undeclared attr
  `foo`"). Loud-but-non-fatal, catches builder mistakes without blocking boot.

## Implementation stages

1. **Types + descriptor structs** — a `TerrainAttrSchema` (the descriptor) and
   the closed `AttrType` enum, deserialized from `[[terrain_attr]]`. Pure data.
2. **Load** — read the array in `load_terrain_palette` (or a sibling), return
   it alongside the palette.
3. **Expose** — `get_terrain_schema()` and/or fold into `get_map_template`.
   This is the surface the web editor consumes to generate its form.
4. **(Editor)** — the web builder renders a form per the schema, using a
   JSON-Schema form generator where the type maps cleanly, custom widgets for
   `color`/`ref`. Out of this repo's scope; the schema is the contract.
5. **(Optional) validation** — warn-only load-time check of attrs against the
   schema.

Stages 1–3 are the engine's whole job. Do them when the editor is being built,
so the editor's real rendering needs shape the descriptor fields rather than
being guessed here.

## Generalization

Terrain is the first consumer, but object and room attributes want the same
treatment in a builder. Design the *mechanism* — a declared
attribute-descriptor set the engine loads and exposes — so it can later cover
`[[object_attr]]` / `[[room_attr]]` (or a shared `[[attr_schema]]` keyed by
kind). Do **not** build the general system now; just don't hardcode
"terrain" so deep that extending it means a rewrite.

## Open questions

- **Schema file home** — inline in `terrain.toml` (proximity to the palette)
  vs. a dedicated `schema.toml` (one place once objects/rooms join). Lean
  inline for terrain now; revisit when generalizing.
- **`ref` source vocabulary** — which entity kinds are addressable
  (`monster`, `item`, `theme`, `area`, `room`, ...) and how the editor
  enumerates each over the API.
- **Declared vs. inferred** — should the engine also *infer* a loose schema by
  scanning attrs actually in use (types + observed keys), so an unschema'd game
  still gets typed-ish fields? Declared wins where present; inference could
  fill gaps. Probably a later nicety.
- **Enforcement dial** — stay advisory forever, or offer an opt-in strict mode
  that rejects undeclared/mistyped attrs at load for teams that want it.

## Risks

- **Over-design ahead of the editor** — the biggest one. The descriptor fields
  should follow the editor's needs; freezing them now risks churn. Mitigate by
  shipping stages 1–3 with the editor, not before.
- **Type-set creep** — every new widget is a new `AttrType`. Keep the set
  closed and small; unknown types degrade to a raw field, never a crash.
- **Drift between schema and reality** — a schema that lies (declares
  `movement_cost` as int while maps store strings) misleads the editor. The
  optional warn-only validation is the cheap guard.
