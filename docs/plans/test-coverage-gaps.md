# Test coverage gaps — ranked by expected bug yield

Audit of where adding tests is most likely to find real, *silent* bugs. Ranked by
expected yield, not coverage. Every claim below was checked against source; items
marked **verified** were additionally re-read line-by-line during this audit (several
are confirmed live bugs, not just "untested"). The recurring pattern from the last
five bugs — silently-dropped declared input, sibling code paths enforcing a rule
inconsistently, kind-based filters that hide things — reproduces at least a dozen
more times.

## Ranked findings

| # | Finding | Kind | Where | Cheapest harness |
|---|---------|------|-------|------------------|
| 1 | `@import` silently drops room `attrs`, exit `attrs`, exit `script` (boot loader applies all three) | **confirmed bug** | `import_export.rs:568-790` | Rust test: import into fresh `World` |
| 2 | Telnet privilege gaps: `system:global` ungated, `@unlock` skips owner check | **confirmed bug** | `engine/mod.rs:1696,7417` | engine unit test / `.session` |
| 3 | REST creates never set `owner_ref`; `CreateObject` discards `area` | **confirmed bug** | `engine/mod.rs:1856-1921` | engine unit test |
| 4 | Dead locks: `teleport` type never consulted; lock-type strings unvalidated; map cell lock stored as `"can_enter"` but read as `"enter"` | **confirmed bug** ×3 | `engine/mod.rs:7409`, `map_template.rs:520` | table-driven lock-wiring test |
| 5 | Object/room `aliases` and `owner_ref` don't survive export→import | **confirmed bug** | `loader.rs:268-339` | round-trip into fresh `World` |
| 6 | Boot loader never hashes exit `.luau` files — edits invisible after restart | **confirmed bug** | `loader.rs:206-226` | loader unit test |
| 7 | Container rules split: `Intent::Move` skips capacity/tag/`put`-lock; `cmd_put` skips depth/circularity; capacity counts only `Kind::Item` | sibling divergence | `softcode/mod.rs:696-744`, `engine/mod.rs:5331-5380` | engine + softcode tests |
| 8 | Failed `@import` half-writes the world despite "writes nothing" contract | contract divergence | `import_export.rs:171-204,639,740,826` | Rust test: bad `to=` mid-bundle |
| 9 | Puppet: dispatch uses puppet, delivery uses Character — messages silently dropped | contract divergence (ADR-0008) | `engine/mod.rs:5051-5172` | engine unit test |
| 10 | `Config::load` falls back to full defaults on parse error; zero `deny_unknown_fields` in the crate | silent input drop | `config.rs:74-92` | config unit test |
| 11 | `grid_move` ignores locks and `can_enter` (only `movement_blocked` mirrored) | sibling divergence | `softcode/api.rs:2266-2283` vs `engine/mod.rs:7072-7123` | `.test.luau` integration |
| 12 | `derive_hooks` misses idiomatic forms; unparseable-by-full_moon script loads with zero hooks, silently, via loader | silent input drop | `softcode/hooks.rs:218-255` | hooks unit tests |
| 13 | `markup::advance` mixes byte offsets with char counts — non-ASCII tags corrupt player text; SGR 30 emits literal `[black]` | **confirmed bug** | `markup.rs:44-48,134-138,140` vs `TAGS` at `8-27` | 3 asserts in markup tests |
| 14 | `migrate.rs`: multi-op plans corrupt; exact renames orphan exit file-keys; typo'd op-name = empty migration recorded as applied forever | logic bug + silent input | `migrate.rs:64-73,249-308` | migrate unit tests |
| 15 | Import merge policies inconsistent per field: locks replaced, attrs merged, tags add-only, `enabled` force-`true`, in-game libs skipped with no report | sibling divergence | `import_export.rs:429-445,604-713`; `hooks.rs:270-284` | field-matrix import test |
| 16 | Session runner sees only `ClientMessage::Text` (6/7 channels invisible; `expect-not:` vacuous); input untrimmed while both transports trim | harness blind spot | `session_test.rs:167-169,250-258` | runner unit tests |
| 17 | Visibility: `examine`, `get`, `who` ignore `system:hidden`/`can_see`; `examine` even skips the `look` lock | sibling divergence | `engine/mod.rs:3984,5189,5638`, `commands.rs:207` | `.session` fixture |
| 18 | `db.rs` load: 6× `unwrap_or_default()` silently empties a field and the next autosave makes it permanent; unknown kind → `Item` | silent data loss | `db.rs:782-803` | db unit test with corrupt row |
| 19 | Reload semantics: `sync_managed_tags` never removes file-removed tags; removed `title` sticky; in-game `@lock` wiped by reload while attrs merge | silent divergence | `loader.rs:684-693,754-760,913-921` | loader unit tests |
| 20 | Archetypes: troupe index reads OWN tags (globals index beside it resolves the chain); locks never delegate; dangling/cyclic tolerance untested | sibling divergence | `engine/mod.rs:666-677`, `world/mod.rs:263-278` | indexes + world tests |
| 21 | Clock: `dusk_hour ≥ hours_per_day` → `on_dusk` never fires; skipped-rollover drop documented but untested; `minutes_per_tick ≤ 0` accepted | silent config drop | `clock.rs:26-29,136-140`, `engine/mod.rs:1005-1040` | clock/engine tests |
| 22 | "Every mutating intent refused on unowned object" test is a hand-list of 7 of ~20 intents; `Intent::CancelAfter` has no authority check at all | weak test / real gap | `softcode/mod.rs:1193-1201,5733-5779` | make the test exhaustive over `Intent` |

---

## 1. `@import` silently drops room attrs, exit attrs, exit scripts (verified)

The shared serde structs (`loader.rs:250-451`) guarantee the *format* can't drift, but
the two consumers read different fields. The boot loader applies `room.attrs`
(`loader.rs:690,713`), `exit.attrs` (`817,840`), and `exit.script` (`818,842`). The
`@import` apply path reads **none of them**: the room branch (`import_export.rs:568-627`)
sets title/description/tags/locks/archetype/attr_schema only; the exit branch
(`import_export.rs:754-782`) sets from/to/aliases/locks only and never calls
`resolve_script`. Export *writes* all three (`1307-1324`, `1384-1388`).

Failure scenario: rebuild a DB from an exported bundle (`load_world_files = false`
deployment, or disaster recovery). Every door loses its `can_traverse` gate and
`closed` state — permanently open, no error, report says "unchanged". The shipped
diku cookbook scaffold (`tests/cookbook-fixtures/diku/scaffold/midgaard/midgaard.toml:50-61`)
is exactly this shape; the mush scaffold's `[rooms.attrs]` climate/phase descriptions
die the same way.

Why no test catches it: `export_then_import_round_trips_to_a_no_op`
(`import_export.rs:1987`) re-imports into the **same in-memory world**, so any field
export writes but import ignores compares equal. Cheapest test: export, import into
`World::new()`, assert field-by-field equality — one test kills findings 1 and 5.

## 2. Telnet privilege gaps (verified)

- `is_ref_global` is called exactly once in the crate, in the REST guard
  (`engine/mod.rs:1696`; defined `7776`). Telnet `@tag` (`6268`) + `@program`
  (`check_program_write`, `4471-4487`) never consult it. A plain Builder can
  `@tag #x = system:global` then `@program` a `cmd_*` into every player's command
  path — the exact escalation the REST guard's comment names. The only existing test
  (`only_admins_can_author_the_system_global_surface`, `mod.rs:11125`) drives REST only.
- `cmd_unlock` (`mod.rs:7417-7448`) checks Builder scope + `system:locked` but not
  `can_modify_object`; `cmd_lock` (`7396`) checks it. Any builder can strip another
  builder's `traverse`/`get` lock and cannot put it back. `can_modify_object` appears
  zero times in the test module.

Cheapest test: telnet mirror of the REST global-surface test, plus a table test that
each `(telnet verb, REST twin)` pair refuses a non-owner and a locked target.

## 3. REST creates never set `owner_ref`; `CreateObject` discards `area` (verified)

`CreateRoom`/`CreateObject`/`CreateExit` (`engine/mod.rs:1856-1921`) mint objects with
no `.with_owner(...)`; the telnet siblings `@dig`/`@open`/`@create` all set it
(`4110,4139,4252`). Consequences are all silent: the web-builder author can't
`@set`/`@lock`/`@destroy` their own object from telnet (`can_modify_object`,
`7837-7845`); `may_modify` (`softcode/mod.rs:489-497`) treats unowned as trusted
system layer; owner quotas (`4797-4800`) don't apply. Separately,
`CreateObject { area: _area, ... }` (`1890`) explicitly discards `area` while
`CreateRoom` stamps `_file_key` from it — a web-created NPC never exports and never
appears in its area slice. No existing test asserts ownership or file-key on any REST
create path.

## 4. Dead locks — three ways to author a lock nothing reads (verified)

- `check_lock` call sites cover `get,put,look,traverse,enter,drop,use`
  (`engine/mod.rs:5200-7268`). `teleport` is documented in the `@lock` usage string
  (`7381`), `docs/commands.md:112`, `docs/softcode-guide.md:765`, ADR-0006 — and
  consulted nowhere. `cmd_teleport` (`4386`) does no lock check at all.
- Neither `cmd_lock` (`7409`) nor `Intent::SetLock` (`softcode/mod.rs:1153`) validates
  the lock-type string: `@lock #8/tarverse = false` reports success and is dead forever.
- `map_template.rs:520` stores a cell lock under hook `"can_enter"`, but `do_move`
  reads `check_lock("enter", ...)` (`engine/mod.rs:7106`) — a map-authored cell lock
  is never evaluated on the room path, and the grid path checks no locks at all (#11).

Cheapest test: one shared `LOCK_TYPES` const + a table-driven test asserting every
documented type has a `check_lock` call site and every author surface rejects unknown
types. Catches all three and prevents recurrence.

## 5. Fields that don't round-trip export→import (verified)

Enumeration of the serde gap (no `deny_unknown_fields` anywhere in `src/`; the three
mentions at `loader.rs:471,1374,1452` are prose):

| GameObject field | TOML slot | Consequence |
|---|---|---|
| `aliases` (rooms/objects) | **none** — only `ExitDef` has it | `aliases = [...]` on `[[objects]]` parses and is silently dropped on *both* import paths; `@alias`-set aliases are unexportable — after a bundle rebuild, `get blade` stops resolving |
| `owner_ref` | **none** | every builder loses ownership of exported content on rebuild; `is_owner()` locks silently flip |
| `script.enabled` | **none** | an admin-disabled script is re-enabled by the next import (`hooks.rs:270-284` hard-codes `enabled: true`) |
| exit `title`/`description`/`tags`/`attr_schema` | none on `ExitDef` | `@desc`ing an exit + export silently drops it |
| room/exit `attrs`, exit `script` | slot exists | exported, **ignored on import** (finding 1) |

Also: `ClockConfig` (`clock.rs:17-36`) and `Config` (`config.rs:5-52`) accept unknown
keys — `minute_per_tick = 0.5` silently runs at the default rate. And `Config::load`
(`config.rs:82-85`) swallows a **parse error** by booting with full defaults: a syntax
error in production `hearth.toml` means default `db_path` (fresh DB, `HEARTH_ADMIN_*`
seeds a new admin) and an empty `locked` list — the std tier silently unlocked.
Cheapest tests: `Config::load` on malformed TOML should be a loud failure (or at least
asserted behavior); add `deny_unknown_fields` to `AreaFile`/`RoomDef`/`ObjectDef`/
`ExitDef`/`ScriptDef`/`MigrationFile`/`Config`/`ClockConfig` and fix what falls out.

## 6. Exit script files never hash-checked at boot (verified)

`referenced_program_files` (`loader.rs:206-226`) visits rooms' and objects' scripts
and libs and `[[scripts]]` — never `area.exits`. The hash-skip (`598-627`) therefore
skips an area whose TOML is unchanged even when its exit's `door.luau` changed:
edit the gate, restart, old gate still runs, reload report says nothing changed.
This is the *same* bug the comment at `585-588` says was fixed for object scripts.
Existing test `editing_only_a_program_file_is_detected` (`loader.rs:2082`) covers an
object script only — add the exit-script twin.

## 7. Container rules enforced in one path, not the sibling

- `cmd_put` (`engine/mod.rs:5331-5380`) enforces the `item:container` tag, capacity,
  the `put` lock, and `can_put` — but not circularity or `MAX_CONTAINER_DEPTH`.
  Nesting bags one command at a time reaches depth 5; `format_container_contents`
  (`commands.rs:171`) stops rendering at depth 4, so the innermost items vanish from
  `inventory` with no message.
- `Intent::Move` (`softcode/mod.rs:696-744`) enforces circularity + depth — but not
  capacity, the container tag, or the `put` lock. REST `SetLocation`
  (`engine/mod.rs:2073`) routes through it, so the web builder can overfill any bag.
- Capacity counting filters `kind == Kind::Item` (`5347`) — an NPC moved into a
  container by softcode doesn't count, and the same filter in `commands.rs:134,159,
  207,357` makes a player inside a container invisible to `inventory`, `examine`, and
  `get X from Y` (the exact `get_contents` bug, three more instances).

Cheapest fix-with-test: lift the checks into one shared helper and add engine tests
for `cmd_put` depth and `Intent::Move` capacity — currently `cmd_put`, capacity, and
`MAX_CONTAINER_DEPTH` have **zero** tests.

## 8. Failed `@import` half-writes the world

`import_bundle`'s doc promises "writing nothing at all" (`import_export.rs:171-174`),
but `check_collisions` only pre-flights identities and room/object scripts. `apply`
can still fail mid-loop on an unresolvable object `location` (`639`), exit `from`/`to`
(`740`), or a missing `[[scripts]]` `.luau` (`826`) — leaving every earlier object
already `add_object`ed with dbrefs minted, and skipping
`world.rebuild_children_index()` (success path only, `198-204`), so the half-imported
objects are also invisible in room contents until restart. Both existing refusal tests
(`1899`, `1930`) exercise only the pre-flight. Test: a bundle with `to = "sqaure"` on
the last exit; assert the world is unchanged.

## 9. Puppet delivery keyed on Character, not effective actor

ADR-0008 (`engine/mod.rs:452-456`) says dispatch **and output routing** use the
effective actor. Dispatch does; every delivery fn matches
`SessionState::Playing { actor_ref, .. }` — `send_to_actor_ref` (`5051`),
`send_to_room` (`5159`), `send_emit_data` (`5061`), `EmitNearby`/`EmitRadius`
(`4880,4909`), `cmd_whisper` target search (`5929`), `do_who` (`5641`). A hook that
`emit(actor, ...)`s at a puppet produces an effect targeting the NPC ref, which no
session owns — message silently discarded; the puppeteer hears their Character's room
instead. The existing test (`puppet_routes_gameplay_to_the_npc_not_the_character`,
`10007`) asserts dispatch only. One test: puppet an NPC in another room, `say` there,
assert the puppeteer's session received it.

## 10–12. grid_move locks, derive_hooks, markup (see table)

- **grid_move** (`softcode/api.rs:2266-2283`): fires terrain `on_leave`/`on_enter`
  and honors `movement_blocked` (tested), but checks no lock and fires no
  `can_enter`/`can_traverse` — while `do_move` runs the full gate chain
  (`engine/mod.rs:7072-7123`). Combined with #4's `"can_enter"` key mismatch, a map
  cell lock is unreachable on *both* paths. `.test.luau` integration test: stamp a
  lock on a cell, assert `grid_move` blocks.
- **derive_hooks** (`softcode/hooks.rs:218-255`): `on_get = make_handler()`,
  multi-assignment, and `local on_get = function()` are silently not derived. Worse,
  a script full_moon (2.2) can't parse but Luau can **runs with zero hooks** — the
  loader and softcode `set_script` give no feedback (`@program` alone reports
  "hooks: none detected", `engine/mod.rs:4509`). Cheapest conversion from silent to
  loud: warn at loader/`set_script` when source is non-empty and derived hooks are
  empty, plus a test with Luau type annotations (`function on_get(this: any): boolean`).
- **markup** (verified): `to_ansi`/`to_plain`/`bbcode_to_html` compute tag length as a
  *byte* offset (`markup.rs:44`) and pass it to `advance`, which skips *chars*
  (`134-138`) — any non-ASCII inside a tag (`[cmd=go süd]`) eats player-visible text.
  All six markup tests are ASCII. Also `ANSI_COLORS` includes `black` (`140`) with no
  `black` in `TAGS` (`8-27`), so SGR 30 renders the literal text `[black]` on the web
  client, and SGR 90–97 lose brightness. Three asserts fix coverage.

## 14. migrate.rs edge cases

- Multi-op plans (`migrate.rs:249-308`): `occupied` is threaded across ops but each
  op's sources come from the unmutated world — an `a→b, b→c` chain hard-errors
  spuriously; two ops matching one source both "apply" (last wins) while the report
  claims both. Every existing rename test uses exactly one op.
- Exact renames don't touch exit identities (`<area>/exit/<from>/<dir>`,
  `import_export.rs:742`): renaming `town/square` → `town/plaza` orphans
  `town/exit/square/*`, and the next load **creates duplicate exits** — precisely the
  duplication the module exists to prevent (`migrate.rs:11-19`). No migrate test
  involves an exit.
- `MigrationFile` (`64-73`): both op arrays are `#[serde(default)]` with no
  `deny_unknown_fields`, so `[[renames]]` (typo) parses as an empty migration,
  validates, and is **recorded as applied forever** (`345-358`).

## 15. Import three-way merge — four policies, none stated

Only scripts get origin-aware merging, and `ProgOutcome::Conflict` is unreachable, so
`ImportReport.conflicts` is always empty (`import_export.rs:54-55,340-379`). In-game
libs are skipped with *no report line at all* (`436`) — asymmetric with scripts'
`kept_local`. Field policies: `locks` wholesale-replaced (in-game `@lock` deleted by
next import), `attrs` merge-never-remove, `tags` add-only with malformed tags
silently swallowed, `title`/`location` never clearable, `archetype` clearable. And a
byte-identical in-game save flips origin to `InGame` (`hooks.rs:260-262`), pinning the
object out of all future upstream updates with no un-pin command. The
`three_way_case_*` tests (`1752-1805`) cover scripts only. Test: field-matrix import
over a world with in-game edits, asserting each policy deliberately.

## 16. Session-test harness blind spots

`drain_plain` (`session_test.rs:250-258`) sees only `ClientMessage::Text` — Room,
Game (`emit_data`), Inventory, Auth, Commands, Prompt are invisible, so `expect-not:`
passes vacuously for anything leaked on a structured channel, and no `.session` can
assert GMCP/sidebar payloads. Inputs are deliberately untrimmed ("a password may be
space-sensitive", `167-169`) but **both** real transports trim (`telnet.rs:238`,
`web.rs:244`) — fixtures can exercise inputs no real client can send, and the stated
rationale is false. `tests/fixtures/` holds a single `login.session`; none of the
engine commands with zero unit tests (`@put`, `@teleport`, `@lock`/`@unlock`, `who`,
containers) have a fixture. The harness is the cheapest place to add most Tier-2
coverage once `drain_plain` folds structured messages into the window.

## 17–21. Remaining (see table for cites)

- **Visibility**: `examine` bypasses even the `look` lock and `can_look`
  (`engine/mod.rs:3984` → `do_examine` directly); `get` and `who` ignore
  `system:hidden`. One `.session` with a hidden object covers all three.
- **db.rs**: `unwrap_or_default()` on `attrs/aliases/script/libs/locks/attr_schema`
  (`791-803`) — one bad row silently strips a field and the next autosave persists
  the loss; unknown `kind` → `Item` (`782-790`) downgrades Code objects into visible
  items. Test: corrupt a column, load, assert a loud error (or at minimum a warn).
- **Reload**: `sync_managed_tags` (`loader.rs:913-921`) only adds — removing
  `system:hidden` from a file leaves the object hidden forever; removing `title`
  never clears it (`684-686`); in-game `@lock` on a managed object is wiped on the
  next file-change reload (`691`) while attrs merge — same rule, two policies.
- **Archetypes**: troupe index reads own tags while the globals index four lines up
  resolves the chain (`engine/mod.rs:666-677`); locks are the one field with no
  `resolved_` accessor (undocumented); dangling/cyclic chains at *resolve* time are
  untested (only `would_cycle_archetype` — the preventer — is, and `@import` never
  calls it); `attr_schema`'s `required`/`min`/`max`/`pattern` are parsed, stored, and
  never consulted by `collect_attr_schema_issues` (`loader.rs:129-180`).
- **Clock**: `dusk_hour ≥ hours_per_day` means `on_dusk` never fires and `is_day`
  sticks true, unvalidated; `dawn > dusk` wrap branch untested (its twin in
  `locks.rs:56-68` is tested — same rule, two implementations, one tested);
  `minutes_per_tick ≤ 0` silently freezes time; documented rollover-skip behavior
  (`engine/mod.rs:1021-1027`) has no test (the only rollover test uses exactly
  60 min/tick, so skipping never occurs).

## Weak existing tests (would pass with the feature broken)

| Test | Why it's weak |
|---|---|
| `import_export.rs:1987` `export_then_import_round_trips_to_a_no_op` | re-imports into the same world — passes with findings 1 and 5 present; import into `World::new()` instead |
| `import_export.rs:2229,2334` ad-hoc round trips | exits/objects constructed with none of the fields import drops; owner set but never asserted after |
| `import_export.rs:1732,1881` idempotency/dry-run | assert bucket counts only — ignored fields are trivially "unchanged", actively rewarding drop bugs |
| `softcode/mod.rs:5733` `every_mutating_intent_is_refused_...` | claims to catch new unchecked intents but hand-lists 7 of ~20; `CancelAfter` already has no check. Make exhaustive over the enum |
| `engine/mod.rs:11125` `only_admins_can_author_the_system_global_surface` | REST-only; the telnet sibling is the unguarded one |
| `engine/mod.rs:10007` `puppet_routes_gameplay_to_the_npc...` | asserts dispatch, not delivery — the broken half |
| `db.rs:1011` `world_delta_round_trip` | mutates only `title`; says nothing about the other 15 delta columns |
| `migrate.rs:384-467` all rename tests | single-op only; no chains, exits, or double-apply |
| `clock.rs:212` `is_day_tracks_dawn_dusk` | default 6/20 only; wrap branch dead |
| all 6 `markup.rs` tests | pure ASCII |
| `loader.rs:1377,1456` attrs-carry tests | pin the boot path for the exact fields `@import` drops — the sibling is unasserted |

## Highest-leverage first moves

1. Round-trip test into a fresh `World` asserting field-by-field equality (kills 1, 5,
   and future field drift by construction).
2. Exhaustive `Intent` authority table test (22) + telnet mirror of the global-surface
   test (2) + `owner_ref` asserts on REST creates (3).
3. Shared `LOCK_TYPES` const with a wiring test (4, and the map cell-lock mismatch).
4. `referenced_program_files` exit-script line + test (6).
5. Fold structured `ClientMessage`s into the session runner's window (16), then add
   `.session` fixtures for the zero-test commands (containers, visibility, teleport).
6. `deny_unknown_fields` across `loader.rs`/`config.rs`/`migrate.rs` serde types, and
   make `Config::load` parse errors loud (5, 10, 14).
