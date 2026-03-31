#!/usr/bin/env bash
#
# Tag a simlin-mcp release.
#
# Bumps Cargo.toml version, updates the wrapper and platform npm packages,
# runs tests, commits, and creates a git tag. Does NOT push automatically --
# prints instructions so the caller can review first.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MCP_DIR="$REPO_ROOT/src/simlin-mcp"

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

echo "Running cargo test -p simlin-mcp ..."
cargo test -p simlin-mcp

# Update the first version = "..." line in [package] section of Cargo.toml
sed -i '0,/^version = ".*"/{s/^version = ".*"/version = "'"$VERSION"'"/}' "$MCP_DIR/Cargo.toml"

# Update wrapper package.json: top-level version + all optionalDependencies
jq --arg v "$VERSION" '
  .version = $v |
  .optionalDependencies = (.optionalDependencies | to_entries | map(.value = $v) | from_entries)
' "$MCP_DIR/package.json" > "$MCP_DIR/package.json.tmp"
mv "$MCP_DIR/package.json.tmp" "$MCP_DIR/package.json"

# Regenerate platform package.json files (reads version from Cargo.toml)
bash "$MCP_DIR/build-npm-packages.sh"

# Verify all 5 npm package.json files agree on version
PACKAGE_FILES=(
  "$MCP_DIR/package.json"
  "$MCP_DIR/npm/@simlin/mcp-darwin-arm64/package.json"
  "$MCP_DIR/npm/@simlin/mcp-linux-arm64/package.json"
  "$MCP_DIR/npm/@simlin/mcp-linux-x64/package.json"
  "$MCP_DIR/npm/@simlin/mcp-win32-x64/package.json"
)

for f in "${PACKAGE_FILES[@]}"; do
  file_version="$(jq -r '.version' "$f")"
  if [ "$file_version" != "$VERSION" ]; then
    echo "error: version mismatch in $f: expected $VERSION, got $file_version" >&2
    exit 1
  fi
done

echo "All 5 package.json files agree on version $VERSION"

git add "$MCP_DIR/Cargo.toml" "$MCP_DIR/package.json"
for f in "${PACKAGE_FILES[@]:1}"; do
  git add "$f"
done
git commit -m "mcp: release $VERSION"
git tag "mcp-v$VERSION"

echo "Tagged mcp-v$VERSION. Push with: git push origin main mcp-v$VERSION"
