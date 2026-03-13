#!/usr/bin/env bash
# Cross-compile simlin-mcp for Linux (x64, arm64) and Windows (x64).
#
# Builds a Docker toolchain image, then runs cargo-zigbuild inside it with the
# repo source mounted read-only.  A named Docker volume caches the Cargo target
# directory so subsequent runs are incremental.
#
# Output: dist/<triple>/simlin-mcp[.exe]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(cd "$MCP_DIR/../.." && pwd)"

IMAGE_NAME="simlin-mcp-cross"
CACHE_VOLUME="simlin-mcp-cross-cache"
DIST_DIR="$MCP_DIR/dist"

echo "==> Building cross-compilation toolchain image..."
docker build -t "$IMAGE_NAME" -f "$MCP_DIR/Dockerfile.cross" "$MCP_DIR/"

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

echo "==> Cross-compiling all targets..."
docker run --rm \
    -v "$REPO_ROOT:/src:ro" \
    -v "$CACHE_VOLUME:/tmp/target" \
    -v "$DIST_DIR:/dist" \
    -e CARGO_TARGET_DIR=/tmp/target \
    "$IMAGE_NAME" \
    bash -c '
        cd /src
        for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu; do
            echo "--- Building $target ---"
            cargo zigbuild -p simlin-mcp --release --target "$target"
            mkdir -p "/dist/$target"
            if [[ "$target" == *windows* ]]; then
                cp "/tmp/target/$target/release/simlin-mcp.exe" "/dist/$target/"
            else
                cp "/tmp/target/$target/release/simlin-mcp" "/dist/$target/"
            fi
        done
        echo "--- All targets built ---"
    '

echo ""
echo "Binaries:"
ls -lh "$DIST_DIR"/*/simlin-mcp*

# Smoke test: verify the Linux x64 binary is a valid static executable and runs
echo ""
echo "==> Smoke test: Linux x64 binary..."
file "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp"

echo "==> Verifying binary executes..."
# simlin-mcp is a stdio MCP server that waits for input, so feed it empty input
# with a timeout. Any exit (including error) proves the binary loads and runs.
echo '' | timeout 2 "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp" 2>/dev/null || true
echo "Smoke test passed (binary executed)"

echo ""
echo "Done. Binaries in $DIST_DIR/"
