# Authoring shares the Intent mechanism, not its authorization

The REST/telnet authoring surface (`@`-edits and `POST /api` write actions) and
softcode both mutate the world through **one** mechanism: `softcode::apply_batch`
applying an `IntentBatch`. A write action with an Intent twin — `SetAttribute`,
`SetTitle`, `SetLocation`, `AddTag`, `SetLock`, `UpdateExit`, `SetArchetype`,
and the rest — is translated to an `IntentBatch` (`engine::authoring::write_batch`)
and applied, rather than reaching into `World` with a bespoke `get_mut`.

They do **not** share authorization, and that separation is deliberate:

- **Softcode is ownership-gated.** A Program runs with the `authority` of the
  object it is attached to; `may_modify` inside `apply_to` refuses any Intent
  whose target the authority does not own. A builder's script can only touch
  what the builder owns.
- **Authoring is scope- and lock-gated at the transport edge.** Before any
  mutation is built, `handle_api_request`'s preamble enforces Builder/Admin
  scope, `system:locked` (definition read-only), and `system:global`
  (admin-only). This is the collaborative in-game building model: any Builder
  may edit any `system:managed` object, regardless of who created it.

Once the preamble has authorized a request, authoring applies its batch with
**`authority = None`** — system-trusted, the `IntentBatch::from_intents` default.
`may_modify` then no-ops, and the per-batch emit/quota caps (which only apply to
owned authorities) are skipped. `apply_batch` supplies *integrity* —
atomicity, rollback, and the existence/containment/parse validation each Intent
arm already carries — not a second authorization pass.

We considered unifying authorization too, running authoring through `may_modify`
with the acting account as the authority. That breaks the building model: a
Builder could no longer edit a `system:managed` room another Builder authored,
because ownership, not scope, would gate every edit. Authorization and integrity
are different concerns with different answers on the two paths; only the
mechanism is shared.

## Consequences

- Authoring inherits the ADR-0001 guarantees it previously lacked: atomic
  rollback, dry-run, and one audited mutation path.
- The mutation semantics for an operation live in exactly one place — the
  Intent's `apply_to` arm — so authoring and softcode cannot drift. The
  ~1,500-line `handle_api_request` shed its inline `get_mut` write arms.
- `write_batch` is a pure function of the request, unit-testable without an
  `Engine`. The mutation behavior is covered by `apply_batch`'s own tests.
- Authoring-only policy that the shared mechanism deliberately omits stays in
  the arm, not the Intent: `SetLocation` refuses relocating a player by ref
  (`Intent::Move` allows it, for builder teleporters); `DeleteObject`'s cascade
  refuses flattening a `system:locked` instance.
- A future change to *how* authoring is authorized is a change to the preamble
  (or the `authority` value it passes), not a retrofit through every write arm.
