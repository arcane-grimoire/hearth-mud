#!/usr/bin/env bash
# Cut a release: stamp the changelog, bump the version, test, commit, tag, push.
#
#   just release              # next rc (0.1.0-rc.19 -> 0.1.0-rc.20)
#   just release 0.1.0        # an explicit version (rc -> final, minor, major)
#   PUSH=0 just release       # do everything locally, push nothing
#
# The two commits stay separate on purpose (CLAUDE.md / AGENTS.md):
#   1. "Changelog: X.Y.Z"  — CHANGELOG.md alone
#   2. "Release vX.Y.Z"    — Cargo.toml + Cargo.lock alone, then the tag
#
# The release commit's body is the changelog section itself, so the summary and
# the changelog can't drift — there is only one copy of the prose.
#
# Set RELEASE_TRAILERS to append commit trailers (e.g. Co-Authored-By lines).
set -euo pipefail

cd "$(dirname "$0")/.."
PUSH="${PUSH:-1}"
step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
die() { echo "error: $1" >&2; exit 1; }

# -- Preconditions ------------------------------------------------------------
# A dirty tree would sweep unrelated edits into the release commits, and the
# whole point of this script is that each commit contains exactly what it says.
[ -z "$(git status --porcelain)" ] || die "working tree is dirty — commit or stash first"
branch="$(git rev-parse --abbrev-ref HEAD)"
release_branch="${RELEASE_BRANCH:-master}"
[ "$branch" = "$release_branch" ] || die "on branch '$branch', not $release_branch (set RELEASE_BRANCH to override)"

current="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"

# -- Pick the version ---------------------------------------------------------
# With no argument, walk the rc counter forward. Any other shape (a final
# version, a minor bump) is a judgement call, so it must be passed explicitly.
version="${1:-}"
if [ -z "$version" ]; then
	case "$current" in
	*-rc.*)
		n="${current##*-rc.}"
		version="${current%-rc.*}-rc.$((n + 1))"
		;;
	*) die "current version '$current' is not an rc — pass the version explicitly (e.g. just release 0.2.0)" ;;
	esac
fi
tag="v$version"
[ "$version" != "$current" ] || die "version $version is already what Cargo.toml says"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "tag $tag already exists"

echo "Cutting $current -> $version"

# -- 1. Stamp the changelog ---------------------------------------------------
# The release's prose has to already exist: an empty (or missing) Unreleased
# section means someone pushed without an entry, which is the thing the rule
# exists to prevent. Refuse rather than cut a release with a hollow section.
grep -q '^## Unreleased' CHANGELOG.md || die "CHANGELOG.md has no '## Unreleased' section to stamp"
if ! awk '/^## Unreleased/ { inside = 1; next } inside && /^## / { exit } inside && NF { found = 1; exit } END { exit !found }' CHANGELOG.md; then
	die "the '## Unreleased' section is empty — write the entry for this release first"
fi

step "Stamping CHANGELOG.md as $version"
today="$(date +%Y-%m-%d)"
awk -v heading="## $version — $today" '
	/^## Unreleased/ && !done { print heading; done = 1; next }
	{ print }
' CHANGELOG.md >CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md

# The stamped section, reused verbatim as the release commit body.
section="$(awk -v want="## $version " '
	index($0, want) == 1 { inside = 1; next }
	inside && /^## / { exit }
	inside { print }
' CHANGELOG.md | sed -e '/./,$!d' | awk 'BEGIN { blank = 0 } { lines[NR] = $0 } END { last = NR; while (last > 0 && lines[last] ~ /^[[:space:]]*$/) last--; for (i = 1; i <= last; i++) print lines[i] }')"

commit() { # commit <message-body-on-stdin>
	if [ -n "${RELEASE_TRAILERS:-}" ]; then
		printf '%s\n\n%s\n' "$(cat)" "$RELEASE_TRAILERS" | git commit -q -F -
	else
		git commit -q -F -
	fi
}

git add CHANGELOG.md
printf 'Changelog: %s\n' "$version" | commit

# -- 2. Bump, build, test -----------------------------------------------------
step "Bumping Cargo.toml to $version"
awk -v v="$version" 'NR <= 10 && /^version = / && !done { print "version = \"" v "\""; done = 1; next } { print }' Cargo.toml >Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

step "Building (refreshes Cargo.lock)"
cargo build

step "Running the test suite"
cargo test

# -- 3. Release commit + gate + tag -------------------------------------------
step "Release commit"
git add Cargo.toml Cargo.lock
printf 'Release %s\n\nSince v%s.\n\n%s\n' "$tag" "$current" "$section" | commit

# The same gate CI runs, but here it runs BEFORE the tag exists — so a failure
# leaves two ordinary commits to amend rather than a bad tag to chase down.
step "Release gate"
./scripts/check-release.sh "$tag"

step "Tagging $tag"
git tag -a "$tag" -m "Release $tag"

# -- 4. Push ------------------------------------------------------------------
if [ "$PUSH" = "1" ]; then
	step "Pushing master + $tag"
	git push origin master --follow-tags
	echo
	echo "Pushed. The Release workflow now builds the binaries, the GitHub release,"
	echo "and ghcr.io/arcane-grimoire/hearth-mud:$version. Watch it with:"
	echo "  gh run list --workflow=release.yml --limit 1"
	echo
	echo "A game only picks this up when its Dockerfile FROM pin moves to $version."
else
	echo
	echo "PUSH=0 — nothing pushed. To finish:  git push origin master --follow-tags"
	echo "To undo:  git tag -d $tag && git reset --hard HEAD~2"
fi
