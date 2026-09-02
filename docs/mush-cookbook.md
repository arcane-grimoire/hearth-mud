# MUSH cookbook — classic gadgets in Hearth

Twenty years of MUSH softcode produced a canon of gadgets: the BBS, the
vendor, the cron, the weather system, the vehicle, the dice roller. If you
came from PennMUSH/TinyMUX/Rhost, you already know what you want to build —
this document shows what those gadgets look like in Hearth.

Each recipe names the MUSH original, explains what changes in the translation,
and gives complete working code. The recipes are meant to be read *and*
pasted: every one is a whole object script you can drop into a `.luau` file
next to your area TOML.

- New to Hearth softcode? Read [the softcode guide](softcode-guide.md) first —
  this is a cookbook, not a reference.
- New to MUSH? You can still use this; the recipes stand on their own.

## Contents

- [Coming from MUSHcode](#coming-from-mushcode)
- [Globals](#globals) — `+finger`, `+who` / `+where`
- [Bulletin boards](#bulletin-boards) — a Myrddin-style BBS
- [Mail systems](#mail-systems) — `+mail`
- [Vendors](#vendors) — the Multi-Vendor
- [Schedulers](#schedulers) — MushCron
- [Time](#time) — Day Describer
- [Weather systems](#weather-systems) — a Keran-style weather engine
- [Vehicles](#vehicles) — a car, and an elevator
- [Games](#games) — a dice roller, and a jukebox
- [Administration](#administration) — `+jobs`
- [Building](#building) — parents become archetypes

---

## Coming from MUSHcode

Hearth is not a MUSH. It has no `@parent` chain of attribute inheritance, no
`$command` patterns, no `%0`-`%9`, no `iter()`/`switch()`/`setq()`. What it has
instead is a real programming language (Luau) running against a typed object
model. Almost every MUSH idiom maps onto something simpler.

### Concept map

| MUSH | Hearth |
|------|--------|
| `&FOO obj=bar`, `v(foo)`, `get(obj/foo)` | `set_attr(ref, "foo", "bar")`, `get_attr(ref, "foo")` |
| Attribute holding a space-delimited list | An attr holding a real Lua **table** |
| `$+order *:` command pattern on an object | `function cmd_order(this, actor, room, args)` |
| Global command in the Master Room | An object tagged `system:global` |
| `@pemit player=msg` | `emit(actor, msg)` |
| `@remit room=msg` | `emit_room(room, msg)` |
| `@oemit player=msg` | `emit_room(room, msg, {actor.ref_id})` |
| `@wait 30=@trigger me/FOO` | `after(30, this, "on_foo")` |
| `@trigger obj/CODE=args` | `trigger(ref, "on_code", { ... })` |
| `@force player=cmd`, puppets | `run_command_as(actor, "cmd")` |
| `@lock obj=FLAG^WIZARD` | `@lock <ref>/get = perm(admin)` — see the [lock DSL](softcode-guide.md#locks) |
| `@startup` | `function on_startup(this, state, room)` |
| `@aahear` / `^*:` listen patterns | `function on_say(this, actor, room)` |
| `@parent obj=template` | [Archetypes](archetypes.md) (`is-a` delegation) |
| `setq(0,...)` / `r(0)` registers | Ordinary Lua `local`s |
| `iter()`, `@dolist`, `switch()` | `for`, `if`, `table.*` |
| `%xr`, `ansi(r,...)` | BBCode: `[red]...[/]` |
| Semaphores (`@wait obj/SEM`) | An attr flag, or the per-object `state` table |
| `@clone` | `clone_object(ref, { location = ... })` |
| `@dig` / `@open` from code | `spawn{...}` / `create_exit{...}` |

### Four rules that change how you write

**1. Everything is one script per object.** MUSH scatters behavior across
dozens of attributes on one object. Hearth gives each object a *single* Luau
chunk whose top-level functions are its hooks. They share file scope, so
helpers and constants are just `local`s at the top — no `u(me/HELPER)`.

```lua
-- All of an object's behavior lives in one chunk.
local GREETING = "Welcome, traveler."

local function is_regular(actor)
  return (get_attr(actor, "tavern_visits") or 0) > 5
end

function cmd_talk(this, actor, room, args)
  emit(actor, GREETING)
  if is_regular(actor) then
    emit(actor, "The barkeep already has your usual poured.")
  end
end

function on_look(this, actor, room)
  emit(actor, is_regular(actor) and "He nods at you." or "He eyes you warily.")
end
```

**2. Writes are queued, not immediate.** Every write function pushes an Intent;
the engine validates and applies the whole batch atomically after your script
returns. If any intent fails, *all* of them roll back. `get_attr`, `has_attr`
and `pick` do see your own pending writes, so read-your-writes works — but a
`get_object()` snapshot taken before a `set_attr` will not show it.

**3. Commands are Luau identifiers.** The engine turns the typed verb into a
hook name: `order pizza` looks for `cmd_order`. That means **no `+` prefix** —
`+bbread` cannot become a hook, because `cmd_+bbread` isn't a valid function
name. Two idiomatic replacements:

- Drop the sigil: `bbread`, `bbpost`, `finger`.
- Collapse the family into one verb with a subcommand, which is usually nicer:
  `bb list`, `bb read 3`, `bb post general=Title`. The recipes below use this.

**4. Attributes hold structured data.** A MUSH BBS spends most of its code
packing and unpacking `|`-delimited strings. In Hearth a post is just a table,
and you `set_attr` the whole list. Reach for `json_encode` only when you're
talking to something outside the engine.

---

## Globals

Anything a player can type anywhere is a `system:global` object. Use
`kind = "code"` — a Code object is behavior with no physical presence, so the
engine never lists it in room contents, `look`, or inventory.

### `+finger` — player profiles

The MUSH `+finger`/`+profile` global: players set descriptive fields on
themselves, anyone can read them.

`world/system/system.toml`:

```toml
area = "system"

[[objects]]
key = "finger"
kind = "code"
title = "Finger System"
tags = ["system:global", "system:hidden"]
script = "finger.luau"
```

`world/system/finger.luau`:

```lua
local text = require("text")
local str = require("str")

-- The settable fields, in display order. Adding a field is a one-line edit.
local FIELDS = {
  { key = "fullname",  label = "Full Name" },
  { key = "position",  label = "Position" },
  { key = "apparent",  label = "Apparent Age" },
  { key = "themesong", label = "Theme Song" },
  { key = "rp_hooks",  label = "RP Hooks" },
}

local function fmt_for(actor)
  return text.for_mode(get_attr(actor, "_display_mode") or "visual")
end

local function show(actor, target)
  local fmt = fmt_for(actor)
  emit(actor, fmt.header(target.display_name))
  for _, field in ipairs(FIELDS) do
    local value = get_attr(target, "finger_" .. field.key)
    if value then
      emit(actor, fmt.stat(field.label, value, "cyan"))
    end
  end
  if target.description and target.description ~= "" then
    emit(actor, "")
    emit(actor, target.description)
  end
  emit(actor, fmt.divider(40))
end

function cmd_finger(this, actor, room, args)
  local name = str.trim(args or "")
  if name == "" then
    show(actor, actor)
    return
  end
  local target = find_player(name)
  if not target then
    emit(actor, "No player matches [b]" .. name .. "[/b].")
    return
  end
  show(actor, target)
end

-- `fset themesong=Ride of the Valkyries`  /  `fset themesong` to clear.
function cmd_fset(this, actor, room, args)
  -- The key stops at the first space or `=`. A plain `%S+` here would be
  -- greedy and swallow the separator, giving you a key of "themesong=Ride".
  local key, value = string.match(args or "", "^([^%s=]+)%s*=?%s*(.*)$")
  if not key then
    local names = {}
    for _, f in ipairs(FIELDS) do table.insert(names, f.key) end
    emit(actor, "Usage: fset <field>=<value>. Fields: " .. table.concat(names, ", "))
    return
  end
  key = string.lower(key)

  local known = false
  for _, f in ipairs(FIELDS) do
    if f.key == key then known = true end
  end
  if not known then
    emit(actor, "Unknown field [b]" .. key .. "[/b].")
    return
  end

  if str.trim(value) == "" then
    set_attr(actor, "finger_" .. key, nil)   -- nil removes the attr
    emit(actor, "Cleared [b]" .. key .. "[/b].")
  else
    set_attr(actor, "finger_" .. key, str.trim(value))
    emit(actor, "Set [b]" .. key .. "[/b] to: " .. str.trim(value))
  end
end
```

> **MUSH note.** The whole `&FINGER_THEMESONG me=...` / `u(FINGER_FORMAT)`
> dance collapses into a `FIELDS` table and a loop. To add a field you edit one
> line, not the formatter.

### `+who` and `+where`

`world/system/where.luau`:

```lua
local text = require("text")

function cmd_where(this, actor, room, args)
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local rows = {}
  for _, p in ipairs(get_all_by_kind("player")) do
    if not has_tag(p, "system:offline") then
      local loc = get_location(p)
      local place = loc and loc.display_name or "Nowhere"
      -- Rooms can opt out of the listing, the MUSH UNFINDABLE flag.
      if not (loc and has_tag(loc, "system:unfindable")) then
        table.insert(rows, { p.display_name, place })
      end
    end
  end
  table.sort(rows, function(a, b) return a[1] < b[1] end)

  emit(actor, fmt.header("Who's Where"))
  emit(actor, fmt.table(rows, { "Player", "Location" }))
  emit(actor, fmt.divider(40))
  emit(actor, #rows .. " connected.")
end
```

`get_all_by_kind("player")` returns every player object, online or not; the
`system:offline` tag is how the engine marks disconnected characters.

---

## Bulletin boards

**MUSH original:** [Myrddin's BBS](https://www.mushcode.com/File/Myrddins-BBS-v4-0-6)
— groups, posting, per-player unread tracking, read/write locks, timeouts.

Myrddin's spends enormous effort on list-packing: message headers in one
attribute, bodies in numbered attributes, unread state as a bitmask string. In
Hearth all of that is just tables. The design below keeps the ergonomics and
drops the string surgery.

**Storage:**

- The board object holds `groups` — a list of group names — plus one
  `posts_<group>` attr per group (an array of post tables). One attr per group
  means posting to `general` doesn't rewrite `staff`.
- Read state lives on the *player*: `bb_read = { general = 12, staff = 3 }`,
  the highest post number they've seen in each group.
- Write access is a per-group tag requirement, checked in code.

`world/system/bbs.toml`:

```toml
area = "system"

[[objects]]
key = "bbs"
kind = "code"
title = "Bulletin Board"
tags = ["system:global", "system:hidden"]
script = "bbs.luau"

[objects.attrs]
groups = ["general", "announcements"]
```

`world/system/bbs.luau`:

```lua
local text = require("text")
local str = require("str")

-- ---------------------------------------------------------------- helpers

local function fmt_for(actor)
  return text.for_mode(get_attr(actor, "_display_mode") or "visual")
end

local function groups(this)
  return get_attr(this, "groups") or {}
end

local function group_exists(this, name)
  for _, g in ipairs(groups(this)) do
    if g == name then return true end
  end
  return false
end

local function posts(this, group)
  return get_attr(this, "posts_" .. group) or {}
end

local function set_posts(this, group, list)
  set_attr(this, "posts_" .. group, list)
end

-- A group is writable by everyone unless it carries a required tag:
--   set_attr(bbs, "write_tag_announcements", "role:staff")
local function can_post(this, actor, group)
  local required = get_attr(this, "write_tag_" .. group)
  return required == nil or has_tag(actor, required)
end

local function read_marks(actor)
  return get_attr(actor, "bb_read") or {}
end

local function mark_read(actor, group, num)
  local marks = read_marks(actor)
  if (marks[group] or 0) < num then
    marks[group] = num
    set_attr(actor, "bb_read", marks)
  end
end

local function unread_count(this, actor, group)
  local seen = read_marks(actor)[group] or 0
  return math.max(0, #posts(this, group) - seen)
end

-- The game clock if one is configured, otherwise the tick counter. Either way
-- it's a stable stamp we can render.
-- str.wrap returns one newline-joined string; emit takes a line at a time.
local function wrapped(s, width)
  return str.split(str.wrap(s, width or 72), "\n")
end

local function stamp()
  local t = get_time()
  if t then
    return string.format("%s %d, Y%d", t.month_name or ("M" .. t.month), t.day, t.year)
  end
  return "tick " .. get_tick
end

-- ---------------------------------------------------------------- commands

local function do_list(this, actor)
  local fmt = fmt_for(actor)
  local rows = {}
  for _, g in ipairs(groups(this)) do
    local unread = unread_count(this, actor, g)
    table.insert(rows, {
      g,
      tostring(#posts(this, g)),
      unread > 0 and ("[bright_yellow]" .. unread .. " new[/]") or "-",
    })
  end
  emit(actor, fmt.header("Bulletin Boards"))
  emit(actor, fmt.table(rows, { "Group", "Posts", "Unread" }))
  emit(actor, fmt.divider(50))
  emit(actor, "[dim]bb read <group> · bb read <group>/<n> · bb post <group>=<title>[/]")
end

local function do_read(this, actor, rest)
  local fmt = fmt_for(actor)
  local group, num = string.match(rest, "^(%w+)/(%d+)$")
  if not group then group = string.match(rest, "^(%w+)$") end

  if not group or not group_exists(this, group) then
    emit(actor, "No such board. Try [cmd=bb list]bb list[/cmd].")
    return
  end

  local list = posts(this, group)

  -- No post number: index of the group.
  if not num then
    if #list == 0 then
      emit(actor, "[dim]No posts on [b]" .. group .. "[/b] yet.[/]")
      return
    end
    local seen = read_marks(actor)[group] or 0
    emit(actor, fmt.header("Board: " .. group))
    for i, post in ipairs(list) do
      local flag = i > seen and "[bright_yellow]*[/]" or " "
      emit(actor, string.format(
        "%s [b]%2d[/b]  %s  [dim]%s — %s[/]",
        flag, i, post.title, post.author, post.date))
    end
    emit(actor, fmt.divider(50))
    return
  end

  local n = tonumber(num)
  local post = list[n]
  if not post then
    emit(actor, "There is no post " .. n .. " on " .. group .. ".")
    return
  end

  emit(actor, fmt.header(post.title))
  emit(actor, "[dim]" .. group .. "/" .. n .. " by " .. post.author .. " — " .. post.date .. "[/]")
  emit(actor, "")
  for _, line in ipairs(wrapped(post.body)) do
    emit(actor, line)
  end
  emit(actor, fmt.divider(50))
  mark_read(actor, group, n)
end

local function do_post(this, actor, rest)
  local group, title = string.match(rest, "^(%w+)%s*=%s*(.+)$")
  if not group then
    emit(actor, "Usage: bb post <group>=<title>")
    return
  end
  if not group_exists(this, group) then
    emit(actor, "No such board: " .. group)
    return
  end
  if not can_post(this, actor, group) then
    emit(actor, "You don't have posting rights on [b]" .. group .. "[/b].")
    return
  end

  -- Stash the draft on the poster and arm a prompt for the body. The actor's
  -- next line of input fires on_body below instead of running as a command.
  set_attr(actor, "bb_draft_group", group)
  set_attr(actor, "bb_draft_title", str.trim(title))
  emit(actor, "[b]" .. str.trim(title) .. "[/b] — on " .. group)
  emit(actor, "[dim]Type your post as a single message. It will be posted immediately.[/]")
  prompt(actor, this, "on_body")
end

-- Fired by prompt(): the 4th parameter is the raw line the player typed.
function on_body(this, actor, room, body)
  local group = get_attr(actor, "bb_draft_group")
  local title = get_attr(actor, "bb_draft_title")
  set_attr(actor, "bb_draft_group", nil)
  set_attr(actor, "bb_draft_title", nil)

  if not group or not title then
    emit(actor, "Your draft was lost.")
    return
  end
  if str.trim(body or "") == "" then
    emit(actor, "Empty post discarded.")
    return
  end

  local list = posts(this, group)
  table.insert(list, {
    title  = title,
    author = actor.display_name,
    body   = str.trim(body),
    date   = stamp(),
    tick   = get_tick,
  })
  set_posts(this, group, list)
  mark_read(actor, group, #list)

  emit(actor, "Posted to [b]" .. group .. "[/b] as #" .. #list .. ".")
  -- Notify everyone else who's online.
  for _, p in ipairs(get_all_by_kind("player")) do
    if p.ref_id ~= actor.ref_id and not has_tag(p, "system:offline") then
      emit(p, "[dim][bright_yellow]NEW[/] " .. group .. ": " ..
               title .. " (" .. actor.display_name .. ") — " ..
               "[cmd=bb read " .. group .. "/" .. #list .. "]read[/cmd][/]")
    end
  end
end

local function do_catchup(this, actor, rest)
  local target = str.trim(rest)
  local marks = read_marks(actor)
  for _, g in ipairs(groups(this)) do
    if target == "" or target == "all" or target == g then
      marks[g] = #posts(this, g)
    end
  end
  set_attr(actor, "bb_read", marks)
  emit(actor, "Marked read.")
end

local function do_newgroup(this, actor, rest)
  local name = string.lower(str.trim(rest))
  if not string.match(name, "^%w+$") then
    emit(actor, "Group names must be a single alphanumeric word.")
    return
  end
  if group_exists(this, name) then
    emit(actor, "That board already exists.")
    return
  end
  local list = groups(this)
  table.insert(list, name)
  set_attr(this, "groups", list)
  set_posts(this, name, {})
  emit(actor, "Created board [b]" .. name .. "[/b].")
end

-- One verb, MUSH's whole `+bb*` family as subcommands.
function cmd_bb(this, actor, room, args)
  local sub, rest = string.match(str.trim(args or ""), "^(%S*)%s*(.*)$")
  sub = string.lower(sub or "")

  if sub == "" or sub == "list" then return do_list(this, actor) end
  if sub == "read"    then return do_read(this, actor, str.trim(rest)) end
  if sub == "post"    then return do_post(this, actor, str.trim(rest)) end
  if sub == "catchup" then return do_catchup(this, actor, rest) end
  if sub == "newgroup" then
    -- Builder-gated: put `@lock <bbs>/use = perm(builder)` on the object, or
    -- check a tag here. Tags are the softcode-side equivalent of a MUSH flag.
    if not has_tag(actor, "role:staff") then
      emit(actor, "Only staff can create boards.")
      return
    end
    return do_newgroup(this, actor, rest)
  end

  emit(actor, "Usage: bb [list | read <group>[/<n>] | post <group>=<title> | catchup]")
end

-- Unread nag at login. system:global objects get lifecycle hooks from
-- everywhere, so this fires wherever the player reconnects.
function on_connect(this, actor, room)
  local total = 0
  for _, g in ipairs(groups(this)) do
    total = total + unread_count(this, actor, g)
  end
  if total > 0 then
    emit(actor, "[bright_yellow]You have " .. total ..
                " unread board post(s).[/] [cmd=bb list]bb list[/cmd]")
  end
end
```

**What was lost, and why that's fine.** Myrddin's has message timeouts, moving
posts between groups, anonymous groups and a buffer-usage gauge. Those exist
because a MUSH database charged you for every attribute. Add timeouts here as a
`bb_expire` field checked in an `on_tick`; add anonymity as one more per-group
attr consulted when rendering the author.

---

## Mail systems

The `+mail` family. Same shape as the BBS, but the store lives on the
*recipient* — which means it persists with the player and needs no central
index.

`world/system/mail.luau`:

```lua
local text = require("text")
local str = require("str")

local function inbox(who)
  return get_attr(who, "mail") or {}
end

-- str.wrap returns one newline-joined string; emit takes a line at a time.
local function wrapped(s)
  return str.split(str.wrap(s, 72), "\n")
end

function cmd_mail(this, actor, room, args)
  local sub, rest = string.match(str.trim(args or ""), "^(%S*)%s*(.*)$")
  sub = string.lower(sub or "")
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local box = inbox(actor)

  if sub == "" or sub == "list" then
    if #box == 0 then
      emit(actor, "[dim]Your mailbox is empty.[/]")
      return
    end
    local rows = {}
    for i, m in ipairs(box) do
      table.insert(rows, {
        (m.read and " " or "[bright_yellow]*[/]") .. i,
        m.from,
        m.subject,
      })
    end
    emit(actor, fmt.header("Mailbox"))
    emit(actor, fmt.table(rows, { "#", "From", "Subject" }))
    return
  end

  if sub == "read" then
    local m = box[tonumber(rest)]
    if not m then emit(actor, "No such message.") return end
    m.read = true
    set_attr(actor, "mail", box)
    emit(actor, fmt.header(m.subject))
    emit(actor, "[dim]From " .. m.from .. "[/]")
    emit(actor, "")
    for _, line in ipairs(wrapped(m.body)) do emit(actor, line) end
    return
  end

  if sub == "delete" then
    local n = tonumber(rest)
    if not box[n] then emit(actor, "No such message.") return end
    table.remove(box, n)
    set_attr(actor, "mail", box)
    emit(actor, "Deleted.")
    return
  end

  if sub == "send" then
    -- mail send <player>/<subject>=<body>
    local to, subject, body = string.match(rest, "^([^/]+)/([^=]+)=(.+)$")
    if not to then
      emit(actor, "Usage: mail send <player>/<subject>=<body>")
      return
    end
    local target = find_player(str.trim(to))
    if not target then
      emit(actor, "No player matches [b]" .. str.trim(to) .. "[/b].")
      return
    end
    local their_box = inbox(target)
    table.insert(their_box, {
      from    = actor.display_name,
      subject = str.trim(subject),
      body    = str.trim(body),
      read    = false,
      tick    = get_tick,
    })
    set_attr(target, "mail", their_box)
    emit(actor, "Sent to " .. target.display_name .. ".")
    if not has_tag(target, "system:offline") then
      emit(target, "[bright_cyan]New mail from " .. actor.display_name ..
                   ": " .. str.trim(subject) .. "[/] [cmd=mail]mail[/cmd]")
    end
    return
  end

  emit(actor, "Usage: mail [list | read <n> | delete <n> | send <player>/<subject>=<body>]")
end
```

> **MUSH note.** `set_attr(target, "mail", ...)` writing to *another* player's
> object is fine — intents are validated by the engine, and a player object is
> an ordinary object. There is no `@force`, no wizbit, no `@chown` dance.

---

## Vendors

**MUSH original:** [Multi-Vendor (PennMUSH)](https://www.mushcode.com/File/Multi-Vendor-(PennMUSH-Version))
— `ORDER <item>`, an `ITEMS` attribute listing stock, `<ITEM>_COST` and
`<ITEM>_BUY` attributes per item, and a **semaphore queue** so two players
can't order at once while the vendor waits for `GIVE VENDOR=<n>`.

The semaphore is the part that disappears. Hearth's engine is single-writer:
your script runs to completion and its whole batch applies atomically, so
there's no window for a second order to interleave. And `transfer_attr` moves
currency in one validated step — it checks the balance and rolls the batch back
if it's short, so "player pays, vendor delivers" can't half-happen.

`world/town/tavern.toml`:

```toml
[[objects]]
key = "vendor"
kind = "item"
title = "a brass vending machine"
description = "A brass contraption with a slot and a hand-lettered menu."
location = "tavern"
script = "vendor.luau"

[objects.attrs]
currency = "coins"

# Stock: one table per item. `spawn` names an archetype-ish blueprint below.
[objects.attrs.stock]
soda    = { cost = 10, title = "a bottle of fizzy soda",  desc = "Cold and faintly medicinal." }
popcorn = { cost = 20, title = "a bag of popcorn",        desc = "Warm, salty, over-buttered." }
lantern = { cost = 75, title = "a tin lantern",           desc = "Burns for hours.", tags = ["light:source"] }
```

`world/town/vendor.luau`:

```lua
local text = require("text")
local str = require("str")

local function currency(this)
  return get_attr(this, "currency") or "coins"
end

local function stock(this)
  return get_attr(this, "stock") or {}
end

local function menu(this, actor)
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local coin = currency(this)
  local rows = {}
  for name, item in pairs(stock(this)) do
    table.insert(rows, { name, item.title, item.cost .. " " .. coin })
  end
  table.sort(rows, function(a, b) return a[1] < b[1] end)

  emit(actor, fmt.header("Vending Machine"))
  emit(actor, fmt.table(rows, { "Order", "Item", "Price" }))
  emit(actor, fmt.divider(50))
  emit(actor, "You have [b]" .. (get_attr(actor, coin) or 0) .. "[/b] " .. coin .. ".")
end

function on_look(this, actor, room)
  menu(this, actor)
end

function cmd_order(this, actor, room, args)
  local want = string.lower(str.trim(args or ""))
  if want == "" then
    menu(this, actor)
    return
  end

  local item = stock(this)[want]
  if not item then
    emit(actor, "The machine buzzes. It doesn't stock [b]" .. want .. "[/b].")
    return
  end

  local coin = currency(this)
  local balance = get_attr(actor, coin) or 0
  if balance < item.cost then
    emit(actor, "You need " .. item.cost .. " " .. coin ..
                "; you have " .. balance .. ".")
    return
  end

  -- Atomic: validates the balance and rolls the whole batch back if short,
  -- so the payment and the goods can never come apart.
  transfer_attr(actor, this, coin, item.cost)

  local ref = spawn({
    key         = want,
    kind        = "item",
    title       = item.title,
    description = item.desc or "",
    location    = actor.ref_id,
  })
  for _, tag in ipairs(item.tags or {}) do
    set_tag(ref, tag)
  end
  set_attr(ref, "bought_from", this.ref_id)

  emit(actor, "The machine clunks and delivers [b]" .. item.title .. "[/b].")
  emit_room(room, actor.display_name .. " buys something from the vending machine.",
            { actor.ref_id })
end

-- Restocking / takings, for whoever owns the machine.
function cmd_collect(this, actor, room, args)
  if get_owner(this) ~= actor.ref_id then
    emit(actor, "That isn't yours to empty.")
    return
  end
  local coin = currency(this)
  local takings = get_attr(this, coin) or 0
  if takings == 0 then
    emit(actor, "The coin box is empty.")
    return
  end
  transfer_attr(this, actor, coin, takings)
  emit(actor, "You collect " .. takings .. " " .. coin .. ".")
end
```

**Adding an item** is now editing the `[objects.attrs.stock]` table in TOML and
running `@reload-world` — no code change, exactly the Multi-Vendor's promise,
but without hand-writing an `<ITEM>_BUY` attribute of MUSH code per item.

If an item genuinely needs custom purchase behavior (not just "spawn a thing"),
give the vendor a `on_sold_<item>` hook and `trigger(this, "on_sold_" .. want)`
after the transfer.

---

## Schedulers

**MUSH original:** [Myrddin's MushCron](https://www.mushcode.com/File/Myrddins-MushCron-1-0-0)
— pairs of `CRON_TIME_*` / `CRON_JOB_*` attributes, a pattern of
`month|date|dow|hour|minute`, and a self-retriggering `@wait 60` loop that
keeps the whole thing alive across reboots.

Hearth has a real heartbeat. `on_tick` fires every second (or every
`tick_interval` seconds), and `after(n, ref, hook)` schedules a one-shot that
**persists to the database** — you don't need the self-retrigger trick, and a
reboot doesn't drop your jobs.

### One-shots: `after`

```lua
function cmd_poison(this, actor, room, args)
  set_attr(actor, "poisoned", true)
  emit(actor, "Your veins burn.")
  after(30, actor, "on_cure", { source = this.ref_id })
end

-- Fires 30 ticks later. `this` and `actor` are both the timer's target;
-- the data table arrives as the 4th parameter.
function on_cure(this, actor, room, data)
  set_attr(this, "poisoned", nil)
  emit(this, "The burning fades.")
end
```

`cancel_after(ref, "on_cure")` cancels it. `get_timers(ref)` lists what's
pending.

### Recurring: a cron object on the game clock

MushCron's real feature is *calendar* scheduling. Hearth's optional in-world
[game clock](../CLAUDE.md#key-features) gives you that directly: configure
`[clock]` in `hearth.toml` and the engine fires `on_hour`, `on_day`, `on_dawn`
and `on_dusk` on `system:global` objects.

```toml
[[objects]]
key = "cron"
kind = "code"
title = "Scheduler"
tags = ["system:global", "system:hidden"]
script = "cron.luau"

# month | day | hour   (nil = wildcard, exactly MushCron's blank fields)
[[objects.attrs.jobs]]
name = "market-day"
day = 1
hour = 8
target = "town/crier"
hook = "on_market_day"

[[objects.attrs.jobs]]
name = "curfew"
hour = 22
target = "town/gate"
hook = "on_close"
```

```lua
local function matches(job, t)
  if job.month and job.month ~= t.month then return false end
  if job.day   and job.day   ~= t.day   then return false end
  if job.hour  and job.hour  ~= t.hour  then return false end
  return true
end

-- Fires once per in-world hour on system:global objects.
function on_hour(this)
  local t = get_time()
  if not t then return end

  for _, job in ipairs(get_attr(this, "jobs") or {}) do
    if matches(job, t) then
      local target = resolve_key(job.target)
      if target then
        log("cron: firing " .. job.name)
        trigger(target, job.hook, { job = job.name, time = t })
      else
        log("cron: unresolved target " .. job.target .. " for job " .. job.name)
      end
    end
  end
end
```

For sub-hour work — a heartbeat that ticks every 10 real seconds — use
`on_tick` with a counter and set `tick_interval` on the object:

```lua
function on_tick(this, state, room)
  state.n = (state.n or 0) + 1
  if state.n % 10 == 0 then
    -- every ~10 ticks
  end
end
```

---

## Time

**MUSH original:** Day Describer II — a room's description changes with the
time of day, so the same room reads differently at dawn and midnight.

In Hearth an `on_look` hook on a room *replaces* the default room rendering
entirely, which makes this trivial — but usually you don't want to reimplement
exits and contents. The lighter touch is to keep the default look and rewrite
the description with the clock.

### Light touch: swap the description on the hour

Put this on a `system:global` object; it drives every room that declares
time-of-day variants.

```lua
local PHASES = { "dawn", "day", "dusk", "night" }

local function phase_at(t)
  if t.hour >= 5  and t.hour < 8  then return "dawn"  end
  if t.hour >= 8  and t.hour < 18 then return "day"   end
  if t.hour >= 18 and t.hour < 21 then return "dusk"  end
  return "night"
end

function on_hour(this)
  local t = get_time()
  if not t then return end
  local phase = phase_at(t)
  if get_attr(this, "current_phase") == phase then return end
  set_attr(this, "current_phase", phase)

  -- Any room tagged desc:timed carries desc_dawn/desc_day/... attrs.
  for _, room in ipairs(find_by_tag("desc:timed")) do
    local desc = get_attr(room, "desc_" .. phase)
    if desc then
      set_description(room, desc)
    end
  end
  log("day phase → " .. phase)
end

function on_startup(this, state, room)
  on_hour(this)   -- get the world into the right phase at boot
end
```

```toml
[[rooms]]
key = "market"
title = "The Market Square"
description = "Stalls crowd the square."
tags = ["desc:timed"]

[rooms.attrs]
desc_dawn  = "Stallholders are still unfolding their awnings in the grey light."
desc_day   = "Stalls crowd the square, loud with haggling."
desc_dusk  = "The last vendors are packing up; the cobbles are littered with straw."
desc_night = "The square is empty, the stalls shuttered and dark."
```

### Heavier touch: a fully custom look

When you want a description assembled per-player — a shared wilderness room, a
darkness system, or a room that reads differently to someone who's been here
before — take over `on_look` on the room. Attaching `on_look` to a **room**
suppresses the engine's default rendering, so the hook owns all the output.

```lua
local text = require("text")

function on_look(this, actor, room)
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local t = get_time()
  local dark = t and not t.is_day

  emit(actor, fmt.header(this.title))

  if dark and not is_carrying(actor, "light:source") then
    emit(actor, "It is too dark to make anything out.")
    return
  end

  emit(actor, this.description)

  local names = {}
  for _, obj in ipairs(get_room_contents(this)) do
    if obj.ref_id ~= actor.ref_id and not has_tag(obj, "system:hidden") then
      table.insert(names, "[cmd=look " .. obj.key .. "]" .. obj.display_name .. "[/cmd]")
    end
  end
  if #names > 0 then
    emit(actor, "You see: " .. table.concat(names, ", "))
  end

  local dirs = {}
  for _, exit in ipairs(get_exits(this)) do
    table.insert(dirs, "[cmd=go " .. exit.key .. "]" .. exit.key .. "[/cmd]")
  end
  emit(actor, "Exits: " .. (#dirs > 0 and table.concat(dirs, " ") or "none"))
end
```

---

## Weather systems

**MUSH original:** [Keran's Weather System](https://www.mushcode.com/File/Kerans-Weather-System-And-Time-Code-4-0-\(PennMUSH\))
— climate zones, seasonal tables, a global weather object stepping the state
and emitting to outdoor rooms.

The Hearth version is one `system:global` object with an `on_tick` (or
`on_hour`, if you have a clock). Rooms opt in with a tag and declare a climate.

```toml
[[objects]]
key = "weather"
kind = "code"
title = "Weather"
tags = ["system:global", "system:hidden"]
script = "weather.luau"
```

```lua
local random = require("random")

-- Per-climate weather tables. Each entry is a state, its odds, and the line
-- players see when the weather turns to it.
local CLIMATES = {
  temperate = {
    { state = "clear",    weight = 5, onset = "The clouds break and the sky clears." },
    { state = "overcast", weight = 4, onset = "The sky greys over." },
    { state = "rain",     weight = 3, onset = "Rain begins to fall." },
    { state = "storm",    weight = 1, onset = "[bright_white]Thunder cracks overhead.[/]" },
  },
  desert = {
    { state = "clear",    weight = 8, onset = "The haze lifts." },
    { state = "sandstorm",weight = 1, onset = "[yellow]Sand rises on the wind.[/]" },
  },
  coastal = {
    { state = "clear",    weight = 4, onset = "The sea mist burns off." },
    { state = "fog",      weight = 4, onset = "Fog rolls in off the water." },
    { state = "rain",     weight = 3, onset = "A cold rain sweeps in." },
  },
}

-- Ambient lines, emitted occasionally while a state holds.
local AMBIENT = {
  rain      = { "Rain patters steadily.", "Water runs from the eaves." },
  storm     = { "Lightning whitens the sky.", "Thunder rolls, closer now." },
  fog       = { "The fog muffles every sound.", "Shapes loom and dissolve in the murk." },
  sandstorm = { "Grit stings your face.", "The wind howls." },
}

local function roll(climate)
  local table_for = CLIMATES[climate] or CLIMATES.temperate
  local total = 0
  for _, e in ipairs(table_for) do total = total + e.weight end
  local pick = seed_random(hash_seed("weather", climate, get_tick), 1, total)
  for _, e in ipairs(table_for) do
    pick = pick - e.weight
    if pick <= 0 then return e end
  end
  return table_for[1]
end

local function outdoor_rooms()
  return find_by_tag("env:outdoor")
end

-- Weather state is stored per *climate*, not per room, so every temperate room
-- shares the same sky.
function on_tick(this, state, room)
  state.n = (state.n or 0) + 1

  -- Turn the weather over roughly every 5 minutes.
  if state.n % 300 == 0 then
    local current = get_attr(this, "weather") or {}
    for climate, _ in pairs(CLIMATES) do
      local next_state = roll(climate)
      if current[climate] ~= next_state.state then
        current[climate] = next_state.state
        for _, r in ipairs(outdoor_rooms()) do
          if (get_attr(r, "climate") or "temperate") == climate then
            emit_room(r, "[cyan]" .. next_state.onset .. "[/]")
          end
        end
      end
    end
    set_attr(this, "weather", current)
    return
  end

  -- Ambient colour every ~90 ticks.
  if state.n % 90 == 0 then
    local current = get_attr(this, "weather") or {}
    for _, r in ipairs(outdoor_rooms()) do
      local climate = get_attr(r, "climate") or "temperate"
      local lines = AMBIENT[current[climate] or ""]
      if lines and #get_players_in_room(r) > 0 then
        local line = seed_choice(hash_seed(r.ref_id, get_tick), lines)
        emit_room(r, "[dim]" .. line .. "[/]")
      end
    end
  end
end

-- Anyone can ask. Put this on the same global object.
function cmd_weather(this, actor, room, args)
  local loc = get_location(actor)
  if not loc or not has_tag(loc, "env:outdoor") then
    emit(actor, "You can't see the sky from in here.")
    return
  end
  local climate = get_attr(loc, "climate") or "temperate"
  local current = (get_attr(this, "weather") or {})[climate] or "clear"
  emit(actor, "The weather is [b]" .. current .. "[/b].")
end
```

```toml
[[rooms]]
key = "cliffs"
title = "The Sea Cliffs"
description = "Wind-scoured turf ends in a long fall to the water."
tags = ["env:outdoor"]

[rooms.attrs]
climate = "coastal"
```

Note the use of `seed_random`/`seed_choice` with `hash_seed`: seeded RNG makes
the weather **deterministic for a given tick**, which means a `.test.luau` can
assert on it. `math.random` would work too, but you couldn't test it.

---

## Vehicles

**MUSH originals:** [General Car](https://www.mushcode.com/File/General-Car),
Airship, Shuttle System, and the Integrated Elevator System. The MUSH pattern
is a room-inside-an-object with `@oemit`-driven arrival messages, and a lot of
`@force`.

Hearth's version rests on one idea: a vehicle is an object that is also a
**location**. Players `enter` it, and moving the vehicle moves everyone whose
`location_ref` is the vehicle.

### A car

```toml
[[objects]]
key = "car"
kind = "item"
title = "a battered green sedan"
description = "It has seen better decades. The doors are unlocked."
location = "town/crossroads"
script = "car.luau"

[objects.attrs]
capacity = 4

# Where it can drive, and how those places connect.
[objects.attrs.routes]
crossroads = { town = "town/square", forest = "forest/edge" }
square     = { crossroads = "town/crossroads" }
edge       = { crossroads = "town/crossroads" }
```

```lua
local str = require("str")

local function occupants(this)
  local list = {}
  for _, o in ipairs(get_contents(this)) do
    if is_player(o) or is_npc(o) then table.insert(list, o) end
  end
  return list
end

local function tell_occupants(this, msg, exclude)
  for _, o in ipairs(occupants(this)) do
    if o.ref_id ~= exclude then emit(o, msg) end
  end
end

function cmd_board(this, actor, room, args)
  if get_location(actor) and get_location(actor).ref_id == this.ref_id then
    emit(actor, "You're already in the car.")
    return
  end
  local cap = get_attr(this, "capacity") or 4
  if #occupants(this) >= cap then
    emit(actor, "There's no room left.")
    return
  end
  emit(actor, "You climb into " .. this.display_name .. ".")
  emit_room(room, actor.display_name .. " gets into " .. this.display_name .. ".",
            { actor.ref_id })
  tell_occupants(this, actor.display_name .. " gets in.")
  move_object(actor, this.ref_id)
end

function cmd_disembark(this, actor, room, args)
  local loc = get_location(actor)
  if not loc or loc.ref_id ~= this.ref_id then
    emit(actor, "You aren't in the car.")
    return
  end
  local outside = get_location(this)
  if not outside then
    emit(actor, "There's nowhere to get out to.")
    return
  end
  tell_occupants(this, actor.display_name .. " gets out.", actor.ref_id)
  move_object(actor, outside.ref_id)
  emit(actor, "You climb out.")
  emit_room(outside, actor.display_name .. " gets out of " .. this.display_name .. ".",
            { actor.ref_id })
end

function cmd_drive(this, actor, room, args)
  local loc = get_location(actor)
  if not loc or loc.ref_id ~= this.ref_id then
    emit(actor, "You aren't behind the wheel.")
    return
  end

  local here = get_location(this)
  if not here then
    emit(actor, "The car isn't anywhere you can drive from.")
    return
  end

  local routes = (get_attr(this, "routes") or {})[here.key] or {}
  local where = string.lower(str.trim(args or ""))

  if where == "" then
    local names = {}
    for name, _ in pairs(routes) do
      table.insert(names, "[cmd=drive " .. name .. "]" .. name .. "[/cmd]")
    end
    table.sort(names)
    emit(actor, "From here you can drive to: " ..
                (#names > 0 and table.concat(names, ", ") or "nowhere"))
    return
  end

  local dest_key = routes[where]
  if not dest_key then
    emit(actor, "You can't get there from here.")
    return
  end
  local dest = resolve_key(dest_key)
  if not dest then
    emit(actor, "The road there is out.")
    log("car: unresolved route target " .. dest_key)
    return
  end

  emit_room(here, this.display_name .. " pulls away.")
  tell_occupants(this, "[dim]The engine turns over and the world slides past.[/]")
  move_object(this, dest)
  emit_room(dest, this.display_name .. " pulls up.")

  -- Occupants ride along automatically: their location is the car, and the
  -- car moved. Give them the arrival view.
  for _, o in ipairs(occupants(this)) do
    if is_player(o) then
      emit(o, "You arrive at [b]" .. (get_object(dest).title or where) .. "[/b].")
    end
  end
end

function on_look(this, actor, room)
  local riders = occupants(this)
  if #riders > 0 then
    local names = {}
    for _, o in ipairs(riders) do table.insert(names, o.display_name) end
    emit(actor, "Inside: " .. table.concat(names, ", "))
  end
end
```

Because occupants are *located in* the car, moving the car moves them — the
engine does the work. No `@force`, no per-passenger teleport loop.

### An elevator

An elevator is a car on rails: fixed stops, a call button in each lobby, and
doors. The interesting difference is that it moves *itself* on a timer.

```lua
local str = require("str")

-- Floors in order, bottom to top. Each is a file key.
local FLOORS = { "tower/lobby", "tower/mezzanine", "tower/offices", "tower/roof" }

local function floor_index(key)
  for i, f in ipairs(FLOORS) do
    if f == key then return i end
  end
  return nil
end

local function riders(this)
  local list = {}
  for _, o in ipairs(get_contents(this)) do
    if is_player(o) then table.insert(list, o) end
  end
  return list
end

local function announce(this, msg)
  for _, o in ipairs(riders(this)) do emit(o, msg) end
end

function cmd_press(this, actor, room, args)
  local want = tonumber(str.trim(args or ""))
  if not want or not FLOORS[want] then
    emit(actor, "Floors 1 to " .. #FLOORS .. ". Which?")
    return
  end
  if get_attr(this, "moving") then
    emit(actor, "The car is already in motion.")
    return
  end

  local here = get_location(this)
  local current = here and floor_index(here.key)
  if current == want then
    emit(actor, "You're already on floor " .. want .. ".")
    return
  end

  set_attr(this, "moving", true)
  set_attr(this, "target_floor", want)
  emit(actor, "The button lights. The doors slide shut.")
  announce(this, "[dim]The car begins to move.[/]")

  -- One tick per floor travelled — the elevator takes time, like elevators do.
  after(math.abs((current or 1) - want) * 2, this, "on_arrive")
end

function on_arrive(this, actor, room, data)
  local want = get_attr(this, "target_floor")
  set_attr(this, "moving", nil)
  set_attr(this, "target_floor", nil)
  if not want then return end

  local dest = resolve_key(FLOORS[want])
  if not dest then
    announce(this, "Something is wrong with the mechanism.")
    log("elevator: unresolved floor " .. tostring(FLOORS[want]))
    return
  end

  local from = get_location(this)
  if from then emit_room(from, "The elevator doors close and the car departs.") end
  move_object(this, dest)
  emit_room(dest, "The elevator arrives with a chime.")
  announce(this, "[b]Floor " .. want .. ".[/b] The doors open.")
end
```

---

## Games

### A dice roller

The `+roll 3d6+2` global every MUSH has. `random` is a bundled module.

`world/system/roll.luau`:

```lua
local random = require("random")
local str = require("str")

function cmd_roll(this, actor, room, args)
  local spec = string.lower(str.trim(args or ""))
  local count, sides, sign, bonus = string.match(spec, "^(%d*)d(%d+)%s*([%+%-]?)%s*(%d*)$")

  if not sides then
    emit(actor, "Usage: roll <n>d<sides>[+/-<mod>]   e.g. [cmd=roll 3d6]roll 3d6[/cmd]")
    return
  end

  count = tonumber(count) or 1
  sides = tonumber(sides)
  bonus = tonumber(bonus) or 0
  if sign == "-" then bonus = -bonus end

  if count < 1 or count > 100 or sides < 2 or sides > 1000 then
    emit(actor, "That's not a die anyone owns.")
    return
  end

  local rolls, total = {}, 0
  for _ = 1, count do
    local r = random.roll(sides)   -- one die; random.dice(n, sides) returns a sum
    table.insert(rolls, r)
    total = total + r
  end
  total = total + bonus

  local detail = table.concat(rolls, ", ")
  local mod = bonus ~= 0 and string.format(" %s %d", bonus > 0 and "+" or "-", math.abs(bonus)) or ""
  local line = string.format("%s rolls %s: [b]%d[/b] [dim](%s%s)[/]",
                             actor.display_name, spec, total, detail, mod)

  emit(actor, line)
  emit_room(room, line, { actor.ref_id })
end
```

Rolls are public by design — a private roll nobody can see is a roll nobody
trusts. Add `roll/private` if your game wants it.

### A jukebox

The classic room-object gadget: a list of songs, `play <song>`, and everyone in
the room hears it. It also shows off `emit_radius` — sound leaking through
exits, which MUSH could only fake.

```toml
[[objects]]
key = "jukebox"
kind = "item"
title = "a chrome jukebox"
description = "Lit from within, humming faintly."
location = "tavern"
script = "jukebox.luau"

[[objects.attrs.songs]]
title = "Ride of the Valkyries"
artist = "the house band"
lines = [
  "The horns come in like weather.",
  "Somebody at the bar starts conducting.",
]

[[objects.attrs.songs]]
title = "Slow Rain"
artist = "Merridy Vane"
lines = [
  "A brushed snare, and Vane's voice under it.",
  "Conversation drops a half-step.",
]
```

`world/town/jukebox.luau`:

```lua
local text = require("text")
local str = require("str")

local function songs(this)
  return get_attr(this, "songs") or {}
end

function cmd_jukebox(this, actor, room, args)
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local rows = {}
  for i, s in ipairs(songs(this)) do
    table.insert(rows, {
      "[cmd=play " .. i .. "]" .. i .. "[/cmd]",
      s.title,
      s.artist,
    })
  end
  emit(actor, fmt.header("Jukebox"))
  emit(actor, fmt.table(rows, { "#", "Title", "Artist" }))
  local now = get_attr(this, "now_playing")
  if now then
    emit(actor, "[dim]Now playing: " .. now .. "[/]")
  end
end

function cmd_play(this, actor, room, args)
  if get_attr(this, "now_playing") then
    emit(actor, "Something's already playing. Wait your turn.")
    return
  end

  local n = tonumber(str.trim(args or ""))
  local song = n and songs(this)[n]
  if not song then
    emit(actor, "Pick a number from the list. [cmd=jukebox]jukebox[/cmd]")
    return
  end

  set_attr(this, "now_playing", song.title)
  set_attr(this, "line_index", 0)
  set_attr(this, "queued_by", actor.ref_id)

  emit_room(room, "[magenta]" .. this.display_name .. " clicks, whirs, and starts up: [b]" ..
                  song.title .. "[/b] by " .. song.artist .. ".[/]")

  -- Bleed into adjacent rooms. Exits with a `muffle` attr add distance;
  -- exits with `blocked_sound = true` stop it dead.
  emit_radius(room, 2, {
    [1] = "[dim]Music starts up somewhere nearby.[/]",
    [2] = "[dim]You catch a few bars of music, far off.[/]",
  })

  after(5, this, "on_verse")
end

function on_verse(this, actor, room, data)
  local title = get_attr(this, "now_playing")
  if not title then return end

  local song
  for _, s in ipairs(songs(this)) do
    if s.title == title then song = s end
  end
  if not song then
    set_attr(this, "now_playing", nil)
    return
  end

  local idx = (get_attr(this, "line_index") or 0) + 1
  set_attr(this, "line_index", idx)

  local here = get_location(this)
  if not here then return end

  local line = song.lines and song.lines[idx]
  if line then
    emit_room(here, "[dim][magenta]" .. line .. "[/][/]")
    after(5, this, "on_verse")
  else
    emit_room(here, "[dim]The record ends. The jukebox falls silent.[/]")
    set_attr(this, "now_playing", nil)
    set_attr(this, "line_index", nil)
    set_attr(this, "queued_by", nil)
  end
end
```

---

## Administration

### `+jobs` — a request tracker

Every MUSH grows one: players file requests, staff claim and close them. It's
the BBS pattern with a status field and a permission gate.

`world/system/jobs.luau`:

```lua
local text = require("text")
local str = require("str")

local STATUSES = { "new", "open", "held", "done" }

local function jobs(this)   return get_attr(this, "jobs") or {} end
local function is_staff(a)  return has_tag(a, "role:staff") end
local function wrapped(s)   return str.split(str.wrap(s, 72), "\n") end

local function notify_staff(msg)
  for _, p in ipairs(get_all_by_kind("player")) do
    if is_staff(p) and not has_tag(p, "system:offline") then
      emit(p, "[bright_cyan]" .. msg .. "[/]")
    end
  end
end

function cmd_job(this, actor, room, args)
  local sub, rest = string.match(str.trim(args or ""), "^(%S*)%s*(.*)$")
  sub = string.lower(sub or "")
  local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
  local list = jobs(this)

  if sub == "" or sub == "list" then
    local rows = {}
    for i, j in ipairs(list) do
      -- Players see only their own; staff see everything.
      if is_staff(actor) or j.author_ref == actor.ref_id then
        if j.status ~= "done" then
          table.insert(rows, { tostring(i), j.status, j.title, j.author, j.claimed or "-" })
        end
      end
    end
    emit(actor, fmt.header("Jobs"))
    if #rows == 0 then
      emit(actor, "[dim]Nothing open.[/]")
    else
      emit(actor, fmt.table(rows, { "#", "Status", "Title", "From", "Claimed" }))
    end
    return
  end

  if sub == "file" then
    -- job file <title>=<body>
    local title, body = string.match(rest, "^([^=]+)=(.+)$")
    if not title then
      emit(actor, "Usage: job file <title>=<description>")
      return
    end
    table.insert(list, {
      title       = str.trim(title),
      body        = str.trim(body),
      author      = actor.display_name,
      author_ref  = actor.ref_id,
      status      = "new",
      tick        = get_tick,
      comments    = {},
    })
    set_attr(this, "jobs", list)
    emit(actor, "Filed as job #" .. #list .. ". Staff have been notified.")
    notify_staff("New job #" .. #list .. ": " .. str.trim(title) ..
                 " (" .. actor.display_name .. ")")
    return
  end

  local n = tonumber(string.match(rest, "^(%d+)"))
  local job = n and list[n]
  if not job then
    emit(actor, "Usage: job [list | file <title>=<body> | read <n> | claim <n> | " ..
                "comment <n>=<text> | close <n>=<resolution>]")
    return
  end
  if not is_staff(actor) and job.author_ref ~= actor.ref_id then
    emit(actor, "That job isn't yours.")
    return
  end

  if sub == "read" then
    emit(actor, fmt.header("#" .. n .. " — " .. job.title))
    emit(actor, "[dim]" .. job.status .. " · filed by " .. job.author ..
                (job.claimed and (" · claimed by " .. job.claimed) or "") .. "[/]")
    emit(actor, "")
    for _, line in ipairs(wrapped(job.body)) do emit(actor, line) end
    for _, c in ipairs(job.comments or {}) do
      emit(actor, "")
      emit(actor, "[dim]" .. c.author .. ":[/] " .. c.text)
    end
    return
  end

  if not is_staff(actor) and sub ~= "comment" then
    emit(actor, "Only staff can do that.")
    return
  end

  if sub == "claim" then
    job.claimed = actor.display_name
    job.status = "open"
    set_attr(this, "jobs", list)
    emit(actor, "Claimed #" .. n .. ".")
    return
  end

  if sub == "comment" then
    local body = string.match(rest, "^%d+%s*=%s*(.+)$")
    if not body then emit(actor, "Usage: job comment <n>=<text>") return end
    job.comments = job.comments or {}
    table.insert(job.comments, { author = actor.display_name, text = str.trim(body) })
    set_attr(this, "jobs", list)
    emit(actor, "Comment added to #" .. n .. ".")
    return
  end

  if sub == "close" then
    local resolution = string.match(rest, "^%d+%s*=%s*(.+)$") or "Closed."
    job.status = "done"
    job.resolution = str.trim(resolution)
    set_attr(this, "jobs", list)
    emit(actor, "Closed #" .. n .. ".")
    -- Tell the player who filed it.
    if exists(job.author_ref) and not has_tag(job.author_ref, "system:offline") then
      emit(job.author_ref, "[bright_green]Your job #" .. n .. " (" .. job.title ..
                           ") was closed:[/] " .. str.trim(resolution))
    end
    return
  end

  emit(actor, "Unknown job subcommand.")
end
```

---

## Building

### `@parent` becomes archetypes

MUSH's `@parent` gives an object read-through access to another object's
attributes — the basis of every "generic sword" or "template room" in the
canon. Hearth has a first-class version: [archetypes](archetypes.md), an `is-a`
delegation chain where a child inherits its parent's attrs, tags, script and
attribute schema, and hot-reloads when the parent changes.

The practical difference: in MUSH you `@parent sword=#123` and then override
attributes one at a time. In Hearth you declare the relationship in TOML and
override only what differs.

```toml
# The archetype — a "generic" in MUSH terms.
[[objects]]
key = "blade"
kind = "item"
title = "a blade"
description = "A length of sharpened steel."
tags = ["item:weapon"]
script = "blade.luau"

[objects.attrs]
damage = 4
weight = 3

# An instance. It inherits blade's script, tags and attrs; it overrides two.
[[objects]]
key = "sword"
kind = "item"
title = "an iron sword"
archetype = "std/blade"
location = "town/armoury"

[objects.attrs]
damage = 6
```

`blade.luau`'s hooks now run for *every* blade in the game. Fix a bug there and
every sword, dagger and cleaver gets the fix on `@reload-world` — the thing
`@parent` promised and `@decompile`-and-repaste usually undid.

### Cloning at runtime

For the MUSH `@clone` reflex — an object that hands out copies of itself:

```lua
function cmd_requisition(this, actor, room, args)
  local template = resolve_key("std/blade")
  if not template then
    emit(actor, "The rack is empty.")
    return
  end
  local copy = clone_object(template, { location = actor.ref_id, owner = actor.ref_id })
  set_title(copy, "a quartermaster's blade")
  set_attr(copy, "issued_to", actor.ref_id)
  emit(actor, "The quartermaster hands you a blade.")
  emit_room(room, actor.display_name .. " is issued a blade.", { actor.ref_id })
end
```

### Digging from code

`@dig` and `@open` as intents, for procedural content:

```lua
function cmd_burrow(this, actor, room, args)
  local new_room = spawn({
    key         = "burrow",
    kind        = "room",
    title       = "A Cramped Burrow",
    description = "Roots hang from a low earth ceiling.",
  })
  create_exit({ source = room.ref_id, direction = "down", target = new_room, aliases = { "d" } })
  create_exit({ source = new_room, direction = "up", target = room.ref_id, aliases = { "u" } })
  emit(actor, "You dig down into the earth.")
  emit_room(room, actor.display_name .. " burrows into the ground.", { actor.ref_id })
end
```

---

## Testing your gadgets

MUSH testing was `@pemit me=[u(FUNCTION)]` and hoping. Hearth has two real
harnesses, and gadgets are exactly what they're for.

**`.test.luau` — logic.** Test the pieces that decide things:

Tests are **top-level `test_*` functions**, not a returned table:

```lua
-- world/system/bbs.test.luau
function test_posting_appends_to_the_group()
  local bbs = resolve_key("system/bbs")
  set_attr(bbs, "posts_general", { { title = "One" }, { title = "Two" } })
  assert_eq(#get_attr(bbs, "posts_general"), 2)
end

function test_unread_is_posts_minus_read_mark()
  local bbs = resolve_key("system/bbs")
  set_attr(bbs, "posts_general", { {}, {}, {} })
  local reader = spawn({ key = "tester", kind = "npc", title = "Tester" })
  set_attr(reader, "bb_read", { general = 1 })
  -- get_attr sees pending writes, so this reads back inside the same script.
  assert_eq(#get_attr(bbs, "posts_general") - get_attr(reader, "bb_read").general, 2)
end
```

You can also co-locate `test_*` functions in the object's own script and run
them against that object with `@test #<ref>` — `ctx.this` is bound to it.

**`.session` — the wire.** A `.session` file drives the real session handler,
which is the only way to catch a gadget that works in Luau but breaks on
dispatch — or gets swallowed by an armed `prompt`:

```
> bb list
expect: Bulletin Boards
> bb post general=Test Post
expect: Type your post
> This is the body of the post.
expect: Posted to
> bb read general/1
expect: This is the body
```

Run it with `hearth session-test <file>`, or drop it under your `game_dir` and
`cargo test` picks it up. See [the testing notes](softcode-guide.md) and
`src/session_test.rs`.

---

## Where to go next

- [Softcode guide](softcode-guide.md) — the full API reference
- [Archetypes](archetypes.md) — `@parent`, done properly
- [Ink dialogue](../CLAUDE.md#key-features) — branching NPC conversation, which
  MUSH never really had an answer for
- [Commands](commands.md) — the builder command surface
- [mushcode.com](https://www.mushcode.com/) — the original archive, still the
  best source of "what gadget should I build next"
