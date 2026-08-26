# The Session resolves three identities; the effective actor completes puppeting

A playing Session conflated three distinct identities into one `actor_ref`
string, and the code that would have separated them — an `effective_ref` local
in `handle_game_input` — was written in the puppeting commit and never once
reached a call site. Clippy flagged it as unused and it was silenced with a
leading underscore, so `@puppet` set a `puppet_ref` that command dispatch
ignored: a player who puppeted an NPC kept acting as their own Character,
contradicting the Puppet definition in CONTEXT.md.

The Session now exposes the three identities as an interface, and dispatch reads
them by role rather than reaching into `SessionState::Playing`:

- **`Session::character()`** — the account's active Character. The
  authoring/ownership identity: `@`-verbs, ownership checks, and character
  management act as the character, always.
- **`Session::effective_actor()`** — the object gameplay acts *as*: the Puppet
  when one is driven, otherwise the Character. Gameplay commands (`look`, `say`,
  `go`, `get`, …), room panels, and output routing follow it.
- **The account's Scopes** (via `session_id`) — authorization for `@`-verbs.
  Never the Puppet's; a puppeteer must not gain authority through the object it
  drives.

`run_command` takes `character_ref` and `effective_ref` separately and routes
each verb to the right one. They are equal for a session-less NPC and whenever
no Puppet is active, so ordinary play is byte-for-byte unchanged; the difference
appears only while puppeting. Re-entering the world (`@charswitch`, reconnect)
rebuilds `Playing` with `puppet_ref: None`, so a puppet is released by
construction rather than by a special case.

We considered the smaller move — keep `effective_actor()` as a defined-but-unused
seam and leave dispatch on the character. Rejected: the concept is specified in
CONTEXT.md, the wiring already existed end-to-end except the last hop, and a
named accessor that nothing calls is exactly the dead code that got us here.

The split also decides where session mode-state lives. Engine-owned multi-line
editors (`@program`/`@eval`/`@dialogue`) move from `_program_editing`/
`_eval_editing`/`_ink_editing` object attrs onto `Session.editor`, because the
input router keys on them every line and they are the human's session state, not
the object's. Softcode's `prompt()` callback stays on the Character object —
only Intents can reach it, so it cannot move to the Session (see ADR-0007 for
the same object-vs-session boundary).

## Consequences

- `@puppet` delivers the CONTEXT.md behavior: gameplay drives the Puppet, while
  authoring and scope stay with the account's Character.
- The identity a piece of code needs is named at the call site
  (`character` vs `effective_actor`), not re-derived from a tuple destructure at
  ~20 sites.
- The effective-actor routing is covered by tests that never existed: gameplay
  routes to the puppet, the character does not move, releasing restores it.
- `SessionState::Playing.actor_ref` is the Character; the dispatch path reads it
  only through `Session::character`/`effective_actor`, never the field directly.
