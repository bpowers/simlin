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

# Previous Docker runs leave root-owned files in dist/; remove via Docker
# to avoid "Permission denied" when cleaning up as a non-root user.
if [ -d "$DIST_DIR" ]; then
    docker run --rm -v "$DIST_DIR:/dist" alpine rm -rf /dist/* 2>/dev/null || true
fi
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

# Smoke test: verify the Linux x64 binary is a valid static executable.
echo ""
echo "==> Smoke test: Linux x64 binary..."
FILE_OUTPUT=$(file "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp")
echo "$FILE_OUTPUT"
if ! echo "$FILE_OUTPUT" | grep -q "ELF.*executable"; then
    echo "FAIL: binary is not an ELF executable"
    exit 1
fi

# The execution test only works on x86_64 Linux since the binary targets
# x86_64-unknown-linux-musl.  Skip on macOS, Windows, and arm64 Linux.
if [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]]; then
    echo "==> Verifying binary executes..."
    # simlin-mcp is a stdio MCP server that waits for input, so feed it empty
    # input with a timeout.  timeout exits 124 when it kills a running process
    # (expected), and the binary itself may exit non-zero on empty input.
    # Fatal failures are: 126 (cannot execute), 127 (not found), >= 128 (signal).
    set +e
    echo '' | timeout 2 "$DIST_DIR/x86_64-unknown-linux-musl/simlin-mcp" 2>/dev/null
    EXIT_CODE=$?
    set -e
    if [ "$EXIT_CODE" -ge 126 ] && [ "$EXIT_CODE" -ne 124 ]; then
        echo "FAIL: binary did not execute properly (exit code $EXIT_CODE)"
        exit 1
    fi
    echo "Smoke test passed (binary executed, exit code $EXIT_CODE)"
else
    echo "==> Skipping execution smoke test (x86_64-linux-musl binary cannot run on $(uname -s)/$(uname -m))"
fi

echo ""
echo "Done. Binaries in $DIST_DIR/"
