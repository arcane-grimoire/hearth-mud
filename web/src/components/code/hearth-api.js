// The Hearth Luau API surface exposed to hooks, for editor autocomplete.
// Enumerated from src/softcode/api.rs (the env functions) plus the object
// fields object_to_value builds. Signatures are hints, not enforced — kept
// honest by the sync-guard tests in api.rs (help_panel_api_reference_matches,
// object_member_reference_matches_engine_snapshot); a future engine
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
  ['emit_nearby', 'emit_nearby(ref, text)', 'Send text to objects near a ref.'],
  ['emit_radius', 'emit_radius(ref, radius, text)', 'Send text within a coordinate radius.'],
  ['emit_data', 'emit_data(target, channel, data)', 'Push a structured game-data message to a client.'],
  ['spawn', 'spawn(area, key)', 'Instantiate an authored object by its file key.'],
  ['destroy', 'destroy(ref)', 'Remove an object from the world.'],
  ['move_object', 'move_object(ref, destination)', 'Relocate an object to a new location ref.'],
  ['transfer_attr', 'transfer_attr(from, to)', 'Move an attribute between objects.'],
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
  ['match_name', 'match_name(ref, needle)', 'True if the object matches a name/alias.'],
  ['get_exits', 'get_exits(room)', 'List a room’s exits.'],
  ['create_exit', 'create_exit(source, dir, target)', 'Create an exit between rooms.'],
  ['get_contents', 'get_contents(ref)', 'Objects located inside a ref.'],
  ['get_room_contents', 'get_room_contents(room)', 'Objects in a room.'],
  ['get_inventory', 'get_inventory(ref)', 'An actor’s carried items.'],
  ['get_location', 'get_location(ref)', 'The ref an object is located in.'],
  ['get_owner', 'get_owner(ref)', 'The owner ref.'],
  ['set_owner', 'set_owner(ref, owner)', 'Set the owner.'],
  ['get_players_in_room', 'get_players_in_room(room)', 'Player refs in a room.'],
  ['get_nearby', 'get_nearby(ref)', 'Objects near a ref (by coordinates).'],
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
  ['set_val', 'set_val(ref, key, value)', 'Set a scratch value.'],
  ['after', 'after(seconds, hook)', 'Schedule a hook to fire later; returns a timer.'],
  ['cancel_after', 'cancel_after(timer)', 'Cancel a scheduled timer.'],
  ['get_timers', 'get_timers(ref)', 'Pending timers on an object.'],
  ['trigger', 'trigger(ref, hook, data?)', 'Fire a hook immediately.'],
  ['get_tick', 'get_tick()', 'The current world tick count.'],
  ['prompt', 'prompt(player, text, hook)', 'Ask a player for input, resume in a hook.'],
  ['pick', 'pick(list)', 'Choose a random element.'],
  ['log', 'log(...)', 'Write to the server log.'],
  ['json_encode', 'json_encode(value)', 'Serialize a value to JSON.'],
  ['json_decode', 'json_decode(text)', 'Parse JSON into a value.'],
  ['apply_template', 'apply_template(ref, template)', 'Apply a map template to a room.'],
  ['instantiate_map', 'instantiate_map(name)', 'Build rooms from a map template.'],
  ['get_map_template', 'get_map_template(name)', 'Read a map template.'],
  ['generate_dungeon', 'generate_dungeon(opts)', 'Procedurally build a dungeon.'],
  ['destroy_dungeon', 'destroy_dungeon(ref)', 'Tear down a generated dungeon.'],
  ['ink_start', 'ink_start(player, knot)', 'Begin an Ink story for a player.'],
  ['ink_continue', 'ink_continue(player)', 'Advance the Ink story.'],
  ['ink_choose', 'ink_choose(player, index)', 'Pick an Ink choice.'],
  ['ink_goto', 'ink_goto(player, knot)', 'Jump to an Ink knot.'],
  ['ink_get_var', 'ink_get_var(player, name)', 'Read an Ink variable.'],
  ['ink_set_var', 'ink_set_var(player, name, value)', 'Set an Ink variable.'],
  ['ink_end', 'ink_end(player)', 'End the Ink story.'],
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
