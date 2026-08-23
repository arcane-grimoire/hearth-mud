-- Hearth MUD API type definitions for Luau LSP.
--
-- This is the CANONICAL, engine-owned declaration of the softcode API surface.
-- It lives in the engine repo so it changes in lockstep with the functions it
-- describes. Games built on Hearth vendor a copy of this file for luau-lsp
-- (see docs/softcode-guide.md).
--
-- The function/value NAME set here is checked hard against the engine's actual
-- registrations (`env.set` in src/softcode/api.rs plus the `lua.globals().set`
-- installs in src/noise.rs and src/grid.rs) and against the web editor's
-- reference (web/src/components/code/hearth-api.js) by
-- `help_panel_api_reference_matches_installed_functions` in api.rs — adding,
-- renaming, or removing a function without updating all three fails
-- `cargo test`. Signatures/docs are hints, kept correct by hand until a future
-- introspection endpoint can generate them.

--- A reference to an object — either a ref string like "area/town/room/square"
--- or an object table with a ref_id field.
type ObjectRef = string | Object

--- An object handle as returned by get_object, or passed to hooks as
--- this/actor/room. Reads are pending-aware (they see your own same-script
--- writes); writes to attrs/title/description push intents, exactly like the
--- write API. Protected fields (ref_id, key, kind, location_ref) reject
--- writes. Note: list results (all_objects, get_contents, …) are plain
--- snapshots — property WRITES only work on single-object handles.
type Object = {
    ref_id: string,
    key: string,
    kind: string,
    title: string?,
    display_name: string,
    description: string,
    location_ref: string?,
    owner_ref: string?,
    attrs: { [string]: any },
    tags: { string },
}

type ExitInfo = {
    key: string,
    target_ref: string,
    aliases: { string },
}

type SpawnOpts = {
    key: string,
    kind: string?,
    title: string?,
    description: string?,
    location: ObjectRef?,
    ref: string?,
}

-- ═══════════════════════════════════════
-- READ API
-- ═══════════════════════════════════════

--- Get an object by ref. Returns nil if not found.
declare function get_object(ref: ObjectRef): Object?

--- Resolve an authoring/file key ("area/key") to a runtime ref_id.
--- Returns nil if the key doesn't resolve to a live object.
declare function resolve_key(file_key: string): string?

--- Get a single attribute value from an object. Returns nil if the attribute
--- doesn't exist or the object doesn't exist.
declare function get_attr(ref: ObjectRef, key: string): any

--- Read a nested value out of an attribute. Reads attr `attr_key` on `ref`
--- (pending-write aware), then descends the trailing path segments — a string
--- indexes an object key, a number indexes a 1-based array position. Returns
--- the value at the leaf, or nil if any segment is missing.
--- e.g. pick(this, "stats", "hp", 1)
declare function pick(ref: ObjectRef, attr_key: string, ...: string | number): any

--- Check if an object has a specific attribute.
declare function has_attr(ref: ObjectRef, key: string): boolean

--- Check if an object has a specific tag. Use "category:key" format,
--- e.g. has_tag(actor, "quest:worthy").
declare function has_tag(ref: ObjectRef, spec: string): boolean

--- Get all tags on an object as a list of "category:key" strings.
declare function get_tags(ref: ObjectRef): { string }

--- List all objects in a room (excludes exits and offline players).
declare function get_room_contents(ref: ObjectRef): { Object }

--- List every Item-kind object located directly inside a ref (container or
--- room).
declare function get_contents(ref: ObjectRef): { Object }

--- List all exits from a room.
declare function get_exits(ref: ObjectRef): { ExitInfo }

--- Get the object that contains this object (its location).
--- Returns nil if the object has no location.
declare function get_location(ref: ObjectRef): Object?

--- Get the kind of an object ("room", "item", "npc", "player", "exit").
declare function kind_of(ref: ObjectRef): string?

--- Find all objects in the world with a specific tag.
--- e.g. find_by_tag("owner:player/sam") to find all of a player's heroes.
declare function find_by_tag(spec: string): { Object }

--- Find all objects whose attribute `key` deep-equals `value`.
declare function find_by_attr(key: string, value: any): { Object }

--- Find an object in a room by fuzzy name match.
--- Returns the first match or nil.
declare function find_in_room(room: ObjectRef, name: string): Object?

--- List items (kind="item") carried by an object.
declare function get_inventory(ref: ObjectRef): { Object }

--- List online players in a room (excludes offline/disconnected players).
declare function get_players_in_room(room: ObjectRef): { Object }

--- List objects in room `ref` whose `_x`/`_y` attrs lie within `radius`
--- (Euclidean) of the point (x, y).
declare function get_nearby(ref: ObjectRef, x: number, y: number, radius: number): { Object }

--- BFS over the room graph from room `ref`, returning each room reachable
--- within `radius` (edge cost 1 plus exit `muffle`; `blocked_sound` exits are
--- skipped). `name` is present when the room object resolves.
declare function get_rooms_in_radius(ref: ObjectRef, radius: number): { { ref: string, distance: number, name: string? } }

--- Find all objects of a specific kind ("room", "item", "npc", "player").
declare function get_all_by_kind(kind: string): { Object }

--- Get every object's ref_id in the world as a flat list of strings.
declare function all_objects(): { string }

--- Get the ref of an object's owner. Returns nil if unowned.
declare function get_owner(ref: ObjectRef): string?

-- ═══════════════════════════════════════
-- PREDICATES
-- ═══════════════════════════════════════

--- Is this object a player?
declare function is_player(ref: ObjectRef): boolean

--- Is this object an NPC?
declare function is_npc(ref: ObjectRef): boolean

--- Is this object an item?
declare function is_item(ref: ObjectRef): boolean

--- Is this object a room?
declare function is_room(ref: ObjectRef): boolean

--- Is this object an exit?
declare function is_exit(ref: ObjectRef): boolean

--- Can this object hold other items (tagged "item:container")?
declare function is_container(ref: ObjectRef): boolean

--- Does this ref point to a real object in the world?
declare function exists(ref: ObjectRef): boolean

--- Is the actor carrying an item with this tag?
--- e.g. is_carrying(actor, "quest:skeleton_key")
declare function is_carrying(actor: ObjectRef, tag: string): boolean

--- Are two objects in the same room?
declare function same_room(a: ObjectRef, b: ObjectRef): boolean

--- Case-insensitive name match: true if `input` equals `name`, is a prefix of
--- it, or is a prefix of any whitespace-separated word in it.
declare function match_name(name: string, input: string): boolean

-- ═══════════════════════════════════════
-- WRITE API
-- These queue intents — changes apply after the script finishes.
-- ═══════════════════════════════════════

--- Set an attribute on an object. Value can be any JSON-compatible type
--- (string, number, boolean, table, nil).
declare function set_attr(ref: ObjectRef, key: string, value: any): ()

--- Write a nested value into an attribute, auto-creating objects/arrays along
--- the way. Trailing args are the path (string = object key, number = 1-based
--- array index) followed by the value to write as the final argument.
--- e.g. set_val(this, "stats", "hp", 1, 20)  -- sets stats.hp[1] = 20
declare function set_val(ref: ObjectRef, attr_key: string, ...: any): ()

--- Remove an attribute from an object.
declare function unset_attr(ref: ObjectRef, key: string): ()

--- Move `amount` of a numeric attribute `key` from one object to another.
declare function transfer_attr(from: ObjectRef, to: ObjectRef, key: string, amount: number): ()

--- Send a message to a player. Only the target player sees it.
declare function emit(ref: ObjectRef, message: string): ()

--- Send a message to everyone in a room. Pass an optional list of refs
--- to exclude (e.g. {actor.ref_id} to skip the actor).
declare function emit_room(room: ObjectRef, message: string, exclude: { ObjectRef }?): ()

--- Send a message to players within `radius` (Euclidean) of the point (x, y)
--- in room `ref`. `exclude` is an optional list of refs to skip.
declare function emit_nearby(ref: ObjectRef, x: number, y: number, radius: number, message: string, exclude: { ObjectRef }?): ()

--- Send distance-attenuated messages outward through the room graph up to
--- `radius`. `messages` maps integer distance → message text; `exclude` is an
--- optional list of refs to skip.
declare function emit_radius(ref: ObjectRef, radius: number, messages: { [number]: string }, exclude: { ObjectRef }?): ()

--- Send structured JSON data to a player's web client.
--- The web client receives {"type":"game","channel":channel,"data":data}.
--- Use for game-specific UI panels (combat, quests, inventory display, etc.).
declare function emit_data(ref: ObjectRef, channel: string, data: any): ()

--- Move an object to a new location (room or container).
declare function move_object(ref: ObjectRef, destination: ObjectRef): ()

--- Add a tag to an object. Use "category:key" format.
declare function set_tag(ref: ObjectRef, spec: string): ()

--- Remove a tag from an object.
declare function unset_tag(ref: ObjectRef, spec: string): ()

--- Set an object's owner. Errors if the owner ref doesn't exist.
declare function set_owner(ref: ObjectRef, owner: ObjectRef): ()

--- Change an object's display name.
declare function set_title(ref: ObjectRef, title: string): ()

--- Change an object's description.
declare function set_description(ref: ObjectRef, description: string): ()

--- Remove an object from the world. Cannot destroy player objects.
declare function destroy(ref: ObjectRef): ()

--- Fire a hook on another object. The triggered hook runs after this
--- script's intents are applied. Use for connected machinery, puzzles,
--- chain reactions. The optional data table is passed to the hook as `args`.
--- e.g. trigger("area/dungeon/item/gate", "on_activate")
declare function trigger(ref: ObjectRef, hook: string, data: any?): ()

--- Spawn a new object in the world. Returns the new object's ref_id.
--- kind defaults to "item". Cannot spawn "player" kind.
declare function spawn(opts: SpawnOpts): string

type CreateExitOpts = {
    source: ObjectRef,
    direction: string,
    target: ObjectRef,
    aliases: { string }?,
}

--- Create an exit between two rooms. Returns the new exit's ref_id.
declare function create_exit(opts: CreateExitOpts): string

--- Attach a program (Luau hook) to an object. Hook names: on_enter, on_leave,
--- cmd_<name>, etc.
declare function set_program(ref: ObjectRef, hook: string, source: string): ()

--- Install several programs on an object at once, from a table mapping hook
--- name → Luau source. Queues one set_program intent per pair.
declare function apply_template(ref: ObjectRef, template: { [string]: string }): ()

--- Generate a procedural dungeon from a seed string. Returns the entrance
--- room's ref_id. Deterministic — same seed always produces the same dungeon.
--- The optional config table tunes zone/room generation.
declare function generate_dungeon(seed: string, config: { [string]: any }?): string

--- Destroy all rooms and exits belonging to a dungeon with the given seed.
declare function destroy_dungeon(seed: string): ()

type InstantiateMapResult = {
    entrance_ref: string,
    room_count: number,
}

--- Instantiate a map template by name. Creates rooms and exits from the
--- template's grid layout. Returns the entrance ref and room count.
declare function instantiate_map(name: string): InstantiateMapResult

-- ═══════════════════════════════════════
-- MAP TEMPLATES
-- ═══════════════════════════════════════

type MapCell = {
    x: number,
    y: number,
    terrain: string,
    theme: string?,
    passable: boolean?,
    title_prefix: string?,
    terrain_attrs: { [string]: any }?,
    title: string?,
    description: string?,
    fixed_room: string?,
    encounters: { { monster: string, count_min: number, count_max: number } }?,
}

type TerrainDef = {
    theme: string,
    passable: boolean,
    title_prefix: string?,
    color: string?,
    tile_image: string?,
    tile_rotation: string?,
    attrs: { [string]: any }?,
}

type MapTemplate = {
    name: string,
    width: number,
    height: number,
    --- Cells keyed by "x,y" (0-based).
    cells: { [string]: MapCell },
    --- Terrain definitions keyed by terrain char.
    terrain: { [string]: TerrainDef },
}

--- Read a parsed map-template descriptor by name. Errors if unknown.
declare function get_map_template(name: string): MapTemplate

-- ═══════════════════════════════════════
-- INK DIALOGUE
-- ═══════════════════════════════════════

type InkChoice = {
    index: number,
    text: string,
    tags: { string },
}

type InkOutput = {
    text: string,
    can_continue: boolean,
    ended: boolean,
    choices: { InkChoice },
    tags: { string },
}

--- Start (or resume) an Ink conversation between actor and npc. Source comes
--- from opts.file, else the npc's `_ink_source` attr. opts.resume (default
--- true) reloads saved state from the npc's `_ink_state_<actor>` attr.
declare function ink_start(actor: ObjectRef, npc: ObjectRef, opts: { file: string?, resume: boolean? }?): InkOutput

--- Advance the story one step.
declare function ink_continue(actor: ObjectRef, npc: ObjectRef): InkOutput

--- Select choice `index` in the current conversation.
declare function ink_choose(actor: ObjectRef, npc: ObjectRef, index: number): InkOutput

--- Jump the conversation to a knot/stitch path.
declare function ink_goto(actor: ObjectRef, npc: ObjectRef, path: string): InkOutput

--- End the conversation. If `save` (default false), serialized state is
--- queued as a write to the npc's `_ink_state_<actor>` attr. Always true.
declare function ink_end(actor: ObjectRef, npc: ObjectRef, save: boolean?): boolean

--- Read an Ink variable from the conversation; nil if unset.
declare function ink_get_var(actor: ObjectRef, npc: ObjectRef, name: string): any

--- Set an Ink variable (JSON-coerced) in the conversation.
declare function ink_set_var(actor: ObjectRef, npc: ObjectRef, name: string, val: any): ()

-- ═══════════════════════════════════════
-- UTILITY
-- ═══════════════════════════════════════

--- The current world tick count at the time this hook began. A value, not a
--- call — use `get_tick`, not `get_tick()`.
declare get_tick: number

--- Print a debug message to the server console log.
declare function log(message: string): ()

--- Serialize a Luau value to a JSON string. Errors on unserializable input.
declare function json_encode(v: any): string

--- Parse a JSON string into a Luau value. Errors on invalid JSON.
declare function json_decode(s: string): any

--- Schedule a hook to fire on an object after a number of engine ticks.
--- e.g. after(5, this, "on_expire") fires on_expire on this object in 5 ticks.
--- The optional data table is JSON-serialized, stored with the timer, and
--- passed as the `args` parameter to the hook when it fires.
declare function after(ticks: number, ref: ObjectRef, hook: string, data: { [string]: any }?): ()

--- Cancel a previously scheduled timer for this object/hook pair.
declare function cancel_after(ref: ObjectRef, hook: string): ()

type ScheduledTimer = {
    hook: string,
    fire_at_tick: number,
    data: any?,
}

--- List an object's active timers.
declare function get_timers(ref: ObjectRef): { ScheduledTimer }

--- Set up an interactive prompt — the player's next input is routed to
--- the specified hook on the specified object.
declare function prompt(actor: ObjectRef, obj: ObjectRef, hook: string): ()

-- ═══════════════════════════════════════
-- NOISE (procedural generation)
-- ═══════════════════════════════════════

--- 2D simplex noise. Returns a value in [-1, 1]. Deterministic for the
--- same seed and coordinates.
declare function simplex2d(seed: number, x: number, y: number): number

--- 3D simplex noise. Returns a value in [-1, 1].
declare function simplex3d(seed: number, x: number, y: number, z: number): number

--- 2D Perlin noise. Returns a value in [-1, 1].
declare function perlin2d(seed: number, x: number, y: number): number

--- 3D Perlin noise. Returns a value in [-1, 1].
declare function perlin3d(seed: number, x: number, y: number, z: number): number

--- 2D Fractal Brownian Motion (layered Perlin). Returns a value roughly
--- in [-1, 1]. Optional parameters control octaves (default 6),
--- frequency (default 1.0), lacunarity (default 2.0), and
--- persistence (default 0.5).
declare function fbm2d(seed: number, x: number, y: number, octaves: number?, frequency: number?, lacunarity: number?, persistence: number?): number

-- ═══════════════════════════════════════
-- SEEDED RNG (deterministic randomness)
-- ═══════════════════════════════════════

--- Deterministic hash from any mix of strings, numbers, and booleans.
--- Same inputs always produce the same integer output.
--- e.g. hash_seed("world", x, y) for coordinate-based generation.
declare function hash_seed(...: string | number | boolean): number

--- Deterministic pseudo-random integer in [min, max] from a seed.
declare function seed_random(seed: number, min: number, max: number): number

--- Deterministic pseudo-random float in [0, 1) from a seed.
declare function seed_float(seed: number): number

--- Deterministic pick from a list (1-indexed sequence) using a seed.
declare function seed_choice(seed: number, list: { any }): any

-- ═══════════════════════════════════════
-- COORDINATE MATH
-- ═══════════════════════════════════════

--- Euclidean distance between two points.
declare function distance(x1: number, y1: number, x2: number, y2: number): number

--- Manhattan distance between two points.
declare function manhattan(x1: number, y1: number, x2: number, y2: number): number

--- Compass direction from one point to another.
--- Returns "n", "s", "e", "w", "ne", "nw", "se", "sw", or "here".
declare function direction_to(x1: number, y1: number, x2: number, y2: number): string

--- Linear interpolation between a and b. t=0 returns a, t=1 returns b.
declare function lerp(a: number, b: number, t: number): number

--- Clamp a value to [min, max].
declare function clamp(value: number, min: number, max: number): number

--- Remap a value from one range to another.
--- e.g. remap(5, 0, 10, 0, 100) returns 50.
declare function remap(value: number, in_min: number, in_max: number, out_min: number, out_max: number): number

-- ═══════════════════════════════════════
-- GRID (2D cell grids: pathfinding, LOS, FOV)
-- All coordinates are 1-based.
-- ═══════════════════════════════════════

type Grid2D = {
    --- Cell value at (x, y), or nil if out of bounds.
    get: (self: Grid2D, x: number, y: number) -> any,
    --- Set the cell at (x, y). Errors if out of bounds.
    set: (self: Grid2D, x: number, y: number, value: any) -> (),
    width: (self: Grid2D) -> number,
    height: (self: Grid2D) -> number,
    --- Returns width and height as two values.
    size: (self: Grid2D) -> (number, number),
    --- Fill the rectangle (x1,y1)-(x2,y2) with `value`, clamped to bounds.
    fill: (self: Grid2D, x1: number, y1: number, x2: number, y2: number, value: any) -> (),
    --- Serialize to a plain table (round-trips through grid_from_value).
    to_value: (self: Grid2D) -> { width: number, height: number, cells: { any } },
    --- Coords of the first cell equal to `value` (row-major), or nil.
    find: (self: Grid2D, value: any) -> { x: number, y: number }?,
    --- Coords of all cells equal to `value`.
    find_all: (self: Grid2D, value: any) -> { { x: number, y: number } },
    --- In-bounds orthogonal (N,E,S,W) neighbors of (x, y) with their values.
    neighbors: (self: Grid2D, x: number, y: number) -> { { x: number, y: number, value: any } },
    --- A* path over cells equal to `walkable` (4-connected), or nil.
    pathfind: (self: Grid2D, x1: number, y1: number, x2: number, y2: number, walkable: any) -> { { x: number, y: number } }?,
    --- Bresenham line-of-sight: true if no `blocking` cell lies between.
    has_los: (self: Grid2D, x1: number, y1: number, x2: number, y2: number, blocking: any) -> boolean,
    --- Shadow-casting field of view from (x, y) out to `radius`.
    fov: (self: Grid2D, x: number, y: number, radius: number, blocking: any) -> { { x: number, y: number } },
    --- New grid of BFS distances from (x, y); unreachable cells hold -1.
    distance_map: (self: Grid2D, x: number, y: number, walkable: any) -> Grid2D,
    --- Flood-fill the region of `target` cells at (x, y) with `replacement`;
    --- returns the number of cells changed.
    flood_fill: (self: Grid2D, x: number, y: number, target: any, replacement: any) -> number,
}

--- Create a Grid2D of width×height cells all set to `default`. Errors if
--- width or height is 0.
declare function grid_new(width: number, height: number, default: any): Grid2D

--- Rebuild a Grid2D from a serialized table (as produced by grid:to_value()).
declare function grid_from_value(value: { width: number, height: number, cells: { any } }): Grid2D
