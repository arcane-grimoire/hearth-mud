# Plan: Program authoring, versioning, and packages

## Goal

Give Programs a version history, and resolve the ambiguity about whether the
database or the game files own a Program.

The starting question was "can we keep program versions in the DB, or do some
kind of git integration?" The answer turned out to be downstream of a bigger
one, so this plan covers both.

## The model

**The database is the source of truth. Files are a distribution format.**

A Program lives in the DB. Game files are a *package* that gets installed into
the DB — a portable bundle another server could also install. Installing is an
explicit, versioned act, not an ambient reconciliation that happens on every
boot.

That gives two versioning stories, each handling what it is good at:

| Layer | Owned by | Versioned by |
| ----- | -------- | ------------ |
| Package content | Game repo | git / semver, upstream |
| In-game edits | Database | Append-only version log in SQLite |

Provenance on each Program says which layer it belongs to, so the two never
fight.

## Why not file-first

The obvious alternative is to make files the truth: in-game edits write back to
`.luau` on disk, and git becomes the version history for free. This is the
LPMUD model and it works well on a durable box.

It was rejected because Hearth is deployed cattle-style. In a container the
filesystem is ephemeral and credential-less: in-game edits vanish on redeploy,
and pushing to git from inside the runtime means giving it a working tree and a
deploy key. The DB (on a volume) is the only durable writable store, so it has
to own live content.

The git integration people actually want from file-first is still available,
just as a *promotion path* rather than a live sync: export DB-owned Programs to
files, commit them in the game repo, and they ship in the next release. The DB
is the working copy; the git repo is the release.

## Background: how this went wrong

Three symptoms, one root cause — the DB was being treated as a cache of the
files rather than as the truth.

1. `install_programs` deleted every Program on a managed object that the TOML
   did not name, and overwrote the ones it did. In-game edits, accumulated
   `state`, and `enabled` were all lost.
2. `Engine::new` loads with *no previous file hashes*, so every startup treats
   every file as changed and reconciles everything. In a container every deploy
   is a startup, so in-game edits to managed objects were guaranteed destroyed
   on deploy.
3. Nothing installed a SIGTERM handler, so the "graceful shutdown" checkpoint
   that ADR 0003 names as one of its three triggers never fired under a
   container stop. Every deploy discarded world state back to the last autosave.

Versioning was unbuildable on top of that: there is no point versioning state
that a deploy throws away.

## Stage 0 — done

Both landed; suite went from 130 to 134 tests.

**Save on SIGTERM** (`src/main.rs`). `shutdown_signal()` listens for ctrl-c and
SIGTERM, `main` sends `EngineMessage::Shutdown` and awaits the engine with a
30s timeout so a wedged engine cannot outlive the container grace period.
Verified end-to-end with autosave disabled: SIGTERM took the DB from empty to
34 persisted objects, a path that was previously dead code.

**Provenance** (`src/softcode/hooks.rs`, `src/loader.rs`). `ProgramOrigin` is
`File` or `InGame`; `set_program_with_origin` records it. The loader now
reconciles only `File` programs — an unlisted `InGame` program is a builder's
addition rather than a stale file program, and a listed one is an override that
shadows the file version.

Two decisions worth remembering:

- **`set_program` preserves the accumulated `state` map** when replacing a
  Program. Rewriting an `on_tick` hook should not reset what it has been
  remembering, and the loader reinstalls file programs on every startup.
- **The serde default for `origin` is `File`, not `InGame`.** Counterintuitive,
  but correct as a migration: on a managed object a legacy record can only be
  file-authored, because in-game ones were already destroyed at the last
  restart. On an unmanaged object the field is never consulted, since the
  loader skips those entirely. So no migration step is needed.

Tests in `src/loader.rs`: `reload_preserves_in_game_programs_on_managed_objects`
(the regression), `in_game_program_shadows_the_file_version`,
`reload_preserves_accumulated_program_state`, and
`reload_removes_file_programs_dropped_from_the_files` as a guard against
over-fixing. The first three were confirmed to fail against the old logic.

## Stage 1 — record versions (write-only)

Only `InGame` Programs need a DB version log. Package content is already
versioned upstream in git, and `origin` now tells the two apart — which is what
makes the storage cost a non-issue rather than merely a managed one.

Deliberately write-only. No UI, no diff, no revert. It is cheap, impossible to
regret, and means that when history is wanted it already exists instead of the
clock starting then. This is the Smalltalk `.changes` lesson: the log is the
primitive, the tools come after.

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
once regardless of how many versions or objects reference it, and a revert
costs zero new bytes because it points at an existing blob.

Notes:

- **Write at edit time, not at checkpoint.** History is append-only audit data,
  not world state, so it should not wait for the wholesale
  `DELETE FROM objects` rewrite in `do_save`. This is a deliberate departure
  from ADR 0003 and deserves its own ADR.
- **Record in the Engine, not in `hooks.rs`.** `hooks` has no `Database`
  handle. The three write paths — `cmd_program` (`src/engine/mod.rs`), the API
  `SetProgram`, and the softcode `set_program` Intent — all land back in the
  engine, which owns the db.
- **Use `blake3`, not the existing `source_hash`.** Both the bytecode cache
  (`src/softcode/mod.rs`) and the ink cache use `DefaultHasher`, which is fine
  in memory but is explicitly not stable across Rust versions. Persisting
  content-addressed keys with it would orphan every blob on a toolchain bump.

## Stage 2 — read them

`@program/history`, `@program/diff`, `@program/revert`. Use the `similar` crate
for diffing (`diffy` if patch *application* is ever wanted for three-way
merge). Add a retention cap per `(obj_ref, hook)` here, not in stage 1.

Fix alongside, or telnet users cannot exercise any of it: **`@program` splits on
the first `=`**, so any Luau source containing `=` cannot be authored from
telnet at all. Real authoring currently only works through the web Editor. The
`@dialogue` multi-line editor in `src/engine/mod.rs` is the pattern to copy —
note its caveats, though: it stores the buffer in actor attrs (so it persists
mid-edit and survives disconnect), rebuilds the string per line, and reserves a
bare `.` with no escape hatch.

## Stage 3 — packages

Generalizes stage 0 rather than replacing it: `ProgramOrigin::File` becomes
`ProgramOrigin::Package(name)`, and the reconciliation rule already written
keeps its shape.

Blast radius is small: `system:managed` is consulted **only in `loader.rs`** —
nothing in the engine, db, or softcode reads it.

**Minimum viable version**, which is worth doing even if install-from-elsewhere
never happens:

- A `[package] name/version` manifest.
- A `packages` table recording what is installed.
- Version-gated loading at boot: if the installed version matches the manifest,
  do nothing at all; otherwise run an upgrade.

That last point kills boot-time reconciliation, which is the half of the deploy
bug stage 0 did not fix — startup still re-reads every file and reinstalls every
file Program. It also turns a rolled-back deploy into a recorded downgrade
rather than a silent rewrite of everyone's Program source. Same shape as schema
migrations.

`@reload-world` then becomes `@package upgrade` — an explicit act with a version
attached. Dev hot-reload is "reinstall the dev package," the same code path.

### The endgame: layering, not merging

Packages provide a base layer; local edits shadow it; the package layer is never
mutated. Upgrade becomes "swap the layer" and "revert to package default"
becomes "delete the override."

The stage 0 origin flag is the cheap 80% of this and grows into it. The cost
when the full version is wanted: hook lookup resolves through two layers,
touching `hooks::get_program` and the firing paths in `src/engine/mod.rs`.

## Open questions

These are the real design work; none are blocking stages 1 and 2.

- **Upgrade semantics.** What happens to objects a new package version renamed
  or removed? What is reported when a local override has diverged from a
  changed package Program?
- **Collisions.** Two packages both defining `cmd_attack`, or the same area key.
  Namespacing, or load-order precedence?
- **Uninstall.** Remove package-owned objects and leave local ones? What happens
  to the orphans, and to in-game overrides of Programs that no longer have a
  base?
- **Export format.** Does promotion emit `.luau` plus TOML stanzas, or a single
  bundle? Does it round-trip?
- **Does ADR 0003 need revising** beyond the append-at-edit-time carve-out?
  The wholesale `DELETE FROM objects` checkpoint is what forces that carve-out
  in the first place.

## Prior art

- **Smalltalk (Squeak/Pharo)** — the closest match to the situation: a live
  editable image that still needs version control. The `.changes` file is an
  append-only log of every source change (stage 1); Monticello exports packages
  from the image to committable files (the promotion path).
- **Fossil** — an SCM built on SQLite, content-addressed blobs in a table with
  delta compression. Proof the storage model works, and the answer if plain-text
  rows are ever outgrown.
- **Foundry VTT modules** — closest analogy for stage 3: a game server installs
  content bundles with a manifest, package content is package-owned, local
  overrides survive upgrades.
- **dpkg conffiles** — the specific "user edited a shipped file" problem, with a
  real answer (three-way merge and a prompt). Do not reinvent this.
- **MOO / PennMUSH / TinyMUSH** — db-first with in-game editing and *no* native
  version history; cores bolted it on later. LambdaCore/JHCore are the MUD
  precedent for "start from this db."
- **Evennia** — answers this exact question with the same split: code in git,
  instances in the DB.

## Risks

- Stage 1 puts a synchronous SQLite write on the single-writer engine loop at
  every Program edit. Small and rare, but it is a new blocking call in the hot
  task.
- Stage 3's upgrade semantics are the largest design surface here and are easy
  to underestimate.
- `list_programs` is in the **unauthenticated** read set in the API handler
  (`src/engine/mod.rs`), so it serves full Program source to anyone who can
  reach `/api`. Unrelated to this plan, but it should not survive contact with
  a deployed server.
