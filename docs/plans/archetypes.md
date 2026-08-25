# Plan: Archetypes — prototype/instance objects (MOO-style)

> **Status: shipped, except traits + the live debugger.** Stage 1 (`is-a`
> delegation up a single archetype chain) and nearly all of Stage 2 are done:
> file-based `archetype = "area/key"` declaration, copy-on-write field
> delegation, tag union, `spawn`/`clone`/`detach`, cycle + delete guards,
> `pass()`, `clear_attr`, the builder authoring surface (`resolved_attrs`/
> `resolved_hooks`/`resolved_tags`, Set/Detach controls), the `@archetype`
> telnet command, tiering + locking (`system:locked`), and declared attrs (see
> `docs/plans/attribute-schema.md`). What remains is the `has-a` axis (traits),
> the live debugger, and one known reconciliation limitation.

## The settled model (kept as rationale for the traits work)

Two axes, and only the first is inheritance:

- **`is-a` — a single archetype parent (delegation up one chain, MOO/`__index`
  style).** No multiple inheritance, ever. The chain may be deep
  (`goblin_chief → goblin → monster`) — a per-hook walk up a *single* parent
  line is never ambiguous, so depth is safe. **This is done.**
- **`has-a` — traits/components (the open work).** Additive behavior that
  *participates, never overrides* (`can_*` = AND, `on_*` = all react, `cmd_*`
  = additive + first-wins-with-a-warning). This is Hearth's existing
  tag/`system:global` mechanism with a theory attached, NOT a second
  inheritance axis. The load-bearing invariant: **a trait can decorate a
  hook, it can never *be* the hook's answer** — that's what keeps traits from
  collapsing back into the multiple inheritance we refused.

Framing: Hearth is a Smalltalk-style live image (world in memory, edited in
place, checkpointed to the DB; `@import`/`@export` = fileIn/fileOut). An
archetype is a **real, in-world object** you can `examine` and edit live, and
reparenting is allowed to happen live.

## Remaining work

- **Traits (`has-a`)** — additive components per the invariant above; unify
  with the existing tags/`system:global` mechanism. A trait decorates a hook
  (`can_*` AND, `on_*` all-react, `cmd_*` additive with a first-wins warning);
  it can never *be* the hook's answer. Build only once a real case needs it.
- **Live debugger** — hook error → inspect `this`/`actor` state → edit →
  re-fire (the Smalltalk-image payoff; most valuable *because* layered behavior
  is where "which script ran, and why did it break" gets murky). The stage-1
  error path already names both the resolving object and the instance, which is
  the groundwork for this.
- **Known limitation — converting an existing managed object to delegate
  doesn't clear its old own-fields.** If a TOML edit adds `archetype =` and
  *drops* fields the object used to define (title/attrs), the stale own-values
  remain and shadow the inherited defaults. The real fix is making managed
  reconciliation *file-authoritative* (replace attrs / clear a dropped title),
  a broader change than archetypes — deferred. Narrow: doesn't affect a fresh
  `archetype =` declaration, only a rarer edit, and no shipped content hits it.

## Trade-offs / open questions (still open)

- Attr resolution is a chain walk per read. Cache per instance if it bites
  (unlikely at MUD object counts).
- `clone` copies the object only, not its contained inventory, by default.
