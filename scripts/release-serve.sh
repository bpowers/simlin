#!/usr/bin/env bash
#
# Tag a simlin-serve release.
#
# Bumps Cargo.toml version, updates the wrapper and platform npm packages,
# runs tests, commits, and creates a git tag. Does NOT push automatically --
# prints instructions so the caller can review first.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVE_DIR="$REPO_ROOT/src/simlin-serve"

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

echo "Running cargo test -p simlin-serve ..."
cargo test -p simlin-serve

# Update the first version = "..." line in [package] section of Cargo.toml
awk -v ver="$VERSION" '!done && /^version = "/ { $0 = "version = \"" ver "\""; done=1 } 1' \
  "$SERVE_DIR/Cargo.toml" > "$SERVE_DIR/Cargo.toml.tmp"
mv "$SERVE_DIR/Cargo.toml.tmp" "$SERVE_DIR/Cargo.toml"

# Refresh Cargo.lock to reflect the new simlin-serve version
cargo check -p simlin-serve --quiet

# Update wrapper package.json: top-level version + all optionalDependencies
jq --arg v "$VERSION" '
  .version = $v |
  .optionalDependencies = (.optionalDependencies | to_entries | map(.value = $v) | from_entries)
' "$SERVE_DIR/package.json" > "$SERVE_DIR/package.json.tmp"
mv "$SERVE_DIR/package.json.tmp" "$SERVE_DIR/package.json"

# Regenerate platform package.json files (reads version from Cargo.toml)
bash "$SERVE_DIR/build-npm-packages.sh"

# Verify all 5 npm package.json files agree on version
PACKAGE_FILES=(
  "$SERVE_DIR/package.json"
  "$SERVE_DIR/npm/@simlin/serve-darwin-arm64/package.json"
  "$SERVE_DIR/npm/@simlin/serve-linux-arm64/package.json"
  "$SERVE_DIR/npm/@simlin/serve-linux-x64/package.json"
  "$SERVE_DIR/npm/@simlin/serve-win32-x64/package.json"
)

for f in "${PACKAGE_FILES[@]}"; do
  file_version="$(jq -r '.version' "$f")"
  if [ "$file_version" != "$VERSION" ]; then
    echo "error: version mismatch in $f: expected $VERSION, got $file_version" >&2
    exit 1
  fi
done

echo "All 5 package.json files agree on version $VERSION"

git add "$SERVE_DIR/Cargo.toml" "$REPO_ROOT/Cargo.lock" "$SERVE_DIR/package.json"
git commit -m "serve: release $VERSION"
git tag "serve-v$VERSION"

echo "Tagged serve-v$VERSION. Push with: git push origin main serve-v$VERSION"
