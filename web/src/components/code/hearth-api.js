// The Hearth Luau API surface exposed to hooks, for editor autocomplete.
// Enumerated from the engine's registrations — the `env.set` functions in
// src/softcode/api.rs plus the globals installed by src/noise.rs and
// src/grid.rs — plus the object fields object_to_value builds. Signatures are
// hints, not enforced — kept honest by the sync-guard tests in api.rs
// (help_panel_api_reference_matches, object_member_reference_matches_engine_snapshot),
// which also check the engine-owned types/hearth.d.luau; a future engine
// introspection endpoint could replace these static lists.
//
// Note: hook-facing objects also support PROPERTY syntax — `this.hp = 0` ≡
// `set_attr(this, "hp", 0)`, `this.title = "x"` ≡ `set_title(...)`, and reads
// are pending-aware like get_attr. See docs/softcode-guide.md.

// Callable API functions: [name, signature, doc]
// Kept honest by src/softcode/api.rs's help_panel_api_reference_matches test:
// adding/renaming/removing an engine function without updating this list
// fails `cargo test`. (Read-side scratch values use get_attr; there is no
// engine `get_val`.)
export const API_FUNCTIONS = [
  ['emit', 'emit(target, text)', 'Send text to a player (or everyone in a room ref).'],
  ['emit_room', 'emit_room(room, text, exclude?)', 'Send text to everyone in a room; `exclude` is a table of refs to skip (e.g. {actor.ref_id}).'],
  ['emit_nearby', 'emit_nearby(ref, x, y, radius, text, exclude?)', 'Send text to players within a Euclidean radius of (x, y).'],
  ['emit_radius', 'emit_radius(ref, radius, messages, exclude?)', 'Send distance-attenuated messages out through the room graph.'],
  ['emit_data', 'emit_data(target, channel, data)', 'Push a structured game-data message to a client.'],
  ['spawn', 'spawn(opts)', 'Instantiate a new object from a {key, kind?, archetype?, …} table; returns its ref.'],
  ['destroy', 'destroy(ref)', 'Remove an object from the world.'],
  ['clone', 'clone(ref)', 'Flatten an archetype instance in place — copy its resolved title/description/attrs/script onto it and clear the delegation.'],
  ['clone_object', 'clone_object(src, opts?)', 'Deep-copy an object into a fresh dbref (title, attrs, tags, aliases, locks, script…) from a { location?, owner? } table; system:* tags are stripped. Returns the new ref.'],
  ['move_object', 'move_object(ref, destination, opts?)', 'Relocate an object. Pass { announce = true } to emit “X leaves/arrives.” and/or { fire_hooks = true } to run on_leave/on_move/on_enter afterward.'],
  ['set_aliases', 'set_aliases(ref, aliases)', 'Replace an object’s alias keywords with a list of strings.'],
  ['transfer_attr', 'transfer_attr(from, to, key, amount)', 'Move `amount` of a numeric attribute between objects.'],
  ['get_object', 'get_object(ref)', 'Resolve a ref to an object value.'],
  ['resolve_key', 'resolve_key(file_key)', 'Resolve an "area/key" file key to a ref.'],
  ['exists', 'exists(ref)', 'True if the ref points at a live object.'],
  ['get_attr', 'get_attr(ref, key)', 'Read an attribute.'],
  ['set_attr', 'set_attr(ref, key, value)', 'Write an attribute.'],
  ['has_attr', 'has_attr(ref, key)', 'True if the attribute is set.'],
  ['unset_attr', 'unset_attr(ref, key)', 'Remove an attribute.'],
  ['clear_attr', 'clear_attr(ref, key)', 'Remove this instance’s own override of an attribute so it inherits the archetype’s value again (MOO clear_property). Same effect as unset_attr.'],
  ['find_by_attr', 'find_by_attr(key, value)', 'Find objects with a matching attribute.'],
  ['get_tags', 'get_tags(ref)', 'List an object’s tags.'],
  ['has_tag', 'has_tag(ref, tag)', 'True if the object has the tag ("category:key").'],
  ['set_tag', 'set_tag(ref, tag)', 'Add a tag.'],
  ['unset_tag', 'unset_tag(ref, tag)', 'Remove a tag.'],
  ['find_by_tag', 'find_by_tag(tag)', 'Find objects with a tag.'],
  ['find_in_room', 'find_in_room(room, needle)', 'Match an object by name in a room.'],
  ['match_name', 'match_name(name, input)', 'True if `input` prefix-matches the name (or any word in it).'],
  ['get_exits', 'get_exits(room)', 'List a room’s exits.'],
  ['create_exit', 'create_exit(opts)', 'Create an exit from a { source, direction, target } table; returns its ref.'],
  ['update_exit', 'update_exit(ref, opts)', 'Retarget or rename an exit from a { direction?, destination? } table; an omitted field is left unchanged.'],
  ['set_lock', 'set_lock(ref, hook, expr)', 'Attach a Lock DSL expression to one of an object’s hooks (e.g. "can_enter", "get").'],
  ['clear_lock', 'clear_lock(ref, hook)', 'Remove the lock from one of an object’s hooks (inverse of set_lock).'],
  ['get_contents', 'get_contents(ref)', 'Objects located inside a ref.'],
  ['get_room_contents', 'get_room_contents(room)', 'Objects in a room.'],
  ['get_inventory', 'get_inventory(ref)', 'An actor’s carried items.'],
  ['get_location', 'get_location(ref)', 'The ref an object is located in.'],
  ['get_owner', 'get_owner(ref)', 'The owner ref.'],
  ['set_owner', 'set_owner(ref, owner)', 'Set the owner.'],
  ['set_archetype', 'set_archetype(ref, archetype)', 'Point an object at an existing archetype (or nil to clear); it then delegates title/attrs/tags/script to it.'],
  ['get_players_in_room', 'get_players_in_room(room)', 'Player refs in a room.'],
  ['get_nearby', 'get_nearby(ref, x, y, radius)', 'Objects within a Euclidean radius of (x, y).'],
  ['get_rooms_in_radius', 'get_rooms_in_radius(ref, radius)', 'Rooms within a coordinate radius.'],
  ['get_all_by_kind', 'get_all_by_kind(kind)', 'Every object of a kind ("room", "npc"…).'],
  ['all_objects', 'all_objects()', 'Every object in the world.'],
  ['kind_of', 'kind_of(ref)', 'The object’s kind string.'],
  ['is_player', 'is_player(ref)', 'True if a player.'],
  ['is_npc', 'is_npc(ref)', 'True if an NPC.'],
  ['is_room', 'is_room(ref)', 'True if a room.'],
  ['is_exit', 'is_exit(ref)', 'True if an exit.'],
  ['is_item', 'is_item(ref)', 'True if an item.'],
  ['is_container', 'is_container(ref)', 'True if a container.'],
  ['is_carrying', 'is_carrying(actor, item_tag)', 'True if the actor carries a matching item.'],
  ['same_room', 'same_room(a, b)', 'True if two refs share a room.'],
  ['set_title', 'set_title(ref, title)', 'Set an object’s title.'],
  ['set_description', 'set_description(ref, description)', 'Set an object’s description.'],
  ['set_script', 'set_script(ref, source)', 'Set an object’s whole script (hooks as functions in it).'],
  ['set_lib', 'set_lib(ref, name, source)', 'Set a require()able lib module on a Code object.'],
  ['set_val', 'set_val(ref, attr_key, ...path, value)', 'Write a nested value into an attribute, auto-creating containers.'],
  ['after', 'after(ticks, ref, hook, data?)', 'Schedule a hook to fire on an object after N ticks.'],
  ['cancel_after', 'cancel_after(ref, hook)', 'Cancel a scheduled timer for this object/hook.'],
  ['get_timers', 'get_timers(ref)', 'Pending timers on an object.'],
  ['trigger', 'trigger(ref, hook, data?)', 'Fire a hook after this script; passes data as `args`.'],
  ['pass', 'pass(args?)', 'Inside a hook, call the inherited version of THIS hook — the next archetype ancestor above the one currently running that defines it (MOO pass()/super). Forwards this call’s args unless given explicit ones; returns nil if there is no ancestor definition.'],
  ['get_tick', 'get_tick', 'The current world tick count (a value, not a call).'],
  ['prompt', 'prompt(actor, obj, hook)', 'Route the actor’s next input to a hook on obj.'],
  ['pick', 'pick(ref, attr_key, ...path)', 'Read a nested value out of an attribute by path.'],
  ['log', 'log(...)', 'Write to the server log.'],
  ['json_encode', 'json_encode(value)', 'Serialize a value to JSON.'],
  ['json_decode', 'json_decode(text)', 'Parse JSON into a value.'],
  ['apply_template', 'apply_template(ref, template)', 'Apply a map template to a room.'],
  ['instantiate_map', 'instantiate_map(name)', 'Build rooms from a map template.'],
  ['get_map_template', 'get_map_template(name)', 'Read a map template.'],
  ['generate_dungeon', 'generate_dungeon(seed, config?)', 'Procedurally build a dungeon from a seed; returns the entrance ref.'],
  ['destroy_dungeon', 'destroy_dungeon(ref)', 'Tear down a generated dungeon.'],
  ['ink_start', 'ink_start(actor, npc, opts?)', 'Begin/resume an Ink conversation between actor and npc.'],
  ['ink_continue', 'ink_continue(actor, npc)', 'Advance the Ink story.'],
  ['ink_choose', 'ink_choose(actor, npc, index)', 'Pick an Ink choice.'],
  ['ink_goto', 'ink_goto(actor, npc, path)', 'Jump to an Ink knot/stitch.'],
  ['ink_get_var', 'ink_get_var(actor, npc, name)', 'Read an Ink variable.'],
  ['ink_set_var', 'ink_set_var(actor, npc, name, value)', 'Set an Ink variable.'],
  ['ink_end', 'ink_end(actor, npc, save?)', 'End the Ink story, optionally saving state.'],

  // Noise & procedural generation (installed as globals by src/noise.rs).
  ['simplex2d', 'simplex2d(seed, x, y)', '2D simplex noise in [-1, 1].'],
  ['simplex3d', 'simplex3d(seed, x, y, z)', '3D simplex noise in [-1, 1].'],
  ['perlin2d', 'perlin2d(seed, x, y)', '2D Perlin noise in [-1, 1].'],
  ['perlin3d', 'perlin3d(seed, x, y, z)', '3D Perlin noise in [-1, 1].'],
  ['fbm2d', 'fbm2d(seed, x, y, octaves?, frequency?, lacunarity?, persistence?)', 'Layered (fractal Brownian) 2D Perlin noise.'],
  // Seeded RNG (deterministic).
  ['hash_seed', 'hash_seed(...)', 'Deterministic integer hash of any mix of strings/numbers/booleans.'],
  ['seed_random', 'seed_random(seed, min, max)', 'Deterministic integer in [min, max].'],
  ['seed_float', 'seed_float(seed)', 'Deterministic float in [0, 1).'],
  ['seed_choice', 'seed_choice(seed, list)', 'Deterministic pick from a 1-indexed list.'],
  // Coordinate math.
  ['distance', 'distance(x1, y1, x2, y2)', 'Euclidean distance.'],
  ['manhattan', 'manhattan(x1, y1, x2, y2)', 'Manhattan distance.'],
  ['direction_to', 'direction_to(x1, y1, x2, y2)', 'Compass direction ("n", "se", "here", …).'],
  ['lerp', 'lerp(a, b, t)', 'Linear interpolation.'],
  ['clamp', 'clamp(value, min, max)', 'Clamp to [min, max].'],
  ['remap', 'remap(value, in_min, in_max, out_min, out_max)', 'Remap a value between ranges.'],
  // 2D cell grids (installed as globals by src/grid.rs); see the Grid2D type.
  ['grid_new', 'grid_new(width, height, default)', 'Create a Grid2D of width×height cells set to `default`.'],
  ['grid_from_value', 'grid_from_value(value)', 'Rebuild a Grid2D from a grid:to_value() table.'],
];

// Implicit locals available in a hook body, in the order hooks receive them:
// (this, actor, room, args). A 4th 'object' element marks the ones that are
// GameObject values — the Help panel expands those to the object members
// below, so searching "actor"/"room" surfaces its properties.
export const API_GLOBALS = [
  ['this', 'this', 'The object the hook is attached to.', 'object'],
  ['actor', 'actor', 'The object that triggered this hook (often the player). May be nil for system ticks.', 'object'],
  ['room', 'room', 'The room context for this hook — often the same as `this` for room hooks.', 'object'],
  ['ctx', 'ctx', 'Hook context: the args and metadata for this firing (a table, not an object).'],
];

// Members available on an object value (after `this.` / `actor.`).
export const OBJECT_MEMBERS = [
  ['ref_id', 'The object’s dbref (e.g. "#12"). Use it where an API wants a ref.'],
  ['key', 'Its authoring key (the "area/key" name it was created under).'],
  ['kind', 'Its kind ("room", "npc", "item", "exit", "player").'],
  ['title', 'Display title. Assigning it (this.title = …) calls set_title.'],
  ['display_name', 'Resolved display name — the title if set, else the key.'],
  ['description', 'Look description. Assigning it calls set_description.'],
  ['location_ref', 'The ref this object is located in (room or container).'],
  ['owner_ref', 'The account/ref that owns this object.'],
  ['archetype_ref', 'The archetype this object delegates to, if it’s an instance (see spawn’s archetype option).'],
  ['attrs', 'Table of attributes. Reads are pending-aware; prefer get_attr/pick.'],
  ['tags', 'Table of tags ("category:key"). Prefer has_tag/get_tags to read.'],
];

// ─────────────────────────────────────────────────────────────────────────
// Presentation metadata for the Help panel. NONE of the below is read by the
// engine sync-guard tests — those parse only the API_FUNCTIONS and
// OBJECT_MEMBERS arrays above (and stop at the first `];` / next `export
// const`). So categories and examples are free to grow without touching the
// engine. Adding/removing an actual engine function still has to happen in
// API_FUNCTIONS, which the tests enforce.

// Ordered function groups, so the ~90-function reference reads as navigable
// sections instead of one flat list. Every function name should appear in
// exactly one group; any that isn't falls into a trailing "Other" bucket in
// the panel so nothing silently disappears.
export const API_CATEGORIES = [
  { id: 'output', label: 'Output & messaging',
    names: ['emit', 'emit_room', 'emit_nearby', 'emit_radius', 'emit_data'] },
  { id: 'objects', label: 'Objects',
    names: ['spawn', 'clone', 'clone_object', 'destroy', 'move_object', 'get_object', 'resolve_key', 'exists',
            'set_title', 'set_description', 'set_aliases', 'get_location', 'get_owner', 'set_owner', 'set_archetype', 'kind_of'] },
  { id: 'attrs', label: 'Attributes',
    names: ['get_attr', 'set_attr', 'has_attr', 'unset_attr', 'find_by_attr',
            'transfer_attr', 'set_val', 'pick'] },
  { id: 'tags', label: 'Tags',
    names: ['get_tags', 'has_tag', 'set_tag', 'unset_tag', 'find_by_tag'] },
  { id: 'rooms', label: 'Rooms, exits & containment',
    names: ['get_exits', 'create_exit', 'update_exit', 'get_contents', 'get_room_contents', 'get_inventory'] },
  { id: 'locks', label: 'Locks',
    names: ['set_lock', 'clear_lock'] },
  { id: 'queries', label: 'Queries & lookup',
    names: ['find_in_room', 'match_name', 'get_players_in_room', 'get_nearby',
            'get_rooms_in_radius', 'get_all_by_kind', 'all_objects'] },
  { id: 'predicates', label: 'Kind & predicates',
    names: ['is_player', 'is_npc', 'is_room', 'is_exit', 'is_item', 'is_container',
            'is_carrying', 'same_room'] },
  { id: 'scheduling', label: 'Programs & scheduling',
    names: ['set_script', 'set_lib', 'after', 'cancel_after', 'get_timers', 'trigger', 'get_tick', 'prompt'] },
  { id: 'maps', label: 'Maps & dungeons',
    names: ['apply_template', 'instantiate_map', 'get_map_template', 'generate_dungeon', 'destroy_dungeon'] },
  { id: 'ink', label: 'Ink dialogue',
    names: ['ink_start', 'ink_continue', 'ink_choose', 'ink_goto', 'ink_get_var', 'ink_set_var', 'ink_end'] },
  { id: 'noise', label: 'Noise',
    names: ['simplex2d', 'simplex3d', 'perlin2d', 'perlin3d', 'fbm2d'] },
  { id: 'rng', label: 'Seeded RNG',
    names: ['hash_seed', 'seed_random', 'seed_float', 'seed_choice'] },
  { id: 'math', label: 'Coordinate math',
    names: ['distance', 'manhattan', 'direction_to', 'lerp', 'clamp', 'remap'] },
  { id: 'grid', label: 'Grid',
    names: ['grid_new', 'grid_from_value'] },
  { id: 'utility', label: 'Utility',
    names: ['log', 'json_encode', 'json_decode'] },
];

// Worked examples for the functions worth showing in use. Keyed by function
// name; shown in the panel's expanded row (nothing else reads them). Kept
// idiomatic — `ipairs` over list results, property syntax where it reads
// cleaner, ref where an API wants a ref. Locals and loop bindings carry Luau
// type annotations (a real Luau feature — erased at runtime, checked by
// luau-lsp); opts tables use a `:: SpawnOpts` / `:: CreateExitOpts` assertion.
// Only types that exist in the game's hearth.d.luau are used (`Object`,
// `SpawnOpts`, `CreateExitOpts`, primitives), never an invented one.
export const API_EXAMPLES = {
  emit: `emit(actor, "A chill runs down your spine.")`,
  emit_room: `-- everyone in the room except the actor
emit_room(room, actor.display_name .. " vanishes in a puff of smoke.", {actor.ref_id})`,
  emit_data: `-- push structured data to the player's client (web sidebar, etc.)
emit_data(actor, "quest", { id = "ember", state = "complete" })`,
  spawn: `-- the opts table can be typed with a :: assertion (SpawnOpts is in hearth.d.luau)
local torch: string = spawn({ key = "torch", kind = "item" } :: SpawnOpts)
move_object(torch, this.ref_id)   -- drop it here (spawn returns a ref)`,
  set_attr: `set_attr(this, "hp", 10)
this.hp = 10          -- property syntax — identical to the line above`,
  get_attr: `local hp: number = get_attr(this, "hp") or 0`,
  set_val: `-- writes attrs.stats.str = 5, creating the nested tables as needed
set_val(this, "stats", "str", 5)`,
  pick: `local str: number = pick(this, "stats", "str")   -- reads attrs.stats.str`,
  find_by_tag: `for _, npc: Object in ipairs(find_by_tag("faction:thieves")) do
  emit(npc, "The signal is given.")
end`,
  find_by_attr: `for _, o: Object in ipairs(find_by_attr("quest", "ember")) do
  set_attr(o, "quest", "done")
end`,
  after: `-- fire on_tick on this object in 5 ticks, with a payload
after(5, this.ref_id, "on_tick", { phase = "wake" })`,
  trigger: `-- run another hook after this script finishes; data arrives as args
trigger(this.ref_id, "on_alarm", { loud = true })`,
  create_exit: `-- exits are made from an opts table, not positional args
create_exit({ source = this.ref_id, direction = "north", target = target_room } :: CreateExitOpts)`,
  get_players_in_room: `for _, p: Object in ipairs(get_players_in_room(room)) do
  emit(p, "Thunder rolls overhead.")
end`,
  is_carrying: `if is_carrying(actor, "key:brass") then
  emit(actor, "The brass key hums in your pack.")
end`,
  ink_start: `ink_start(actor, this)   -- begin THIS npc's dialogue with the actor`,
  seed_random: `-- deterministic d20: same actor + tick always rolls the same
local roll: number = seed_random(hash_seed(actor.ref_id, get_tick), 1, 20)`,
  grid_new: `local g = grid_new(10, 10, 0)   -- a Grid2D userdata
g:set(3, 4, 1)
local v: number = g:get(3, 4)`,
};
