# RBAC Security Audit

Audit of the authentication and authorization model: `src/accounts.rs`,
`src/net/web.rs`, the REST/token path in `src/engine/mod.rs`, and the telnet
command gates. Findings dated 2026-08-23.

## Verification status (2026-08-25)

Re-checked every finding against the current tree. Only the locking work
landed since the audit; the rest stand.

| # | Finding | Status |
|---|---------|--------|
| H1 | Builder can rewrite any object / system rules object | **Partially mitigated** — the `system:locked` guard now makes file-authoritative objects read-only to every authoring surface, and games lock `std/*` (rules + base archetypes) via `locked = [...]`. The *ownership* dimension is still open: a Builder can still edit any non-locked object, including other players' items. See H1 note below. |
| H2 | `hash_token` uses `DefaultHasher` | **Open** — still `DefaultHasher` (`engine/mod.rs` `hash_token`). |
| M1 | No rate limiting on authentication | **Open** — no throttle/backoff on telnet or API login. |
| M2 | Unauthenticated world enumeration | **Open (narrowed)** — `Examine` and `ListPrograms` were correctly pulled out of the public `is_read` set and are now Builder-gated, but `ListRooms`/`ListObjects`/`ListExits` (and `ListHooks`) are still anonymous and still ignore `system:hidden` / `can_see`. |
| M3 | `CorsLayer::permissive()` on every route | **Open** — still unconditional (`web.rs`), no config flag. |
| L1–L4 | Notes | **Unchanged** — still applicable. |

Net: **H2, M1, M2, M3 remain open**; H1 is partially addressed by the locking
system but its ownership recommendation is not yet implemented.

## Model summary

**Roles** (`Scope` in `src/accounts.rs`): `Player`, `Builder`, `Admin`,
`Puppeteer`. Admin implies all other scopes (`has_scope`). First account
created gets Player + Builder + Admin.

**Auth surfaces:**

- Telnet login → Argon2id password verify, session bound to a WebSocket or TCP connection.
- REST `POST /api` with `{ action, ... }` + `Authorization: Bearer <token>`.
  Tokens are UUIDv4, hashed before storage (SQLite), 30-day expiry for
  session tokens, expired tokens purged on a 60-tick sweep.
- Authorization is checked per action:
  - Unauthenticated reads: `ListRooms`, `ListObjects`, `ListExits`.
  - Builder-gated writes: everything else in `ApiRequest`.
  - Admin-gated: `SaveWorld`, `Eval`, `Import`, `Export`, `PutMap`, `PutTerrain`.
  - Telnet `@grant`, `@revoke`, `@eval`, `@import`, `@export` each check
    `session_has_scope(Admin)` individually.

The overall shape is sound. Findings below, ordered by severity.

---

## High

### H1. Builders can rewrite any object — including the admin-owned global rules object

`ApiRequest::SetProgram` / `RemoveProgram` / `SetAttribute` / `AddTag` /
`DeleteObject` (engine/mod.rs, ~L1112+) check only that the caller holds
Builder scope. There is **no ownership check on `ref_id`**, and no protection
for `system:*` objects.

Consequences for a malicious or compromised Builder token:

- `SetProgram` on `area/system/item/rules` — the `system:global` object whose
  `cmd_*` hooks run for every player regardless of location — injects Luau
  that executes with full Intent write access in every player's command path.
- `AddTag(ref_id, "system:global")` turns any object into globally dispatched
  command surface.
- `DeleteObject` can delete managed/system content or other players' items.
- `SetAttribute` can forge arbitrary attrs on anything.

Net effect: **Builder is effectively equivalent to Admin for world content**.
If that equivalence is intentional, it should be documented as a decision;
if not, add:

1. An ownership rule (`owner_ref`) limiting writes to owned objects, with
   `system:managed` / `system:*` tags writable by Admin only.
2. A hard guard: non-Admin cannot modify objects tagged `system:global`,
   `system:managed`, or set programs carrying `cmd_*` hooks on global objects.

This touches the domain model ("who owns what"), so it likely warrants an ADR.

> **Update (2026-08-25): partially mitigated.** The `system:locked` guard
> (`is_ref_locked` / `locked_target` in `handle_api_request`, config-driven
> `locked = [prefixes]`) now makes file-authoritative objects read-only across
> the whole authoring surface — `SetProgram`/`SetScript`, `SetAttribute`,
> `AddTag`/`RemoveTag`, `DeleteObject`, etc. A game that locks its `std/*`
> tier (The Last Stag does) blocks the specific "rewrite the `system:global`
> rules object" and "delete managed content" attacks above. **Still open:**
> the *ownership* rule (recommendation 1) — a Builder can still edit any
> object that is *not* locked, including other players' items — and the
> tag-based hard guard (recommendation 2) is realized only for objects a
> game opts into locking, not for `system:global`/`system:managed` by tag.

### H2. Token hashing uses `DefaultHasher` (non-cryptographic)

`hash_token` (engine/mod.rs:826) uses
`std::collections::hash_map::DefaultHasher` — keyed SipHash with effectively
fixed keys, truncated to 64 bits, persisted to SQLite.

Tokens are UUIDv4, so online preimage attacks are not currently practical,
but this is a stored-credential function and should use SHA-256 like any
token hash. Small, self-contained fix; do it alongside (or before) any token
table migration.

---

## Medium

### M1. No rate limiting on authentication — Argon2 becomes a DoS amplifier

`AccountStore::authenticate` has no throttling, and both telnet login and
API paths hit Argon2id (~50–100ms CPU per verify). An unauthenticated
attacker can burn server CPU cheaply with garbage logins. Add per-source /
per-username failure backoff or a small failure counter.

### M2. Unauthenticated world enumeration

The `is_read` set in `handle_api_request` allows anonymous callers to list
all rooms, objects (with titles), and exits. These responses ignore
`system:hidden` and `can_see`, leaking hidden quest objects and full area
layout. The deliberate tradeoffs around other endpoints are already
documented in code comments; apply the same scrutiny here. Options: require
a valid Player token for reads, or filter through the visibility rules.

### M3. `CorsLayer::permissive()` on every route

Acceptable for local development; risky in deployment. Gate behind a config
flag (e.g. allow-listed origins in production).

---

## Low / notes

- **L1.** Session tokens live 30 days (`persistent: true`, saved to DB).
  Explicitly created tokens (`expires_at: None`, engine/mod.rs ~L3128) never
  expire — confirm intended, and confirm revocation UI covers them.
- **L2.** `Puppeteer` scope is nearly dead surface (one check at
  engine/mod.rs:5620 plus the ownership branch in `@puppet`). Either document
  its semantics or remove it.
- **L3.** `@grant` can grant Admin with no audit trail beyond tracing, and
  nothing prevents an Admin from revoking their own last-admin scope
  (lockout). Consider DB-backed audit log and a last-admin guard.
- **L4.** `Import`/`Export` take arbitrary filesystem paths but are
  Admin-gated ✅. Note that an Admin token is RCE-class anyway (`Eval`);
  the code documents this honestly. Treat admin tokens accordingly.
- **Positives:** generic login-failure message (no user enumeration);
  expired-token purge loop; `valid_map_name` blocks path traversal for
  `PutMap`; `@eval` goes through the intent batch like everything else
  (ADR 0001 respected); first-account-gets-admin is clean.

---

## Recommended priority

| # | Finding | Effort |
|---|---------|--------|
| 1 | H2 — swap `hash_token` to SHA-256 | Small |
| 2 | H1 — ownership / `system:*` write guards (+ ADR) | Medium |
| 3 | M1 — login rate limiting | Small |
| 4 | M2 — auth or visibility filtering on reads | Small–Medium |
| 5 | M3 — CORS config flag | Small |
