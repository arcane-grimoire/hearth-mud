#!/usr/bin/env bash
# Release gate: the tag, Cargo.toml, and CHANGELOG.md must agree.
#
# Run by the Release workflow before anything publishes, so a release that
# skipped its changelog entry cannot ship binaries, a GitHub release, or the
# ghcr image games pin. Runnable by hand before tagging:
#
#   scripts/check-release.sh v0.1.0-rc.20
#
# Exits non-zero with an explanation on any disagreement.
set -euo pipefail

tag="${1:-${GITHUB_REF_NAME:-}}"
if [ -z "$tag" ]; then
	echo "usage: $0 vX.Y.Z   (or set GITHUB_REF_NAME)" >&2
	exit 2
fi
version="${tag#v}"
fail=0

# 1. The tag names the version the crate actually builds as. A tag that
#    disagrees with Cargo.toml publishes an image whose name lies about what's
#    inside it.
cargo_version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
if [ "$cargo_version" != "$version" ]; then
	echo "::error::Tag $tag does not match Cargo.toml version $cargo_version" >&2
	fail=1
fi

# 2. The changelog has a stamped section for this version. Prefix-matched with
#    awk rather than a regex so dots and dashes in "0.1.0-rc.19" need no
#    escaping, and an "## Unreleased" section left unstamped fails.
if ! awk -v want="## $version " 'index($0, want) == 1 { found = 1 } END { exit !found }' CHANGELOG.md; then
	echo "::error::CHANGELOG.md has no '## $version' section — stamp '## Unreleased' as '## $version — YYYY-MM-DD' before tagging" >&2
	fail=1
fi

# 3. The release commit is version-only (CLAUDE.md / AGENTS.md). A warning, not
#    a failure: this one is a convention about commit shape, and blocking a
#    release over it would be worse than the untidiness. Needs full history, so
#    it is skipped on a shallow checkout rather than reported wrongly.
if git rev-parse --verify HEAD >/dev/null 2>&1 && [ "$(git rev-parse --is-shallow-repository)" = "false" ]; then
	touched="$(git show --name-only --pretty=format: HEAD | grep -v '^$' | sort | tr '\n' ' ')"
	if [ "$touched" != "Cargo.lock Cargo.toml " ]; then
		echo "::warning::Release commit touches more than the version files: $touched"
	fi
fi

if [ "$fail" -ne 0 ]; then
	echo "Release checks failed for $tag." >&2
	exit 1
fi
echo "Release checks passed for $tag (Cargo.toml $cargo_version, CHANGELOG section present)."
