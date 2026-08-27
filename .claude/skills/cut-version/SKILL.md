---
description: Cut a new hearth-mud version — bump the version in Cargo.toml, write a Release commit summarizing changes since the last release, tag it, and push. Use when asked to cut/ship a release, bump the version, cut an rc, or promote an rc to a final version.
---

# Cutting a version

Hearth-mud releases are a **version-only commit + annotated tag** pair — no
release script. The version lives in `Cargo.toml` (`version = "X.Y.Z"`, currently
a `0.1.0-rc.N` pre-release) and is mirrored into `Cargo.lock`. Each release is a
commit titled `Release vX.Y.Z` whose body summarizes what changed since the
previous release, plus a matching annotated tag `vX.Y.Z`.

`CHANGELOG.md` is kept too, and it is **not** part of the release commit. Land
the entry in its own commit first (see step 1), then cut a release commit that
touches only `Cargo.toml` + `Cargo.lock`. Note the file starts partway through
the project's history and has gaps (rc.8–rc.16 have no sections), so don't
assume a missing section means a version wasn't released.

## Pick the new version

Ask what kind of bump if it isn't stated. Current scheme is semver with a
release-candidate pre-release:

- **Next rc:** `0.1.0-rc.N` → `0.1.0-rc.(N+1)` — the common case during a cycle.
- **Promote rc → final:** `0.1.0-rc.N` → `0.1.0`.
- **Patch / minor / major:** `0.1.0` → `0.1.1` / `0.2.0` / `1.0.0` after a final.

Match the existing tag style (`git tag --sort=-creatordate | head -3`) — don't
invent a new format.

## The short version

```sh
just release            # next rc: stamp changelog, bump, test, commit, tag, push
just release 0.1.0      # any other version (rc -> final, minor, major)
PUSH=0 just release     # everything except the push
```

`scripts/release.sh` runs the steps below in order and refuses to skip them: a
dirty tree, a missing or empty `## Unreleased` section, or a tag that disagrees
with `Cargo.toml` each abort before anything is committed. The release commit's
body is the changelog section itself, so there's one copy of the prose. Set
`RELEASE_TRAILERS` to add commit trailers (agents: your `Co-Authored-By` +
`Claude-Session` lines).

**Still decide the version yourself**, and make sure the `## Unreleased` section
actually describes this release before running it — the script enforces that an
entry exists, not that it's any good.

## Steps (what the script does; do this by hand for anything unusual)

1. **Confirm the tree is clean and land real work first.**
   ```sh
   grep -m1 '^version' Cargo.toml
   git status --short           # the release commit contains ONLY Cargo.toml + Cargo.lock
   ```
   All feature work goes in its own commits before the release; never fold
   changes into the Release commit — the CHANGELOG entry included.

2. **Update `CHANGELOG.md` — required, before the version bump.** This is a
   project rule (`CLAUDE.md`, `AGENTS.md`), not a nicety: no release ships
   without its entry.

   ```sh
   git log --oneline <previous-tag>..HEAD    # everything this release carries
   grep -n '^## ' CHANGELOG.md               # current sections
   ```

   Stamp the `## Unreleased` section as the version being cut
   (`## X.Y.Z — YYYY-MM-DD`), and add entries for anything in that log that
   reached origin without one. Write what changed and why it matters to someone
   using Hearth, grouped under `### Added` / `### Fixed` / `### Changed` /
   `### Performance` / `### Docs` — not a restated diff.

   **Commit this on its own** (`Changelog: X.Y.Z`) before step 3. It must not
   ride along with the version bump.

3. **Bump `version`** in `Cargo.toml` to the chosen value.

4. **Build** so `Cargo.lock` picks it up and the release compiles; run tests for
   a real release:
   ```sh
   cargo build
   grep -A1 'name = "hearth-mud"' Cargo.lock | grep version   # shows the new version
   cargo test                                                  # should be all green
   ```

5. **Write the summary.** Group the same log by area (Security / Softcode /
   World / Clients / Docs — whatever applies), a few lines each, not a raw
   commit dump. The changelog entry from step 2 is the raw material.

6. **Commit, tag, push** (version files only):
   ```sh
   git add Cargo.toml Cargo.lock
   git commit -F - <<'MSG'
   Release vX.Y.Z

   Since <previous version> — <grouped summary>.

   <the session's configured commit trailers: Co-Authored-By + Claude-Session>
   MSG
   git tag -a vX.Y.Z -m "Release vX.Y.Z"
   git push && git push origin vX.Y.Z
   ```

7. **(Optional) restart a running local backend** so the release goes live — the
   built binary is already current after step 4:
   ```sh
   PID=$(lsof -nP -iTCP:8000 -sTCP:LISTEN -t); [ -n "$PID" ] && kill $PID
   ./target/debug/hearth-mud ../the-last-stag-mud/hearth.toml   # run detached
   ```
   A restart drops active telnet/web sessions (players re-login) — confirm with
   the user first.

## Notes

- Tags are annotated (`git tag -a`), named `vX.Y.Z`, pointing at the Release
  commit (recent releases all have a matching tag).
- The release commit is **version-only**. Keep everything else out of it.
- **No release without a changelog entry.** The same rule governs ordinary
  pushes (`CLAUDE.md` / `AGENTS.md`: an entry under `## Unreleased` before
  anything lands on origin), so by release time the section is usually already
  written and step 2 is just stamping it with the version and date.
- Pushing the tag is what publishes: the `Release` workflow builds the platform
  binaries, cuts the GitHub release, and pushes
  `ghcr.io/arcane-grimoire/hearth-mud:X.Y.Z` — the image games pin in their
  Dockerfile. A hearth release alone doesn't move a game's production; that
  needs the game repo to bump its `FROM` pin and redeploy.
