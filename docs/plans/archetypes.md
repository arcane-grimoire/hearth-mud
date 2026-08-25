# Plan: Archetypes — prototype/instance objects (MOO-style)

> **Model settled (design session).** Two axes, and only the first is
> inheritance:
> - **`is-a` — a single archetype parent (delegation up one chain, MOO/`__index`
>   style).** No multiple inheritance, ever. The chain may be deep
>   (`goblin_chief → goblin → monster`) — a per-hook walk up a *single* parent
>   line is never ambiguous, so depth is safe.
> - **`has-a` — traits/components (STAGE 2).** Additive behavior that
>   *participates, never overrides* (`can_*` = AND, `on_*` = all react, `cmd_*`
>   = additive + first-wins-with-a-warning). This is Hearth's existing
>   tag/`system:global` mechanism with a theory attached, NOT a second
>   inheritance axis. The load-bearing invariant: **a trait can decorate a
>   hook, it can never *be* the hook's answer** — that's what keeps traits from
>   collapsing back into the multiple inheritance we refused.
>
> Framing: Hearth is a Smalltalk-style live image (world in memory, edited in
> place, checkpointed to the DB; `@import`/`@export` = fileIn/fileOut). So an
> archetype is a **real, in-world object** you can `examine` and edit live, and
> reparenting is allowed to happen live. **Build `is-a` first (Stage 1 below);
> add traits only once a real case needs them.**

## Goal

Let a game define "a goblin looks like this" once (an **archetype**) and spawn
many **instances** that share its behavior and defaults while carrying their
own state. This is the blueprint/clone problem (LPMUD) or prototype/instance
(MOO, Evennia). The object-script model (`docs/plans/object-scripts.md`) makes
it urgent: a script lives *on the object*, so 50 goblins must not mean 50 copies
of the goblin script to edit.

Decided model: **delegate to parent (MOO `chparent` + `clear`-able
properties)**, not copy-on-spawn. Editing an archetype updates every live
instance; instances override per-field, copy-on-write.

## The split

| Lives on the **archetype** (resolved by instances) | Lives on the **instance** |
| --- | --- |
| the script (behavior) | `state` (this instance's memory) |
| default title, description, base attrs, tags | position, HP, per-field overrides |

Title/description/attrs resolve copy-on-write (instance value wins if set,
else the archetype's); tags resolve as a **union** (instance's own plus every
ancestor's) since there's no single "value" for a tag to override — see
Decision 3.

This mirrors the object-script `source` (shared) vs `state` (per-object) split
one layer up: **program shared, object-variables per-clone** (LP), **member
vars per-node** (Godot). `state` is *never* resolved from the archetype — it is
always the instance's own.

## Decisions (locked)

1. **`spawn` defaults to a reference (delegate), not a copy.** The instance
   holds `archetype_ref` and no script of its own; behavior resolves from the
   archetype. An explicit `clone`/`detach` makes divergence the loud choice.
2. **Copy-on-write per field, uniformly** — title, description, attrs, script:
   instance value if set, else the archetype's. One rule, no special cases.
   `state` is always per-instance (not an exception — a different axis).
3. **Tags are the one field that doesn't fit the copy-on-write shape —
   they union instead.** A tag isn't a single value with an instance-vs-
   archetype winner; `resolved_tags(obj)` is the instance's own tags plus
   every ancestor's, unioned up the chain (same walk as `resolved_attrs`).
   This is additive-only for Stage 1: there is no per-instance "clear an
   inherited tag" yet (that's `clear_attr`'s sibling, deliberately deferred to
   Stage 2 alongside it — see Stage 2+ below). Every tag-reading call site
   (`has_tag`, the `is_*` predicates, `system:hidden`/`can_see` visibility,
   `system:global` command dispatch and the `globals_by_hook` index) resolves
   through this union, not the raw field.
4. **Refuse to delete an archetype while instances exist, unless `--cascade` —
   and `--cascade` means flatten-then-delete, never delete-and-orphan.**
   Orphaned instances that silently lose behavior are a three-sessions-later
   bug; that's true whether the orphaning is silent (no guard) or the result
   of an opt-in cascade that doesn't clean up after itself. So cascade
   detaches (flattens) every live instance — same mechanism as `clone`/
   `detach`: resolved title/description/attrs/tags/script copied down,
   `archetype_ref` cleared — *before* the archetype they depended on is
   actually removed. Fail loud at delete time by default, matching the
   `system:managed` reconciliation instinct; cascade is the loud, deliberate
   opt-out, not a quieter way to lose behavior.
5. **Single parent, depth allowed.** One `archetype_ref` per object (never
   multiple — no MI). The chain may be deep; a per-hook walk up a single parent
   line is unambiguous, so `goblin_chief → goblin → monster` is fine. (This
   reverses an earlier "one level only" caution — that fear was class-MRO
   fragility, which single-parent prototype delegation doesn't have.) Cycles
   are refused (an object can't be its own ancestor).

## Where it plugs in (already prepared)

`GameObject.archetype_ref: Option<String>` exists as a dormant field
(`src/world/object.rs`), serialized and defaulted, resolved by nothing yet.

Every hook-dispatch site already funnels through two accessors:
`hooks::get_script(obj)` and `hooks::object_defines_hook(obj, hook)`. Archetype
fallback slots in at exactly those two seams — they grow a `&World` param and a
single "on miss, look up `obj.archetype_ref`" branch. That is the bulk of the
runtime change; it is ~10 lines at two seams, not a scatter-hunt.

## Stage 1 — the `is-a` chain (build this first)

The smallest thing that lets The Last Stag's dungeon monsters be archetype-based
end to end. Traits, `pass()`, `@export`, and the builder authoring surface are
deliberately OUT of stage 1.

**1. Resolution seams (the ~2-function core).** The two accessors grow a
`&World` and walk the parent chain:
- `resolve_script(world, obj, hook) -> Option<(&ObjectScript, resolving_ref)>`
  — the object's own script if it defines `hook`, else walk `archetype_ref`
  upward to the first ancestor whose script defines it.
- `object_responds(world, obj, hook) -> bool` — same walk, boolean.
`run_hook` runs the *resolving ancestor's* script but binds `this` to the
**instance** — so `state`, attrs, and `ref_id` seen by the code are the
instance's, only the *code* comes from the ancestor. Every current dispatch
site (fire_hook, tick loop, lifecycle, cmd resolution, can_see/on_look/on_say
fast-paths, globals index) routes through these, so this is the bulk of the
change and it's localized.

**2. Field delegation, copy-on-write (plus tags, unioned).** Reading `title`,
`description`, and any `attr` resolves instance-first, then up the chain;
`state` never delegates (always the instance's own). Tags resolve as a
**union** of the instance's own and every ancestor's (Decision 3) — additive
only, no instance-side clear yet. Implement resolvers used by both the engine
reads and the Lua `this`/object snapshot — every attr-reading Luau function
(`get_attr`, `has_attr`, `pick`, `find_by_attr`) and every tag-reading one
(`has_tag`, `get_tags`, the `is_*` predicates) goes through them, not just
property syntax. (Option to evaluate: point the Lua object table's metatable
`__index` up the chain so in-script `this.max_hp` delegates for free — the
native Lua mechanism — rather than resolving in Rust.) Writing an attr on an
instance sets it on the instance (the override); a future `clear_attr` reverts
it to inheriting (MOO `clear_property`) — clear can be stage 1b.

`set_val` (the nested-path attr writer) needs copy-on-write at the *whole
value* level, not just the leaf: if the target attr isn't set on the instance
itself but resolves from an archetype, start the write from the full resolved
value, edit the leaf, then write the whole thing back onto the instance — an
edit that started from just the leaf would silently drop every untouched
sibling in the inherited object.

**3. Spawn + clone.**
- `spawn({ archetype = "<ref-or-key>", ... })` → instance with `archetype_ref`
  set; any explicit fields become overrides. Fire the archetype's `on_create`
  on the new instance (the constructor seam — seeds instance state/attrs).
- `clone(ref)` / `detach(ref)` → copy the *resolved* fields + script onto the
  object and clear `archetype_ref`. This is the escape hatch named for the verb
  the Luau/Roblox audience reaches for; delegation stays the default.

**4. Guards.**
- Refuse setting `archetype_ref` to self or any descendant (cycle prevention).
- Refuse `destroy` of an object that has live instances, unless `--cascade` —
  and `--cascade` flattens (detaches) every instance first, then deletes.
  Orphaning with a dangling `archetype_ref` is never an option, opted-in or
  not (see Decision 4).

**5. Proof.** Convert the dungeon monsters (goblin/skeleton/etc. in
`src/dungeon.rs` / the encounter tables) from inline attr-stamping to
`spawn({archetype=...})` against real archetype objects. If that reads nicer
than today's inline stats, stage 1 earned its keep.

## Stage 2+ (only once stage 1 proves out)

- **File-based archetype declaration — DONE.** `archetype = "area/key"` on a
  room/object in TOML resolves to `archetype_ref` at load time (both the boot
  loader and `@import`); `@export` round-trips it. Cycles are broken in a
  post-pass against the *final* graph (`break_archetype_cycles`), so the guard
  is order-independent even when a reload rewires a chain. This is the mudlib
  foundation — base archetypes ship as file content. (Runtime *creation* of new
  archetypes is deliberately still out; the type layer is file-first.)
  - *Known limitations (tracked, both narrow — they don't affect a fresh
    `archetype =` declaration, only rarer edits, and no shipped content hits
    them yet):*
    - **Converting an existing managed object to delegate doesn't clear its old
      own-fields.** If a TOML edit adds `archetype =` and *drops* fields the
      object used to define (title/attrs), the stale own-values remain and
      shadow the inherited defaults. The real fix is making managed
      reconciliation *file-authoritative* (replace attrs / clear a dropped
      title), a broader change than archetypes — deferred.
    - **A room instance can't inherit its title through export.** `RoomDef.title`
      is a required `String`, so an archetyped room with no own title exports
      `title = ""`, which re-imports as an override. Fix: make `RoomDef.title`
      `Option` (as `ObjectDef.title` already is) — deferred until archetyped
      rooms actually exist.
- **Traits (`has-a`)** — additive components per the invariant above; unify with
  tags/`system:global`.
- **`pass()`** — single-chain `super`; call the inherited hook from an override.
- **`clear_attr`** — revert an override to inheriting (MOO `clear_property`).
- **Authoring surface** — `@archetype`, builder "N instances / which fields are
  overridden", the "where does this hook come from" inspector view.
- **`@export` / declared attrs** — ties into `docs/plans/attribute-schema.md`;
  an archetype's declared attrs become an instance's typed inspector defaults.
- **Live debugger** — hook error → inspect `this`/`actor` state → edit → re-fire
  (the Smalltalk-image payoff; most valuable *because* layered behavior is where
  "which script ran, and why did it break" gets murky).

## Integration details (surfaced by code review, verified against the tree)

These don't change the design — they're the touch-points stage 1 must get
right.

- **Sequencing vs. dbref migration: resolved.** The dbref migration is *done*
  (`World::next_dbref` mints `#N`; string-path values are only the
  `FILE_KEY_ATTR` identity, not refs). So `archetype_ref: Option<String>`
  already holds a final `#N` ref — no double-migration risk, build order is
  free. (`docs/plans/dbref-migration.md` was stale and has been removed.)

- **Guards enforce at the intent, not the API wrapper (ADR 0001).** All
  mutations flow through `apply_batch`, the single choke point covering Lua
  `spawn`, the REST API, and the editor alike. The cycle check (can't set
  `archetype_ref` to self or a descendant) and the delete guard belong there,
  not only in the Luau wrapper — otherwise a path that bypasses the wrapper
  bypasses the guard.

- **`Intent::Spawn` must grow.** Today it's
  `{ ref_id, key, kind, title, description, location, owner }`
  (`src/softcode/mod.rs`, applied in `engine/mod.rs`). Stage 1 adds an
  `archetype` and override fields → a variant change + `apply_batch` handling +
  serde compat for any batch already queued. Plus a `Clone`/`Detach` intent.

- **Loader / `@reload-world` interaction (decide before building).** Managed
  objects reconcile from TOML every boot. Open questions the spec must answer:
  can a *file-defined* object be an archetype for script-spawned instances
  (presumably yes)? Then what happens when a TOML file stops defining an
  archetype that live instances reference — does reconciliation refuse (the
  delete-guard rule), orphan, or cascade? And does the loader ever set
  `archetype_ref` between managed objects, and how does the two-pass loader
  order that? Lean: reconciliation obeys the same delete guard (refuse to drop
  an archetype with live instances; log loudly).

- **Globals index must walk the chain.** `DerivedIndexes::build`
  (`engine/mod.rs`) routes through the accessors and has `&World`, so an
  instance tagged `system:global` *can* inherit `cmd_*`/`on_tick` from its
  archetype — but only if `build()` actually walks the chain. Easy to miss; pin
  it with a "global instance inherits command dispatch" test.

- **The Rust attr resolver is the mechanism; Lua `__index` is only a bonus.**
  The engine reads attrs natively in many places (~18 `.attrs.get()` sites in
  `engine/mod.rs` alone), so chain-resolving attrs *must* exist in Rust
  regardless. A `__index` metatable on the in-script `this` would be a
  convenience on top, not the primary path.

- **Error attribution (forward pointer to the stage-2 debugger).** When an
  ancestor's script errors while running bound to an instance, the message/log
  must name *both* the resolving object and the instance — otherwise live
  debugging of inherited behavior is baffling. Cheap to do in stage 1's
  error path; pays off with the live debugger.

## Trade-offs / open questions

- Attr resolution is now a chain walk per read. Cache per instance if it bites
  (unlikely at MUD object counts).
- `clone` copies the object only, not its contained inventory, by default.
- Whether `spawn`'s `archetype` accepts a bare key (convenient) or requires a
  dbref (unambiguous) — probably both, resolved like other refs.
