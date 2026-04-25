#!/usr/bin/env bash
# Cross-compile simlin-serve for Linux (x64, arm64) and Windows (x64).
#
# Builds a Docker toolchain image (Rust + Zig + cargo-zigbuild + Node + pnpm),
# then runs cargo-zigbuild inside it. Diverges from simlin-mcp's cross-build
# in two places:
#   1. The image bundles Node + pnpm because simlin-serve's build.rs shells to
#      pnpm to produce web/dist/ for rust-embed to ingest.
#   2. The repo source is copied into the container instead of bind-mounted
#      read-only, because pnpm needs to write node_modules/ and the build
#      writes web/dist/. Bind-mounting :rw would pollute the host workspace.
#
# A named Docker volume caches the Cargo target directory so subsequent runs
# are incremental. node_modules is written inside the container's copy of the
# tree and discarded on container exit.
#
# Output: dist/<triple>/simlin-serve[.exe]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVE_DIR="$(dirname "$SCRIPT_DIR")"
REPO_ROOT="$(cd "$SERVE_DIR/../.." && pwd)"

IMAGE_NAME="simlin-serve-cross"
CACHE_VOLUME="simlin-serve-cross-cache"
PNPM_STORE_VOLUME="simlin-serve-cross-pnpm"
DIST_DIR="$SERVE_DIR/dist"

RUST_VERSION=$(grep '^channel' "$REPO_ROOT/rust-toolchain.toml" | sed 's/channel = "\(.*\)"/\1/')

# Optional first positional arg restricts the build to a single platform name
# (matching the npm package suffixes). Empty = build all three Docker targets.
TARGET_FILTER="${1:-}"

case "$TARGET_FILTER" in
  ""|all)
    TARGETS="x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu"
    ;;
  linux-x64)
    TARGETS="x86_64-unknown-linux-musl"
    ;;
  linux-arm64)
    TARGETS="aarch64-unknown-linux-musl"
    ;;
  win32-x64)
    TARGETS="x86_64-pc-windows-gnu"
    ;;
  *)
    echo "error: unknown target filter '$TARGET_FILTER'" >&2
    echo "usage: $0 [all|linux-x64|linux-arm64|win32-x64]" >&2
    exit 1
    ;;
esac

echo "==> Building cross-compilation toolchain image (rust $RUST_VERSION)..."
docker build --build-arg "RUST_VERSION=$RUST_VERSION" -t "$IMAGE_NAME" -f "$SERVE_DIR/Dockerfile.cross" "$SERVE_DIR/"

# Previous Docker runs leave root-owned files in dist/; remove via Docker
# to avoid "Permission denied" when cleaning up as a non-root user.
if [ -d "$DIST_DIR" ]; then
    docker run --rm -v "$DIST_DIR:/dist" alpine sh -c 'rm -rf /dist/*' 2>/dev/null || true
fi
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

echo "==> Cross-compiling targets: $TARGETS"
# Stream the repo into the container via a tar pipe rather than a bind mount
# so the host source tree stays clean of node_modules/web/dist artifacts.
# .git is excluded (large, not needed); target/ is excluded (cargo cache lives
# in the named volume). pnpm's content-addressed store also gets a named
# volume to avoid re-downloading on every run.
docker run --rm -i \
    -v "$CACHE_VOLUME:/tmp/target" \
    -v "$PNPM_STORE_VOLUME:/root/.local/share/pnpm/store" \
    -v "$DIST_DIR:/dist" \
    -e CARGO_TARGET_DIR=/tmp/target \
    -e SIMLIN_SERVE_BUILD_WEB=1 \
    -e TARGETS="$TARGETS" \
    "$IMAGE_NAME" \
    bash -c '
        set -euo pipefail
        mkdir -p /work
        cd /work
        tar -x
        for target in $TARGETS; do
            echo "--- Building $target ---"
            cargo zigbuild -p simlin-serve --locked --release --target "$target"
            mkdir -p "/dist/$target"
            if [[ "$target" == *windows* ]]; then
                cp "/tmp/target/$target/release/simlin-serve.exe" "/dist/$target/"
            else
                cp "/tmp/target/$target/release/simlin-serve" "/dist/$target/"
            fi
        done
        echo "--- All targets built ---"
    ' < <(tar -C "$REPO_ROOT" --exclude=./.git --exclude=./target --exclude=./node_modules --exclude='**/node_modules' -cf - .)

echo ""
echo "Binaries:"
ls -lh "$DIST_DIR"/*/simlin-serve* 2>/dev/null || echo "(no binaries produced)"

# Smoke test: verify the Linux x64 binary is a valid static executable.
LINUX_X64_BIN="$DIST_DIR/x86_64-unknown-linux-musl/simlin-serve"
if [ -f "$LINUX_X64_BIN" ]; then
    echo ""
    echo "==> Smoke test: Linux x64 binary..."
    FILE_OUTPUT=$(file "$LINUX_X64_BIN")
    echo "$FILE_OUTPUT"
    if ! echo "$FILE_OUTPUT" | grep -q "ELF.*executable"; then
        echo "FAIL: binary is not an ELF executable"
        exit 1
    fi

    # The execution test only works on x86_64 Linux since the binary targets
    # x86_64-unknown-linux-musl. Skip on macOS, Windows, and arm64 Linux.
    if [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]]; then
        echo "==> Verifying binary executes..."
        # simlin-serve is an HTTP server that binds an ephemeral port; --help
        # exits cleanly without binding. Anything >= 126 (cannot exec / signal
        # crash) is a real failure.
        set +e
        "$LINUX_X64_BIN" --help >/dev/null 2>&1
        EXIT_CODE=$?
        set -e
        if [ "$EXIT_CODE" -ge 126 ]; then
            echo "FAIL: binary did not execute properly (exit code $EXIT_CODE)"
            exit 1
        fi
        echo "Smoke test passed (--help exited with $EXIT_CODE)"
    else
        echo "==> Skipping execution smoke test (x86_64-linux-musl binary cannot run on $(uname -s)/$(uname -m))"
    fi
fi

echo ""
echo "Done. Binaries in $DIST_DIR/"
