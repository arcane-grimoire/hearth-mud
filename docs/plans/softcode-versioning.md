# Plan: Softcode versioning, merge, and edit locks (REST-native)

> **Status: implemented (REST + extension), all write surfaces covered but one.**
> Server side is done and tested (`script_versions`/`script_locks` tables, `me`,
> versioned `set_script`/`set_lib` with `diffy` 3-way merge on stale base,
> `list/get_script_version`, `revert_script`, `lock/unlock_script`, 30-min lock
> renewed on publish; version + lock ride
> `get_script`/`list_libs`/`list_programs_all`). The VS Code extension consumes
> all of it (explicit Publish with conflict flow, version history + diff + revert,
> Claim/Release lock, `me` on connect). History is now captured on **every**
> authoring surface: REST (`set_script`/`set_lib`), telnet `@program` (author =
> the session's account), and file/deploy changes (boot, `@reload-world`,
> `@import` snapshot File-origin programs as `system:file`, hash-suppressed so an
> unchanged file is a no-op). `get_script` returns an empty-script object (not
> `null`) for a script-less object, so version/lock ride there too. **One
> deliberate non-goal:** softcode-*driven* `set_script`/`set_lib` (a script
> rewriting another object's code at runtime) is not versioned — it flows through
> `apply_batch`, which has no DB handle and many call sites; rare enough not to
> justify that refactor now. Grew out of the VS Code extension
> (`clients/vscode/`), which was hand-rolling dirty-tracking and local↔server
> diff. Rather than push that into the client (or make the server a git remote —
> considered and rejected as the wrong fit for DB-is-truth), the versioning,
> history, and merge live **in the server, over the REST API**. The extension is
> just a REST client that surfaces them.

## The one principle everything hangs off

Hearth is **DB-is-truth** (see CLAUDE.md: "anything a user can edit belongs in
the database"). Versions, locks, and merge results are therefore **DB rows** —
they checkpoint and restore like everything else, and no file or client is ever
the source of truth. This is the same rule Programs and the map builder already
follow; softcode history is one more thing the DB owns.

## Scope: the softcode slice, linear history

Covers **object scripts and lib modules** — the two things a `set_script`/
`set_lib` writes. Explicitly *not* the rest of the world (attrs, tags, locks,
exits, maps, ink): those aren't script-shaped and stay on their existing REST
actions. History is **linear** (append log + merge-on-stale-base), not branches.
Branches/PR-review are a possible later axis, deliberately out of scope here.

## Part 1 — Versioning

Every softcode write **appends a version** instead of blind-overwriting.

**One append point — a single choke.** `src/softcode/hooks.rs` already routes
*every* softcode write through `set_script_with_origin(obj, source, origin)` and
`set_lib(obj, name, source, origin)`. REST, telnet `@program`, the web editor,
the boot/loader reconciliation, and softcode's own `set_script`/`set_lib` write
API all pass through those two functions. **Put the version-append there and
everything versions for free** — no per-handler work, and telnet/web/REST get
identical history by construction.

**Provenance is already modelled.** `ProgramOrigin { File, InGame }` exists today
(File-origin scripts are reconciled against game files at boot; InGame are
DB-owned and survive). A version records its origin plus the author, so history
distinguishes "a deploy changed this file" from "Sam edited it in the builder" —
and the same signal breaks the feedback loop and gates edit-lockability (Part 3).

**Storage.** A `script_versions` table (additive `CREATE TABLE IF NOT EXISTS` in
`db.rs::migrate()`, matching the existing `scripts`/`file_hashes` tables — no
schema-version pragma):

```
id, ref_id, kind ('script' | 'lib'), name (lib only),
source, source_hash, author_account, origin ('file' | 'in_game'),
merged_from (nullable version), created_at, version
```

The live object still holds the current script (unchanged); the table is the
append-only history behind it. `version` is a per-target monotonic counter.

**What counts as a version.**
- **No-op writes don't** — an identical `source_hash` to the current version
  appends nothing (reconciliation re-applying an unchanged file, or a save with
  no edits, is silent).
  - **Hash choice:** `source_hash` is **blake3**, the same algorithm already
    used for `file_hashes` — Hearth does *not* currently hash saved code at rest
    (the `scripts`/object rows store raw source), so this is new, but it reuses
    an existing dependency. Do **not** reuse the bytecode cache's `source_hash`
    (`softcode/mod.rs`): that's a non-persisted `DefaultHasher` `u64`, fine as a
    cache key but collision-unsafe for version identity — a collision there just
    recompiles; here it would silently drop a distinct version.
- **Autosave is not a version** — autosave checkpoints the *world*, it doesn't
  call `set_script`; only real writes append. So history tracks authored change,
  not the 5-minute heartbeat.
- Optional later: coalesce rapid successive writes by the same author within a
  short window into one version, if churn proves noisy.

**REST.**
- `get_script` / `list_libs` gain a `version` (and lock fields, Part 3) on each
  entry — history-awareness rides existing reads, no extra round-trip to render.
- `list_script_versions { ref_id, name? }` → `[{ version, author, origin,
  created_at, hash }]`, newest first.
- `get_script_version { ref_id, name?, version }` → `{ source }`.
- `revert_script { ref_id, name?, version }` → re-applies an old version's
  source **as a new version** (never rewrites history). Rollback falls straight
  out of the append log; author = the reverter, with `merged_from` unset. Same
  Builder gate + lock/tier rules as any other write.

**Diff is client-side.** The extension fetches two versions and opens VS Code's
native diff — the server never diffs.

## Part 2 — Merge on stale base (optimistic concurrency)

The server does the merge, over REST, when your base is stale:

- Reads hand back `{ source, version }`. You edit locally from version *N*.
- Publish sends `set_script { ref_id, source, base_version: N }` (same for
  `set_lib` with `name`).
- Server resolves:
  - **current == N** → apply, becomes *N+1*.
  - **current == M (someone published since)** → **3-way merge**: base = version
    *N*'s source, *theirs* = current *M*, *ours* = incoming. Clean → apply the
    merged result as a new version, author = publisher, with `merged_from = M`
    recorded so history shows the lineage without needing a branch model.
    Conflict → **reject** with `{ conflict: true, base, theirs, ours }` so the
    client presents markers / a merge editor.
  - **base_version omitted** → legacy last-write-wins (a plain overwrite),
    preserved so non-versioning callers (telnet `@program`, old clients) keep
    working. Versioning is opt-in per write, never a hard break.

This is the "server merge" without git: versioned rows + a text 3-way merge
(`diff3`-style; a small Rust crate like `diffy` does it — it's line-based, not
semantic, so a real conflict falls back to markers exactly like git) in the
write path.

## Part 3 — Edit locks (pessimistic, person-held)

Claim-before-edit, so conflicts are prevented up front — the fast path that
makes Part 2 the rare fallback.

**Naming — do not conflate with `system:locked`.** `system:locked` is a
permanent, inherited-nowhere tag meaning "this *definition* is file-authoritative
and read-only to authoring" (the `std/*` code tier). An **edit lock** is a
transient, person-held, advisory-but-enforced claim: "I'm editing this, hands
off." Different lifecycle, different purpose. Keep "lock" for the tier tag; this
feature is an **edit lock** everywhere in code and UI.

**State (DB rows).** `ref_id (+name for libs) → { held_by: account, held_at,
expires_at }`.

**REST.**
- `lock_script { ref_id, name? }` → claims it; if already held by someone else,
  fails and returns the holder (`held_by`, `held_at`) so the client can say
  "held by Sam since 10:42."
- `unlock_script { ref_id, name? }` → releases; **holder or an admin** only.
- Lock fields ride along in `get_script` / `list_libs` / `list_programs_all`.

**Enforcement: hard block, with two escape hatches** (chosen over soft-warn — no
silent overwrites — but degrading gracefully):
1. `set_script`/`set_lib` by a non-holder while locked is **rejected**.
2. **Auto-expiry** — a lock past `expires_at` is claimable by anyone (kills the
   "claimed it, went to lunch, blocked the team" failure). Publishing/renewing
   by the holder pushes `expires_at` out.
3. **Admin force-unlock** — `unlock_script` by an admin releases anyone's lock.

**Edit locks apply only to person-edited scripts.** A lock is claimable only on
an **InGame-origin, non-`system:locked`** object — the content tier a person
actually authors. `File`-origin scripts and `std/*` locked code are reconciled
*from disk*, never claimed by a builder, so they're never edit-lockable. This
falls straight out of the `ProgramOrigin` + tier split; the two lock concepts
never interact.

## API contract

All actions are `POST /api` with `#[serde(tag="action", rename_all="snake_case")]`
variants and the `{ ok, data?, error? }` envelope. Auth is `Authorization:
Bearer <token>`, resolved to an account in the existing auth block.

### `me` — whoami (new; needed by the client for lock ownership)

The client can't render "held by *you*" vs "held by someone else", or disable
publish on others' locks, without knowing which account its token is. There is
no such endpoint today.

- **Gate:** authenticated (valid token) but **not** builder — same tier as the
  world reads (`ListRooms`/`ListObjects`). A token identifying itself needs no
  privilege.
- **Request:** `{ "action": "me" }`
- **Response:** `{ account_id, username, scopes: ["builder","admin"], email?,
  active_character? }` — straight off the resolved `Account`. `account_id` is the
  stable key the client compares against lock `held_by` to decide "held by *me*"
  (usernames can change; ids don't).

### Tables (additive `CREATE TABLE IF NOT EXISTS` in `db.rs::migrate()`)

```sql
CREATE TABLE IF NOT EXISTS script_versions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ref_id        TEXT    NOT NULL,
    kind          TEXT    NOT NULL,             -- 'script' | 'lib'
    name          TEXT,                         -- lib module name; NULL for scripts
    version       INTEGER NOT NULL,             -- per-(ref_id,kind,name) monotonic
    source        TEXT    NOT NULL,
    source_hash   TEXT    NOT NULL,             -- blake3, for no-op suppression
    author        TEXT    NOT NULL,             -- account id, or 'system:file'/'system:script'
    origin        TEXT    NOT NULL,             -- 'file' | 'in_game'
    merged_from   INTEGER,                      -- version merged across, if any
    created_at    INTEGER NOT NULL,             -- unix secs
    UNIQUE(ref_id, kind, name, version)
);

CREATE TABLE IF NOT EXISTS script_locks (
    ref_id      TEXT NOT NULL,
    name        TEXT,                           -- lib name; NULL for scripts
    held_by     TEXT NOT NULL,                  -- account id
    held_at     INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    PRIMARY KEY (ref_id, name)
);
```

### Changed actions (fields added, back-compatible)

- **`get_script { ref_id }`** → adds `version` and, when present, `lock:
  { held_by, held_by_name, held_at, expires_at }`. Existing `{ source, hooks,
  enabled }` stay. `held_by` is the account id (compare to `me.account_id`);
  `held_by_name` is the username, resolved server-side for display.
- **`list_libs { ref_id }`** → each entry adds `version` + optional `lock`.
- **`list_programs_all`** → each object adds `version` + optional `lock` per
  script (libs carry theirs), so the tree renders dirty/lock state in one call.
- **`set_script { ref_id, source, base_version? }`** and
  **`set_lib { ref_id, name, source, base_version? }`** → the merge-aware write:
  - omit `base_version` → today's behaviour (overwrite), unchanged.
  - with `base_version` → `{ version }` on clean apply/merge (plus `merged_from`
    when it merged), or **`ok:false`** with
    `error:"conflict"` and `data:{ conflict:true, base, theirs, ours, current_version }`.
  - **Gates (all pre-existing, reused):** Builder scope; `locked_target` refusal
    for `system:locked` objects; **edit-lock enforcement** — held by another
    account and unexpired → `ok:false, error:"locked by <username>"`.

### New actions

- **`list_script_versions { ref_id, name? }`** → `{ versions: [{ version,
  author, author_name, origin, created_at, hash, merged_from? }] }`, newest
  first. `author` is the stored id (`system:file`/`system:script` for
  non-human); `author_name` is resolved server-side for display, so the client
  never maps ids → usernames itself. *Builder.*
- **`get_script_version { ref_id, name?, version }`** → `{ source }`. *Builder.*
- **`revert_script { ref_id, name?, version }`** → re-applies that version's
  source as a **new** version; returns `{ version }`. Same gates as `set_script`
  (Builder, tier lock, edit lock). *Builder.*
- **`lock_script { ref_id, name? }`** → claims the edit lock; on success
  `{ held_by, held_by_name, held_at, expires_at }`; if held by another unexpired
  account, `ok:false` with `data:{ held_by, held_by_name, held_at }`. *Builder;
  InGame + non-locked targets only.*
- **`unlock_script { ref_id, name? }`** → releases; holder **or admin** only.
  `{ ok:true }`. *Builder (own) / Admin (any).*

### Author resolution (one helper, all write paths)

`set_script_with_origin`/`set_lib` derive the version author once: token account
→ that id; `ProgramOrigin::File` → `system:file`; softcode-driven write →
`system:script`. Every surface therefore versions consistently without each
call site knowing the rule.

## Server vs extension — who builds what

| Concern | Server (REST) | Extension (client) |
| --- | --- | --- |
| Version append + table | ✅ | — |
| `list/get_script_version` | ✅ | history list, pick-a-version |
| 3-way merge on stale base | ✅ | present conflict → markers/merge editor |
| Edit lock state + enforce | ✅ | lock icon + holder, Claim/Release, disable publish when held by other |
| `me` identity | ✅ | resolve "held by you" vs another, gate publish UI |
| Diff | — | native VS Code diff of two fetched versions |

The extension never *decides* anything authoritative — it renders lock/version
state and drives the REST calls. Everything enforceable is enforced server-side,
so telnet/web/any REST client get the same guarantees.

## Extension-side consumption — which calls, and when

The guiding rule: **fetch on events the user triggers, never poll.** State that
rides existing reads (version, lock) costs nothing extra to render; history and
old-version bodies are fetched lazily, only when a panel is opened.

### Identity — once per connection

- **On connect** (token set / server change): `me` → cache `{ account_id,
  username }` for the session. Every "is this lock mine?" is a local
  `lock.held_by === cached.account_id` compare — **no per-item call**. Re-run
  `me` only when the token or server URL changes.

### The tree — one call, already made

- **On connect / Refresh:** `list_programs_all` (already the tree's feed) now
  carries `version` + optional `lock` per object/lib. Render a lock glyph +
  `held_by_name` (or "you"), and the dirty marker stays a local
  working-copy-vs-baseline compare. **Zero new per-node calls** — the whole tree's
  lock/version state arrives in the one existing request.

### Opening a script — fetch + capture the base

- **On open:** `get_script { ref_id }` → `{ source, version, lock? }`. **Cache
  `version` as this buffer's `base_version`** — it's the merge base every publish
  will send. Render:
  - unlocked → editable; offer **Claim** (explicit — decided; no silent
    auto-claim on keystroke);
  - locked by you → editable, "held by you" affordance;
  - locked by another (unexpired) → open **read-only** with a banner ("held by
    Sam since 10:42 — publish disabled"); a **Diff vs mine** is still allowed.

### Claiming + holding a lock (Phase 3)

- **Claim** — an **explicit command** (decided; never auto-claimed on a
  keystroke, so a lock is always a deliberate act): `lock_script { ref_id,
  name? }`. Success → update banner, start a **renewal timer**. Failure (raced) →
  show the returned `held_by_name`, flip the buffer read-only.
- **Renew while editing:** the holder's `set_script`/`set_lib` pushes
  `expires_at` out server-side, so a normal publish renews for free. For a long
  edit with no publish, a lightweight timer re-issues `lock_script` a bit before
  `expires_at`. No heartbeat when the editor isn't focused — let it expire.
- **Release:** `unlock_script` on an explicit **Release** command and
  best-effort on document close; otherwise expiry reclaims it. **Admin
  force-unlock** is a separate command that calls `unlock_script` on someone
  else's lock.

### Publishing — the base_version + conflict flow

Publish is an **explicit command** (decided — not bound to ⌘S). ⌘S saves the
local working copy only; a separate **Publish** pushes to the server. This keeps
local editing free of server round-trips, and gives the conflict flow a place to
land (a bare save/`writeFile` can only throw). The sequence:

1. `set_script { ref_id, source, base_version }` (the cached base).
2. **Clean apply / clean merge** → `{ version }` (maybe `merged_from`). Update
   the buffer's `base_version` to the new `version`; clear dirty; refresh the
   node. Done.
3. **Conflict** → `ok:false, error:"conflict", data:{ base, theirs, ours,
   current_version }`. The extension:
   - opens a **3-way view** — `theirs` (current server *M*) vs `ours`, with
     `base` available — using VS Code's merge/diff UI over the three in-memory
     strings;
   - the user resolves into their buffer;
   - **re-publish with `base_version = current_version`** (*M*) → now a clean
     apply. The client never merges; it only renders the server's three sides and
     re-sends.
4. **Locked by another** → `ok:false, error:"locked by <name>"`. Publish stays
   disabled in the UI for others' locks, so this is a backstop, not the normal
   path.

### History, diff, revert — lazy, on request

- **Open History** (per object/lib): `list_script_versions { ref_id, name? }` →
  render `version · author_name · created_at`, a ⑃ marker where `merged_from` is
  set. One call, only when the panel opens.
- **View a version:** `get_script_version { ref_id, name?, version }` → open a
  **read-only** doc.
- **Diff:** fetch the two sides (`get_script_version` ×2, or current
  `get_script` for "vs server", or the local buffer for "vs working") → native
  `vscode.diff`. The server never diffs.
- **Revert:** `revert_script { ref_id, name?, version }` → `{ version }`; reload
  the buffer from it, set `base_version` to the new version, refresh. Gated
  server-side exactly like a publish (Builder, tier lock, edit lock).

### Call budget

| Event | Calls |
| --- | --- |
| Connect | `me` (×1), `list_programs_all` (×1) |
| Refresh tree | `list_programs_all` (×1) |
| Open a script | `get_script` (×1) [+ `lock_script` if claiming] |
| Keep editing | none (timer-renew only on a long unpublished edit) |
| Publish (clean) | `set_script` (×1) |
| Publish (conflict) | `set_script` (×1) → resolve → `set_script` (×1) |
| Open History | `list_script_versions` (×1) |
| View / diff a version | `get_script_version` (×1–2) |
| Revert | `revert_script` (×1) |

Steady-state editing is **quiet** — no background polling, no per-node fan-out.
Everything is either piggybacked on the one tree read or triggered by an explicit
user action.

## Fits the single-writer engine

Versions, merges, and lock claims are all writes → they funnel through the
existing `EngineMessage`/intent path like every other mutation. No second
writer, no new locking primitive on world state; a lock *claim* is itself just a
serialized engine write.

## Interactions with Hearth's existing machinery

The load-bearing edges — a versioning scheme that ignores these breaks Hearth's
own reconciliation model:

- **Boot / `@reload-world` file reconciliation.** When a changed `<area>` file
  updates a `File`-origin managed object's script (blake3 hash mismatch), that
  write goes through `set_script_with_origin(.., File)` — so it **appends a
  version authored as the file/system**, and history captures deploys, not just
  in-game edits. An *unchanged* file re-applies nothing (no-op by hash). It never
  touches `InGame` scripts — the reconciliation already skips those, and the
  version log makes that visible ("last change: file" vs "last change: Sam").
- **`File`-origin scripts are never edit-lockable** (Part 3) — they're owned by
  disk, reconciled on boot; a person never claims them. Edit locks live entirely
  on the `InGame` content tier.
- **`@import` / `@export`.** `@import` re-establishes a `File`-origin baseline →
  appends versions like reconciliation. `@export` emits **current source only** —
  history is DB-only and does **not** round-trip through files. Say this plainly:
  `git log`-through-`@export` is not a thing; the DB is the history, and it
  travels with the checkpoint, not the exported bundle.
- **Softcode-driven writes.** Luau can call `set_script`/`set_lib` via the write
  API. Those append versions too (origin `InGame`, author = the acting object /
  system), so a script that rewrites another object's code is in the history like
  any other write — no special case.

## Staged rollout

1. **Versioning + history (low risk).** Append versions on every softcode write;
   ship `list/get_script_version`. Read-only history + client-side diff — real
   value, no behaviour change to publishing (base_version still optional).
2. **Merge on stale base.** Add `base_version` handling + the 3-way merge and
   conflict response. Now concurrent edits stop silently clobbering.
3. **Edit locks.** `lock/unlock_script`, lock fields on reads, hard-block
   enforcement + expiry + admin override, and the extension's lock UX. Ship
   **`me`** here (it's a trivial standalone action, and it's what the client
   needs to render lock ownership).

## Risks / open questions

- **History growth** — a busy world writes a lot of versions. Fine for SQLite;
  add compaction (keep last N / coalesce rapid saves by one author) only if it
  bites.
- **Merge quality** — a line-based 3-way merge on Luau is "good enough," not
  semantic; conflicts fall back to markers, same as git. Don't oversell it.
- **Lock expiry window** — **30 min, renewed on publish** (decided). Long enough
  not to thrash a real editing session, short enough that a forgotten lock frees
  itself before it blocks the team for long.
- **Author identity across surfaces** — REST/web carry an account token;
  `set_script` from the loader or from softcode has no human author. The version
  record needs a small "who" resolution: token account → builder; File-origin →
  `system:file`; softcode-driven → `system:script`. Cheap, but decide the vocab
  up front so history reads consistently.
- **Is Part 3 needed if Part 2 is good?** Possibly not for a solo builder. Build
  locks when a *team* actually collides; versioning + merge covers the solo case
  alone.

## Relationship to the VS Code extension

The extension's current build edits softcode over REST already. This plan is what
turns it from "last write wins, no history" into "versioned, merge-aware, and
claimable" — all as additional REST actions it consumes. None of it requires
git, a mirror, or making the server anything other than what it is: the single
writer that owns the world.
