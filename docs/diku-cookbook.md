# DikuMUD cookbook — resets, spec_procs and mobprogs in Hearth

DikuMUD and its descendants — Merc, ROM, Circle, Smaug — are where most of the
world's MUD content lives. If you have `.wld`/`.mob`/`.obj`/`.zon` files, a
shelf of spec_procs, or a head full of DG Scripts, this document shows what
those become in Hearth.

The translation is closer than the [MUSH one](mush-cookbook.md), because Diku's
object model is already Hearth's: rooms, mobiles and objects are all records
with flags and numbered fields, wired together by an area file. What changes is
that the parts Diku implements in **C** — and which therefore required a
recompile — are ordinary softcode here.

- New to Hearth softcode? Read [the softcode guide](softcode-guide.md) first.
- The recipes here are executable: `tests/cookbook.rs` extracts every
  path-labelled script below and runs it against the real session handler.

## Contents

- [Coming from Diku](#coming-from-diku) — the two layers, and the concept map
- [DG Scripts and MobProgs](#dg-scripts-and-mobprogs) — trigger → hook
- [Zone resets](#zone-resets) — the big one: repop, from scratch
- [spec_procs](#spec_procs) — cityguard, janitor, wanderer, healer
- [Shops](#shops) — the `.shp` file
- [Doors](#doors) — closed, locked, and the key
- [What doesn't translate](#what-doesnt-translate)

---

## Coming from Diku

A Diku codebase is two very different things wearing one name, and they
translate differently.

**The C hardcode** — combat rounds, class and level tables, spell tables,
`spec_proc`s, shop handling, the reset engine. Changing any of it meant editing
C and recompiling. In Hearth all of this is softcode. That is the single biggest
difference, and it's why this document has recipes for things Diku considered
part of the server.

**The data layer** — your area files. This maps almost directly onto Hearth's
area TOML.

### Concept map

| Diku / Merc / ROM / Smaug | Hearth |
|---|---|
| `.wld` room, `vnum` | a `[[rooms]]` entry; identity is the **file key** `area/key` |
| `.mob` mobile | `[[objects]]` with `kind = "npc"` |
| `.obj` object | `[[objects]]` with `kind = "item"` |
| mob/obj **prototype** loaded by vnum | an [archetype](archetypes.md) + `clone_object` |
| `value[0]`..`value[3]` on an item type | named attrs, with an `attr_schema` to type them |
| room `sector_type` | an attr (`sector = "forest"`) or a tag (`env:outdoor`) |
| room flags (`DARK`, `NO_MOB`, `SAFE`) | tags (`room:dark`, `room:safe`) |
| `.zon` reset list (`M`/`O`/`G`/`E`/`P`/`D`) | **you build it** — see [Zone resets](#zone-resets) |
| `.shp` shop file | a shopkeeper's object script — see [Shops](#shops) |
| `spec_proc` on a mob | that mob's own object script |
| MobProgs (`>greet_prog`, …) | hooks — see below |
| DG Scripts triggers | hooks — see below |
| door `EX_CLOSED` / `EX_LOCKED` + key vnum | exit attrs + `can_traverse` — see [Doors](#doors) |
| `act()` to room, `send_to_char` | `emit_room(room, …)`, `emit(actor, …)` |
| `WAIT_STATE` | the `movement_blocked` attr, or a cooldown on `get_tick` |
| `affect` with a duration | an attr plus `after(ticks, ref, "on_expire")` |
| `number_range(1, 6)` | `random.roll(6)`, or `seed_random` when you want it testable |
| `wizlock`, `@` immortal commands | account **scopes** (`perm(builder)`, `perm(admin)`) |

### vnums become file keys

Diku identity is a global integer. Hearth identity is `area/key` — the object's
`_file_key`. Two consequences worth internalising:

- **You don't allocate number ranges.** `midgaard/temple` is the identity, and
  areas can't collide.
- **Renaming a key orphans the object** and builds a duplicate, exactly like
  changing a vnum would. That's what [migrations](migrations.md) are for; a
  rename is a tracked `hearth migrate` op, not a silent edit.

### Prototypes become archetypes

Diku's "load mob 3001" instantiates a prototype. Hearth's equivalent is an
[archetype](archetypes.md) — a real object that others delegate to — plus
`clone_object`:

```toml
# The prototype. Nothing is "in the world" at this key; it's the template.
[[objects]]
key = "guard_proto"
kind = "npc"
title = "a city guard"
description = "A bored-looking man in the city livery."
tags = ["mob:aggressive"]
script = "guard.luau"

[objects.attrs]
hp = 40
damage = 6
```

```lua
local ref = clone_object(resolve_key("midgaard/guard_proto"), { location = room })
```

Unlike Diku, the clone keeps a live link to its prototype: fix the script on
`guard_proto` and every guard already in the world gets the fix on
`@reload-world`. That is the thing `#define`-and-recompile could never do.

---

## DG Scripts and MobProgs

CircleMUD's DG Scripts and Merc/Smaug's MobProgs are already softcode, so this
is the most mechanical translation in the document. A trigger becomes a hook;
the object the trigger is attached to becomes the object whose script defines
that hook.

### Trigger → hook

| DG Scripts / MobProg | Hearth hook | Notes |
|---|---|---|
| `greet` / `>greet_prog` | `on_enter` | On the mob or the room |
| `entry` | `on_enter` on the room | |
| `command` / `>speech_prog` with a verb | `cmd_<verb>` | Real command dispatch, not pattern matching |
| `speech` / `>speech_prog` | `on_say` | Read the text from the `_say_message` attr |
| `act` | `on_emote` | |
| `death` / `>death_prog` | `on_death` | **The engine never fires this** — see below |
| `fight` / `hitprcnt` | your combat loop's own `trigger()` | |
| `random` / `>rand_prog` | `on_tick` + a chance roll | |
| `load` | `on_create` | |
| `get` / `drop` / `give` | `on_get` / `on_drop` / `on_receive` | |
| `bribe` / `>bribe_prog` | `on_receive` | Check what arrived |
| `leave` | `on_leave` | |
| `door` | `can_traverse` on the exit | |
| `zone-reset` | your repop driver — see [Zone resets](#zone-resets) | |

### Variable → value

| DG Scripts | Hearth |
|---|---|
| `%actor%` | the `actor` parameter |
| `%self%` | the `this` parameter |
| `%actor.name%` | `actor.display_name` |
| `%actor.vnum%` | `actor.ref_id` |
| `%actor.varexists(x)%` | `has_attr(actor, "x")` |
| `%actor.x%` / `set x` | `get_attr(actor, "x")` / `set_attr(actor, "x", v)` |
| `wait 2 sec` | `after(2, this, "on_resume")` — see the note below |
| `%send% %actor% text` | `emit(actor, "text")` |
| `%echoaround% %actor% text` | `emit_room(room, "text", {actor.ref_id})` |
| `%teleport% %actor% room` | `move_object(actor, ref)` |
| `%purge% %self%` | `destroy(this)` |
| `%load% mob 3001` | `clone_object(resolve_key("area/proto"), {...})` |
| `eval`, `nop`, `%random.4%` | ordinary Lua |

**`wait` is the one real shape change.** A DG Script blocks mid-script and
resumes. A Hearth hook runs to completion and its whole batch applies
atomically — there is no yielding. A delayed second half becomes a second hook:

```lua
-- DG:  say Ah, a visitor.
--      wait 3 sec
--      say Do come in.
function on_enter(this, actor, room)
  emit_room(room, this.display_name .. ' says, "Ah, a visitor."')
  after(3, this, "on_followup")
end

function on_followup(this, actor, room, data)
  local here = get_location(this)
  if here then
    emit_room(here, this.display_name .. ' says, "Do come in."')
  end
end
```

Note `on_followup` re-reads its location rather than trusting a captured one:
three seconds is long enough for the mob to have been moved or the player to
have left. Diku's `wait` had the same hazard and most MobProgs ignored it.

### `on_death` and `on_damage` are yours to fire

Both are known hook names, but **the engine never fires them** — it has no
combat system, because Hearth is a framework and combat is a game rule. Your
combat softcode fires them:

```lua
local function apply_damage(target, amount, attacker)
  local hp = (get_attr(target, "hp") or 0) - amount
  set_attr(target, "hp", hp)
  trigger(target, "on_damage", { amount = amount, from = attacker.ref_id }, attacker)
  if hp <= 0 then
    trigger(target, "on_death", { killer = attacker.ref_id }, attacker)
  end
end
```

The fourth argument to `trigger` fires the hook **as** a chosen actor, which is
how a `death_prog` gets a meaningful `actor` (the killer) instead of the corpse.

---

## Zone resets

**This is the recipe with no Hearth equivalent to lean on.** Diku's whole
content model assumes a repop: mobs come back, chests refill, doors re-close.
Hearth has no reset system at all — the world is persistent and stays however
you left it. So you build one, and the good news is that a `.zon` file is a
declarative list, which means the Hearth version is a data table plus a small
driver.

### The Diku reset commands

| Cmd | Meaning | Here |
|---|---|---|
| `M` | load mobile into room, if fewer than N exist | `op = "mob"` |
| `O` | load object into room | `op = "obj"` |
| `G` | give object to the last-loaded mob | `op = "give"` |
| `E` | equip the last-loaded mob | `op = "give"` + a `worn` attr |
| `P` | put object in a container | `op = "put"` |
| `D` | set a door's state | `op = "door"` |

Diku's `if_flag` (0 = always, 1 = only if the previous command ran) and its
"max in world/room" counts exist to stop a repop stacking twenty guards in the
square. That's the part you must not skip.

The scheme below counts by tag: everything a reset spawns is tagged
`spawn:<zone-key>`, so the driver can count what's already alive and top up to
`max` rather than blindly loading.

`world/midgaard/zone.luau`:

```lua
-- A Diku .zon file, as data plus a driver.
--
-- Each entry in the `resets` attr is one reset command. The driver tops each
-- one up to its `max` (Diku's "load if fewer than N exist"), so running it
-- twice does not double the population — the property that makes a repop safe
-- to run on a timer.
local str = require("str")

-- Everything a reset spawns carries this tag category, so the driver can count
-- and clean up its own population without touching player-made objects.
local function spawn_tag(this, id)
  return "spawn:" .. this.key .. "_" .. id
end

local function living(this, id)
  local found = {}
  for _, obj in ipairs(find_by_tag(spawn_tag(this, id))) do
    -- A corpse or a destroyed object is gone from the world already; anything
    -- still present counts against the max.
    if exists(obj) then
      table.insert(found, obj)
    end
  end
  return found
end

-- `M` and `O`: clone a prototype into a room, up to `max` of them.
local function reset_load(this, r, id)
  local proto = resolve_key(r.proto)
  local room = resolve_key(r.room)
  if not proto then
    log("zone " .. this.key .. ": unknown proto " .. tostring(r.proto))
    return nil
  end
  if not room then
    log("zone " .. this.key .. ": unknown room " .. tostring(r.room))
    return nil
  end

  local have = #living(this, id)
  local want = r.max or 1
  local last = nil
  for _ = have + 1, want do
    last = clone_object(proto, { location = room })
    set_tag(last, spawn_tag(this, id))
  end
  return last
end

-- `G` / `E`: give a cloned object to a mob. `into` may be any ref.
local function reset_give(this, r, id, into)
  local proto = resolve_key(r.proto)
  if not proto or not into then return nil end
  if #living(this, id) > 0 then return nil end   -- already carried
  local obj = clone_object(proto, { location = into })
  set_tag(obj, spawn_tag(this, id))
  if r.worn then set_attr(obj, "worn", r.worn) end
  return obj
end

-- `D`: force a door back to a known state, exactly as Diku's D command did.
-- The door itself — its gate script and `is_door`/`key_tag` — is declared on
-- the exit in area TOML; this only resets what players change.
local function reset_door(this, r)
  local room = resolve_key(r.room)
  if not room then
    log("zone " .. this.key .. ": unknown door room " .. tostring(r.room))
    return
  end
  for _, exit in ipairs(get_exits(room)) do
    if exit.key == r.exit then
      set_attr(exit, "closed", r.state ~= "open")
      set_attr(exit, "locked", r.state == "locked")
    end
  end
end

-- Run the whole reset list. Returns how many objects it created.
local function run_resets(this)
  local resets = get_attr(this, "resets") or {}
  local before = 0
  local previous = nil   -- the last mob loaded, for `give` (Diku's G/E)

  for id, r in ipairs(resets) do
    if r.op == "mob" or r.op == "obj" then
      local made = reset_load(this, r, id)
      if r.op == "mob" and made then previous = made end
      if made then before = before + 1 end
    elseif r.op == "give" then
      local target = r.to and resolve_key(r.to) or previous
      if reset_give(this, r, id, target) then before = before + 1 end
    elseif r.op == "put" then
      local into = resolve_key(r.into)
      if into and reset_give(this, r, id, into) then before = before + 1 end
    elseif r.op == "door" then
      reset_door(this, r)
    else
      log("zone " .. this.key .. ": unknown reset op " .. tostring(r.op))
    end
  end
  return before
end

-- Diku repops a zone every `lifespan` minutes. Here: every `interval` ticks.
function on_tick(this, state, room)
  local interval = get_attr(this, "interval") or 300
  state.n = (state.n or 0) + 1
  if state.n % interval ~= 0 then return end
  local made = run_resets(this)
  if made > 0 then
    log("zone " .. this.key .. ": repop created " .. made .. " object(s)")
  end
end

-- Populate on boot, so a fresh database comes up with a stocked world.
function on_startup(this, _actor, room)
  run_resets(this)
end

-- The immortal `repop` command. Diku had one; you want one, because waiting
-- five minutes to see whether your reset list is right is miserable.
function cmd_repop(this, actor, room, args)
  local made = run_resets(this)
  emit(actor, "Zone [b]" .. this.key .. "[/b] reset: " .. made .. " object(s) loaded.")
end

-- Count what the zone currently owns — Diku's `zstat`.
function cmd_zstat(this, actor, room, args)
  local resets = get_attr(this, "resets") or {}
  emit(actor, "[b]Zone " .. this.key .. "[/b] — " .. #resets .. " reset(s)")
  for id, r in ipairs(resets) do
    local n = #living(this, id)
    emit(actor, string.format("  %2d  %-5s %-28s %d/%d",
      id, r.op, r.proto or r.exit or "-", n, r.max or 1))
  end
end
```

And the `.zon` file itself, as TOML:

```toml
[[objects]]
key = "zone"
kind = "code"
title = "Midgaard Zone"
tags = ["system:global", "system:hidden"]
script = "zone.luau"

[objects.attrs]
interval = 300            # ticks between repops (Diku's lifespan)

# M 3060 2 3001  — up to 2 guards in the square
[[objects.attrs.resets]]
op = "mob"
proto = "midgaard/guard_proto"
room = "midgaard/square"
max = 2

# G 3062 — give each guard a sword
[[objects.attrs.resets]]
op = "give"
proto = "midgaard/sword_proto"
worn = "wielded"

# O 3010 1 3005 — a fountain in the square
[[objects.attrs.resets]]
op = "obj"
proto = "midgaard/bread_proto"
room = "midgaard/square"
max = 3

# D 3005 1 1 — the storeroom door goes back to closed at repop
[[objects.attrs.resets]]
op = "door"
room = "midgaard/shop"
exit = "north"
state = "closed"
```

**The `max` check is what makes this safe.** `reset_load` counts what's alive
and tops up, so `repop` is idempotent: run it ten times and you still have two
guards. Diku got this wrong often enough that "the zone stacked" is a folk
memory; here it's one `#living()` call.

**What's deliberately missing.** Diku's `E` (equip) sets a wear slot, and
Hearth has no equipment model — that's a game rule, not a framework one. The
recipe stores a `worn` attr and leaves the meaning to your combat code. If you
want real slots, an [attr schema](softcode-guide.md) on your weapon archetype
is where the typed field belongs.

---

## spec_procs

A `spec_proc` was a C function bound to a mob by vnum, dispatched every pulse
and on every command in the room. In Hearth it is just that mob's script, which
means it can be edited from inside the game and doesn't need a recompile.

### The cityguard

Diku's cityguard attacks flagged criminals and breaks up fights. The Hearth
version below is the interesting half — recognising a wanted player on sight and
refusing to let them pass — because it shows the two-object split that movement
gating requires.

`world/midgaard/guard.luau`:

```lua
-- Diku's cityguard spec_proc. Two behaviors:
--   1. React when a wanted player walks in (the `greet` half).
--   2. Refuse to let them leave by the guarded exit.
--
-- (2) is why this recipe exists: an NPC cannot veto movement directly. The
-- exit owns `can_traverse`, so the guard publishes its intent as state — a
-- `guarded_by` attr on the exit — and the exit's own hook reads it.
local WANTED = "law:wanted"

local function guarded_exits(room)
  local out = {}
  for _, exit in ipairs(get_exits(room)) do
    if get_attr(exit, "guarded") then table.insert(out, exit) end
  end
  return out
end

function on_enter(this, actor, room)
  if not is_player(actor) then return end
  if has_tag(actor, WANTED) then
    emit_room(room, this.display_name .. " levels a spear at " ..
                    actor.display_name .. ". [red]\"You! Stand where you are.\"[/]")
  else
    emit(actor, this.display_name .. " nods to you as you pass.")
  end
end

-- Diku's cityguard also stopped fights. Here that's a command anyone can see
-- the effect of, which makes it testable.
function cmd_surrender(this, actor, room, args)
  if not has_tag(actor, WANTED) then
    emit(actor, this.display_name .. ' says, "You have done nothing, citizen."')
    return
  end
  unset_tag(actor, WANTED)
  emit(actor, this.display_name .. ' says, "Sensible. The matter is closed."')
  emit_room(room, actor.display_name .. " surrenders to " .. this.display_name .. ".",
            { actor.ref_id })
end

-- An immortal/testing hook to flag someone, standing in for whatever your
-- game's crime system is.
function cmd_outlaw(this, actor, room, args)
  set_tag(actor, WANTED)
  emit(actor, "[red]You are now wanted in Midgaard.[/]")
end
```

The gate itself lives on the exit, where `can_traverse` belongs:

`world/midgaard/gate.luau`:

```lua
-- On the EXIT. `can_traverse` is the exit's hook (`this` is the exit) and
-- pairs with the exit's `traverse` lock; `can_enter` is the destination
-- room's. A guarded exit refuses a wanted actor, but only while a living
-- guard is actually standing there — kill the guard and the road opens.
function can_traverse(this, actor, room)
  if not has_tag(actor, "law:wanted") then return true end

  local watcher = nil
  for _, obj in ipairs(get_room_contents(room)) do
    if is_npc(obj) and has_tag(obj, "mob:guard") then watcher = obj end
  end
  if not watcher then return true end

  emit(actor, watcher.display_name .. " bars the way. [red]\"Not you.\"[/]")
  emit_room(room, watcher.display_name .. " blocks " .. actor.display_name ..
                  " from leaving.", { actor.ref_id })
  return false
end
```

> **Why two objects?** An NPC has no way to veto a move — the engine asks the
> **exit** (`can_traverse`) and the **destination room** (`can_enter`), and
> nothing else. So mob-driven movement gating is always "the mob is state, the
> exit is the gate". This is more flexible than Diku's version, where the
> block was hardcoded into the guard's C function: here the same exit script
> works for a guard, a portcullis, or a sleeping dragon.

### The janitor

Diku's janitor wanders picking up trash. The Hearth version is an `on_tick`.

`world/midgaard/janitor.luau`:

```lua
-- Diku's janitor spec_proc: every pulse, pick up one worthless item lying
-- in the room. `tick_interval` on the object controls how often this runs.
local function is_trash(obj)
  if not is_item(obj) then return false end
  if has_tag(obj, "item:nosalvage") then return false end
  return (get_attr(obj, "value") or 0) <= 0
end

function on_tick(this, state, room)
  if not room then return end
  for _, obj in ipairs(get_room_contents(room)) do
    if is_trash(obj) then
      emit_room(room, this.display_name .. " picks up " .. obj.display_name .. ".")
      move_object(obj, this.ref_id)
      return   -- one per tick, like the original
    end
  end
end

-- Diku's janitor was also a mobile. Wandering is the `random` trigger:
-- a chance roll on the tick, then an ordinary move.
function cmd_sweep(this, actor, room, args)
  local held = get_inventory(this)
  if #held == 0 then
    emit(actor, this.display_name .. ' mutters, "Nothing to sweep."')
    return
  end
  local names = {}
  for _, o in ipairs(held) do table.insert(names, o.display_name) end
  emit(actor, this.display_name .. " is carrying: " .. table.concat(names, ", "))
end
```

### The wanderer (`puff`)

The `random` trigger, in its most familiar form. Note the use of seeded RNG so
the behavior is reproducible in a test.

```lua
function on_tick(this, state, room)
  if not room then return end
  -- Roughly one move in ten.
  if seed_random(hash_seed(this.ref_id, get_tick), 1, 10) ~= 1 then return end

  local exits = get_exits(room)
  if #exits == 0 then return end
  local exit = seed_choice(hash_seed(this.ref_id, get_tick, "dir"), exits)
  local dest = exit.target_ref
  if not dest then return end

  emit_room(room, this.display_name .. " wanders " .. exit.key .. ".")
  move_object(this, dest)
  emit_room(dest, this.display_name .. " arrives.")
end
```

---

## Shops

A `.shp` file is a keeper vnum, a list of vnums they trade in, a buy multiplier,
a sell multiplier, and opening hours. All of that is attrs on the keeper, and
the four verbs are `cmd_` hooks on the keeper's own script — so unlike Diku,
where shop handling lived in `shop.c`, a shopkeeper here can be given quirks
without touching anything else.

`world/midgaard/shop.luau`:

```lua
-- The Diku .shp file, as a keeper script.
--
--   profit_buy   what the keeper charges, as a multiple of base value
--   profit_sell  what the keeper pays, as a multiple of base value
--   trades       tag the keeper will deal in
--   open / close in-world hours (omit for always open)
local text = require("text")
local str = require("str")

local function currency(this) return get_attr(this, "currency") or "coins" end

local function is_open(this)
  local t = get_time()
  local open, close = get_attr(this, "open"), get_attr(this, "close")
  if not t or not open or not close then return true end
  return t.hour >= open and t.hour < close
end

local function closed_msg(this, actor)
  emit(actor, this.display_name .. ' says, "We are closed. Come back later."')
end

-- What the keeper has for sale: their own inventory, filtered to tradeables.
local function stock(this)
  local out = {}
  local trades = get_attr(this, "trades")
  for _, obj in ipairs(get_inventory(this)) do
    if not trades or has_tag(obj, trades) then table.insert(out, obj) end
  end
  return out
end

local function buy_price(this, obj)
  local base = get_attr(obj, "value") or 1
  return math.max(1, math.floor(base * (get_attr(this, "profit_buy") or 1.5)))
end

local function sell_price(this, obj)
  local base = get_attr(obj, "value") or 1
  return math.max(1, math.floor(base * (get_attr(this, "profit_sell") or 0.5)))
end

function cmd_list(this, actor, room, args)
  if not is_open(this) then return closed_msg(this, actor) end
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local items = stock(this)
  if #items == 0 then
    emit(actor, this.display_name .. ' says, "I have nothing to sell today."')
    return
  end
  local rows = {}
  for _, obj in ipairs(items) do
    table.insert(rows, {
      "[cmd=buy " .. obj.key .. "]" .. obj.key .. "[/cmd]",
      obj.display_name,
      buy_price(this, obj) .. " " .. currency(this),
    })
  end
  emit(actor, fmt.header(this.display_name))
  emit(actor, fmt.table(rows, { "Order", "Item", "Price" }))
end

function cmd_buy(this, actor, room, args)
  if not is_open(this) then return closed_msg(this, actor) end
  local want = str.trim(args or "")
  if want == "" then
    emit(actor, "Buy what? Try [cmd=list]list[/cmd].")
    return
  end

  local found
  for _, obj in ipairs(stock(this)) do
    if match_name(obj.display_name, want) or obj.key == want then found = obj end
  end
  if not found then
    emit(actor, this.display_name .. ' says, "I do not stock that."')
    return
  end

  local price = buy_price(this, found)
  local coin = currency(this)
  local balance = get_attr(actor, coin) or 0
  if balance < price then
    emit(actor, this.display_name .. ' says, "You cannot afford it." (' ..
                price .. " " .. coin .. ")")
    return
  end

  -- Atomic: validated, and rolls the whole batch back if the buyer is short,
  -- so goods and payment can never come apart. Diku did this by hand and got
  -- it wrong in at least three derivatives.
  transfer_attr(actor, this, coin, price)
  move_object(found, actor.ref_id)

  emit(actor, "You buy " .. found.display_name .. " for " .. price .. " " .. coin .. ".")
  emit_room(room, actor.display_name .. " buys " .. found.display_name ..
                  " from " .. this.display_name .. ".", { actor.ref_id })
end

function cmd_sell(this, actor, room, args)
  if not is_open(this) then return closed_msg(this, actor) end
  local want = str.trim(args or "")
  local trades = get_attr(this, "trades")

  local found
  for _, obj in ipairs(get_inventory(actor)) do
    if match_name(obj.display_name, want) or obj.key == want then found = obj end
  end
  if not found then
    emit(actor, "You aren't carrying that.")
    return
  end
  if trades and not has_tag(found, trades) then
    emit(actor, this.display_name .. ' says, "I do not deal in such things."')
    return
  end

  local price = sell_price(this, found)
  local coin = currency(this)
  if (get_attr(this, coin) or 0) < price then
    emit(actor, this.display_name .. ' says, "I have not the coin for that."')
    return
  end

  transfer_attr(this, actor, coin, price)
  move_object(found, this.ref_id)
  emit(actor, "You sell " .. found.display_name .. " for " .. price .. " " .. coin .. ".")
end

function cmd_value(this, actor, room, args)
  local want = str.trim(args or "")
  for _, obj in ipairs(get_inventory(actor)) do
    if match_name(obj.display_name, want) or obj.key == want then
      emit(actor, this.display_name .. ' says, "I would give you ' ..
                  sell_price(this, obj) .. " " .. currency(this) .. ' for that."')
      return
    end
  end
  emit(actor, "You aren't carrying that.")
end
```

---

## Doors

Diku puts door state in the exit itself: `EX_CLOSED`, `EX_LOCKED`, `EX_PICKPROOF`
and a key vnum. Hearth does the same — attrs on the exit — but getting them
*there* runs into two engine facts that together shape the whole recipe.

**1. `cmd_` dispatch does not reach exits.** The engine's candidate list is the
room itself, objects in the room, the actor's inventory, and globals — and
`World::objects_in` excludes exits. So `cmd_open` cannot live on the exit.

**2. A door verb needs a target, so it can't hang off one exit anyway.** Even
setting dispatch aside, `open` is meaningless without a direction — a room has a
`north` and a `northeast`, and `cmd_open` on one of them would fire for both,
because dispatch picks the first candidate defining the hook and never looks at
the argument. A verb that takes a target belongs on something that can see every
exit.

So the split is:

- The **exit** owns the gate — `can_traverse`, reading its own attrs.
- A **global** owns the verbs — `open`, `close`, `lock`, `unlock`, resolving the
  direction through `get_exits(room)`.

One global implementation then serves every door in the game, which is closer to
Diku's engine-level doors than a script per door would be. For per-door flavor,
the global can `trigger(exit, "on_open")` once it has resolved which door you
meant — targeting without the ambiguity.

`world/midgaard/doors.luau`:

```lua
-- Diku's door commands, as one global. EX_CLOSED / EX_LOCKED / key vnum
-- become `closed` / `locked` / `key_tag` attrs on the exit.
local str = require("str")

-- Every verb needs the same preamble: name a direction, find it, and require
-- that it actually be a door rather than an open archway. `find_exit` matches
-- a direction OR an alias, exactly as movement does, so `open n` and
-- `open north` resolve identically without the command re-deriving it.
local function door_for(actor, room, args, verb)
  local name = string.lower(str.trim(args or ""))
  if name == "" then
    emit(actor, verb .. " what?")
    return nil
  end
  local exit = find_exit(room, name)
  if not exit then
    emit(actor, "There is no " .. name .. " exit here.")
    return nil
  end
  if not get_attr(exit, "is_door") then
    emit(actor, "You cannot " .. verb .. " that.")
    return nil
  end
  return exit
end

function cmd_open(this, actor, room, args)
  local exit = door_for(actor, room, args, "open")
  if not exit then return end
  if get_attr(exit, "locked") then
    emit(actor, "It is locked.")
    return
  end
  if not get_attr(exit, "closed") then
    emit(actor, "It is already open.")
    return
  end
  set_attr(exit, "closed", false)
  emit(actor, "You open the " .. exit.key .. " door.")
  emit_room(room, actor.display_name .. " opens the " .. exit.key .. " door.",
            { actor.ref_id })
end

function cmd_close(this, actor, room, args)
  local exit = door_for(actor, room, args, "close")
  if not exit then return end
  if get_attr(exit, "closed") then
    emit(actor, "It is already closed.")
    return
  end
  set_attr(exit, "closed", true)
  emit(actor, "You close the " .. exit.key .. " door.")
  emit_room(room, actor.display_name .. " closes the " .. exit.key .. " door.",
            { actor.ref_id })
end

function cmd_unlock(this, actor, room, args)
  local exit = door_for(actor, room, args, "unlock")
  if not exit then return end
  local key = get_attr(exit, "key_tag")
  if not key then
    emit(actor, "It has no lock.")
    return
  end
  if not is_carrying(actor, key) then
    emit(actor, "You don't have the key.")
    return
  end
  if not get_attr(exit, "locked") then
    emit(actor, "It is already unlocked.")
    return
  end
  set_attr(exit, "locked", false)
  emit(actor, "You unlock the " .. exit.key .. " door.")
end

function cmd_lock(this, actor, room, args)
  local exit = door_for(actor, room, args, "lock")
  if not exit then return end
  local key = get_attr(exit, "key_tag")
  if not key or not is_carrying(actor, key) then
    emit(actor, "You don't have the key.")
    return
  end
  if not get_attr(exit, "closed") then
    emit(actor, "You must close it first.")
    return
  end
  set_attr(exit, "locked", true)
  emit(actor, "You lock the " .. exit.key .. " door.")
end
```

The gate itself is one hook on the exit:

`world/midgaard/door.luau`:

```lua
-- On the EXIT. `can_traverse` is the exit's own hook (`this` is the exit),
-- pairing with the exit's `traverse` lock. A closed door refuses passage; the
-- verbs that change that live on the doors global, because a door verb needs a
-- direction argument.
function can_traverse(this, actor, room)
  if get_attr(this, "closed") then
    emit(actor, "The " .. this.key .. " door is closed.")
    return false
  end
  return true
end
```

An exit carries its own script and attrs in area TOML, so the whole door is
declared in one place — `EX_CLOSED`, `EX_LOCKED` and the key vnum, in the file:

```toml
[[exits]]
from = "shop"
direction = "north"
to = "storeroom"
script = "door.luau"

[exits.attrs]
is_door = true
closed = true
locked = true
key_tag = "key:storeroom"
```

The zone's `D` reset then only has to do what Diku's did — put an existing door
back into a known state at repop:

```toml
[[objects.attrs.resets]]
op = "door"
room = "midgaard/shop"
exit = "north"
state = "locked"
```

> Diku's `EX_PICKPROOF` and the `pick` skill are a game rule — add a
> `pickproof` attr and check it in whatever your thief code is. The framework's
> job stops at "the exit knows it is shut".

---

## What doesn't translate

Being honest about the edges, because the gaps are all in the same place — the
things Diku considered part of the server and Hearth considers part of a game.

**Combat.** There is no combat round, no THAC0, no damage table, no `violence
update` pulse. `on_damage` and `on_death` are hook *names* the engine never
fires; your softcode fires them with `trigger`. The Last Stag implements a
turn-based system entirely in Luau — that's the reference.

**Classes, levels, skills, spells.** Same story: attrs and softcode. There's no
`class_table[]` to edit because there's no class concept to begin with.

**Equipment slots.** No wear locations. An item is in your inventory or it
isn't. `worn = "wielded"` in the reset recipe is a convention your combat code
interprets, not something the engine knows.

**Rent and saving.** Diku's receptionist/rent system exists because saving was
expensive and manual. Hearth persists everything continuously and restores
players on reconnect, so the whole feature is gone rather than translated. If
you want an inn *for flavor*, write it as flavor.

**Global integer identity.** No vnums. If you're porting real areas, budget for
the identity mapping — a `vnum → area/key` table you keep — because it's the
part that can't be automated away.

**Diku's licence.** Worth stating plainly: the DikuMUD licence has terms about
credit and about not charging for access, and it has historically been read as
covering derived *code*. Concepts, mechanics and your own area content aren't
the issue; lifting Diku C source into a Luau port is a question for you and the
licence, not something this document can answer.

---

## Where to go next

- [Softcode guide](softcode-guide.md) — the full API reference
- [MUSH cookbook](mush-cookbook.md) — the other tradition; its `+mail` and BBS
  recipes drop straight into a Diku-shaped game
- [Archetypes](archetypes.md) — prototypes, done as delegation
- [Migrations](migrations.md) — renaming a file key without orphaning content
- [MudBytes](https://www.mudbytes.net/files/) — the code archive, still live
