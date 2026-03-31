#!/usr/bin/env bash
#
# Tag a pysimlin release.
#
# Updates pysimlin.version, commits, and creates a git tag. Does NOT push
# automatically -- prints instructions so the caller can review first.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  echo "Usage: $0 <version>" >&2
  echo "  version must be semver: MAJOR.MINOR.PATCH (e.g. 1.2.3)" >&2
  exit 1
}

if [ $# -ne 1 ]; then
  usage
fi

VERSION="$1"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: invalid version '$VERSION' -- must match MAJOR.MINOR.PATCH" >&2
  usage
fi

cd "$REPO_ROOT"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "error: working tree is dirty -- commit or stash changes first" >&2
  exit 1
fi

VERSION_FILE="src/simlin-mcp/pysimlin.version"

printf '%s\n' "$VERSION" > "$VERSION_FILE"
git add "$VERSION_FILE"
git commit -m "mcp: update pysimlin version reference to $VERSION"
git tag "pysimlin-v$VERSION"

echo "Tagged pysimlin-v$VERSION. Push with: git push origin main pysimlin-v$VERSION"
