# Plan: Program authoring, libraries, and the file boundary

## Goal

Make in-game authoring a first-class way to build a Hearth world, and settle
what files are for once that is true.

This supersedes an earlier version of this plan that tried to reconcile files
and the database on every boot. See [Superseded](#superseded) for what changed
and why.

**Status:** stages 1–3 are implemented (194 tests, up from 137). Stage 4 is not
started and needs a decision on the dev loop before it should be — see the note
at the end of that section.

## The model

**The database owns what is edited at runtime. Files own what is not.**

That is the whole rule, and every question in this plan resolves against it.

| Thing | Lives in | Versioned by |
| ----- | -------- | ------------ |
| Programs authored in-game | Database | Version log in SQLite |
| User libraries | Database | Version log in SQLite |
| Shipped lib modules (`lib/`, embedded stdlib) | Files → runtime registry | git |
| World definition (TOML) | Files, imported once | git upstream, version log after import |

The container argument that drove the original plan was about **writes**, not
reads. Files in a deployed image are perfectly durable and git versions them for
free. What files cannot be is *written to* by a running server on an ephemeral
filesystem. So read-only-at-runtime content stays in files, and that is correct
rather than a compromise.

### Why not files-as-truth

The LPMUD/Diku model — in-game edits write back to `.c`/`.wld` on disk, git
becomes the history for free — works well on a durable box. It was rejected
because Hearth deploys cattle-style: in a container the filesystem is ephemeral
and credential-less, so in-game edits vanish on redeploy, and pushing to git
from inside the runtime means giving the process a working tree and a deploy
key. The DB on a volume is the only durable writable store.

Evennia's answer is the opposite: no in-game code authoring at all, so the
conflict cannot arise. That is coherent, but it trades away the property that
makes MUSH and MOO worlds what they are — the world is editable from inside
itself. Hearth keeps that.

### Why not reconcile on boot

The rejected design read the files at every startup and mutated the DB to match.
That is what destroyed in-game edits on deploy (see [Superseded](#superseded)).
Note the distinction that took a while to surface:

- **Reconcile** — read files, compute the difference, *mutate the DB*. Harmful.
- **Diff** — read files, compute the difference, *report it*. Free and useful.

Nothing in this plan reconciles. Reporting a divergence is fine and cheap if it
is ever wanted.

## Stage 1 — `@eval`

An admin command that runs a one-shot Luau script against the live world.

This is first because everything after it needs a migration mechanism, and
because Hearth has no way to fix up existing world data today. It is Evennia's
`@batchcode` and MUSH's paste-a-command-script, which are the mechanisms those
systems actually use when code changes and old instances need updating.

Requirements:

- **Admin scope only.** This is arbitrary code with the full write API — the
  most dangerous command in the system.
- **Runs under a `Budget`**, like any hook.
- **Multi-line input.** Same problem `@program` has; use the `@dialogue`
  multi-line editor in `src/engine/mod.rs` as the pattern rather than inventing
  a second one. Note its caveats: it stores the buffer in actor attrs (so it
  survives disconnect mid-edit), rebuilds the string per line, and reserves a
  bare `.` with no escape hatch.
- **Records what was run.** An `@eval` that rewrites world data is exactly the
  event most worth an audit trail, and it is the one write path that would not
  otherwise leave one.
- **World enumeration.** `get_all_by_kind` exists but returns full object tables
  one kind at a time, so reaching the whole world means five calls. Add
  `all_objects()` returning bare refs.
- **Its own budget.** `Budget::default()` is 200k instructions, sized so a
  runaway hook dies well inside a tick — far too small for sweeping every object
  in the world, which is the job `@eval` exists for. `Budget::for_eval()` is
  orders of magnitude larger but still finite: `@eval` runs on the single-writer
  loop, so the world is frozen while it runs.

## Stage 2 — scripts and libraries as an object kind

`src/world/script.rs` defines `Script { name, source, entry, interval, enabled,
state }`. That is `ProgramRecord` with an interval bolted on. Hearth currently
maintains two near-identical homes for code, with separate persistence, state
handling, and reload paths — and user libraries would make three.

Collapse them into one. Add a single neutral `Kind::Code`; a global tick script
becomes an object with an `on_tick` Program and an interval, which objects
already support. A library becomes an object whose Programs are named
`lib_<name>`.

**One Kind, not two.** Scripts and libraries differ in *behaviour* — one runs on
a schedule, the other is required by others — but the property the `Kind` is
buying is *exclusion*, and that is identical for both. Behaviour is already
expressed by what Programs an object carries: `on_tick` plus an interval means
it ticks, `lib_*` Programs mean it is requireable, and an object may be both.
Two Kinds would double the match arms for types that behave identically
everywhere except the tick scheduler and `require` resolution, and would put a
`Kind::Script` label on things that are libraries in `@list` output.

Everything downstream then has one code path: versioning, ownership via
`can_modify_object`, `check_syntax` on write, the web Editor's program CRUD.
Stage 3 covers Scripts for free rather than needing a parallel version table.

**A `Kind` variant, not a `system:library` tag.** The critical property is
*exclusion* — a library must never appear in room contents, `look`, inventory,
`get`, or a container. With a tag, every one of those sites has to remember to
filter and a missed one is found in play. With a `Kind`, match arms are
exhaustive and the compiler enumerates every site that has to decide. This is
the case where the enum earns its keep over Hearth's usual tag convention.

**Migrating existing `Script` rows** into objects is a Rust-side migration at
load, not an `@eval` job — softcode has no access to the `scripts` table, so
`@eval` is the tool for world *data* (attrs and state on objects), not for
schema-level moves like this one.

### `require` resolution

Two populations, each with exactly one owner, so nothing can diverge:

- **Shipped libs** — `load_modules` reads the embedded stdlib and
  `<game_dir>/lib/` into the Lua module registry at boot. Not persisted. Not
  changed by this plan.
- **User libs** — library objects in the DB, authored in-game, covered by the
  version log.

Loading shipped libs into the DB was considered and rejected: it puts
file-owned content in the DB, which forces every boot to decide whether to
overwrite it, which re-creates the provenance problem this plan deletes.

One rule: **authoring a user lib whose name collides with a shipped module is
refused at write time.** Loud and early beats someone shadowing `str`
server-wide and finding out three hooks later.

Cycle detection landed already (`4c50204`) and matters more once users author
libraries, since cycles stop being an author-time mistake caught in dev.

## Stage 3 — Program versions

Once the DB owns authored code, git is no longer its history. There is no other
history. This is the safety net that makes stage 4 acceptable.

Scope it honestly: this is **versions, not version control**. A numbered list of
prior sources per program — no branching, no merging, no changesets. The
Smalltalk `.changes` file is the precedent; Monticello, which exported packages
to committable files, is stage 4's `@export`.

```sql
CREATE TABLE program_blobs (
  hash   TEXT PRIMARY KEY,
  source TEXT NOT NULL
);
CREATE TABLE program_versions (
  id         INTEGER PRIMARY KEY,
  obj_ref    TEXT NOT NULL,
  hook       TEXT NOT NULL,
  blob_hash  TEXT NOT NULL REFERENCES program_blobs(hash),
  created_at INTEGER NOT NULL,
  author     TEXT
);
```

Content-addressing (git's and Fossil's model) means identical source is stored
once no matter how many versions reference it, and a restore costs zero new
bytes.

Notes:

- **Write at edit time, not at checkpoint.** History is append-only audit data,
  not world state, so it should not wait for the wholesale `DELETE FROM objects`
  rewrite in `do_save`. A useful side effect: after a crash the log holds
  sources *newer* than the last checkpoint, making it recovery for the content
  most expensive to lose. This is a deliberate departure from ADR 0003 and
  deserves its own ADR.
- **Retention cap belongs here, not later.** A version list that grows without
  bound is the only way this bites. Oldest rolls off, VMS-style. Implemented as
  50 per `(obj_ref, hook)`.
- **Version the authoring paths, not the instantiation one.** `@program`,
  `@lib`, `@script`, REST `SetProgram`, and the loader's file installs all
  record; softcode's `Intent::SetProgram` does not (see "Instantiation is not
  authoring" below). `@script-interval` writes an attr rather than program
  source, so it records nothing. The loader's installs are recorded to give a
  baseline for a future package-vs-local hash comparison; dedupe makes repeated
  boot installs free.
- **Restore is non-destructive.** `@program/restore <n>` writes an old source
  back as a *new* version rather than rewinding. History can then never be made
  worse by using it, which is the property that makes people willing to touch
  it.
- **Record deletions as tombstone versions**, or deletion is the one path where
  "versions make it safe" is false. This needs a `deleted` column, not just an
  empty blob: a tombstone and a program whose source is genuinely empty
  otherwise hash identically and become indistinguishable.
- **Dedupe on content.** Skip the insert when the newest row for
  `(obj_ref, hook)` already has that `blob_hash`. Makes repeated installs
  no-ops and removes any need to special-case where a write came from.
- **Use `blake3`, not the existing `source_hash`.** The bytecode and ink caches
  use `DefaultHasher`, which is explicitly not stable across Rust versions.
  Persisting content-addressed keys with it would orphan every blob on a
  toolchain bump.

### Author

All three write paths have an actor available:

- `cmd_program` — `actor_ref`, in hand.
- API `SetProgram` — `account_id`, resolved in the auth block
  (`src/engine/mod.rs:674`).
- Softcode `Intent::SetProgram` — `run_hook` (`src/softcode/mod.rs:766`) takes
  `actor_ref` and builds the `IntentBatch` a few lines later; stamp the batch and
  let the intent inherit it. `actor_ref` is the right one: when a builder runs a
  command that calls `set_program`, the human is the actor, not the object the
  hook is attached to.

One nullable `TEXT` column. Null covers ticks and timers, where there genuinely
is no one. Store the ref and resolve to a display name at read time — names
change, and a stale name in a history listing is worse than none.

**While stamping the batch, stamp the owning authority too.** MUSH-level UGC —
players authoring code behind a builder flag — rests on one property: code runs
with the authority of the object's *owner*, not whoever triggered it. Hearth has
nowhere to put that today, because `apply_to` (`src/softcode/mod.rs:264`)
validates only that intent targets exist and the batch carries no authority at
all. Adding the field alongside `actor_ref` is one extra assignment in the same
place, and it makes the eventual permission work an enforcement change inside
`apply_to` rather than a retrofit through every call site. Enforcement, API
authority tiers, and creation quotas are a separate plan; this is only the hook
they will need.

### Prerequisite: multi-line authoring

`@program` reads a single line, so no multi-line Luau can be authored from
telnet at all — real authoring currently only works through the web Editor. Fix
alongside stage 3, sharing whatever stage 1 builds for `@eval`.

(The splitting-on-`=` behaviour is *not* the problem: `split_once('=')` takes
the first `=`, and the path before it cannot contain one, so
`@program #1/on_tick = local x = 5` already parses correctly.)

### Instantiation is not authoring

The Last Stag calls `set_program` from softcode to attach behaviour to
*procedurally generated* objects — `on_enter_map.luau:120-133` wires a spawned
guide NPC and a generated room, `cmd_delve.luau:69` a generated dungeon
entrance. The source is already in files as string constants; this is Evennia's
typeclass instantiation done manually, because Hearth has no classes.

Do not version these and do not treat them as authoring. They cannot conflict
with anything, because their source is in git.

**Latent trap:** softcode `set_program` currently marks programs `InGame`, so if
one ever targets a file-declared object it permanently shadows the file version
across every reload. Nothing does this today — all three call sites target
generated objects — but there is no guard and the failure is silent and sticky.
Resolved by stage 4 removing boot-time file programs entirely; worth a guard
before then.

## Stage 4 — `@import` / `@export`

Boot reads the DB only. Files become a distribution format crossing an explicit
boundary in both directions.

- **`@import`** installs a TOML+`.luau` bundle into the DB. Once. Explicitly.
  Must be **idempotent by identity** — importing `town.toml` twice cannot create
  two crossroads. `FILE_KEY_ATTR` already stamps a `<area>/<key>` identity on
  every object the loader creates; keep that mechanism and drop only the
  ownership and reconcile semantics layered on top of it.
- **`@export`** emits DB-owned content back to files as **TOML + `.luau`** — the
  same format `@import` reads and the same format game authors hand-write today.
  Round-trips by construction and diffs well in git. A MUSH-style command script
  (`@decompile`) was considered: its advantage is that installing becomes replay
  of authoring, so no reconciler is possible, but `@import` is already explicit
  so that advantage is small, and it diffs worse. An asymmetry between what
  authors write and what export emits would be the worse outcome.
- **Key collisions** — two imports defining `cmd_attack`, or the same area key —
  **refuse the import**. Fail at import time and make the author resolve it.
  Same rule as a user lib colliding with a shipped module name, and the only
  option where you find out before the world is in a strange state.
  Last-import-wins is the shape of the original bug: content quietly replaced
  with no signal.
- **No uninstall in v1.** `@eval` covers removal. Uninstall is the largest
  design surface here for the least early value — what happens to objects a
  builder edited, to overrides whose base disappears, to orphans. Defer until
  there is a real case. If it is built, "remove only what is untouched" composes
  with content-addressed versions via the recorded-hash comparison.

**Export is not optional.** Today, losing the DB means restarting and having
files rebuild the world. After this stage the DB is the only copy of anything
authored in-game — which is the entire point — so `@export` is simultaneously
the git story and disaster recovery. If it does not round-trip, the world's only
copy is a SQLite file on a container volume.

### The dev loop: a CLI, not an overwrite mode

Removing boot-time loading appears to force a choice between authoring in-game
(losing LSP, git, and `.test.luau`) and giving `@import` a dev overwrite mode
(which is the reconciler wearing a different hat, and the path back to deploys
eating edits). A CLI client dissolves it.

Edit `.luau` in a normal editor, run `hearth program set <ref>/<hook> file.luau`
from the shell or an editor hook, and the change lands in the DB over the REST
API — live, no restart. Files stay the authoring surface for systems code; the
DB stays the truth; boot still loads nothing.

`hearth import` from the shell *is* the dev loop. Import is already idempotent
by identity, dedupe makes unchanged programs no-ops, and the hash comparison
below only overwrites what nobody edited — so no separate dev mode has to exist.

This makes the REST API load-bearing rather than a convenience for the web
client. Two consequences: an `Eval` action must be gated on `Scope::Admin`
rather than the generic `Builder` write check, because a token that can eval
owns the server; and `list_programs` must leave the unauthenticated read set.

A CLI can also work against a **cold** database directly, without a running
server — useful for bootstrapping a fresh DB before first boot. It must never do
so against a live one: the world lives in memory and `save_world` does
`DELETE FROM objects` and rewrites (`src/db.rs:246`), so a direct write to a
running server's DB is not a race, it is guaranteed to be discarded at the next
checkpoint.

### Upgrade: what re-import does

Re-importing a changed bundle is an **upgrade**, and it is a reconciler. That is
fine — what made boot-time reconciliation harmful was that it was ambient and
silent, not that it diffed. Explicit trigger, plus a report, plus recoverable
history is the difference.

Four cases per object:

- **In the file, not in the DB** — create.
- **In both, unchanged locally** — overwrite.
- **In both, edited locally** — the real question, below.
- **In the DB, gone from the file** — report it, do not remove it. Auto-deleting
  is exactly how `install_programs` caused the original bug.

For the third case, record the hash the import installed and compare three
things — recorded, current, incoming:

| recorded vs current | recorded vs incoming | action |
| --- | --- | --- |
| same | changed | overwrite — nobody touched it |
| changed | same | keep local — upstream did not change it |
| changed | changed | genuine conflict |

That is dpkg's conffile algorithm. Hearth can do better than dpkg's prompt,
because stage 3 gives it something dpkg lacks: **overwrite the conflict, preserve
the local version in the log, and report it loudly.** Non-destructive by
construction, the same property that makes `@program/restore` safe, so an import
never has to block on a prompt.

**Upgrade therefore depends on stage 3.** First-install import does not, and can
ship earlier.

`@import --dry-run` should report those four buckets before anything is written.
Cheap, and it turns "will this eat my work?" into something checkable.

### Upgrade vs copy

Two distinct operations worth keeping separate. **Upgrade** updates in place —
same dbrefs, state preserved. **Copy** instantiates a fresh set of objects from
the same bundle with new dbrefs, which is what instanced dungeons, template
areas, and a scratch sandbox to edit against all want.

## Open questions

Export format, key collisions, and uninstall are settled — see stage 4.

- **Migration of world data.** Programs change; object `state` and attrs shaped
  for the old version do not. `set_program` deliberately preserves accumulated
  `state`, which is right, but means a rewritten `on_tick` inherits whatever the
  old one remembered, silently. Evennia's highest-leverage answer is not
  migrations but defensive defaults (`AttributeProperty`); Hearth's equivalent
  is free — `get_attr(this, "hp") or 100` — and belongs in the softcode guide.
- **Does ADR 0003 need revising** beyond the append-at-edit-time carve-out? The
  wholesale `DELETE FROM objects` checkpoint is what forces it.

## Superseded

The previous version of this plan diagnosed a real bug and then proposed the
wrong shape for fixing it. Keeping the diagnosis, discarding the shape.

**The bug (fixed, stage 0, landed).** Three symptoms, one cause — the DB was
treated as a cache of the files:

1. `install_programs` deleted every Program on a managed object the TOML did not
   name. In-game edits, accumulated `state`, and `enabled` were all lost.
2. `Engine::new` loads with no previous file hashes, so every startup treated
   every file as changed. In a container every deploy is a startup.
3. Nothing installed a SIGTERM handler, so the graceful-shutdown checkpoint ADR
   0003 names never fired under a container stop.

Both fixes landed (`46750a0`, `bbeccce`): `shutdown_signal()` in `src/main.rs`
with a 30s bound, and `ProgramOrigin` in `src/softcode/hooks.rs` so the loader
reconciles only file programs. Tests in `src/loader.rs`, notably
`reload_preserves_in_game_programs_on_managed_objects`.

**What is superseded.** `ProgramOrigin`, `system:managed`-as-permission, and
boot-time reconciliation are scaffolding, not foundations. Stage 4 removes the
question they answer rather than answering it better. Two more ideas from that
draft are dropped:

- **A package manifest with a version gate at boot.** Install-once-never-verify
  loses the mirror's one genuine virtue — idempotence — and turns boot into a
  one-shot migration that can half-apply. Explicit `@import` gets the same
  benefit without pretending boot is a package manager.
- **Provenance as a flag.** If package-installed hashes are ever recorded,
  comparing hashes tells you more than a bit ever could — including whether a
  builder edited a program *back* to what the package said. This is dpkg's
  conffile algorithm and it falls out of content-addressed versions for free.

## Prior art

- **Smalltalk (Squeak/Pharo)** — the closest match: a live editable image that
  still needs version control. `.changes` is an append-only log of every source
  change (stage 3); Monticello exports packages from the image to committable
  files (stage 4's `@export`).
- **MUSH / MOO** — db-first with in-game authoring and *no* native version
  history; the culture is "keep a copy in a text file," which is the gap stage 3
  closes. Their distribution format is a command script (`@decompile`), not a
  data file.
- **LPMUD / Diku** — files-as-truth. Diku's OLC is the notable detail: builders
  edit in memory and nothing touches disk until an explicit save, answering
  "is this edit permanent?" by asking rather than inferring.
- **Evennia** — code in git, instances in the DB. Worth knowing that this does
  *not* eliminate migrations, it relocates them: `at_object_creation` runs once,
  so changing it never updates existing objects, and the real mechanism is
  `@batchcode` running imperative Python against the live DB. Stage 1 is that.
- **Fossil** — an SCM built on SQLite, content-addressed blobs in a table with
  delta compression. Proof the storage model works.
- **dpkg conffiles** — the "user edited a shipped file" problem with a real
  answer (recorded hash, three-way merge, prompt). Do not reinvent this.
- **MOO cores** — LambdaCore/JHCore are the cautionary one: the packaging answer
  was "ship the entire world," so cores forked and never merged back. Evidence
  that granularity matters more than mechanism.

## Risks

- Stage 3 puts a synchronous SQLite write on the single-writer engine loop at
  every Program edit. Small and rare, but a new blocking call in the hot task.
- Stage 4 makes `@export` load-bearing for disaster recovery. If it lags, the
  window where the DB is the only copy is a real exposure.
- `@eval` is arbitrary admin code against the live world. That is the point, but
  it raises the stakes on API hardening — and `list_programs` currently sits in
  the **unauthenticated** read set (`src/engine/mod.rs:669`), serving full
  Program source to anyone who can reach `/api`. Unrelated to this plan, one
  line to fix, and it should not survive contact with a deployed server.
