#!/usr/bin/env bash
# Generate platform-specific npm package.json files for @simlin/mcp distribution.
#
# Output goes to npm/@simlin/mcp-<platform>/ relative to this script. CI copies
# the native binary into bin/ inside each platform package before publishing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Read the version from Cargo.toml so npm and cargo versions stay in sync.
VERSION=$(grep '^version = ' "$SCRIPT_DIR/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')

if [ -z "$VERSION" ]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

echo "Generating platform packages for version $VERSION"

# Each entry: "<platform-name> <os> <cpu>"
declare -a PLATFORMS=(
  "darwin-arm64 darwin arm64"
  "linux-arm64 linux arm64"
  "linux-x64 linux x64"
  "win32-x64 win32 x64"
)

for entry in "${PLATFORMS[@]}"; do
  read -r platform os cpu <<< "$entry"
  pkg_name="@simlin/mcp-${platform}"
  pkg_dir="$SCRIPT_DIR/npm/$pkg_name"

  mkdir -p "$pkg_dir/bin"

  cat > "$pkg_dir/package.json" <<JSON
{
  "name": "$pkg_name",
  "version": "$VERSION",
  "description": "Platform binary for @simlin/mcp ($platform)",
  "os": ["$os"],
  "cpu": ["$cpu"],
  "files": ["bin"],
  "license": "Apache-2.0"
}
JSON

  echo "  wrote $pkg_dir/package.json"
done

echo "Done."
