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
  ['emit_room', 'emit_room(room, text)', 'Send text to everyone in a room.'],
  ['emit_nearby', 'emit_nearby(ref, x, y, radius, text, exclude?)', 'Send text to players within a Euclidean radius of (x, y).'],
  ['emit_radius', 'emit_radius(ref, radius, messages, exclude?)', 'Send distance-attenuated messages out through the room graph.'],
  ['emit_data', 'emit_data(target, channel, data)', 'Push a structured game-data message to a client.'],
  ['spawn', 'spawn(opts)', 'Instantiate a new object from a {key, kind?, …} table; returns its ref.'],
  ['destroy', 'destroy(ref)', 'Remove an object from the world.'],
  ['move_object', 'move_object(ref, destination)', 'Relocate an object to a new location ref.'],
  ['transfer_attr', 'transfer_attr(from, to, key, amount)', 'Move `amount` of a numeric attribute between objects.'],
  ['get_object', 'get_object(ref)', 'Resolve a ref to an object value.'],
  ['resolve_key', 'resolve_key(file_key)', 'Resolve an "area/key" file key to a ref.'],
  ['exists', 'exists(ref)', 'True if the ref points at a live object.'],
  ['get_attr', 'get_attr(ref, key)', 'Read an attribute.'],
  ['set_attr', 'set_attr(ref, key, value)', 'Write an attribute.'],
  ['has_attr', 'has_attr(ref, key)', 'True if the attribute is set.'],
  ['unset_attr', 'unset_attr(ref, key)', 'Remove an attribute.'],
  ['find_by_attr', 'find_by_attr(key, value)', 'Find objects with a matching attribute.'],
  ['get_tags', 'get_tags(ref)', 'List an object’s tags.'],
  ['has_tag', 'has_tag(ref, tag)', 'True if the object has the tag ("category:key").'],
  ['set_tag', 'set_tag(ref, tag)', 'Add a tag.'],
  ['unset_tag', 'unset_tag(ref, tag)', 'Remove a tag.'],
  ['find_by_tag', 'find_by_tag(tag)', 'Find objects with a tag.'],
  ['find_in_room', 'find_in_room(room, needle)', 'Match an object by name in a room.'],
  ['match_name', 'match_name(name, input)', 'True if `input` prefix-matches the name (or any word in it).'],
  ['get_exits', 'get_exits(room)', 'List a room’s exits.'],
  ['create_exit', 'create_exit(source, dir, target)', 'Create an exit between rooms.'],
  ['get_contents', 'get_contents(ref)', 'Objects located inside a ref.'],
  ['get_room_contents', 'get_room_contents(room)', 'Objects in a room.'],
  ['get_inventory', 'get_inventory(ref)', 'An actor’s carried items.'],
  ['get_location', 'get_location(ref)', 'The ref an object is located in.'],
  ['get_owner', 'get_owner(ref)', 'The owner ref.'],
  ['set_owner', 'set_owner(ref, owner)', 'Set the owner.'],
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
  ['set_program', 'set_program(ref, hook, source)', 'Attach/replace a hook program.'],
  ['set_val', 'set_val(ref, attr_key, ...path, value)', 'Write a nested value into an attribute, auto-creating containers.'],
  ['after', 'after(ticks, ref, hook, data?)', 'Schedule a hook to fire on an object after N ticks.'],
  ['cancel_after', 'cancel_after(ref, hook)', 'Cancel a scheduled timer for this object/hook.'],
  ['get_timers', 'get_timers(ref)', 'Pending timers on an object.'],
  ['trigger', 'trigger(ref, hook, data?)', 'Fire a hook after this script; passes data as `args`.'],
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

// Implicit locals available in a hook body.
export const API_GLOBALS = [
  ['actor', 'actor', 'The object that triggered this hook (often the player).'],
  ['this', 'this', 'The object the hook is attached to.'],
  ['ctx', 'ctx', 'Hook context (args and metadata for this firing).'],
];

// Members available on an object value (after `this.` / `actor.`).
export const OBJECT_MEMBERS = [
  ['ref_id', 'The object’s dbref.'],
  ['key', 'Its authoring key.'],
  ['kind', 'Its kind ("room", "npc", "item"…).'],
  ['title', 'Display title.'],
  ['display_name', 'Resolved display name.'],
  ['description', 'Look description.'],
  ['location_ref', 'Where it is located.'],
  ['owner_ref', 'Owner ref.'],
  ['attrs', 'Table of attributes.'],
  ['tags', 'Table of tags.'],
];
