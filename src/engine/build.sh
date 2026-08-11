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
#
# The target directory is RESOLVED, not assumed: `CARGO_TARGET_DIR` and a cargo
# config's `build.target-dir` both move it, and a hardcoded `../../target` turns
# that into a `cp: cannot stat` below -- which reads as a broken wasm build
# rather than as a path mismatch.
TARGET_DIR="$("$DIR/../../scripts/cargo-target-dir.sh")"
WASM_SRC="$TARGET_DIR/wasm32-unknown-unknown/release/simlin.wasm"

build_wasm() {
  local out_name="$1"
  shift
  echo "Building $out_name for wasm32-unknown-unknown..."
  # cargo build is idempotent and no-ops when nothing has changed.
  cargo build -p simlin --lib --release --target wasm32-unknown-unknown "$@"

  # Whether this invocation will optimize. Decided BEFORE the cache check
  # because it is part of the cache key -- see below.
  local want_mode="opt"
  if ! command -v wasm-opt &> /dev/null || [ "1" = "${DISABLE_WASM_OPT-0}" ]; then
    want_mode="raw"
  fi
  local have_mode=""
  [ -f "core/$out_name.mode" ] && have_mode="$(cat "core/$out_name.mode")"

  # Copy WASM only if the staged artifact is stale (avoids re-running wasm-opt
  # and invalidating downstream TypeScript builds when Rust source is
  # unchanged). Staleness has TWO inputs, and both are in the key:
  #
  #   1. the raw cargo output changed -- compared against a stashed copy of the
  #      pre-optimization WASM, because wasm-opt transforms core/$out_name
  #      in-place and it therefore differs from the cargo output even when
  #      nothing changed; and
  #   2. the staged artifact was produced in the OTHER mode.
  #
  # Without (2) the key described the input but not the artifact, and a
  # DISABLE_WASM_OPT=1 build (`scripts/pre-commit`) staged an unoptimized blob
  # whose .raw then satisfied the next optimizing build's check -- so a
  # subsequent `pnpm build` on an unchanged tree kept the unoptimized blob and
  # never ran wasm-opt again. Both deploy scripts happen to `pnpm clean` first,
  # which deletes core/ and hid this; nothing about the cache made it safe.
  if [ ! -f "core/$out_name" ] \
      || [ "$have_mode" != "$want_mode" ] \
      || ! cmp -s "$WASM_SRC" "core/$out_name.raw"; then
    # Invalidate the stamp BEFORE restaging, not merely write it after.
    #
    # Writing it last protects a FIRST build: an abort leaves no stamp, so the
    # next run redoes the work. It does NOT protect an update, because a valid
    # stamp from the previous build is still on disk. If wasm-opt then fails or
    # is interrupted after the copies below, `.raw` already matches the new
    # cargo output while the staged blob is raw -- and the surviving `opt`
    # stamp makes the next run early-out, treat the raw blob as optimized, and
    # exit 0. That is the wrong answer the stamp exists to prevent, arriving
    # through the update path, and it also defeats verify-deploy-build.sh's
    # REQUIRE_WASM_OPT check, which reads this stamp.
    #
    # Removing it here makes the whole window self-invalidating: no stamp means
    # indeterminate, and indeterminate means rebuild.
    rm -f "core/$out_name.mode"

    # Blob before `.raw`, deliberately. If the second copy fails, `.raw` still
    # holds the PREVIOUS output, so the `cmp` above fails next run and the work
    # is redone. Reversed, a failure between the two would leave a `.raw`
    # describing the new source beside a blob built from the old one -- which
    # `cmp` cannot detect, since it only ever compares `.raw` to cargo.
    cp "$WASM_SRC" "core/$out_name"
    cp "$WASM_SRC" "core/$out_name.raw"

    if [ "$want_mode" = "opt" ]; then
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

    # Written LAST, and only now that the artifact matches it. Paired with the
    # `rm -f` above this makes the stamp transactional: it exists only while it
    # is true of what is on disk.
    printf '%s\n' "$want_mode" > "core/$out_name.mode"
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
