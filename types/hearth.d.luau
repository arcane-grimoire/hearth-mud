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
    -- The archetype this object delegates to, if it's an instance (see
    -- docs/plans/archetypes.md). `title`, `description`, and `attrs` above
    -- are already resolved up this chain — spawn()'s `archetype` option and
    -- clone() are the write side.
    archetype_ref: string?,
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
    -- Delegate to an archetype (a dbref, a file key like "town/goblin", or an
    -- Object) — see docs/plans/archetypes.md. The instance inherits the
    -- archetype's script and any unset title/description/attrs; explicit
    -- fields above still override.
    archetype: ObjectRef?,
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

--- Remove this instance's OWN override of `key` so it inherits the
--- archetype's value again (MOO `clear_property`) — see
--- docs/plans/archetypes.md. Same effect as unset_attr; use this name when
--- the intent is "revert to the archetype", not "delete the attribute".
declare function clear_attr(ref: ObjectRef, key: string): ()

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

type MoveOpts = {
    announce: boolean?,
    fire_hooks: boolean?,
}

--- Move an object to a new location (room or container). By default this is a
--- silent relocation. Pass { announce = true } to emit "X leaves." / "X
--- arrives." to the rooms involved, and/or { fire_hooks = true } to run the
--- on_leave/on_move/on_enter hooks afterward with the moved object as the
--- actor. Hooks run after the move commits and do NOT evaluate can_enter/
--- can_traverse denial — scripted moves are unrestricted (lock-respecting
--- movement is the player `go` path).
declare function move_object(ref: ObjectRef, destination: ObjectRef, opts: MoveOpts?): ()

--- Replace an object's alias keywords (the extra names it answers to) with a
--- new list. Entries are trimmed; blanks are dropped.
declare function set_aliases(ref: ObjectRef, aliases: { string }): ()

--- Add a tag to an object. Use "category:key" format.
declare function set_tag(ref: ObjectRef, spec: string): ()

--- Remove a tag from an object.
declare function unset_tag(ref: ObjectRef, spec: string): ()

--- Set an object's owner. Errors if the owner ref doesn't exist.
declare function set_owner(ref: ObjectRef, owner: ObjectRef): ()

--- Attach a Lock DSL expression to one of an object's hooks (e.g. "can_enter",
--- "get", "traverse"). Predicates: perm(), has_tag(), has_attr(),
--- in_inventory(), is_kind(), is_owner(), time_between(), with AND/OR/NOT.
declare function set_lock(ref: ObjectRef, hook: string, expr: string): ()

--- Remove the lock from one of an object's hooks (the inverse of set_lock).
--- A no-op if that hook carries no lock.
declare function clear_lock(ref: ObjectRef, hook: string): ()

--- Point an object at an existing archetype (or nil to clear the delegation).
--- The object then inherits the archetype's title/description/attrs/tags/script
--- where it has no value of its own. Reparents to an existing object; it does
--- not create a new archetype. See docs/plans/archetypes.md.
declare function set_archetype(ref: ObjectRef, archetype: ObjectRef?): ()

--- Change an object's display name.
declare function set_title(ref: ObjectRef, title: string): ()

--- Change an object's description.
declare function set_description(ref: ObjectRef, description: string): ()

--- Remove an object from the world. Cannot destroy player objects. Refuses to
--- remove an archetype that still has live instances (objects whose
--- archetype_ref points at it) — see docs/plans/archetypes.md.
declare function destroy(ref: ObjectRef): ()

--- Flatten an archetype instance in place: copy its resolved title,
--- description, attrs, and (if it has none of its own) its nearest
--- archetype's script onto the object, then clear archetype_ref. The
--- delegation escape hatch — spawn() defaults to delegating, this is the
--- loud opt-out. Returns the same ref. A no-op on an object that isn't an
--- instance.
declare function clone(ref: ObjectRef): string

type CloneOpts = {
    location: ObjectRef?,
    owner: ObjectRef?,
}

--- Deep-copy an existing object into a fresh dbref: title, description, attrs,
--- attr-schema, tags, aliases, locks, script, libs, and archetype. system:*
--- tags and the file-key are stripped, so the copy is always a plain player
--- object (never treated as managed/locked/global). Defaults location and
--- owner to the source's; override via opts. Returns the new ref. Distinct
--- from clone(), which flattens an archetype instance in place.
declare function clone_object(src: ObjectRef, opts: CloneOpts?): string

--- Fire a hook on another object. The triggered hook runs after this script's
--- intents are applied. Use for connected machinery, puzzles, chain reactions.
--- `data` (a table) is passed to the hook as its 4th argument:
---   function on_activate(this, actor, room, data) ... end
--- The optional `actor` fires the hook *as* that object (default: the ambient
--- actor — whoever caused this batch).
--- e.g. trigger(gate, "on_activate", { key = "brass" }, player)
declare function trigger(ref: ObjectRef, hook: string, data: any?, actor: ObjectRef?): ()

--- Force a player or NPC to run a command as if they had typed it — charm,
--- puppet, confusion, or a self-driving NPC. The command runs through the
--- normal parser under the *target's* own scopes; `@`-commands (all authoring/
--- admin verbs) and quit are refused on this path, and forced-command chains
--- are depth-bounded. A player runs through their live session; an NPC (no
--- session) runs the gameplay subset with no client to echo to.
--- e.g. run_command_as(goblin, "go north")
declare function run_command_as(actor: ObjectRef, command: string): ()

--- Available ONLY inside a hook body (an archetype delegation feature — see
--- docs/plans/archetypes.md): call the inherited version of the CURRENT
--- hook — the next archetype ancestor above the one whose code is running
--- that defines it (MOO pass() / Smalltalk super). Bound to the same
--- this/actor/room and IntentBatch as the calling hook. With no `args`,
--- forwards this call's own args unchanged; with `args`, forwards that
--- instead. Returns the ancestor hook's return value, or nil if no ancestor
--- defines this hook (a safe no-op).
declare function pass(args: string?): any

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

type UpdateExitOpts = {
    direction: string?,
    destination: ObjectRef?,
}

--- Retarget and/or rename an existing exit. `direction` renames the exit (its
--- key); `destination` repoints it. An omitted field is left unchanged.
declare function update_exit(ref: ObjectRef, opts: UpdateExitOpts): ()

--- Set an object's whole behavior script — one Luau chunk that defines its
--- hooks as top-level functions (on_enter, on_leave, cmd_<name>, ...) sharing
--- one scope. Replaces any existing script on the object.
declare function set_script(ref: ObjectRef, source: string): ()

--- Set a require()able library module on a Kind::Code object, by bare name
--- (loaded elsewhere as require("<name>")).
declare function set_lib(ref: ObjectRef, name: string, source: string): ()

--- Set an object's script from a template: either the whole source string, or
--- a table of source fragments (each defining hook functions) concatenated
--- into one chunk.
declare function apply_template(ref: ObjectRef, template: string | { [string]: string }): ()

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

type GridMoveResult = {
    ok: boolean,        -- false only when the map is unknown (reason="no_map")
    moved: boolean?,    -- true if the actor moved; false when blocked
    reason: string?,    -- "no_map" | "blocked"
    x: number?,         -- new cell (present when moved)
    y: number?,
    terrain: string?,   -- terrain char of the new cell (present when moved)
}

--- Move `actor` one cell in `dir` ("n"/"north"/"s"/"e"/"w"/…) on the named map
--- template, using the actor's `_x`/`_y` attrs as its position — the 2D-grid
--- "wilderness" model, where the whole grid lives in ONE room and there is NO
--- room per cell. Honors terrain/cell passability (impassable or off-grid →
--- moved=false) and, on a successful step, fires the terrain's `on_leave`
--- (old cell) and `on_enter` (new cell) hooks on that terrain's `archetype`
--- object, if it declares one — so terrain behavior ("lava burns") is defined
--- once per terrain, not per square. Updates `_x`/`_y` on the actor.
declare function grid_move(actor: ObjectRef, map: string, dir: string): GridMoveResult

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

type GameTime = {
    total_minutes: number,
    minute: number,
    hour: number,
    day: number,
    month: number,
    year: number,
    is_day: boolean,
    weekday: number?,
    day_name: string?,
    month_name: string?,
}

--- The in-world game clock, or nil when no `[clock]` is configured. Unlike
--- `get_tick` this is a CALL. `year`/`month`/`day` are 1-based; `hour`/`minute`
--- 0-based; `is_day` is dawn ≤ hour < dusk. See the game clock in the docs.
declare function get_time(): GameTime?

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
