#!/usr/bin/env bash
# Cut a release: bump the version, commit, push main, tag, push the tag.
#
#   MESA_ALLOW_PUSH=1 scripts/release.sh 0.2.17
#   scripts/release.sh --dry-run 0.2.17     # every check, no mutation
#
# Modelled on mesa's scripts/release.sh so a release is a single allowlisted
# invocation instead of a chain of ad-hoc `git push` calls.
#
# Cargo.toml's [workspace.package] `version` is the only version string that
# matters — every crate uses `version.workspace = true`. The tag push is the
# trigger, and .github/workflows/release.yml does everything after it (tests,
# binaries, agent bundles, GitHub release, Homebrew tap via repository_dispatch).
# There is no local tap checkout and this script must never try to update one.
#
# SAFETY: the push guard is a PreToolUse hook on the *Bash tool*, so it only
# ever sees the outer command string — a `git push` inside this script is
# invisible to it. This script therefore re-implements the gate itself: it
# refuses to run the mutating path unless MESA_ALLOW_PUSH=1 is in its
# environment. Do not remove that check; it is the entire safety story.
set -euo pipefail

cd "$(dirname "$0")/.."

dry_run=0
if [ "$#" -eq 2 ] && [ "$1" = "--dry-run" ]; then
  dry_run=1
  shift
fi
if [ "$#" -ne 1 ]; then
  echo "usage: $(basename "$0") [--dry-run] <version>" >&2
  exit 2
fi

# Accept 0.2.17 or v0.2.17; normalise to the bare semver Cargo.toml wants.
version="${1#v}"
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "usage: $(basename "$0") [--dry-run] <version>" >&2
  echo "  <version> must be bare semver, e.g. 0.2.17" >&2
  exit 2
fi

tag="v$version"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# --- preflight: everything that can refuse must refuse before anything moves ---

# 1. The gate the Bash hook cannot enforce from out here (see SAFETY above).
if [ "$dry_run" -eq 0 ] && [ "${MESA_ALLOW_PUSH:-}" != "1" ]; then
  fail "refusing to push without MESA_ALLOW_PUSH=1 in the environment;" \
    "run: MESA_ALLOW_PUSH=1 $(basename "$0") $version (or --dry-run)"
fi

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = "main" ] || fail "on branch '$branch'; releases are cut from main"

dirty=$(git status --porcelain)
[ -z "$dirty" ] || fail "working tree is dirty; commit or stash first:
$dirty"

current=$(grep -m1 '^version = ' Cargo.toml | sed 's/^version = "\(.*\)"$/\1/')
[ "$version" != "$current" ] || fail "Cargo.toml is already at $current
  if a previous run was interrupted after pushing main, the step left is: git push origin $tag"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  fail "tag $tag already exists locally
  if a previous run was interrupted before pushing it, run: git push origin $tag"
fi
if [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
  fail "tag $tag already exists on origin"
fi

# main must be at or ahead of origin/main — a release cut from a stale main
# would be rejected or carry history the caller has not seen.
git fetch origin
behind=$(git rev-list --count "HEAD..origin/main")
[ "$behind" -eq 0 ] ||
  fail "main is $behind commit(s) behind/diverged from origin/main; pull first"

echo "release: $current -> $version (tag $tag)"
if [ "$dry_run" -eq 1 ]; then
  echo "dry run: printing the mutating steps, changing nothing"
fi

# `run` is the one mutation chokepoint: it echoes every command and, under
# --dry-run, stops there. Nothing below may mutate outside of it.
run() {
  echo "+ $*"
  if [ "$dry_run" -eq 1 ]; then
    return 0
  fi
  "$@"
}

# The commit body carries the repo's Claude-Session trailer when the caller
# knows its session URL; a hand-run release has none and gets a bare subject.
message="bump version to $version for release"
if [ -n "${CLAUDE_SESSION_URL:-}" ]; then
  message="$message

Claude-Session: ${CLAUDE_SESSION_URL}"
fi

# Only the [workspace.package] version — the first `version = ` line in the
# file — is rewritten; dependency pins are left alone.
if [ "$dry_run" -eq 1 ]; then
  echo "+ bump Cargo.toml [workspace.package] version -> $version"
else
  perl -i -pe "BEGIN{\$d=0} if (!\$d && /^version = \"/) { \$_ = qq{version = \"$version\"\n}; \$d=1 }" Cargo.toml
  grep -q "^version = \"$version\"\$" Cargo.toml || fail "Cargo.toml bump did not take"
fi

# cargo check refreshes Cargo.lock's qorvex-* entries to the new version. It is
# not the release build (CI is), but it IS a hard gate: under set -e a failure
# aborts here, leaving Cargo.toml bumped and uncommitted for you to fix or
# revert. Never make this failure non-fatal — a release that does not compile
# must not reach the tag push.
run cargo check --workspace

run git add Cargo.toml Cargo.lock
run git commit -m "$message"
run git push origin main
run git tag -a "$tag" -m "$tag"
run git push origin "$tag"

echo
if [ "$dry_run" -eq 1 ]; then
  echo "ok: dry run complete; nothing was changed or pushed"
  exit 0
fi
echo "ok: $tag pushed"
echo "  CI (.github/workflows/release.yml) now runs the tests, builds the binaries and"
echo "  agent bundles, publishes the GitHub release and dispatches the Homebrew tap"
echo "  update — nothing local to do there."
echo "  Locally, when you want the new binaries: brew upgrade simonspoon/tap/qorvex"
