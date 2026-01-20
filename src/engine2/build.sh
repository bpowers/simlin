#!/usr/bin/env bash
set -euo pipefail

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

# Clean previous builds (including tsbuildinfo for incremental compilation)
rm -rf lib lib.browser core *.tsbuildinfo

# Build libsimlin as WASM with vensim support
echo "Building libsimlin for wasm32-unknown-unknown..."
cargo build -p simlin --lib --release --target wasm32-unknown-unknown

# Create core directory and copy WASM
mkdir -p core
cp ../../target/wasm32-unknown-unknown/release/simlin.wasm core/libsimlin.wasm

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

# Build TypeScript
echo "Compiling TypeScript..."
yarn run tsc
yarn run tsc -p tsconfig.browser.json

# Copy WASM to output directories
cp -r core lib/
cp -r core lib.browser/

echo "Build complete!"
