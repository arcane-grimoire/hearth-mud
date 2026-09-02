# LPMUD cookbook — mudlib patterns in Hearth

Of the three traditions this repo documents, LPMUD is the one Hearth is
descended from. The softcode guide says so outright: an object is a single
script whose top-level functions are its hooks, sharing one file scope — the
"object is the unit, hooks are its methods" model. If you have written LPC, you
have already written Hearth softcode; you just spelled `init()` differently.

So this document spends less time on idiom-mapping than the
[MUSH](mush-cookbook.md) or [Diku](diku-cookbook.md) ones, and more on the
three places where the shape genuinely differs — because those are where LPC
habits will actively mislead you.

- New to Hearth softcode? Read [the softcode guide](softcode-guide.md) first.
- The recipes here are executable: `tests/cookbook.rs` extracts every
  path-labelled script below and runs it against the real session handler.

## Contents

- [Driver and mudlib](#driver-and-mudlib)
- [Applies → hooks](#applies--hooks)
- [Efuns → API](#efuns--api)
- [The three real differences](#the-three-real-differences)
- [Rooms and inheritance](#rooms-and-inheritance) — `/std/room`
- [add_action → cmd_ hooks](#add_action--cmd_-hooks)
- [A soul](#a-soul) — the emote daemon
- [Daemons](#daemons)
- [heart_beat and reset](#heart_beat-and-reset)
- [What doesn't translate](#what-doesnt-translate)

---

## Driver and mudlib

LPMUD's defining idea is the split: a **driver** written in C provides efuns and
the object lifecycle, and a **mudlib** written in LPC provides everything a
player can see — rooms, combat, the login sequence, the command set. Swap the
mudlib and you have a different game on the same driver.

That is exactly Hearth's architecture, and the mapping is one-to-one:

| LPMUD | Hearth |
|---|---|
| the driver (C) | the engine (`hearth-mud`, Rust) |
| efuns | the [softcode API](softcode-guide.md) |
| the mudlib (LPC) | your game directory — `.toml` + `.luau` |
| `/std/`, `/obj/`, `/room/` | your `std/` and `world/` areas |
| the master object | engine config + [account scopes](softcode-guide.md#locks) |
| `/daemon/*.c` | `system:global` `Kind::Code` objects |

The Last Stag's layout follows it deliberately: `game/std/*` is code (rules and
base archetypes, file-authoritative and locked) and `game/world/*` is content
(live, DB-authoritative, edited in-game). That's a mudlib's `/std` versus a
wizard's realm, with the locking made explicit.

**One real difference in kind:** LPC is compiled by the driver from files on
disk, and `update /obj/torch` recompiles. Hearth's database is the source of
truth for world content, and code can be edited from inside the game (`@program`,
the web builder, the VS Code extension) as well as from files. Files reconcile
by content hash at boot, so an in-game edit isn't clobbered by a stale file —
it's reported as diverged instead. Closer to `update` than to a restart, but the
authority runs the other way.

---

## Applies → hooks

LPC "applies" are functions the driver calls on your object at defined moments.
That is precisely what a Hearth hook is, so this table is the bulk of the
translation.

| LPC apply | Hearth hook | Notes |
|---|---|---|
| `init()` | `on_enter` | See the [add_action](#add_action--cmd_-hooks) section — this is the big simplification |
| `reset()` | `on_tick` + a repop driver | No driver-side reset; see [heart_beat and reset](#heart_beat-and-reset) |
| `heart_beat()` | `on_tick` | `set_heart_beat(1)` → a `tick_interval` attr |
| `create()` / `setup()` | `on_create` | Fires when the object is first created at runtime |
| `long()` | `description`, or `on_look` for a computed one | On a **room**, `on_look` suppresses default rendering |
| `short()` | `title` (`display_name` falls back to `key`) | |
| `id(str)` | `match_name(name, input)` | The engine already does this for `get`/`look` |
| `can_put_and_get()` | `can_get` / `can_drop` | |
| `prevent_insert()` | `can_put` | |
| `catch_tell(msg)` | `on_say` | Message is on the room's `_say_message` attr |
| `receive_message()` | `on_receive` | |
| `move_object()` hooks | `on_move`, `on_leave`, `on_enter` | |
| `die()` | `on_death` | **The engine never fires it** — your combat code does |
| `exit_fun` / `leave()` | `on_leave`, or `can_traverse` on the exit | |
| `query_prevent_shadow()` | — | No shadows; see [what doesn't translate](#what-doesnt-translate) |

---

## Efuns → API

| Efun | Hearth |
|---|---|
| `this_object()` | the `this` parameter |
| `this_player()` | the `actor` parameter |
| `environment(ob)` | `get_location(ref)` |
| `all_inventory(ob)` | `get_inventory(ref)` / `get_contents(ref)` |
| `present("sword", ob)` | iterate `get_inventory` with `match_name`, or `find_in_room` |
| `move_object(ob, dest)` | `move_object(ref, dest)` |
| `clone_object("/obj/torch")` | `clone_object(resolve_key("area/torch"))` |
| `destruct(ob)` | `destroy(ref)` |
| `write(s)` / `tell_object(ob, s)` | `emit(actor, s)` |
| `say(s)` | `emit_room(room, s, {actor.ref_id})` |
| `tell_room(r, s)` | `emit_room(room, s)` |
| `notify_fail(s)` | `emit(actor, s)` then `return false` from a `can_` hook |
| `add_action("f", "verb")` | a `cmd_verb` hook — no `init()` needed |
| `call_out(f, delay)` | `after(ticks, ref, "on_hook", data?)` |
| `remove_call_out()` | `cancel_after(ref, "on_hook")` |
| `set_heart_beat(1)` | `tick_interval` attr + `on_tick` |
| `explode(s, d)` / `implode(a, d)` | `str.split(s, d)` / `table.concat(a, d)` |
| `sscanf(s, "%s and %s", a, b)` | `string.match(s, "^(.-) and (.+)$")` |
| `sizeof(x)` | `#x` |
| `member_array(e, a)` | a loop, or `collections` helpers |
| mappings `([ k : v ])` | Lua tables |
| arrays `({ a, b })` | Lua tables |
| `random(n)` | `random.roll(n)`, or `seed_random` when you want it testable |
| `objectp` / `stringp` / `intp` | `type(x)`, `is_item`/`is_npc`/`is_room` |
| `capitalize(s)` | `str.title_case(s)` |
| `query_verb()` | the command name is the hook name; args arrive as `args` |

---

## The three real differences

Everything above is renaming. These three are not, and they're where LPC
instincts will lead you wrong.

### 1. `call_other` is not synchronous — and state is public

This is the big one. In LPC, an object's variables are private and you ask for
them through lfuns:

```c
/* LPC */
int hp = ob->query_hp();
if (ob->query_weight() > 100) write("Too heavy.\n");
ob->add_hp(-5);
```

Hearth has no synchronous cross-object call. `trigger(ref, hook, data)` fires a
hook on another object, but it runs **after** your batch commits and cannot
return a value. Reaching for it as a substitute for `->` will not work.

The reason you don't need it is that **object state is public**. Attrs are
readable and writable directly, so the query lfun disappears:

```lua
-- Hearth
local hp = get_attr(ob, "hp")
if (get_attr(ob, "weight") or 0) > 100 then emit(actor, "Too heavy.") end
set_attr(ob, "hp", hp - 5)
```

For a *computed* answer — the LPC `query_armour_class()` that sums worn items —
you have two options, and neither is `trigger`:

**Put the computation in a shared module.** This is the direct equivalent of
inheriting a utility, and it's usually right:

```lua
-- lib/body.luau
local M = {}

function M.armour_class(ref)
  local ac = get_attr(ref, "base_ac") or 10
  for _, item in ipairs(get_inventory(ref)) do
    if get_attr(item, "worn") then
      ac = ac + (get_attr(item, "ac_bonus") or 0)
    end
  end
  return ac
end

return M
```

```lua
local body = require("body")
local ac = body.armour_class(target)   -- any object can ask about any other
```

**Or cache the derived value as an attr** and recompute it when its inputs
change — the same trick a mudlib uses when `query_armour_class()` gets hot.

Use `trigger` for what it is: *notifying* another object that something
happened, not asking it a question. And note it takes a fourth argument that
fires the hook **as** a chosen actor:

```lua
trigger(monster, "on_alerted", { threat = actor.ref_id }, actor)
```

### 2. `inherit` is single, and it's data as well as code

LPC has multiple inheritance:

```c
inherit "/std/monster";
inherit "/std/humanoid";
```

Hearth has [archetypes](archetypes.md) — a single `is-a` chain. An object
delegates to one archetype, which may delegate to another. What you inherit is
broader than LPC, though: not just the script, but title, description, tags,
attrs, and the attribute schema, with the child overriding whatever it declares.

For the "mix in two behaviors" case, use the chain for the *primary* identity
and `require()` for the shared behavior — the shared module is your second
parent, and unlike LPC there is no inheritance-order puzzle to solve.

The compensation is real: an archetype is a **live** link, not a compile-time
copy. Edit `/std/monster` in LPC and you must `update` every clone. Edit the
archetype here and every instance already in the world picks it up on
`@reload-world`.

### 3. There are no shadows

`shadow(ob, 1)` lets one object intercept another's function calls without the
target's knowledge. There is no equivalent, and it isn't coming — it's exactly
the kind of spooky action the intent model exists to prevent.

What shadows were actually used for, and what to use instead:

| Shadow use | Instead |
|---|---|
| Temporary effects (curse, blindness) | An attr + `after()` to expire it; the hook checks the attr |
| Adding behavior to an existing object | Edit its script, or give it an archetype |
| Intercepting `catch_tell` | An `on_say` hook on the room |
| Wrapping a command | A `system:global` object defining the same `cmd_` — though a room/inventory object shadows a global, not the other way round |

---

## Rooms and inheritance

The canonical mudlib room inherits `/std/room` and calls setters in `create()`.
In Hearth the data is declarative and only the *behavior* is code.

```c
/* LPC: /room/village/square.c */
inherit "/std/room";
void create() {
    ::create();
    set_short("The village square");
    set_long("A cobbled square...\n");
    add_exit("north", "/room/village/tavern", "door");
    set_light(1);
}
```

```toml
# Hearth. `archetype` is the inherit; the setters are just fields.
[[rooms]]
key = "square"
title = "The village square"
description = "A cobbled square, worn smooth."
archetype = "std/room_outdoor"
tags = ["env:outdoor"]

[rooms.attrs]
light = 1

[[exits]]
from = "square"
direction = "north"
to = "tavern"
aliases = ["n"]
```

The archetype carries whatever `/std/room_outdoor` would have: shared attrs,
tags, and a script whose hooks every outdoor room inherits.

An exit is an object too, and carries its own attrs and script — so the LPC
`add_exit("north", dest, "door")` door argument becomes a real gate:

```toml
[[exits]]
from = "tavern"
direction = "down"
to = "cellar"
script = "trapdoor.luau"    # defines can_traverse

[exits.attrs]
closed = true
```

---

## add_action → cmd_ hooks

The LPC command dance is `init()` plus `add_action`, run for every object that
enters your environment, with the function returning 1 for "handled" and 0 to
fall through:

```c
/* LPC */
void init() {
    add_action("do_polish", "polish");
}
int do_polish(string str) {
    if (!str || str != "lamp") return 0;      /* not mine — fall through */
    write("You polish the lamp.\n");
    return 1;
}
```

In Hearth there is no `init()` and no registration. Define the hook and the
engine finds it, searching the room, then objects in the room, then your
inventory, then globals:

`world/village/lamp.luau`:

```lua
-- The whole LPC init()/add_action/return-1 protocol collapses to "define the
-- hook". Resolution order: room → objects in room → inventory → globals, so a
-- lamp you are carrying shadows a global `polish` without any registration.
local str = require("str")

function cmd_polish(this, actor, room, args)
  local what = str.trim(args or "")
  if what ~= "" and not match_name(this.display_name, what) then
    emit(actor, "You can't polish that.")
    return
  end
  local rubs = (get_attr(this, "rubs") or 0) + 1
  set_attr(this, "rubs", rubs)

  if rubs >= 3 then
    emit(actor, "[bright_yellow]The lamp flares, and something stirs inside.[/]")
    set_attr(this, "rubs", 0)
  else
    emit(actor, "You polish the lamp. (" .. rubs .. "/3)")
  end
  emit_room(room, actor.display_name .. " polishes " .. this.display_name .. ".",
            { actor.ref_id })
end

-- `long()` with state in it. On an ITEM this adds to the default look; on a
-- room it would replace the whole rendering.
function on_look(this, actor, room)
  local rubs = get_attr(this, "rubs") or 0
  if rubs > 0 then
    emit(actor, "[dim]The brass is showing through in patches.[/]")
  end
end
```

**The `return 0` fall-through has no equivalent, and mostly doesn't need one.**
LPC needed it because every object in the room registered the same verb and they
had to negotiate. Here exactly one object answers a command — the first in
resolution order defining that hook — so a command that doesn't apply should
say so, as above, rather than silently declining.

---

## A soul

The soul — `smile`, `bow`, `grin at bob` — is the most-copied file in the LPMUD
world, and a good demonstration of the pattern: one global object, a data table
of emotes, and one hook doing the work for all of them.

`world/std/soul.luau`:

```lua
-- The mudlib soul as a table plus one dispatcher. In LPC this was a soul object
-- everyone inherited, with an add_action per verb; here the verbs are data and
-- there is a single cmd_ hook, so adding an emote is a one-line edit.
local str = require("str")

-- self = what you see, room = what others see, at_* = the targeted forms.
local EMOTES = {
  smile = {
    self = "You smile.",
    room = "$N smiles.",
    at_self = "You smile at $T.",
    at_room = "$N smiles at $T.",
  },
  grin  = {
    self = "You grin evilly.",
    room = "$N grins evilly.",
    at_self = "You grin evilly at $T.",
    at_room = "$N grins evilly at $T.",
  },
  bow   = {
    self = "You bow deeply.",
    room = "$N bows deeply.",
    at_self = "You bow to $T.",
    at_room = "$N bows to $T.",
  },
  cackle = {
    self = "You cackle with glee.",
    room = "$N cackles with glee.",
  },
}

local function expand(template, actor_name, target_name)
  local out = string.gsub(template, "%$N", actor_name)
  out = string.gsub(out, "%$T", target_name or "someone")
  return out
end

-- `present(str, environment(this_player()))`, essentially. find_in_room does
-- the name matching; this only adds "must be someone you can emote at".
local function find_target(room, name)
  if not room or name == "" then return nil end
  local obj = find_in_room(room, name)
  if obj and (is_player(obj) or is_npc(obj)) then return obj end
  return nil
end

local function perform(verb, this, actor, room, args)
  local emote = EMOTES[verb]
  if not emote then return end
  local want = str.trim(args or "")

  if want == "" then
    emit(actor, emote.self)
    emit_room(room, expand(emote.room, actor.display_name), { actor.ref_id })
    return
  end

  if not emote.at_self then
    emit(actor, "You can't " .. verb .. " at someone.")
    return
  end

  local target = find_target(room, want)
  if not target then
    emit(actor, "There is no '" .. want .. "' here.")
    return
  end
  if target.ref_id == actor.ref_id then
    emit(actor, "You " .. verb .. " at yourself.")
    return
  end

  emit(actor, expand(emote.at_self, actor.display_name, target.display_name))
  emit(target, expand(emote.at_room, actor.display_name, "you"))
  emit_room(room, expand(emote.at_room, actor.display_name, target.display_name),
            { actor.ref_id, target.ref_id })
end

-- One hook per verb, each a one-liner delegating to the shared dispatcher.
-- (A hook must be a named top-level function for the engine to derive it, so
-- these can't be generated in a loop — that's the one place the data-driven
-- approach still costs a line.)
function cmd_smile(this, actor, room, args)  perform("smile", this, actor, room, args)  end
function cmd_grin(this, actor, room, args)   perform("grin", this, actor, room, args)   end
function cmd_bow(this, actor, room, args)    perform("bow", this, actor, room, args)    end
function cmd_cackle(this, actor, room, args) perform("cackle", this, actor, room, args) end

-- `souls` / `emotes`: list what's available.
function cmd_emotes(this, actor, room, args)
  local names = {}
  for verb, _ in pairs(EMOTES) do table.insert(names, verb) end
  table.sort(names)
  local out = {}
  for _, n in ipairs(names) do
    table.insert(out, "[cmd=" .. n .. "]" .. n .. "[/cmd]")
  end
  emit(actor, "Emotes: " .. table.concat(out, ", "))
end
```

> **The one wart.** The engine derives an object's hooks by *parsing* its script
> (`derive_hooks`), and it recognises exactly two forms, both of which must be
> **statically present at the top level** of the chunk:
>
> ```lua
> function cmd_smile(this, actor, room, args) ... end   -- declaration
> cmd_smile = function(this, actor, room, args) ... end -- assignment
> ```
>
> So what you cannot do is generate them:
>
> ```lua
> for verb, _ in pairs(EMOTES) do          -- NOT detected: inside a loop, and
>   _G["cmd_" .. verb] = function(...) end -- the name isn't static
> end
> ```
>
> The parser sees no `cmd_smile`, the engine registers no hook, and `smile`
> answers "Huh?" — with no error anywhere, because nothing failed. Hence the row
> of one-line wrappers. Everything else stays data-driven: adding an emote is one
> table entry plus one wrapper line.

---

## Daemons

An LPC daemon is a single persistent object other code calls into —
`/daemon/weather`, `/daemon/channel`, `/daemon/quest`. In Hearth that is a
`Kind::Code` object tagged `system:global`.

The difference is how you *reach* it. LPC does
`"/daemon/quest"->query_completed(this_player(), "dragon")`, a synchronous call.
Here you either read its attrs directly, or — better — put the logic in a
`require()`able module and let every caller run it locally.

`world/std/quest.luau`:

```lua
-- A quest daemon. State lives in attrs on this object and on the player, so
-- any other script can read it with get_attr — no call_other needed.
local text = require("text")

local QUESTS = {
  dragon = { title = "Slay the dragon", points = 50 },
  rats   = { title = "Clear the cellar", points = 5 },
}

local function completed(who)
  return get_attr(who, "quests_done") or {}
end

function cmd_quests(this, actor, room, args)
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local done = completed(actor)
  local rows = {}
  local total = 0
  local names = {}
  for id, _ in pairs(QUESTS) do table.insert(names, id) end
  table.sort(names)
  for _, id in ipairs(names) do
    local q = QUESTS[id]
    local got = done[id] and "[green]done[/]" or "[dim]open[/]"
    if done[id] then total = total + q.points end
    table.insert(rows, { id, q.title, tostring(q.points), got })
  end
  emit(actor, fmt.header("Quests"))
  emit(actor, fmt.table(rows, { "Id", "Title", "Points", "Status" }))
  emit(actor, "Score: [b]" .. total .. "[/b]")
end

-- Called by game code via trigger(daemon, "on_complete", {who=..., quest=...}).
-- This is what trigger is FOR — notifying, not querying.
function on_complete(this, actor, room, data)
  if not data or not data.who or not QUESTS[data.quest] then return end
  local done = completed(data.who)
  if done[data.quest] then return end
  done[data.quest] = true
  set_attr(data.who, "quests_done", done)
  emit(data.who, "[bright_green]Quest complete: " ..
                 QUESTS[data.quest].title .. "[/] (+" ..
                 QUESTS[data.quest].points .. " points)")
end

-- A testing/immortal hook standing in for whatever completes the quest.
function cmd_finish(this, actor, room, args)
  local id = args and args:lower() or ""
  if not QUESTS[id] then
    emit(actor, "No such quest: " .. id)
    return
  end
  trigger(this, "on_complete", { who = actor.ref_id, quest = id }, actor)
  emit(actor, "Reporting " .. id .. "...")
end
```

Note `cmd_finish` uses `trigger` and then says "Reporting…" rather than
announcing completion itself — because the triggered hook runs *after* this
batch commits. Writing it as though `trigger` returned is the single most
common LPC-to-Hearth mistake.

---

## heart_beat and reset

`set_heart_beat(1)` plus `heart_beat()` becomes a `tick_interval` attr plus
`on_tick`. The `state` table is per-object working memory that survives between
ticks and restarts but never shows up in `examine` — LPC's private variables,
essentially.

```lua
function on_tick(this, state, room)
  state.beats = (state.beats or 0) + 1
  if state.beats % 10 ~= 0 then return end
  if not room then return end
  emit_room(room, "[dim]The brazier gutters.[/]")
end
```

```toml
[objects.attrs]
tick_interval = 2     # every 2 ticks rather than every one
```

**`reset()` has no engine equivalent.** LPMUD's driver called `reset()`
periodically to re-stock rooms; Hearth's world is persistent and nothing
repopulates it unless you say so. That is the same gap Diku's zone resets fall
into, and the [Diku cookbook's reset recipe](diku-cookbook.md#zone-resets) is
the answer — a declarative reset table plus an idempotent driver. The `max`
check there is what stops a repop stacking, which LPC's `reset()` had to
open-code in every room:

```c
/* LPC: the shape everyone wrote, badly, in every room */
void reset(int arg) {
    if (!arg) return;
    if (!present("orc")) move_object(clone_object("/mob/orc"), this_object());
}
```

---

## What doesn't translate

**Shadows.** Covered above. The intent model exists so that one object cannot
silently rewrite another's behavior.

**Multiple inheritance.** Archetypes are a single chain; `require()` covers the
mixin case.

**Wizard file directories and `update`.** There is no `/w/<name>/` realm and no
per-file recompile command. In-game authoring goes to the **database**, not to
files — a builder creates scripts and `require()`able libraries through the web
builder or `@program`, with versioning, three-way merge and edit locks. Files in
the game directory are image content, read-only at runtime.

**The master object and `valid_read`/`valid_write`.** Security is account
[scopes](softcode-guide.md#locks) (`player` / `builder` / `admin`) plus the lock
DSL and the `system:locked` tier, not an LPC object you can edit.

**`sscanf` and LPC's parser.** Commands get a raw `args` string; parse it with
Lua patterns. There is no `parse_command` verb grammar.

**Synchronous cross-object calls.** The first section. Worth repeating because
it is the one thing that will make you write code that looks right and silently
does nothing.

---

## Where to go next

- [Softcode guide](softcode-guide.md) — the full API reference
- [Archetypes](archetypes.md) — `inherit`, as live delegation
- [Diku cookbook](diku-cookbook.md) — its zone-reset recipe is your `reset()`
- [MUSH cookbook](mush-cookbook.md) — its BBS and `+mail` drop in unchanged
