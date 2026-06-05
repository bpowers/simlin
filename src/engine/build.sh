#!/usr/bin/env bash
set -euo pipefail

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"
cd "$DIR"

mkdir -p core

# Build libsimlin as WASM and stage it into core/ under $out_name.
#
# Two artifacts are produced (see optimize_wasm calls below):
#   - libsimlin.wasm          full build; loaded by Node (wasm.node.ts). The
#                             server's PNG model previews need png_render.
#   - libsimlin-browser.wasm  --no-default-features build; imported by
#                             wasm.browser.ts and bundled into the SPA. The
#                             png_render stack (resvg + text shaping + an
#                             embedded font) is ~28% of the optimized binary
#                             and nothing in the browser calls it.
#
# Both feature sets share the cargo target dir (artifacts coexist keyed by
# feature hash), but cargo uplifts each build to the same simlin.wasm path,
# so we stage into core/ immediately after each build.
#
# The xmutil feature is always off here (C++ dependency, not wasm-buildable).
WASM_SRC="../../target/wasm32-unknown-unknown/release/simlin.wasm"

build_wasm() {
  local out_name="$1"
  shift
  echo "Building $out_name for wasm32-unknown-unknown..."
  # cargo build is idempotent and no-ops when nothing has changed.
  cargo build -p simlin --lib --release --target wasm32-unknown-unknown "$@"

  # Copy WASM only if the raw cargo output changed (avoids re-running
  # wasm-opt and invalidating downstream TypeScript builds when Rust source
  # is unchanged). We compare against a stashed copy of the pre-optimization
  # WASM because wasm-opt transforms core/$out_name in-place, making it
  # differ from the raw cargo output even when nothing changed.
  if [ ! -f "core/$out_name" ] || ! cmp -s "$WASM_SRC" "core/$out_name.raw"; then
    cp "$WASM_SRC" "core/$out_name"
    cp "$WASM_SRC" "core/$out_name.raw"

    # Optimize WASM if wasm-opt is available
    if command -v wasm-opt &> /dev/null && [ "1" != "${DISABLE_WASM_OPT-0}" ]; then
      echo "Running wasm-opt on $out_name..."
      wasm-opt "core/$out_name" -o "core/$out_name-opt" -O3 \
        --enable-mutable-globals \
        --enable-bulk-memory \
        --enable-bulk-memory-opt \
        --enable-nontrapping-float-to-int
      mv "core/$out_name-opt" "core/$out_name"
    else
      echo "Skipping wasm-opt (not installed or disabled)"
    fi
  fi
}

build_wasm libsimlin.wasm
build_wasm libsimlin-browser.wasm --no-default-features

# Clean stale outputs (deleted/renamed sources leave orphan .js/.d.ts files).
# tsbuildinfo must also be removed so tsc knows to recompile into the empty dirs.
rm -rf lib lib.browser tsconfig.tsbuildinfo tsconfig.browser.tsbuildinfo

echo "Compiling TypeScript..."
pnpm run tsc
pnpm run tsc -p tsconfig.browser.json

echo "Build complete!"
