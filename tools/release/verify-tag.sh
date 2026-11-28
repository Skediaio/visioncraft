#!/usr/bin/env bash
# Checks that a tag can be released, and prints what the release pipeline needs from it.
#
# Usage: tools/release/verify-tag.sh [TAG]        inside a checkout; DRY_RUN=true|false
#
# With a TAG:
# - it exists and is annotated: its message is the release title (a lightweight tag has none);
# - it is named v<workspace version>: the version of [workspace.package] in Cargo.toml, read
#   from the tagged commit, which every package of the workspace must share;
# - a release tag (no pre-release label) is reachable from origin/main. A pre-release such as
#   v0.1.0-rc.1 may be on any branch.
# Without a TAG the checked-out commit is rehearsed: it must carry a valid version, and nothing
# can be published (DRY_RUN must be true, the default).
#
# Prints NAME=value lines: tag, version, commit, title, prerelease.
set -euo pipefail

fail() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error::$*" >&2
  else
    echo "error: $*" >&2
  fi
  exit 1
}

tag="${1:-}"
dry_run="${DRY_RUN:-true}"

if [ -n "$tag" ]; then
  commit="$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}")" || fail "tag $tag does not exist"
  if [ "$(git cat-file -t "refs/tags/$tag")" != tag ]; then
    fail "$tag is a lightweight tag: create it with git tag -a, its message is the release title"
  fi
  title="$(git for-each-ref --format='%(contents:subject)' "refs/tags/$tag")"
  [ -n "$title" ] || fail "$tag has an empty message: the release needs a title"
  # Read the manifests of the tagged commit, whatever HEAD is.
  tree="$(mktemp -d)"
  trap 'git worktree remove --force "$tree/checkout" 2>/dev/null; rmdir "$tree" 2>/dev/null' EXIT
  git worktree add --quiet --detach "$tree/checkout" "$commit"
  checkout="$tree/checkout"
else
  [ "$dry_run" = true ] || fail "publishing needs a tag: give one, or keep dry_run on"
  commit="$(git rev-parse HEAD)"
  title=""
  checkout=.
fi

versions="$(
  cd "$checkout"
  cargo metadata --no-deps --format-version 1 | jq -r '.packages[].version' | sort -u
)"
[ "$(echo "$versions" | wc -l)" = 1 ] \
  || fail "the workspace packages do not share one version: $(echo "$versions" | tr '\n' ' ')"
version="$versions"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
  || fail "'$version' in Cargo.toml is not MAJOR.MINOR.PATCH, optionally with a -pre-release label"
[ -z "$tag" ] || [ "$tag" = "v$version" ] || fail "tag $tag does not match the workspace version $version in Cargo.toml"

case "$version" in
  *-*) prerelease=true ;;
  *) prerelease=false ;;
esac

if [ "$prerelease" = false ]; then
  git rev-parse --verify --quiet origin/main > /dev/null || fail "origin/main not found: git fetch origin main"
  if git merge-base --is-ancestor "$commit" origin/main; then
    :
  elif [ -n "$tag" ]; then
    fail "$tag is not on origin/main: a release tag must be reachable from it (a pre-release may be on any branch)"
  else
    echo "warning: $(git rev-parse --short HEAD) is not on origin/main. Fine for a rehearsal; a release tag goes on main." >&2
  fi
fi

printf 'tag=%s\nversion=%s\ncommit=%s\ntitle=%s\nprerelease=%s\n' "$tag" "$version" "$commit" "$title" "$prerelease"
