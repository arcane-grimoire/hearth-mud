# RBAC Security Audit

Audit of the authentication and authorization model: `src/accounts.rs`,
`src/net/web.rs`, the REST/token path in `src/engine/mod.rs`, and the telnet
command gates. Findings dated 2026-08-23.

## Resolution status (2026-08-25)

The high/medium findings were fixed on 2026-08-25 (commit follows this doc).
The design choice for H1 (keep Builder broad, hard-guard the `system:global`
surface — not full ownership enforcement) and M2 (require a token for reads)
was made deliberately; see the notes on each finding.

| # | Finding | Status |
|---|---------|--------|
| H1 | Builder can rewrite any object / system rules object | **Fixed (scoped).** Two layers now: the pre-existing `system:locked` guard makes file-authoritative objects (`std/*`) read-only to authoring, and a new guard makes the **`system:global` command surface admin-only to author** — a non-admin Builder can neither edit a `system:global` object nor add/remove that tag (`handle_api_request`, `is_ref_global`). `system:managed` content stays Builder-editable by design (that is the in-game building model). Full per-object ownership enforcement was **deliberately not** adopted (MUD-traditional broad building kept). |
| H2 | `hash_token` uses `DefaultHasher` | **Fixed.** Now SHA-256 (`sha2` crate). Existing tokens (old digests) simply stop matching, so those sessions re-authenticate once. |
| M1 | No rate limiting on authentication | **Fixed.** Per-username failed-login throttle in `handle_login_password` (`login_failures`): 5 failures → 30 s lockout, checked before Argon2 runs, so login can't be used as a CPU-DoS amplifier. Cleared on success or when the window elapses. |
| M2 | Unauthenticated world enumeration | **Fixed.** `ListRooms`/`ListObjects`/`ListExits` now require a valid token (any authenticated account, not Builder). `ListHooks` stays public (static engine vocabulary, not world data); `Examine`/`ListPrograms` remain Builder-gated as before. |
| M3 | `CorsLayer::permissive()` on every route | **Fixed.** `cors_allowed_origins` config allow-list (`web.rs`); permissive only when unset (dev), with a startup warning. |
| L1–L4 | Notes | **Unchanged** — still applicable (see below). |

Tests: `world_reads_require_a_token_but_not_builder` (M2) and
`only_admins_can_author_the_system_global_surface` (H1) in `src/engine/mod.rs`.
M1's timing-based throttle is covered by inspection, not a unit test.

The **L-level notes below are unchanged** and remain open follow-ups (token
lifetimes, the near-dead `Puppeteer` scope, `@grant` audit trail / last-admin
lockout).

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

> **Resolution (2026-08-25): fixed, scoped to the global surface.**
> Recommendation 2 is implemented directly: `handle_api_request` refuses any
> non-admin write to a `system:global` object (`is_ref_global`, resolved up the
> archetype chain) and refuses a non-admin `AddTag`/`RemoveTag` of
> `system:global`, so the command-injection path above is closed regardless of
> whether a game opts into locking. This composes with the pre-existing
> `system:locked` guard (`is_ref_locked` / `locked_target`, config-driven
> `locked = [prefixes]`) that makes file-authoritative `std/*` objects
> read-only to authoring. Recommendation 1 (full per-object ownership
> enforcement) was **deliberately declined**: broad Builder world-editing is
> kept (MUD-traditional), and `system:managed` content stays Builder-editable
> because that *is* the in-game building model — the code tier is protected by
> locking, the global surface by admin-gating, not by ownership.

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
