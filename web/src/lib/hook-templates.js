// Starter code for a fresh hook. When the editor opens a hook that has no
// program yet, it seeds the buffer with one of these instead of a blank file,
// so a builder starts from a working example rather than a bare signature.
//
// Every hook is a Luau function named exactly after the hook, called with
// (this, actor, room, args):
//   this  — the object the program is attached to
//   actor — who triggered it (a player/npc); may be nil for system ticks
//   room  — the room context (often the same as `this` for room hooks)
//   args  — extra input: nil for most events, the typed text for cmd_*/on_reply
// A can_* guard returns false to veto the action (emit a reason first).
// See the API reference for emit/get_attr/has_tag/spawn/after/… .

const HEADER =
  '-- Called as (this, actor, room, args). `this` is this object; `actor` is\n' +
  '-- who triggered it. Delete what you don\'t need.\n';

// Full, hand-written examples for the fixed KNOWN_HOOKS.
const KNOWN = {
  can_get: `-- Gate whether \`actor\` may pick this object up. Return false to veto.
function can_get(this, actor, room)
  if has_tag(this, "quest:bound") then
    emit(actor, "It won't budge — some force holds it in place.")
    return false
  end
  return true
end
`,
  on_get: `-- Runs after \`actor\` picks this object up.
function on_get(this, actor, room)
  emit(actor, "As you lift " .. (this.title or this.key) .. ", it feels warm.")
  emit_room(room, actor.display_name .. " picks up " .. (this.title or this.key) .. ".", {actor.ref_id})
end
`,
  can_drop: `-- Gate whether \`actor\` may drop this object. Return false to veto.
function can_drop(this, actor, room)
  if has_tag(this, "curse:sticky") then
    emit(actor, "You can't let go of it!")
    return false
  end
  return true
end
`,
  on_drop: `-- Runs after \`actor\` drops this object.
function on_drop(this, actor, room)
  emit_room(room, actor.display_name .. " drops " .. (this.title or this.key) .. ".", {actor.ref_id})
end
`,
  can_put: `-- Gate whether \`actor\` may put an item into this container. Return false to veto.
function can_put(this, actor, room)
  if has_tag(this, "locked:true") then
    emit(actor, "It's locked shut.")
    return false
  end
  return true
end
`,
  on_put: `-- Runs after \`actor\` puts an item into this container.
function on_put(this, actor, room)
  emit(actor, "You tuck it inside " .. (this.title or this.key) .. ".")
end
`,
  can_traverse: `-- Gate whether \`actor\` may travel through this exit. Return false to veto.
function can_traverse(this, actor, room)
  if not has_tag(actor, "key:brass") then
    emit(actor, "The way is barred — you need the brass key.")
    return false
  end
  return true
end
`,
  can_enter: `-- Gate whether \`actor\` may enter this room. Return false to veto.
function can_enter(this, actor, room)
  if has_tag(this, "flag:sealed") then
    emit(actor, "A wall of force blocks the way.")
    return false
  end
  return true
end
`,
  on_enter: `-- Runs after \`actor\` enters this room.
function on_enter(this, actor, room)
  if not is_player(actor) then return end
  emit(actor, "The air here is colder than you expected.")
  emit_room(room, actor.display_name .. " arrives.", {actor.ref_id})
end
`,
  on_leave: `-- Runs as \`actor\` leaves this room.
function on_leave(this, actor, room)
  if not is_player(actor) then return end
  emit_room(room, actor.display_name .. " leaves.", {actor.ref_id})
end
`,
  can_look: `-- Gate whether \`actor\` may look at this object. Return false to veto.
function can_look(this, actor, room)
  if has_tag(this, "hidden:shadow") and not has_tag(actor, "sense:truesight") then
    return false
  end
  return true
end
`,
  on_look: `-- Runs when \`actor\` looks at / examines this object. Add extra flavor.
function on_look(this, actor, room)
  emit(actor, "You notice faint runes etched along the edge.")
end
`,
  can_say: `-- Gate whether \`actor\` may speak in this room. Return false to veto.
function can_say(this, actor, room)
  if has_tag(this, "flag:silence") then
    emit(actor, "No sound escapes your lips here.")
    return false
  end
  return true
end
`,
  on_say: `-- Runs after \`actor\` speaks in this room.
function on_say(this, actor, room)
  emit_room(room, "A faint echo answers from the walls.", {})
end
`,
  can_use: `-- Gate whether \`actor\` may use this object. Return false to veto.
function can_use(this, actor, room)
  local uses = get_attr(this, "uses") or 0
  if uses >= 3 then
    emit(actor, "It's spent — nothing happens.")
    return false
  end
  return true
end
`,
  on_use: `-- Runs when \`actor\` uses this object.
function on_use(this, actor, room)
  local uses = (get_attr(this, "uses") or 0) + 1
  set_attr(this, "uses", uses)
  emit(actor, "You activate " .. (this.title or this.key) .. ". (used " .. uses .. "x)")
end
`,
  can_see: `-- Gate whether \`actor\` can see this object (hides it from look/room lists).
function can_see(this, actor, room)
  if has_tag(this, "hidden:shadow") and not has_tag(actor, "sense:truesight") then
    return false
  end
  return true
end
`,
  on_move: `-- Runs on \`actor\` (this == actor) each time they move between rooms.
function on_move(this, actor, room)
  -- e.g. tick down a torch, count steps, drop a trail…
  local steps = (get_attr(this, "steps") or 0) + 1
  set_attr(this, "steps", steps)
end
`,
  on_destroy: `-- Runs just before this object is destroyed. Clean up anything it owns.
function on_destroy(this, actor, room)
  local pet = get_attr(this, "pet_ref")
  if pet then destroy(pet) end
end
`,
  on_connect: `-- Runs when a player connects (this == the player, or a room/global hook).
function on_connect(this, actor, room)
  if is_player(actor) then
    emit(actor, "Welcome back, " .. actor.display_name .. ".")
  end
end
`,
  on_disconnect: `-- Runs when a player disconnects.
function on_disconnect(this, actor, room)
  emit_room(room, actor.display_name .. " fades away.", {actor.ref_id})
end
`,
  on_whisper: `-- Runs after someone whispers in this room.
function on_whisper(this, actor, room)
  -- react to a hushed conversation nearby…
end
`,
  on_emote: `-- Runs after \`actor\` emotes in this room.
function on_emote(this, actor, room)
  -- react to the gesture…
end
`,
  on_receive: `-- Runs on \`actor\` (this == actor) after they receive an item.
function on_receive(this, actor, room)
  -- e.g. auto-equip, acknowledge a gift…
end
`,
  on_damage: `-- Runs when this object takes damage. \`args\` may carry amount/source.
function on_damage(this, actor, room, args)
  local hp = (get_attr(this, "hp") or 10) - 1
  set_attr(this, "hp", hp)
  if hp <= 0 then trigger(this, "on_death") end
end
`,
  on_death: `-- Runs when this object dies.
function on_death(this, actor, room, args)
  emit_room(room, (this.title or this.key) .. " collapses.", {})
  -- drop loot, spawn a corpse, award the killer…
end
`,
  on_tick: `-- Runs on the world tick for this object. \`actor\` is usually nil, so guard.
-- Reschedule with after() if you want it to keep ticking.
function on_tick(this, actor, room)
  local players = get_players_in_room(this)
  if #players == 0 then return end
  emit_room(this, "The braziers flicker and hiss.", {})
end
`,
  on_startup: `-- Runs once when the world boots. Seed state, spawn wandering npcs, etc.
function on_startup(this, actor, room)
  set_attr(this, "spawned_at", 0)
end
`,
  on_shutdown: `-- Runs once as the world shuts down. Persist anything transient.
function on_shutdown(this, actor, room)
  -- final bookkeeping…
end
`,
  on_reload: `-- Runs when the world hot-reloads. Re-derive any cached state.
function on_reload(this, actor, room)
  -- rebuild caches, re-arm timers with after()…
end
`,
  on_save: `-- Runs when the world is saved. Flush anything you keep in memory.
function on_save(this, actor, room)
  -- nothing to persist by default
end
`,
  on_create: `-- Runs once when this object is spawned. Initialize its attributes.
function on_create(this)
  set_attr(this, "uses", 0)
end
`,
};

function customName(name) {
  // Function name must equal the hook name; make a readable body placeholder.
  return name.replace(/[^a-zA-Z0-9_]/g, '_');
}

// Luau types for the standard hook parameters. Annotations are erased at
// runtime (mlua compiles Luau bytecode and ignores them), so they never affect
// execution or the engine's compile-check; their value is real completion and
// type-checking in an external editor with luau-lsp, where `Object` resolves
// from the game's hearth.d.luau. `Object` is the field set the engine hands
// each hook — see object_to_table in src/softcode/api.rs and OBJECT_MEMBERS in
// hearth-api.js (kept in sync by a cargo test).
const PARAM_TYPES = { this: 'Object', actor: 'Object', room: 'Object', args: 'any' };

// Annotate the parameters on a hook's `function <name>(...)` signature line.
function typeSignature(src) {
  return src.replace(
    /^(\s*function\s+[A-Za-z_]\w*\s*\()([^)]*)(\))/m,
    (_, pre, params, post) =>
      pre +
      params
        .split(',')
        .map((p) => {
          const name = p.trim();
          return name && PARAM_TYPES[name] ? `${name}: ${PARAM_TYPES[name]}` : p;
        })
        .join(', ') +
      post,
  );
}

// Return starter source for `name`, with typed parameters. KNOWN hooks get a
// hand-written example; open-ended cmd_/lib_/on_/can_ names get a shaped stub.
export function hookTemplate(name) {
  return typeSignature(rawTemplate(name));
}

function rawTemplate(name) {
  const n = (name || '').trim();
  if (!n) return '';
  if (KNOWN[n]) return KNOWN[n];
  const fn = customName(n);
  if (n.startsWith('cmd_')) {
    const word = n.slice(4);
    return `-- A player-typeable command: they type "${word} <args>". Runs when no
-- builtin command matches. \`args\` is the text after the command word.
function ${fn}(this, actor, room, args)
  emit(actor, "You ${word}. (you said: " .. tostring(args) .. ")")
end
`;
  }
  if (n.startsWith('lib_')) {
    const mod = n.slice(4);
    return `-- A module. Other programs load it with require("${mod}").
-- Return a table of the functions/values it exposes.
local M = {}

function M.greet(name)
  return "Hello, " .. name
end

return M
`;
  }
  if (n.startsWith('can_')) {
    return `${HEADER}-- A guard: return false to veto the action (emit a reason first).
function ${fn}(this, actor, room, args)
  return true
end
`;
  }
  // Any other on_* (custom event you fire yourself) or bare name.
  const howFired = n.startsWith('on_')
    ? `-- Fire it yourself from another program with\n--   trigger(this, "${n}")   or   after(ticks, this, "${n}")\n-- \`args\` carries whatever you pass (or a player's reply via prompt()).\n`
    : '';
  return `${howFired}function ${fn}(this, actor, room, args)
  emit(actor, "${fn} ran.")
end
`;
}
