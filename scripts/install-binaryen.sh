#!/usr/bin/env bash
# Install a pinned binaryen release and put `wasm-opt` on the PATH.
#
# `apt-get install binaryen` is NOT sufficient: the version Ubuntu ships is
# older than the flags `src/engine/build.sh` passes, and the failure is a bare
# `Unknown option '--enable-bulk-memory-opt'` from a `wasm-opt` that ran at all.
# Both workflows that optimize the bundle -- the optimized-WASM check and the
# npm publish -- therefore install from the upstream release rather than from
# the distro, so CI runs the same binaryen a developer does instead of whatever
# the runner image happens to carry.
#
# Bump VERSION when build.sh starts using a newer flag. Keep it at or below the
# version developers have locally, since this is the one that gates a release.
set -euo pipefail

VERSION="${BINARYEN_VERSION:-125}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) ASSET="x86_64-linux" ;;
  Linux-aarch64) ASSET="aarch64-linux" ;;
  Darwin-arm64) ASSET="arm64-macos" ;;
  Darwin-x86_64) ASSET="x86_64-macos" ;;
  *)
    echo "install-binaryen.sh: no pinned asset for $(uname -s)-$(uname -m)." >&2
    echo "Install binaryen >= $VERSION yourself and put wasm-opt on PATH." >&2
    exit 1
    ;;
esac

PREFIX="${BINARYEN_PREFIX:-$HOME/.local/binaryen}"
URL="https://github.com/WebAssembly/binaryen/releases/download/version_${VERSION}/binaryen-version_${VERSION}-${ASSET}.tar.gz"

echo "Installing binaryen $VERSION ($ASSET) into $PREFIX"
mkdir -p "$PREFIX"
curl --fail --location --silent --show-error "$URL" \
  | tar -xz -C "$PREFIX" --strip-components=1

BIN="$PREFIX/bin"
if [ ! -x "$BIN/wasm-opt" ]; then
  echo "install-binaryen.sh: $BIN/wasm-opt missing after extraction" >&2
  exit 1
fi

# Fail here rather than mid-build if the pinned release cannot run our flags:
# a wasm-opt that rejects an option exits non-zero *after* build.sh has already
# staged the unoptimized blob.
"$BIN/wasm-opt" --version
"$BIN/wasm-opt" --help 2>&1 | grep -q -- '--enable-bulk-memory-opt' || {
  echo "install-binaryen.sh: binaryen $VERSION does not support" >&2
  echo "  --enable-bulk-memory-opt, which src/engine/build.sh passes." >&2
  exit 1
}

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$BIN" >>"$GITHUB_PATH"
else
  echo "Add to PATH: $BIN"
fi
