# Documentation accuracy audit

Audit date: 2026-09-02. Tree: `main`, clean, `cargo test` green
(**435 passed + 1 ignored = 436**; 429 lib + 3 cookbook + 1 game_smoke + 2 session_test).
Nothing in this report changed any file except this one.

**How to read the confidence labels.** Every finding is marked either
**[EXEC]** — reproduced by running something (a `.session` script against the
real session handler, or `cargo test`) — or **[READ]** — established by reading
the implementing code, with the `file:line` cited. Nothing here is inferred
from naming or from other docs. Things suspected but not confirmed are
quarantined in the last section.

---

## Summary

| # | Sev | Doc location | One line |
|---|-----|--------------|----------|
| 1 | BROKEN | softcode-guide.md:445 | "Triggers don't recurse … prevents infinite loops" — they do recurse; a self-trigger crashes the engine with a stack overflow |
| 2 | BROKEN | softcode-guide.md:920, SKILL.md:195 | "Modules … **not** the game write API" — `require`d modules DO get `emit`/`set_attr`, by design |
| 3 | BROKEN | softcode-guide.md:153-157, SKILL.md:134-147, mush-cookbook.md:56 | Lifecycle hooks documented as `(this, state, room)`; only `on_tick` gets a `state` table |
| 4 | BROKEN | softcode-guide.md:765, commands.md:112, CONTEXT.md:97 | The `teleport` lock type is never evaluated by anything |
| 5 | BROKEN | diku-cookbook.md:120, :456 | `on_enter` documented as firing "on the mob or the room"; it never fires on a mob |
| 6 | BROKEN | mush-cookbook.md:96 | "a `get_object()` snapshot … will not show it" — `get_object` returns a pending-aware proxy |
| 7 | BROKEN | mush-cookbook.md:530 | `@lock <obj>/use` suggested to gate a `cmd_` hook; no lock type gates `cmd_` dispatch |
| 8 | BROKEN | commands.md:10-11 | `"<message>` / `:<action>` don't parse — the sigil needs a trailing space |
| 9 | BROKEN | CLAUDE.md:174 | `grid_can_move(actor, map, dir)` — real signature is `grid_can_move(map, x, y, dir)` |
| 10 | BROKEN | softcode-guide.md:830, commands.md:168 | "the buffer … survives a disconnect mid-edit" — editor mode is session state; reopening wipes the buffer |
| 11 | BROKEN | CONTEXT.md:20 | "Transient attributes (nattrs) die on restart" — no such concept exists anywhere in the code |
| 12 | BROKEN | softcode-guide.md:614, SKILL.md:424 | `generate_dungeon(opts)` — the first argument is a seed **string**, not a table |
| 13 | BROKEN | SKILL.md:320 | `trigger`'s `data` documented as the `_trigger_data` attr; it is the hook's 4th argument |
| 14 | MISLEADING | softcode-guide.md:101, mush:99, lpmud:309, diku:122 | `cmd_` dispatch is name-agnostic and loses to every builtin verb; docs imply name targeting |
| 15 | MISLEADING | softcode-guide.md:146-147, CLAUDE.md:157, SKILL.md:161 | `on_damage`/`on_death` listed like engine events; the engine never fires them |
| 16 | MISLEADING | softcode-guide.md:853, commands.md:157 | "`@eval` … same instruction budget as any hook" — 50,000,000 vs 200,000 |
| 17 | MISLEADING | diku-cookbook.md:123 | `on_say` documented as a mob trigger; it fires on the room only |
| 18 | MISLEADING | lpmud-cookbook.md:99 | `all_inventory(ob)` → `get_inventory`/`get_contents`; both are items-only |
| 19 | MISLEADING | mush-cookbook.md:812, CONTEXT.md:88 | `tick_interval` is in **ticks**, not seconds; the heartbeat is `tick_secs` |
| 20 | MISLEADING | mush-cookbook.md:665 | "There is no `@force`, no `@chown` dance" — both commands exist |
| 21 | MISLEADING | commands.md:168, :199, :269, :373, :32, :41, :136 | Seven smaller command-reference claims (see §21) |
| 22 | STALE | CLAUDE.md:21, :333 | "433 tests" — 436 |
| 23 | STALE | CLAUDE.md:151, :329 | "6 architectural decision records (ADR 0001–0006)" — there are 8 |
| 24 | STALE | CLAUDE.md:197 | "POST /api with ~44 actions" — 62 `ApiRequest` variants |
| 25 | STALE | CLAUDE.md:173 | "~91 Luau API functions" — 108 registered |
| 26 | STALE | CLAUDE.md:356-387 | The whole "The Last Stag" section describes a layout the game no longer has |
| 27 | STALE | commands.md:124, :112, :306 | `@grant` scope list omits `puppeteer`; lock-type list omits `put` and is unvalidated; "four literal first words" is six |
| 28 | STALE | SKILL.md:96, :129, :161, :245, :296 | `kind` list omits `code`; hook lists omit `can_put`/`on_put`/clock hooks; API counts wrong |
| 29 | MISSING | softcode-guide.md (whole), SKILL.md (whole) | The **ownership-authority model** on intents — `may_modify` refuses writes to objects the authority doesn't own |
| 30 | MISSING | softcode-guide.md:494-510, SKILL.md:325 | `OWNER_TIMER_QUOTA = 100` silently drops `after()` calls; `EMIT_BATCH_LIMIT = 50` errors a batch |
| 31 | MISSING | CLAUDE.md:206-210 (asset table), all docs | WASM plugins (`<game_dir>/wasm/*.wasm`, `wasm_call`) are documented **nowhere** |
| 32 | MISSING | softcode-guide.md, SKILL.md | 18 / 24 registered API functions absent from the guide / the skill |
| 33 | MISSING | softcode-guide.md:150-151 | `on_whisper`/`on_emote` get no message payload and no suppression (unlike `on_say`) |
| 34 | MISSING | softcode-guide.md:391, SKILL.md:314 | `spawn` also accepts `owner`, `archetype`, `ref` |
| 35 | MISSING | softcode-guide.md:845, commands.md:155 | `@eval` binds an `actor` global |
| 36 | MISSING | commands.md | ~11 commands and 5 aliases exist but are undocumented (see §36) |
| 37 | MISSING | softcode-guide.md:174-178, SKILL.md:178, CONTEXT.md:108 | The `system:global` hook list and the command-resolution order are both incomplete |
| 38 | MISSING | CLAUDE.md:41-140, :296 | Source layout omits 5 modules; the `hearth.toml` sample omits `max_characters` |
| 39 | MISLEADING | CONTEXT.md:57, :60 | "Program: a script attached … via a named hook" — the one-script-per-object model replaced this |
| 40 | BROKEN | README.md:163 | `set_program(ref, hook, source)` is not an API function |
| 41 | BROKEN | README.md:179 | `prompt(ref, message)` — real signature is `prompt(actor, obj, hook)`; there is no `on_reply` |
| 42 | BROKEN | README.md:146 | `is_carrying(actor, item)` — the 2nd argument is a **tag spec**, not an item ref |
| 43 | BROKEN | getting-started.md:82 | `@program #13/cmd_light = …` — the per-hook ref form doesn't parse |
| 44 | BROKEN | getting-started.md:131, :133 | `hearth program get #5/on_look` — the CLI takes the ref verbatim; no `/hook` split |
| 45 | BROKEN | getting-started.md:138 | `hearth program history`/`restore` and `@program/history`/`@program/restore` don't exist |
| 46 | BROKEN | CLAUDE.md, commands.md, getting-started.md, migrations.md, mush:1839, diku:76 | The binary is `hearth-mud`; every `hearth <subcommand>` example names a command that isn't installed |
| 47 | BROKEN | adr/0005:7 | "Tick order is deterministic (sorted by `stable_ref`)" — unsorted `HashMap` order; `stable_ref` doesn't exist |
| 48 | MISLEADING | README.md:164 | `apply_template(ref, table)` — the `{ hook = source }` keys are **discarded** |
| 49 | MISLEADING | README.md:204 | `trigger`'s `data` documented as `_trigger_data` (same bug as #13, second file) |
| 50 | MISLEADING | README.md:214 | `random.weighted_choice` — the module exports `weighted` |
| 51 | MISLEADING | README.md:195 | `generate_dungeon(opts)` (same bug as #12, third file) |
| 52 | MISLEADING | CONTEXT.md:30, adr/0003:3 | "Areas are the unit of save/load" — checkpoints are whole-world; the DB has no `area` column |
| 53 | STALE | adr/0006:6-13 | Lock-function list omits `is_owner` + `game_time_between`; lock-point list has `teleport` and omits `put` |
| 54 | STALE | archetypes.md:185-187 | The documented "known limitation" (`RoomDef.title` required) was fixed |
| 55 | STALE | adr/0006:15 | "A `can_` hook escape hatch … is a planned future extension" — nine `can_` hooks shipped |
| 56 | STALE | adr/0001:9, adr/0002:3 | "MCP endpoints / MCP tooling" — no MCP surface exists (only **G**MCP, telnet 201) |
| 57 | STALE | getting-started.md:66 | Quoted `@create` output doesn't match the engine's actual string |
| 58 | MISSING | README.md:25-36, :53-92, :98-205 | Config block omits 5 keys incl. the whole clock; hook table omits the 4 clock hooks; API tables cover ~55 of 108 |

Counts: **BROKEN 20 · MISLEADING 13 · STALE 13 · MISSING 12** (58 findings).

---

## 1. BROKEN — "Triggers don't recurse … prevents infinite loops"

> **softcode-guide.md:445-446** — "Triggers don't recurse — if the triggered hook
> also calls `trigger`, the second trigger fires after the first finishes. This
> prevents infinite loops."

**What the code does.** `deliver_effects` collects `Effect::TriggerHook` into a
local `triggers` vec and then, at `src/engine/mod.rs:4953-4964`, calls
`fire_hook_data` for each — which runs the hook and calls `deliver_effects`
again (`src/engine/mod.rs:4771`). That is ordinary recursion. There is no depth
counter, no visited set, and no cycle guard anywhere on the path.

**How verified — [EXEC].** Throwaway game at `/tmp/audit_game2` with one item:

```lua
function cmd_ring(this, actor, room, args) set_attr(this,"n",0) trigger(this,"on_ping") end
function on_ping(this, actor, room, data)
  set_attr(this, "n", (get_attr(this,"n") or 0) + 1)
  trigger(this, "on_ping")
end
```

`cargo run -- session-test /tmp/audit4.session --config /tmp/audit_game2/hearth.toml`:

```
thread 'main' (62566440) has overflowed its stack
fatal runtime error: stack overflow, aborting
```

The whole process dies. This is also a real engine defect, not only a doc bug.

**Suggested correction.** "A triggered hook runs *nested* inside the current
one, after the current batch commits. Triggers therefore **do** recurse and
there is no depth limit — a cycle (`A` triggers `B` triggers `A`) will overflow
the stack and take the server down. Guard your own chains." (And/or add a depth
cap in `deliver_effects`.)

---

## 2. BROKEN — "Modules … but **not** the game write API"

> **softcode-guide.md:920-921** — "Modules have stdlib access (string, table,
> math, require) but **not** the game write API (emit, set_attr, etc.). They're
> for pure utility code."
> **.claude/skills/softcode/SKILL.md:195** — "Modules have stdlib + require but
> **not** the write API."

**What the code does.** `install_require` (`src/softcode/mod.rs:1497-1576`) gives
a module chunk an env whose `__index` resolves free names against
`CURRENT_HOOK_ENV_KEY` — the *calling hook's* env — falling back to globals. The
comment at `src/softcode/mod.rs:1528-1533` says so explicitly: "so module
functions can reach the per-script API installed there — `emit`, `ink_*`,
`grid_move`, etc."

**How verified — [EXEC].** `/tmp/audit_game3/world/lib/mylib.luau`:

```lua
local M = {}
function M.shout(actor) emit(actor, "LIB-CALLED-EMIT") set_attr(actor, "lib_wrote", 42) end
return M
```

called from a `cmd_ring` hook. `session-test` PASS on both
`expect: LIB-CALLED-EMIT` and `@eval return get_attr(actor,"lib_wrote")` →
`expect: 42`.

**Suggested correction.** "A module's free names resolve against the env of
whatever hook `require`d it, so a module *can* call `emit`, `set_attr`, and the
rest of the per-run API. With no hook running (unit-mode tests, a module loaded
outside a hook) they fall back to globals and are absent — so a module that
calls the write API is only usable from a hook."

---

## 3. BROKEN — lifecycle hooks do not receive a `state` table

> **softcode-guide.md:153-157** — `on_startup` / `on_shutdown` / `on_reload` /
> `on_save` / `on_create` all listed with signature `(this, state, room)`.
> **SKILL.md:134-147** — "Same signature as `on_tick` (`this`, `state`, `room`).
> State is persistent."
> **mush-cookbook.md:56, :954; diku-cookbook.md:346** — same signature in
> the concept map and two recipes.

**What the code does.** `src/softcode/mod.rs:1789` — `let is_tick = hook == "on_tick";`
The state-table branch at `src/softcode/mod.rs:1979-1987` is taken only when
`is_tick`; every other hook is called `(this, actor, room[, data|args])`
(`:1988-2004`). For lifecycle hooks the engine passes `this_ref` as *both* `this`
and `actor` (`src/engine/mod.rs:1224-1226`), so the 2nd parameter is a proxy for
the object itself. And `write_back_script_state` persists nothing unless
`hook_name == "on_tick"` (`src/engine/mod.rs:1267-1271`).

**How verified — [READ]** (`src/softcode/mod.rs:1789`, `:1979`, `:1988`;
`src/engine/mod.rs:1224`, `:1267`).

**Why it bites.** `state.count = 1` inside `on_startup` is not a scratch write —
the 2nd parameter is the live object proxy, so it becomes `set_attr(this,
"count", 1)` and silently lands as a real, persisted attribute.

**Suggested correction.** Give these five hooks the signature
`(this, actor, room)` and add a note: "`state` exists on `on_tick` only; on the
lifecycle hooks the second parameter is the object itself (the engine passes
`this` as its own actor)."

---

## 4. BROKEN — the `teleport` lock type is never evaluated

> **softcode-guide.md:765** — "| `teleport` | rooms | Before teleporting |"
> **commands.md:112** — "types: traverse, get, drop, enter, use, look, teleport"
> **CONTEXT.md:97** — "gates an action (traverse, get, drop, enter, use, look, teleport)"

**What the code does.** Every `check_lock` call site in the engine:
`get` (`src/engine/mod.rs:5200`, `:5260`), `put` (`:5361`), `look` (`:7021`),
`traverse` (`:7078`), `enter` (`:7106`), `drop` (`:7206`), `use` (`:7268`).
There is no `check_lock("teleport", …)`. `cmd_teleport`
(`src/engine/mod.rs:4386-4401`) checks the Builder scope and that the target is a
room, then calls `commands::move_player` — no lock consulted. Grepping the whole
of `src/` for `teleport` finds it only in `cmd_teleport`, the help text, and the
`@lock` **usage string** (`src/engine/mod.rs:7381`) that advertises it.

**How verified — [EXEC].**

```
> @dig Vault
> @lock #5/teleport = false     → accepted
> @teleport #5                  → the Vault room renders; the move succeeded
```

**Suggested correction.** Remove `teleport` from all three lists and from the
`@lock` usage string; add `put` (which *is* checked, at `:5361`) to
softcode-guide.md's lock-type table and to CONTEXT.md. Note also that the lock
type is stored as a free-form string with no validation
(`src/engine/mod.rs:7404-7409`) — `@lock here/frobnicate = …` is accepted — so
these lists document the set the engine *checks*, not an enforced enum.

---

## 5. BROKEN — `on_enter` never fires on a mob

> **diku-cookbook.md:120** — "| `greet` / `>greet_prog` | `on_enter` | On the mob
> or the room |", under the framing at `:112`; the cityguard recipe at `:456`
> defines `function on_enter(this, actor, room)` on an NPC.

**What the code does.** `do_move` fires `on_enter` on `target_ref` (the
destination room) and then `fire_global_hooks("on_enter", …)`
(`src/engine/mod.rs:7169-7170`); the `move_object` path does the same
(`:4977-4978`). `fire_global_hooks` reads `indexes().globals_by_hook`
(`:4788`), which is populated only from objects whose resolved tags contain
`system:global` (`:667`). Nothing iterates the destination room's contents.

**How verified — [EXEC].** NPC in the destination room defining `on_enter` that
emits a marker; `> north` produced the room description and no marker
(`expect-not` held).

**Suggested correction.** Change the Notes column to "on the **room** (or a
`system:global` object)", and rewrite the cityguard's greet half as a room hook,
an exit hook, or a global that filters on the actor's location — the same split
the doc already teaches at `diku-cookbook.md:512-517`. Note the recipe is
compile-checked by `tests/cookbook.rs` but exercised by no `.session` fixture,
which is why this survived.

---

## 6. BROKEN — `get_object()` results *are* pending-aware

> **mush-cookbook.md:96-97** — "`get_attr`, `has_attr` and `pick` do see your own
> pending writes … **but a `get_object()` snapshot taken before a `set_attr` will
> not show it**."

**What the code does.** `get_object` calls `object_to_value(..., Some(&mt))`
(`src/softcode/api.rs:727-731`) — the proxy metatable. A proxied
`object_to_table` returns an empty table carrying only `_hearth_ref`
(`:165-174`); `__index` resolves attrs through `resolve_attr(&pending, …)`
(`:548-558`) and title/description through `pending_title`/`pending_description`
(`:592-597`). All pending-aware.

**How verified — [EXEC].**
`@eval set_attr(actor,"zz",1) local snap = get_object(actor.ref_id) set_attr(actor,"zz",99) return "SNAP="..tostring(snap.attrs.zz)` → `SNAP=99`.

**Suggested correction.** "`get_attr`, `has_attr`, `pick` and the object proxies
(`this`, `actor`, `get_object(...)`) all see your own pending writes. What does
not is a **list** result — `get_room_contents`, `get_inventory`, `get_contents`,
`get_all_by_kind`, `find_by_tag`, `get_exits` return plain snapshots taken when
your script started (`src/softcode/api.rs:900`, `:1081`, `:1231`) — nor does
`pairs()` iteration over a proxy (`:440-443`)." softcode-guide.md:275-277 already
says the list half correctly; the MUSH cookbook contradicts it.

---

## 7. BROKEN — no lock type gates a `cmd_` hook

> **mush-cookbook.md:530-531** — "Builder-gated: put `@lock <bbs>/use =
> perm(builder)` on the object, or check a tag here."

**What the code does.** `dispatch_fallback` (`src/engine/mod.rs:5396-5455`)
performs no `check_lock` at all before `fire_hook`. The `use` lock is consulted
only by the builtin `use` verb (`:7268`).

**How verified — [EXEC].** `use` lock set to `has_tag(nope:nope)` on the global
via `@eval set_lock(...)`; typing the command still fired the hook.

Doubly broken in context: the BBS is a `Kind::Code` object, which
`World::objects_in` excludes (`src/world/mod.rs:245-249`), so it can never be
the target of `use` in the first place.

**Suggested correction.** Drop the `@lock …/use` suggestion; the in-hook tag/scope
check the recipe already performs is the only mechanism.

---

## 8. BROKEN — `"<message>` and `:<action>` need a trailing space

> **commands.md:10-11** — "| say `<message>` | `"<message>` |", "| emote
> `<action>` | pose, `:<action>` |"

**What the code does.** Input is split on the first space *before* matching
(`src/engine/mod.rs:3957-3960`), and the arms are the exact literals `"`
(`:3970`) and `:` (`:3986`). `"hello` becomes command `"hello`, misses every
arm, and falls through to `dispatch_fallback` → `Huh? Type 'help' for commands.`
(`:5453`).

**How verified — [EXEC].** `> "hello there` → `Huh?`; `> " hello there` → say.
`> :waves` → `Huh?`; `> : waves` → emote.

**Suggested correction.** Document as `" <message>` / `: <action>`, or strip the
sigil prefix in the parser.

---

## 9. BROKEN — `grid_can_move`'s signature

> **CLAUDE.md:174** — "`grid_can_move(actor, map, dir)` is a pure peek (same
> passability logic)"

**What the code does.** `src/softcode/api.rs:2303-2304` —
`create_function(move |lua, (map_name, x, y, dir): (String, i64, i64, String)|`.
Four arguments, map name first, explicit coordinates — it deliberately does not
take an actor (which is why `movement_blocked` is not consulted, as
softcode-guide.md:548-551 correctly notes). `types/hearth.d.luau:435` has it
right; CLAUDE.md does not.

**How verified — [READ]** (`src/softcode/api.rs:2303`; contrast `grid_move` at
`:2168-2169`, which *is* `(actor, map, dir)`).

**Suggested correction.** `grid_can_move(map, x, y, dir)`.

---

## 10. BROKEN — the multi-line editor does not survive a disconnect

> **softcode-guide.md:829-830** — "The buffer lives on your player object's
> attrs, so it survives a disconnect mid-edit."
> **commands.md:168-169** — same claim for `@eval`.

**What the code does.** Only the *text* is on the player
(`_program_buffer`/`_eval_buffer`). The editor **mode** is per-session
(`Session.editor`, `src/engine/mod.rs:437`), initialized `None` on every new
session (`:3254`), and set only by `set_editor` (`:3860-3864`) — nothing restores
it on reconnect. Worse, reopening the editor overwrites the saved buffer:
`@program <ref>` with no `=` re-seeds `_program_buffer` from the object's current
script (`:4540-4551`), and `@eval` with no args resets `_eval_buffer` to `""`
(`:6744-6748`). Either way the in-progress edit is destroyed.

**How verified — [READ]** (`src/engine/mod.rs:437`, `:3254`, `:3860`, `:4540`,
`:6744`).

**Suggested correction.** Drop the survival claim from both files, or say the
buffer attr persists but the edit session does not resume and reopening
discards it.

---

## 11. BROKEN — "transient attributes (nattrs)"

> **CONTEXT.md:18-20** — "Persisted attributes survive restarts. Transient
> attributes (nattrs) die on restart."

**What the code does.** Nothing. `grep -rn nattr src/ types/ web/src docs/`
returns zero hits. `GameObject::attrs` (`src/world/object.rs`) is one
`HashMap<String, serde_json::Value>` and all of it is serialized to SQLite. The
only non-persisted per-object store is the `on_tick` `state` table, which is
itself persisted (`src/engine/mod.rs:1267-1271`, `src/db.rs`).

**How verified — [EXEC]** (the grep) **+ [READ]**.

**Suggested correction.** Delete the sentence, or replace it with the `state`
table's actual semantics.

---

## 12. BROKEN — `generate_dungeon`'s first argument

> **softcode-guide.md:614** — "| `generate_dungeon(opts)` | string | …"
> **SKILL.md:424** — same.

**What the code does.** `src/softcode/api.rs:2009-2010` —
`create_function(move |_, (seed, config): (String, Option<Table>)|`. The first
argument is a **seed string**; the options table is second and optional.
softcode-guide.md's own example at `:1042` uses it correctly
(`generate_dungeon("my-seed")`), contradicting its own table.
`types/hearth.d.luau:384` is right.

**How verified — [READ]**.

**Suggested correction.** `generate_dungeon(seed, config?)` in both tables.

---

## 13. BROKEN — `trigger`'s `data` is not an attr

> **SKILL.md:320** — "`trigger(ref, hook, data?)` | Fire a hook on another
> object. Optional `data` (table) is available as `_trigger_data` attr during
> execution."

**What the code does.** `data` is passed as the hook's 4th argument, a real Lua
table (`src/softcode/mod.rs:1996-1999`; `src/engine/mod.rs:4956-4963`). The code
comment at `:4958` reads "no more `_trigger_data` attr", and grepping `src/`
finds `_trigger_data` only in that comment and in a test name asserting its
absence (`:11726`, `:11772`). SKILL.md also omits `trigger`'s 4th parameter
(fire-as-actor), which softcode-guide.md:428-432 documents correctly.

**How verified — [READ]** + `grep -rn _trigger_data src/`.

**Suggested correction.** "`trigger(ref, hook, data?, actor?)` — `data` arrives
as the hook's 4th parameter; `actor` fires the hook as that actor."

---

## 14. MISLEADING — how `cmd_` dispatch actually resolves

Three separate misconceptions, in four docs.

**(a) Dispatch never matches on the argument.**

> **softcode-guide.md:101-103** — "If the player types `push button`, the engine
> finds the button object in the room, sees it has `cmd_push`, and runs it."

`find_cmd_hook` (`src/softcode/hooks.rs:546-555`) returns the **first** candidate
that responds to `cmd_push`, in fixed resolution order, ignoring `args`
entirely.

*How verified — [EXEC].* Two items in one room, `button` and `lever`, both with
`cmd_push`. Typing `push lever` ran the **button's** hook
(`expect: BUTTON-PUSH` passed).

*Correction:* "the engine runs the first object in resolution order that defines
`cmd_push`, whatever the player named — the hook itself must interpret `args` to
figure out which thing was meant."

**(b) Builtin verbs are matched first and never reach softcode.**

> **mush-cookbook.md:99-101** — "The engine turns the typed verb into a hook
> name: `order pizza` looks for `cmd_order`."
> **lpmud-cookbook.md:309-316**, **diku-cookbook.md:122** — same framing.

`run_command` matches builtins in a `match cmd.as_str()` before any softcode
(`src/engine/mod.rs:3965-4080`); `dispatch_fallback` is reached only from the
`_ =>` arms (`:4075`, `:4080`). Reserved: `look`/`l`, `say`/`"`, `go`,
`inventory`/`inv`/`i`, `get`/`take`, `put`/`place`, `drop`, `use`,
`examine`/`ex`, `whisper`, `emote`/`pose`/`:`, `quit`/`q`, `who`, `help`/`?`,
and every `@`-verb.

*How verified — [EXEC].* `cmd_use`, `cmd_look`, `cmd_who` on a `system:global`
object never fired.

**(c) `cmd_` hooks beat exit names.**

`dispatch_fallback` tries `cmd_` hooks (`src/engine/mod.rs:5423`) before exit
matching (`:5448`), so `cmd_north` silently disables walking north.
*How verified — [EXEC]:* `cmd_north` on a global made `> north` print the hook's
output instead of moving.

*Correction (b+c):* add to the MUSH rule-3 box and the LPMUD `add_action`
section: "The engine's own verbs are matched first and never reach softcode …
naming a hook after one of them is silent dead code. Exit directions resolve
*after* `cmd_` hooks, so `cmd_north` shadows walking north."

softcode-guide.md:89-90 gets (b) right ("something that doesn't match a builtin
command") and only needs (a).

---

## 15. MISLEADING — `on_damage` / `on_death` are never fired by the engine

> **softcode-guide.md:146-147** — "| `on_damage` | When this object takes damage |",
> "| `on_death` | When this object dies |"
> **CLAUDE.md:157** and **SKILL.md:161** list them beside genuine engine hooks.

**What the code does.** Grepping `src/` for `on_damage`/`on_death` outside
`hooks.rs` returns only test code (`src/engine/mod.rs:9683-9801`,
`src/loader.rs:1293`). No `fire_hook` call site names either. They are
*known* hook names (so `derive_hooks` and `@programs` recognise them) that a
game must fire itself with `trigger()`.

**How verified — [READ]** (exhaustive grep + the `fire_hook`/`fire_global_hooks`
/`fire_lifecycle_hook` call-site inventory at `src/engine/mod.rs:940-7344`).
Every other one of the 36 `KNOWN_HOOKS` does have an engine call site.

The cookbooks already say this (`diku-cookbook.md:178-181`,
`lpmud-cookbook.md:86`); the reference does not.

**Suggested correction.** Mark both rows "**never fired by the engine** — fire it
yourself with `trigger(target, "on_death", …)`".

---

## 16. MISLEADING — `@eval`'s instruction budget

> **softcode-guide.md:852-853** — "runs under the same instruction
> [Budget](#sandboxing) as any hook"
> **commands.md:157** — same.

**What the code does.** `cmd_eval` uses `Budget::for_eval()` = **50,000,000**
instructions (`src/softcode/mod.rs:1283-1288`, used at `src/engine/mod.rs:6814`,
`:2958`, `:3031`); a hook uses `Budget::default()` = **200,000**
(`src/softcode/mod.rs:1291-1298`). 250× larger, and the doc comment on
`for_eval` explains exactly why.

**How verified — [READ]**.

**Suggested correction.** "runs under a deliberately much larger one-shot budget
(50M instructions vs a hook's 200k) — large enough for a world sweep, still
finite, and the world is frozen while it runs."

---

## 17. MISLEADING — `on_say` is a room hook only

> **diku-cookbook.md:123** — "| `speech` / `>speech_prog` | `on_say` | Read the
> text from the `_say_message` attr |", under the `:112` framing that the hook
> lives on the object the trigger was attached to.

**What the code does.** `cmd_say` checks only whether the **room** responds
(`src/engine/mod.rs:7335-7338`), stamps `_say_message` onto the room object
(`:7342`), fires `on_say` on the room (`:7344`), then removes the attr (`:7346`).
No other object is consulted.

**How verified — [EXEC].** An NPC in the room defining `on_say` produced nothing
on `> say hello`.

**Suggested correction.** Add "on the **room**" to the Notes column (matching
`lpmud-cookbook.md:230`) and note `_say_message` lives on the room and only for
the duration of that hook run.

---

## 18. MISLEADING — `all_inventory(ob)` mapping

> **lpmud-cookbook.md:99** — "| `all_inventory(ob)` | `get_inventory(ref)` /
> `get_contents(ref)` |"

**What the code does.** `get_inventory` (`src/softcode/api.rs:1077-1091`) and
`get_contents` (`:1226-1240`) apply the identical filter: `objects_in(&r)`
restricted to `obj.kind == Kind::Item`. LPC's `all_inventory` returns everything
in the environment, living included; the equivalent is `get_room_contents` →
`World::objects_in` (`src/world/mod.rs:245-249`), which excludes only `Exit` and
`Code`. The MUSH cookbook warns about precisely this trap
(`mush-cookbook.md:1188-1191`, `:1327-1328`), so the two contradict each other.

**How verified — [READ]**.

**Suggested correction.** `| all_inventory(ob) | get_room_contents(ref) —
get_inventory/get_contents are items-only |`.

---

## 19. MISLEADING — `tick_interval` units and the heartbeat

> **mush-cookbook.md:812-813** — "`on_tick` fires every second (or every
> `tick_interval` seconds)"
> **CONTEXT.md:88** — "The global heartbeat of the engine, fired at a fixed
> interval (1 second)."

**What the code does.** `tick_interval` is a per-object **tick multiplier**
(`src/engine/mod.rs:659-664`, `unwrap_or(1)`, stored as
`tickables: Vec<(String, u64)>`). Wall-clock tick length is the config key
`tick_secs` (`src/config.rs:12`, default `1` at `:61`) — configurable, not fixed.
`lpmud-cookbook.md:586` gets this right.

**How verified — [READ]**.

**Suggested correction.** MUSH: "fires once per heartbeat (`tick_secs`, default
1s), or every `tick_interval` **ticks**". CONTEXT.md: "fired at a fixed interval
(`tick_secs`, default 1 second)". `mush-cookbook.md:896-897` should also pick
one of its two mechanisms rather than naming `tick_interval` and then using a
`state.n % 10` counter.

---

## 20. MISLEADING — "There is no `@force`, no `@chown` dance"

> **mush-cookbook.md:665**

**What the code does.** `"@force" => self.cmd_force(...)`
(`src/engine/mod.rs:4033`) and `"@chown" => self.cmd_chown(...)` (`:4046`) both
exist. The doc's own concept map at `mush-cookbook.md:54` maps `@force
player=cmd` → `run_command_as`.

**How verified — [READ]**.

**Suggested correction.** State the intended point: "you don't need to force
anyone or reassign ownership in order to write another player's attrs." Both
verbs exist (`@force` admin-only, `@chown` admin-only).

---

## 21. MISLEADING — seven smaller command-reference claims

All **[READ]** unless marked.

| commands.md | Claim | Reality |
|---|---|---|
| :136 | `@reload` listed under "Admin commands … require the `admin` scope" | `cmd_reload` gates on `Scope::Builder` (`src/engine/mod.rs:6849`); already listed correctly at :57. Delete the duplicate row. |
| :199-200 | "the same convention `@test`'s path argument and `game_dir` itself already use" | `@import`/`@export` resolve against the process CWD (`:6461`, `:6502`); `@test <file>` resolves against `game_dir` (`:7704-7705`) — as commands.md:59 itself says. Drop `@test` from the comparison. |
| :32 | `@open <direction> = <room_ref>` | `cmd_open` checks only that the target exists (`:4132`); no `Kind::Room` check. **[EXEC]:** `@open weird = #7` onto an *item* succeeded, and traversing it put the player inside the item. Say `<target_ref>`, or add a room check. |
| :41-42 | `@describe`/`@name` with a `<ref> =` prefix | If the text before `=` doesn't resolve, the split is discarded and the **whole** argument is applied to the current room (`:4150-4172`, `:4408-4429`). **[EXEC]:** `@describe #999 = a new description` → `Description set on #1.` and the room's description became the literal `#999 = a new description`. |
| :269-273 | "the one case `@export` can't represent" | Four skip reasons: carried by a player (`src/import_export.rs:1409`), unresolvable location (`:1411`), exit key collision (`:1219`), exit with unresolvable from/to (`:1298`). |
| :373 | web client at `/play` | The router registers only `/ws` and `/api` (`src/net/web.rs:69-70`); everything else is the SPA catch-all (`:86-88`, `:141-149`). The Svelte router knows only `/builder/workspace` (`web/src/App.svelte:110`). Say `http://localhost:8000`. |
| :85 | "1 tick = 1 second" | `tick_secs` is configurable (`src/config.rs:12`, `:61`). |

---

## 22-25, 27-28, 38. STALE numbers, lists and layouts

**22. Test count — CLAUDE.md:21 ("`cargo test` # 433 tests") and :333 ("433 tests
across: …").** Actual: 435 passed + 1 ignored = **436**. **[EXEC]:** `cargo test`
→ `429 passed; 1 ignored` (lib) + `3` (cookbook) + `1` (game_smoke) + `2`
(session_test). The per-module breakdown at :333-338 should be re-derived at the
same time.

**23. ADR count — CLAUDE.md:151 ("6 architectural decision records (ADR
0001–0006)") and :329 ("See `docs/adr/` (6 ADRs)").** `docs/adr/` holds **8**:
0001–0008, including `0007-authoring-shares-the-intent-mechanism.md` and
`0008-effective-actor-completes-puppeting.md` (both referenced by name elsewhere
in CONTEXT.md). **[EXEC]:** `ls docs/adr/`.

**24. REST action count — CLAUDE.md:197 ("POST /api with ~44 actions").** The
`ApiRequest` enum (`src/engine/mod.rs:112`) has **62** variants. **[EXEC]:**
brace-matched parse of the enum body. Actions absent from the parenthetical
list include `RunTests`, `PreviewHook`, `ListWorldSlice`, `ListRefCandidates`,
`GetScriptVersion`/`ListScriptVersions`/`RevertScript`, `LockScript`/`UnlockScript`,
`DetachObject`, `SetArchetype`, `SetLocation`, `CheckProgram`, `EvalPreview`,
`SaveWorld`, `Me`, `ListAreas`/`ListRooms`/`ListExits`/`ListHooks`/`ListLibs`/
`ListMaps`/`ListProgramsAll`, `CreateLibrary`, `InkPlay*`.

**25. API function count — CLAUDE.md:173 ("~91 Luau API functions" with a
per-category breakdown).** **108** names are registered. **[EXEC]:** the same
install-site scan the in-repo guard test uses
(`env.set("…")` in `api.rs` + `run_hook_level`'s body in `mod.rs`, plus
`globals().set("…")` in `noise.rs`/`grid.rs`), which the guard test itself
asserts is `> 80`. Full list in §32. The sub-counts drift too — e.g. "read (21)"
against the read table's actual membership.

**27. commands.md list drift.**
- **:124** — "`@grant <username> <scope>` (`player`, `builder`, `admin`)". A
  fourth scope, `puppeteer`, is grantable (`src/accounts.rs:10-15`, parsed at
  `:27-35`, gates `@puppet` at `src/engine/mod.rs:8059`). **[EXEC]:**
  `@grant admin7 puppeteer` → `Granted 'puppeteer' scope to admin7.`
- **:112** — lock types omit `put` and imply an enum; the type is stored
  unvalidated (`src/engine/mod.rs:7404-7409`). **[EXEC]:**
  `@lock here/frobnicate = perm(builder)` → `Lock 'frobnicate' set on #1.`
- **:306-307** — "only these four literal first words are ever treated as a
  subcommand". `is_known_subcommand` matches **six**: `eval | program | import |
  export | session-test | migrate` (`src/cli.rs:71`).

**28. SKILL.md list drift.**
- **:96** — "`kind` is one of: `room`, `item`, `npc`, `exit` (never `player`)".
  `Kind::parse` also accepts **`code`** (`src/world/object.rs:43-53`) — and
  SKILL.md's own recipe at :473 uses `kind = "code"`. (`spawn` refuses both
  `player` and `code`, `src/softcode/api.rs:1683-1692`, but the loader accepts
  `code` — that's the case this line is about.)
- **:129** — `can_` list omits `can_put`.
- **:161** — `on_` list omits `on_put`, `on_hour`, `on_day`, `on_dawn`, `on_dusk`.
- **:245** — "Read API (22 functions + 1 value)": the table omits
  `find_in_inventory`, `find_player`, `find_exit`, `responds_to`, `get_time`,
  `get_nearby`/`get_rooms_in_radius` (tabled separately), `has_attr`-adjacent
  helpers. **:296** — "Write API (25 functions)": omits the 24 names in §32.
- **:556-575** — the "File organization" tree shows `world/lib`, `world/system`;
  the reference game's actual layout is `game/world/*` + `game/std/*` +
  `game/lib/*` (`../the-last-stag-mud/game/`). SKILL.md:7 already says this
  correctly, so the two contradict.

**26. CLAUDE.md:356-387, "The Last Stag (game)".** Every path in the tree is
wrong. **[EXEC]:** `cat ../the-last-stag-mud/hearth.toml` →
`game_dir = "../the-last-stag-mud/game"`, `spawn_room = "world/town/crossroads"`.
`ls ../the-last-stag-mud/game` → `lib maps std terrain.toml tests themes wasm
world`. There is no `world/system/` (it is `game/std/`), and `game/lib/` holds
only `dialog.luau` + `wilderness.luau` — `text`/`str`/`collections`/`random`/
`state_machine`/`signal`/`grids`/`Grid3D` are the **framework's** embedded
stdlib (`src/loader.rs:1100-1116`, `include_str!("../lib/*.luau")`), not the
game's. CLAUDE.md's own "Tiering + locking" section (`:308-320`) describes the
`game/world` + `game/std` layout correctly, so the file contradicts itself.
The config sample at :296 (`spawn_room = "town/crossroads"`,
`game_dir = "../the-last-stag-mud/world"`) is wrong for the same reason.

**38. CLAUDE.md source layout and config sample.**
- The `src/` tree at :41-140 omits `attr_schema.rs` (225 lines, and its feature
  is described at :168), `clock.rs` (252 lines, feature described at :159),
  `engine/authoring.rs` (195), `softcode/wasm.rs` (601), and `lib.rs`. The
  repo-root listing omits `lib/` (the embedded Luau stdlib), `plugins/`,
  `benches/`, and `clients/`.
- The `hearth.toml` sample at :296 omits `max_characters` (`src/config.rs:16`,
  default 3), which is a real key and is settable per-account with `@maxchars`.
  Every other documented key does exist, including all of `[clock]`
  (`src/clock.rs:22-45`) — verified field by field.
- `docs/plans/` is described at :152 as "(archetype traits + live debugger,
  terrain attr schemas, Mudlet/GMCP terrain-legend + tiles)"; the directory holds
  `archetypes.md`, `attribute-schema.md`, `mudlet-client-integration.md`,
  `softcode-versioning.md`.
- The `docs/` listing at :141-156 never mentions the three cookbooks, which are
  the largest documents in the repo (3,448 lines combined).

---

## 29. MISSING — the ownership-authority model on intents

Neither softcode-guide.md nor SKILL.md mentions that a softcode write can be
**refused for lack of ownership**. Both describe only atomic rollback
("If any intent is invalid, the entire batch is rolled back",
softcode-guide.md:365-367).

**What the code does.** `apply_to` checks `may_modify(world, authority, target)`
before every mutating intent and returns `"<verb>: permission denied on
'<ref>'"` otherwise (`src/softcode/mod.rs:489-501`, used at `:656`, `:668`, and
throughout). `authority` is the running script's object's `owner_ref`; `None`
(every file-authored object, since neither the loader nor `@import` sets an
owner) is system-trusted and unrestricted, while **any owned object's script may
only modify what that same owner owns** — including, deliberately, being unable
to touch unowned system objects. `Intent::Move` is the documented exception
(`:697-700`). Firing a *lifecycle* hook via `trigger` is restricted the same way
(`is_lifecycle_hook`, `src/softcode/mod.rs:603-616`).

**How verified — [READ]**.

CONTEXT.md:71-77 *does* define "Ownership authority" and "Authoring authority"
correctly. The reference and the skill — the two documents someone actually
writes softcode from — never mention it, so a builder writing a script on their
own object hits `permission denied` with nothing to look up.

**Suggested correction.** Add a "Who may write what" section to
softcode-guide.md's Write API, cross-referencing ADR 0007 and CONTEXT.md.

---

## 30. MISSING — the two silent softcode quotas

- **`OWNER_TIMER_QUOTA = 100`** (`src/softcode/mod.rs:601`). When an owner
  already has 100 pending timers, further `after()` calls are **silently
  dropped** — `continue` plus a `tracing::warn!`, no error to the script and no
  message to the player (`src/engine/mod.rs:4835-4842`, quota computed at
  `:4797-4811`). softcode-guide.md:494-510 and SKILL.md:325 describe `after()`
  with no mention of a cap.
- **`EMIT_BATCH_LIMIT = 50`** (`src/softcode/mod.rs:592`). A batch running under
  a non-`None` authority that queues more than 50 emit-family intents is refused
  **whole** with `"emit: N messages in one run exceeds the limit of 50"`
  (`src/softcode/mod.rs:632-651`). Undocumented.

**How verified — [READ]**.

---

## 31. MISSING — WASM plugins are undocumented everywhere

`grep -in wasm CLAUDE.md README.md CONTEXT.md docs/*.md .claude/skills/softcode/SKILL.md`
returns **zero** hits. Yet:

- `wasm_call(module, func, arg?)` is a registered softcode API function
  (`src/softcode/api.rs:2641`) and is declared in `types/hearth.d.luau:527`;
- plugins load from `<game_dir>/wasm/*.wasm` on every boot and on
  `@reload-world` (`src/softcode/wasm.rs:10`, `:239`;
  `src/softcode/mod.rs:1399`; reload at `src/engine/mod.rs:6361-6364`);
- `src/softcode/wasm.rs` is 601 lines with an instance pool, and the reference
  game ships `game/wasm/`.

This is exactly the class CLAUDE.md:206-215 enumerates ("Code and narrative
assets below still load from disk on every boot and are never persisted to the
database" — `lib/*.luau`, `**/*.ink`, `themes/*.toml`). `<game_dir>/wasm/*.wasm`
belongs in that table and is absent from it.

**How verified — [EXEC]** (the greps) **+ [READ]**.

---

## 32. MISSING — API functions absent from the reference and the skill

Ground truth: 108 registered names (see §25 for the extraction method — it is
the same scan the in-repo guard test
`help_panel_api_reference_matches_installed_functions` uses, and that test keeps
`web/src/components/code/hearth-api.js` and `types/hearth.d.luau` in exact sync.
`docs/softcode-guide.md` and `SKILL.md` are **not** covered by any guard, which
is why they drift).

**Absent from `docs/softcode-guide.md` (18):** `clear_attr`, `clear_lock`,
`clone`, `clone_object`, `pass`, `run_command_as`, `set_aliases`,
`set_archetype`, `set_lock`, `update_exit`, `wasm_call`, and the whole ink
family (`ink_start`, `ink_continue`, `ink_choose`, `ink_goto`, `ink_end`,
`ink_get_var`, `ink_set_var`). CLAUDE.md:173 counts the ink functions in its
API total, and the guide has a "Ink dialog" feature bullet nowhere.

**Absent from `SKILL.md` (24):** the 18 above plus `find_exit`,
`find_in_inventory`, `find_player`, `get_time`, `grid_move`, `grid_can_move`,
`responds_to`.

**How verified — [EXEC]:** word-boundary regex of each registered name against
each document. `types/hearth.d.luau` came back with **zero** missing, as its
guard test guarantees.

Related signature gaps, both files: `move_object(ref, destination)` omits the
third `opts` table (`announce` / `fire_hooks`, `types/hearth.d.luau:254`;
CLAUDE.md:173 does mention it).

---

## 33. MISSING — `on_whisper` / `on_emote` carry no payload

> **softcode-guide.md:150-151** — "| `on_whisper` | When an actor whispers in this
> room |", "| `on_emote` | When an actor emotes in this room |"

**What the code does.** Both fire on the room with `args = None` and no attr
stamped (`src/engine/mod.rs:5952` for whisper, `:5989` for emote) — unlike
`on_say`, which stamps `_say_message` (`:7342`). Neither suppresses the default
broadcast; the message goes out first and the hook runs after
(`:5946-5952`, `:5977-5989`). `on_whisper` additionally does **not** fire when
the whisper target isn't found (`:5955-5957`).

**How verified — [READ]**.

**Suggested correction.** Note that these two hooks are notifications only —
they cannot read the message text and cannot suppress or alter delivery.

*(Correcting an earlier suspicion: `on_emote` **is** fired, at
`src/engine/mod.rs:5989`.)*

---

## 34. MISSING — `spawn` accepts three more keys

> **softcode-guide.md:395-402** shows `key`, `kind`, `title`, `description`,
> `location`. **SKILL.md:314** shows the same five.

`spawn` also reads `owner` (`src/softcode/api.rs:1725-1729`), `archetype`
(`:1732-1764` — a dbref, an object table, or a **file key** resolved like
`resolve_key`), and `ref` (`:1718-1723`, an explicit dbref). `archetype` in
particular is the softcode entry point to the whole archetype system and is
documented in neither file. `types/hearth.d.luau:346` (`SpawnOpts`) has them.

Also: softcode-guide.md:397 says kind is `"item", "npc", or "room" (not
"player")`. `spawn` rejects `player` **and** `code` (`:1683-1692`) but accepts
`exit`. **[EXEC]:** `@eval local r = spawn({key="e1", kind="exit"}) return r`
returned a fresh dbref. (The engine's own error string, "want room, item, or
npc", is wrong too.)

**How verified — [READ] + [EXEC]**.

---

## 35. MISSING — `@eval` binds `actor`

> **softcode-guide.md:842-844** — "it runs arbitrary Luau against the live world
> with **no attached object, hook, or lock check**"

True about `this`, but `run_eval` installs one extra global the docs never
mention: `actor`, the object table for the caller
(`src/softcode/mod.rs:2074-2079`, "The caller running the eval, for
convenience"). **[EXEC]:** `@eval return get_attr(actor, "yy")` works.

Worth a line in both softcode-guide.md's `@eval` section and commands.md:155.

---

## 36. MISSING — commands that exist but aren't in `docs/commands.md`

All at `src/engine/mod.rs:4001-4066` in the dispatcher, with the handler line
noted. **[READ]** unless marked.

`@password` (`:4008`/`:5592`), `@email` (`:4009`/`:5611`),
`@display <accessible|visual>` (`:4066`/`:6872` — softcode-guide.md:973-975 uses
it, commands.md never lists it), `@token list|revoke` (`:5114`, `:5130` — only
`@token create` is documented, at commands.md:321), `@charlist`/`@charcreate`/
`@charswitch`/`@chardelete` (`:4037-4040`/`:7847-8042`), `@puppet`/`@unpuppet`
(`:4043-4044`/`:8043`, `:8077`), `@chown` (`:4046`/`:8119`, admin),
`@archetype` (`:4047`/`:4193`, builder), `@dialogue` with its
`show|edit|test|clear|export` subcommands (`:4048`/`:6515`, builder),
`@maxchars <user> <n>` (`:4061`/`:8096`, admin). `@abort` appears in
commands.md prose (:73, :168) but in no table.

**Undocumented aliases:** `@desc` (`:4012`), `@tel` (`:4016`), `@dialog`
(`:4048`), `@chparent` (`:4047`), `@tokens` (`:4065`).

**Undocumented behaviour on documented commands:**
- `@destroy <ref> --cascade` — `@destroy` on an archetype with live instances is
  refused unless `--cascade` is passed, and the flag is only recognised as a
  trailing suffix (`:4263-4292`). commands.md:35 mentions neither.
- `get <item> from <container>` — parsed via `split_on_preposition(args, "from")`
  (`:5181-5183`, `:5233`) and advertised in the in-game help
  (`src/engine/commands.rs:382`), but commands.md:13 shows only `get <item>`
  while documenting the `put … in …` counterpart.
- `examine me` / `examine self` — special-cased to the actor
  (`src/engine/commands.rs:257`); commands.md:18 doesn't say so.
- `here` — commands.md:363 says it works "in `@set`, `@tag`, `@lock`". It also
  works in `@untag` (`:6313`), `@unlock` (`:7425`), `@alias` (`:7458`),
  `@clone` (`:7495`), `@programs` (`:4640`), and `@program`/`@rmprogram`/
  `@reload` via `resolve_object_ref` (`:4562-4571`). It does **not** work in
  `@locks` (`:7591-7597`) — **[EXEC]:** `@locks here` → `No object with ref
  'here'.`
- `@reload-world` — commands.md:135, :147-148 lists libs + ink + bytecode cache.
  It also reloads WASM plugins (`:6361-6364`), re-stamps `system:locked` from
  config (`:6384`), validates attr schemas (`:6385`), and fires the `on_reload`
  lifecycle hook (`:6392`).
- `@export` — commands.md:245-252 describes area TOML + `.luau`; `cmd_export`
  also emits `file_sources` (maps + `terrain.toml`) via `export_file_sources`
  (`:6505-6507`).
- CLI — the code block at commands.md:288-294 lists four subcommands; the binary
  documents six (`src/cli.rs:37-44`), with `session-test` and `migrate` building
  the world in-process from `--config`/`--db` and needing no token (`:46-47`).

---

## 37. MISSING — incomplete global-hook and resolution-order lists

- **softcode-guide.md:176-178 / SKILL.md:178** — "This includes `on_enter`,
  `on_leave`, `on_connect`, and `on_disconnect`, in addition to `cmd_*`."
  `fire_global_hooks` is generic over the hook name and is also called for the
  four game-clock rollovers, `on_hour`/`on_day`/`on_dawn`/`on_dusk`
  (`src/engine/mod.rs:1030-1039`). Those are `system:global`-only hooks, so this
  is the section where a reader would look for them. **[READ]**
- **softcode-guide.md:89-91** — "the engine searches objects in the room and the
  player's inventory". The real order is **the room object itself → objects in
  the room (excluding the actor) → the actor's inventory → `system:global`
  objects** (`src/engine/mod.rs:5411-5429`). Both ends are missing: a `cmd_` hook
  on the *room* works, and globals are the documented mechanism elsewhere in the
  same file. SKILL.md:176 states this correctly.
- **CONTEXT.md:108-110** and **docs/adr/0004-cmd-hooks-for-command-extensibility.md:3**
  — "builtin commands, then `cmd_` hooks on objects in the room and player
  inventory. First match wins." Same two omissions, plus neither says exits
  resolve *after* `cmd_` hooks (`:5423` vs `:5448`).

---

## 39. MISLEADING — CONTEXT.md's Program / Hook definitions

> **CONTEXT.md:56-58** — "**Program:** A Luau script attached to an Object via a
> named hook. The code lives on the object."
> **CONTEXT.md:60-62** — "**Hook:** A named slot on an Object where a Program can
> be attached."

This is the pre-refactor one-program-per-hook model. The current model is one
script per object: `GameObject` carries a single `ObjectScript` whose hooks are
top-level functions in one shared scope, and the engine *derives* the hook set by
parsing it (`src/softcode/hooks.rs:1-15`, `derive_hooks`). A hook is not a slot
you attach to; it is a function name the parser finds. Every other doc
(softcode-guide.md:14-35, SKILL.md:103-111, CLAUDE.md:159-166) describes the
current model, so the glossary is the outlier — and it's the glossary that's
meant to fix the vocabulary. **[READ]**

**Suggested correction.** "**Script:** the single Luau chunk attached to an
Object; its top-level functions are the Object's hooks." / "**Hook:** a
top-level function in an Object's Script whose name the engine recognises …"

---

## 40-42. BROKEN — three wrong API signatures in `README.md`

**40. `set_program` does not exist.**

> **README.md:163** — "| `set_program(ref, hook, source)` | Installs a Luau
> program on the object |"

The installed API set is the 108 names in §32; `set_program` is not one of them.
The one-script-per-object refactor replaced it with `set_script(ref, source)`
(`src/softcode/api.rs`, `types/hearth.d.luau:370`).
**[EXEC]:** `@eval emit(actor, tostring(set_program))` → `nil`, while
`set_script`, `apply_template`, `clone`, `get_time` all printed `function: 0x…`.
*Fix:* `set_script(ref, source)` — "sets the object's whole behavior script;
hooks are top-level functions in it."

**41. `prompt`'s signature and semantics.**

> **README.md:179** — "| `prompt(ref, message)` | Prompts a player for input
> (fires `on_reply` with their response) |"

`src/softcode/api.rs:1975-1992` registers `prompt(actor, obj, hook)` — three
required arguments — storing `_prompt_object` and `_prompt_hook` on the actor.
The hook name is caller-supplied; there is no fixed `on_reply` and no `message`
parameter. `types/hearth.d.luau:584` and softcode-guide.md:521 both have it right.
**[EXEC]:** `pcall(function() prompt(actor, "What is your name?") end)` →
`false  bad argument #3: error converting Lua nil to String`.
*Fix:* `prompt(actor, obj, hook)` — "the actor's next line fires `<hook>` on
`obj` instead of running as a command; emit the question yourself."

**42. `is_carrying`'s second argument.**

> **README.md:146** — "| `is_carrying(actor, item)` | Returns `true` if actor has
> item in inventory |"

`src/softcode/api.rs:1189-1198` takes `(actor, item_tag: String)`, runs
`parse_tag`, and looks for any inventory object whose *resolved tags* contain it.
Passing a dbref never matches. softcode-guide.md:344 has it right.
**[EXEC]:** created `#6`, `get torch` succeeded, then
`is_carrying(actor, "#6")` → `false`.
*Fix:* `is_carrying(actor, "category:key")`.

---

## 43-45. BROKEN — `docs/getting-started.md` teaches the pre-refactor per-hook forms

**43. `@program <ref>/<hook> = …`** (getting-started.md:82,
`@program #13/cmd_light = function cmd_light(...)`). `cmd_program`
(`src/engine/mod.rs:4516-4561`) splits on `=` and passes the whole left side to
`resolve_object_ref`, which only special-cases `here`. Its own usage string is
`Usage: @program <ref> [= <luau source>]  (defines the object's whole script;
hooks are functions in it)`.
**[EXEC]:** `@program #6/cmd_light = …` → `No object with ref '#6/cmd_light'.`
The corrected `@program #6 = function cmd_light(this, actor, room, args)
emit(actor, "LIT-OK") end` installs, and `light torch` then prints `LIT-OK`.
The surrounding prose ("`@rmprogram` to remove them", plural) needs the same
treatment: `@rmprogram <ref>` clears the whole script
(`src/engine/mod.rs:4672-4692`).

**44. `hearth program get #5/on_look`** (getting-started.md:131, :133).
`cmd_program_get` (`src/cli.rs:382-415`) takes the first positional verbatim as
`ref_id` and sends `{"action":"get_script","ref_id":…}` — no `/hook` split.
`PROGRAM_USAGE` (`src/cli.rs:62-65`) is `hearth program get <ref>`, documented
at `:29` as "Print an object's whole script source to stdout." **[READ]**
*Fix:* `program get #5 > look.luau` / `program set #5 look.luau`.

**45. `hearth program history`/`restore`, `@program/history`/`@program/restore`**
(getting-started.md:138). `cmd_program` in the CLI dispatches only `get` and
`set` (`src/cli.rs:367-379`); the engine's `cmd_program` has no history branch.
Version history exists **only** as REST actions — `ListScriptVersions`,
`GetScriptVersion`, `RevertScript` (`src/engine/mod.rs:211-215`) — consumed by
the web builder. **[READ]** (and the in-game help's `@program/history` is dead
too — see the unverified-suspicions list, item 6).
*Fix:* delete the sentence or point at the builder / the REST actions.

---

## 46. BROKEN — the binary is `hearth-mud`, not `hearth`

`Cargo.toml:2` — `name = "hearth-mud"`, with no `[[bin]]` rename. The build
produces `target/debug/hearth-mud`; `justfile:35` installs
`target/release/hearth-mud`; `Dockerfile:20,23` copies and entry-points
`hearth-mud`. Every documented invocation — `hearth eval`, `hearth program
get|set`, `hearth import`, `hearth export`, `hearth session-test`,
`hearth migrate` — names a command that is not installed.

Affected: CLAUDE.md:44, :45, :90, :162, :183, :188, :263, :346;
commands.md:280, :285, :289-293, :296; getting-started.md:131, :133, :138;
migrations.md:34, :94, :116, :117; mush-cookbook.md:1839; diku-cookbook.md:76.
The CLI's own usage string (`src/cli.rs:25`) also says `hearth <subcommand>`.

**How verified — [EXEC]** (`ls target/debug/hearth*`, `grep` of Cargo.toml /
justfile / Dockerfile).

**Suggested correction.** Either add `[[bin]] name = "hearth"` to `Cargo.toml`
(making every doc true at once, and matching `src/cli.rs:25`), or rewrite the
examples as `hearth-mud <subcommand>`. The former is almost certainly the intent.

---

## 47. BROKEN — ADR 0005's determinism claim

> **docs/adr/0005-hybrid-tick-system.md:7** — "Tick order is deterministic
> (sorted by `stable_ref`) but otherwise unspecified — no priority system yet."

`src/engine/mod.rs:1059-1077` iterates `self.indexes().tickables` in order;
`tickables` is built at `:655-663` by `for obj in world.objects.values()` with
**no sort**, and `World::objects` is a `HashMap<String, GameObject>`
(`src/world/mod.rs:19`) using the default `RandomState` hasher — so iteration
order varies per process. `grep -rn stable_ref src/` returns zero hits: the
identifier does not exist. **[READ]**

**Suggested correction.** Either sort `tickables` by `ref_id` in
`DerivedIndexes::build` (making the ADR true), or amend the ADR to "tick order
is unspecified (hash-map iteration order)."

---

## 48-51. MISLEADING — four more `README.md` API-table errors

**48. `apply_template`'s table keys are discarded** (README.md:164 — "Installs
multiple programs from a `{ hook = source }` table"). `src/softcode/api.rs:2453-2482`
iterates `t.pairs::<Value, String>()`, **drops the key**, joins the values with
`\n\n`, and pushes one `Intent::SetScript`. So `{ on_get = "emit(actor,'hi')" }`
installs `emit(actor,'hi')` as top-level statements, not as a hook.
softcode-guide.md:474-492 describes it correctly (fragments, each defining its
own hook functions). **[READ]**

**49. `_trigger_data`** (README.md:204). Same defect as §13, in a second file;
README additionally omits `trigger`'s 4th fire-as-actor parameter
(`src/softcode/api.rs:1919`, `types/hearth.d.luau:323`). **[READ]**

**50. `random.weighted_choice`** (README.md:214). `lib/random.luau:27` defines
`Random.weighted(choices)`; the module exports `chance, dice, pick, range, roll,
sample, shuffle, weighted`. The other four names in that row are correct. **[READ]**

**51. `generate_dungeon(opts)`** (README.md:195). Same defect as §12, third file.
**[READ]**

---

## 52. MISLEADING — "areas are the unit of save/load"

> **CONTEXT.md:30** — "**Area:** The unit of persistence and organization. A
> group of rooms, objects, and exits that save and load together."
> **docs/adr/0003-in-memory-world-with-checkpointed-persistence.md:3** — "Areas
> are the unit of save/load." … "Demand-loading can be added later behind the
> area boundary without changing the object model — areas are already the
> save/load unit."

`src/db.rs:613 save_world(&World)` and `:748 load_world()` operate on the whole
world; the `objects` table (`:70-85`) has no `area` column, and the string
"area" does not occur anywhere in `src/db.rs`. "Area" exists only as the
loader's file-key namespace (`_file_key = "<area>/<key>"`). **[READ]**

This matters because ADR-0003 names it as the basis for a future demand-loading
path that the schema does not currently support.

**Suggested correction.** CONTEXT.md: define Area as the file/organizational
unit and the file-key namespace. ADR-0003: keep the decision (in-memory +
checkpoint), annotate the save/load-unit clause as superseded — checkpoints are
whole-world.

---

## 53-57. STALE — ADR and guide statements overtaken by the code

**53. ADR 0006's two lists.**
- `:6-11` — the built-in lock functions omit `is_owner` (`src/locks.rs:238`) and
  `game_time_between` (`:259`).
- `:13` — "Lock points the engine checks: traverse (exits), get, drop, enter
  (rooms), use, look, teleport." Real set (every `check_lock` site,
  `src/engine/mod.rs:5200, 5260, 5361, 7021, 7078, 7106, 7206, 7268`):
  `get, put, look, traverse, enter, drop, use`. Same `teleport` phantom as §4,
  same missing `put`. **[READ]**

**54. `docs/archetypes.md:185-187`** — "**A room instance can't inherit its title
through `@export`.** `RoomDef.title` is required…". `src/loader.rs:271-275` now
has `title: Option<String>` with `skip_serializing_if = "Option::is_none"`, and
its doc comment states verbatim that this exists so an archetyped room inherits
rather than exporting `title = ""`. Delete the bullet. **[READ]**

**55. `docs/adr/0006-lock-system.md:15`** — "A `can_` hook escape hatch (calling
a Luau program for complex lock logic) is a planned future extension but not part
of the initial implementation." Nine `can_*` hooks are in `KNOWN_HOOKS`
(`src/softcode/hooks.rs:24-63`) and fire alongside the lock check
(`src/engine/mod.rs:5205`, `:7086`, `:7111`, …). Historical ADR, but read today
it tells a builder a shipped feature doesn't exist. **[READ]**

**56. `docs/adr/0001:9` and `docs/adr/0002:3`** — "MCP endpoints" / "MCP tooling".
`grep -rni mcp src/` matches only **G**MCP, the telnet 201 option
(`src/net/telnet.rs:21`). There is no MCP server or endpoint. The real message
sources are telnet, the web WebSocket, `POST /api`, the tick scheduler, and the
in-process session-test/migrate paths. **[READ]** Low severity, but a reader
will go looking for a surface that isn't there.

**57. `docs/getting-started.md:66`** — `@create a flickering torch  # prints e.g.
"Item created: ... (#13)"`. Actual output: `Created a flickering torch (#6).`
**[EXEC]** (the neighbouring `@dig` comment, `Room created: The Dark Dungeon
(#12)`, *is* exact.) Everything else in that Building section works verbatim —
`@open down = #6`, `down`, `@describe`, `@describe #8 = …`, `@set #8/lit = true`,
`@set here/mood = "eerie"`, `@programs`, `@rmprogram` — all **[EXEC]** confirmed.

---

## 58. MISSING — `README.md`'s three reference tables are substantially incomplete

- **Config block (:25-36)** omits five real keys: `max_characters`,
  `load_world_files`, `locked`, `cors_allowed_origins`, and the whole `[clock]`
  table (`src/config.rs:16-51`). `load_world_files` and `locked` change boot
  semantics materially, and `cors_allowed_origins` is *the* production security
  knob. README never mentions the game clock, `get_time()`, or
  `game_time_between()` anywhere.
- **Hook table (:53-92)** lists 32 of the 36 `KNOWN_HOOKS` plus `cmd_<name>`.
  Missing: `on_hour`, `on_day`, `on_dawn`, `on_dusk`
  (`src/softcode/hooks.rs:52-57`, fired at `src/engine/mod.rs:1030-1039`).
- **API tables (:98-205)** cover roughly 55 of the 108 installed functions.
  Absent whole categories: ink (7), grid (`grid_new`, `grid_from_value`,
  `grid_move`, `grid_can_move`), noise (5), seeded RNG (4), coordinate math (6),
  plus `get_time`, `wasm_call`, `pass`, `responds_to`, `all_objects`,
  `clear_attr`, `find_player`, `find_exit`, `find_in_inventory`, `set_script`,
  `set_lib`, `set_aliases`, `set_archetype`, `set_lock`, `clear_lock`,
  `update_exit`, `clone`, `clone_object`, `run_command_as`. `move_object`'s
  `opts` table is also unmentioned.

**How verified — [READ]** against `types/hearth.d.luau` (which the guard test at
`src/softcode/api.rs:2784` pins to the engine name-for-name).

---

## Verified-correct claims (do not "fix" these)

Checked and found accurate, listed so a future pass doesn't re-litigate them:

- `can_traverse` fires on the **exit** and `can_enter` on the **destination
  room**, both receiving the room being left as `room`
  (softcode-guide.md:165-172 ↔ `src/engine/mod.rs:7086`, `:7111`). This was
  wrong historically; it is right now.
- `on_look` on a room fully suppresses default rendering
  (`src/engine/mod.rs:6957-6966`); `on_say` on a room suppresses the default
  broadcast and exposes `_say_message` for the duration of the hook
  (`:7334-7347`).
- All 36 `KNOWN_HOOKS` entries exist and CLAUDE.md's "36 hooks" count is right
  (`src/softcode/hooks.rs:24-58`).
- Every documented `hearth.toml` key exists in `src/config.rs`, including the
  whole `[clock]` table field-for-field (`src/clock.rs:22-45`). Only
  `max_characters` is missing in the other direction (§38).
- The Grid2D method table is complete and correct, including 1-indexing
  (`src/grid.rs:451-620`; `fill`'s `max(1)` at `:481-482`).
- Widget types `map` / `list` / `meter` / `text`, and panels rendering above
  Who's Here / What's Here / Exits (`web/src/components/GamePanel.svelte:9`,
  `web/src/components/Sidebar.svelte:28-32`).
- `emit`/`emit_room` newline → CRLF normalization (`src/engine/mod.rs:12332-12339`).
- `set_attr(ref, key, nil)` removes the attribute; read-your-writes on
  `get_attr`/`has_attr`/`pick`. **[EXEC]**
- `get_room_contents` = everything but exits and Code objects; `get_contents` =
  items only (`src/softcode/api.rs:896-905`, `:1225-1240`;
  `src/world/mod.rs:245-249`).
- `find_in_room` can never return an exit; `find_exit` matches direction or alias
  the way movement does (`src/softcode/api.rs:980-992`; `src/world/mod.rs:551-576`).
- Bundled-module contents (`random`, `collections`, `state_machine`, `signal`,
  `text`, `str`, `Grid3D`) — every documented function exists in `lib/*.luau`;
  they are `include_str!`-embedded and overridable by `<game_dir>/lib`
  (`src/loader.rs:1100-1147`).
- `@test` with no argument sweeps `.test.luau` files and never sweeps embedded
  object tests (`src/engine/mod.rs:7724-7748`).
- `hearth session-test <file>... [--config PATH] [--db PATH]` (`src/cli.rs:458-466`).
- The lock predicates `perm`, `has_tag`, `has_attr`, `in_inventory`, `is_kind`,
  `is_owner`, `time_between`, `game_time_between` all exist (`src/locks.rs`).
  (softcode-guide.md:769-779 omits `is_owner`, which CLAUDE.md documents.)
- **Every TOML key shown in every in-scope doc exists on its serde struct.** This
  was the highest-priority hunt (the `ExitDef::attrs` precedent) and it came up
  clean. Checked field by field against `src/loader.rs:254-424`
  (`AreaFile`/`RoomDef`/`ObjectDef`/`ExitDef`/`ScriptDef`/`ScriptSource`/
  `ProgramSource`), `src/migrate.rs:65-90`, `src/attr_schema.rs`, and
  `src/config.rs`. `archetypes.md`'s bestiary example was additionally run:
  **[EXEC]** `look grunt` / `look scout` printed
  `SNARL Goblin HP=5 ATK=2` / `SNARL Goblin Scout HP=3 ATK=2`, confirming
  delegation, per-field override, and hook-inheritance-bound-to-the-instance.
- **docs/migrations.md is accurate end to end:** removes-before-renames
  (`src/migrate.rs:227-243`), rename-onto-occupied-key a hard error with nothing
  written (`:265-271`, plan-before-mutate at `:288`), revision-string ordering
  and duplicate-revision rejection (`:151-167`), `_file_key`-only safety
  (`:121-123`, `:231`), the `migrations` table (`src/db.rs:170`, `:247`, `:267`),
  `<game_root>/migrations` = parent of `game_dir` (`src/migrate.rs:317-320`),
  and the `[--config PATH] [--db PATH] [--dry-run]` flags (`src/cli.rs:40-44`,
  `:597-618`). One nit: `<revision>_<slug>.toml` is convention only —
  `load_migrations` globs `*.toml` and orders by the `revision` **field**.
- **docs/archetypes.md's model section:** `MAX_ARCHETYPE_DEPTH = 32` cycle/depth
  guard (`src/world/mod.rs:15`, `:264-278`), single-parent `archetype_ref`
  (`src/world/object.rs:74`), copy-on-write `resolved_title`/`resolved_description`/
  `resolved_attr(s)` (`src/world/mod.rs:313-371`), tag **union** (`:404-410`),
  per-object `state` never delegated (`hooks::ensure_own_state_slot`),
  `--cascade` flattening before delete (`src/engine/mod.rs:4263-4305`),
  `spawn({archetype=…})` and `clone(ref)`.
- **ADR 0007 and ADR 0008 match the code precisely** (`src/engine/authoring.rs:55-98`
  `write_batch`; `src/softcode/mod.rs:489` `may_modify`; `src/engine/mod.rs:411`,
  `:437`, `:445`, `:456` — `Playing{actor_ref, puppet_ref}`, `Session.editor`,
  `character()`, `effective_actor()`).
- **getting-started.md's** empty-room default (`src/engine/mod.rs:807`, reachable
  because the repo's `hearth.toml` leaves `game_dir` commented out), the
  sibling-repo config it quotes (`game_dir = "../the-last-stag-mud/game"`,
  `spawn_room = "world/town/crossroads"` — matching the game's own
  `hearth.toml:8-9` exactly, unlike CLAUDE.md's copy, §26), `@token create`,
  `@grant`, `@scopes`, `@wall`, `@import`/`@export`, `@save`, and the
  `load_world_files` narrative.
- **README's** stdlib module list (all 8 match `src/loader.rs:1107-1116`, with
  game-`lib/`-overrides-stdlib at `:1120-1147`), the BBCode color set
  (`src/markup.rs:13-26`), `on_say`'s `_say_message`, `on_tick(this, state, room)`,
  the `system:global` lifecycle list, and the Docker example.
- `transfer_attr` whole-batch rollback; `derive_hooks` recognising exactly the
  declaration and assignment forms at top level; `state` surviving restarts and
  staying out of `examine`; `trigger`'s 4th fire-as-actor argument;
  `movement_blocked` honoured by `do_move` and `grid_move` but not
  `grid_can_move`; `muffle`/`blocked_sound` on `emit_radius` and
  `get_rooms_in_radius`; `test_*` functions with `@test #<ref>` and `ctx.this`.

---

## Unverified suspicions

Listed as suspicions precisely because they were *not* confirmed.

1. **CLAUDE.md:341, "21 Luau tests across str and collections modules."** Not
   re-counted. `lib/` holds `str.test.luau` and `collections.test.luau`; the
   number may or may not still be 21.
2. **diku-cookbook.md:932-935, "Hearth persists everything continuously."**
   Persistence is autosave-interval based (default 300s) plus
   offline-marking on disconnect. "Continuously" reads as an overstatement, but
   the sentence is loose enough that it may be intended figuratively.
3. **commands.md:8, `look <target>`.** After the `can_look`/`on_look` hooks it
   delegates to `do_examine` (`src/engine/mod.rs:7034`), so an ordinary player's
   `look sword` prints the full examine block (Ref / Owner / attrs / tags). Read
   but not confirmed as unintended, so not filed as a defect.
4. **mush-cookbook.md:815-817, `after()` surviving a reboot.** Reading supports
   it (`src/db.rs:132`, `:910`, `:931`) but it was not exercised across an actual
   restart.
5. **mush-cookbook.md:1743** declares `key = "blade"` and references it as
   `archetype = "std/blade"` with no `area = "std"` header shown. Cosmetic at
   worst.
6. **docs/adr/0005:5** — "the engine runs as many scripts as fit within the
   budget and defers the rest to the next tick." Literally true for `after()`
   timers (`src/engine/mod.rs:1082` pushes the undone one back), but the
   `on_tick` loop `break`s (`:1063`) and the skipped objects are simply not run —
   an object with `tick_interval = 60` that gets skipped waits another 60 ticks,
   not one. No load scenario was built to observe it.
7. **docs/adr/0003:3** — "SQLite is not queried during gameplay." True for world
   objects; `file_sources`, script version history, and script edit locks *are*
   read from the DB at runtime via REST. Whether that counts as "gameplay" is a
   judgement call, not a defect.
8. **README.md:55** — "Hook functions receive `(this, actor, room)`." `cmd_*`
   hooks take a 4th `args`, and a triggered hook takes a 4th `data` (and see §3
   for the lifecycle hooks). Not every hook's arity was enumerated against the
   README's table.
9. **A dead test in `src/loader.rs`.** Not a doc finding, but found while
   counting tests for §22: `skipped_areas_still_carry_their_program_hashes_forward`
   (`src/loader.rs:2203`) has **no `#[test]` attribute** — an earlier insertion
   orphaned it from the attribute above — so it never runs. Adding the attribute
   back makes it a 437th test; it was left unchanged here because this audit
   touches no code.
10. **The in-game `help` text is stale where `docs/commands.md` is right.**
   `src/engine/commands.rs:409-417` still advertises
   `@program <ref>/<hook> = <luau>`, `@rmprogram <ref>/<hook>`,
   `@reload <ref>/<hook>` and `@program/history|restore|diff`, none of which are
   in the dispatcher. **[EXEC]:** `@program/history #1/on_get` → `Huh? Type
   'help' for commands.` That's a code bug rather than a doc bug, so it is here
   rather than in the table above.
