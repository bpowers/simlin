#!/usr/bin/env bash
set -euo pipefail

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

# Build libsimlin as WASM (without xmutil feature due to C++ xmutil dependency)
# cargo build is idempotent and no-ops when nothing has changed.
echo "Building libsimlin for wasm32-unknown-unknown..."
cargo build -p simlin --lib --release --target wasm32-unknown-unknown --no-default-features

# Copy WASM only if it changed (avoids re-running wasm-opt and invalidating
# downstream TypeScript builds when Rust source is unchanged).
mkdir -p core
WASM_SRC="../../target/wasm32-unknown-unknown/release/simlin.wasm"
if [ ! -f core/libsimlin.wasm ] || ! cmp -s "$WASM_SRC" core/libsimlin.wasm; then
  cp "$WASM_SRC" core/libsimlin.wasm

  # Optimize WASM if wasm-opt is available
  if command -v wasm-opt &> /dev/null && [ "1" != "${DISABLE_WASM_OPT-0}" ]; then
    echo "Running wasm-opt..."
    wasm-opt core/libsimlin.wasm -o core/libsimlin.wasm-opt -O3 \
      --enable-mutable-globals \
      --enable-bulk-memory \
      --enable-bulk-memory-opt \
      --enable-nontrapping-float-to-int
    mv core/libsimlin.wasm-opt core/libsimlin.wasm
  else
    echo "Skipping wasm-opt (not installed or disabled)"
  fi
fi

# Build TypeScript (tsc is incremental via .tsbuildinfo files)
echo "Compiling TypeScript..."
pnpm run tsc
pnpm run tsc -p tsconfig.browser.json

# Copy WASM to output directories
mkdir -p lib lib.browser
cp -r core lib/
cp -r core lib.browser/

echo "Build complete!"
