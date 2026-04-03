#!/usr/bin/env bash
#
# Tag a TypeScript packages release (@simlin/engine, @simlin/core, @simlin/diagram).
#
# Bumps all three package.json versions, runs tests, commits, and creates
# a git tag. Does NOT push automatically -- prints instructions so the
# caller can review first.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ENGINE_DIR="$REPO_ROOT/src/engine"
CORE_DIR="$REPO_ROOT/src/core"
DIAGRAM_DIR="$REPO_ROOT/src/diagram"

usage() {
  echo "Usage: $0 <version>" >&2
  echo "  version must be semver: MAJOR.MINOR.PATCH (e.g. 3.0.0)" >&2
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

CURRENT_BRANCH="$(git branch --show-current)"
if [ "$CURRENT_BRANCH" != "main" ]; then
  echo "error: releases must be tagged from the main branch (currently on '$CURRENT_BRANCH')" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet || [ -n "$(git status --porcelain)" ]; then
  echo "error: working tree is dirty or has untracked files -- commit or stash changes first" >&2
  exit 1
fi

echo "Updating all three packages to version $VERSION..."

PACKAGE_FILES=(
  "$ENGINE_DIR/package.json"
  "$CORE_DIR/package.json"
  "$DIAGRAM_DIR/package.json"
)

for f in "${PACKAGE_FILES[@]}"; do
  jq --arg v "$VERSION" '.version = $v' "$f" > "$f.tmp"
  mv "$f.tmp" "$f"
done

# Verify all three package.json files agree on version
for f in "${PACKAGE_FILES[@]}"; do
  file_version="$(jq -r '.version' "$f")"
  if [ "$file_version" != "$VERSION" ]; then
    echo "error: version mismatch in $f: expected $VERSION, got $file_version" >&2
    exit 1
  fi
done

echo "All 3 package.json files agree on version $VERSION"

# Update lockfile to reflect new workspace versions
pnpm install

echo "Building packages..."
pnpm --filter @simlin/engine --filter @simlin/core --filter @simlin/diagram build

echo "Running tests..."
pnpm --filter @simlin/engine --filter @simlin/core --filter @simlin/diagram test

git add "${PACKAGE_FILES[@]}" pnpm-lock.yaml
git commit -m "ts: release $VERSION"
git tag "ts-v$VERSION"

echo "Tagged ts-v$VERSION. Push with: git push origin main ts-v$VERSION"
