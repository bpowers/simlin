#!/usr/bin/env bash
#
# Verify the artifacts produced by `pnpm --filter @simlin/app run deploy:assemble`
# (which populates `public/` from the rsbuild output). Run BEFORE
# `deploy:clean` so the populated `public/` is still in place.
#
# Used by CI to gate the deploy assembly path. The actual deploy is
# local-only with no CI gate; without this we have no proof the deploy
# build still produces the structure app.yaml expects.
#
# This does NOT run `gcloud app deploy`; it only checks local artifacts.
#
# Exit non-zero on any failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

errors=0

fail() {
    echo "FAIL: $*" >&2
    errors=$((errors + 1))
}

pass() {
    echo "PASS: $*"
}

# 1. The built SPA index.html exists and the template expression was
#    substituted. A literal '<%= PUBLIC_URL %>' would mean the build
#    step was skipped or rsbuild's html plugin didn't run; the doc's
#    smoke test calls this out as the canary for a missed build.
if [ ! -f public/index.html ]; then
    fail "public/index.html missing -- did 'pnpm --filter @simlin/app run deploy:assemble' run?"
elif grep -q '<%= PUBLIC_URL %>' public/index.html; then
    fail "public/index.html still contains '<%= PUBLIC_URL %>' (build template not substituted)"
else
    pass "public/index.html exists and template is substituted"
fi

# 2. The hashed app bundle is referenced from index.html. The single
#    bundle (chunkSplit: 'all-in-one') makes this the canonical entry
#    point for the SPA -- if it isn't named correctly the page loads
#    but JavaScript never runs.
if [ -f public/index.html ]; then
    if ! grep -qE '<script[^>]+src="[^"]*/static/js/index\.[a-f0-9]+\.js"' public/index.html; then
        fail "public/index.html does not reference a hashed /static/js/index.<hash>.js bundle"
    else
        pass "public/index.html references the hashed app bundle"
    fi
fi

# 3. The web component bundle is at the single-level path
#    /static/js/sd-component.js (NOT /static/js/static/js/...).
#    External sites embed this exact URL; a doubled path silently
#    breaks every embed. Caught a regression in commit 831392fc.
if [ ! -f public/static/js/sd-component.js ]; then
    fail "public/static/js/sd-component.js missing (web component build did not produce it at the expected path)"
else
    if [ -f public/static/js/static/js/sd-component.js ]; then
        fail "public/static/js/static/js/sd-component.js exists -- the doubled-path regression is back"
    fi
    # Sanity check: the bundle should be at least a few KB. A near-empty
    # file would suggest the rsbuild config emitted a stub or the copy
    # in the deploy script ran before the component build finished.
    size=$(wc -c < public/static/js/sd-component.js)
    if [ "$size" -lt 1024 ]; then
        fail "public/static/js/sd-component.js is suspiciously small ($size bytes)"
    else
        pass "public/static/js/sd-component.js exists at the single-level path ($size bytes)"
    fi
fi

# 4. A hashed WASM blob exists somewhere under public/static/wasm/.
#    rsbuild emits the engine WASM via Rspack's asset module with a
#    content hash. The exact filename varies; the directory should
#    contain at least one *.wasm.
if [ ! -d public/static/wasm ]; then
    fail "public/static/wasm/ directory missing"
else
    wasm_count=$(find public/static/wasm -name '*.wasm' -type f | wc -l)
    if [ "$wasm_count" -lt 1 ]; then
        fail "no *.wasm files found under public/static/wasm/"
    else
        pass "public/static/wasm/ contains $wasm_count WASM file(s)"
    fi
fi

# 5. The engine package's source WASM was built. The server runtime
#    loads this via require('@simlin/engine'); a missing or empty WASM
#    means the Rust+WASM step was skipped or failed silently.
#    ~1MB minimum is well under any real build (release WASM is ~5MB;
#    DISABLE_WASM_OPT bumps it to ~12MB).
if [ ! -f src/engine/core/libsimlin.wasm ]; then
    fail "src/engine/core/libsimlin.wasm missing (engine WASM build skipped?)"
else
    size=$(wc -c < src/engine/core/libsimlin.wasm)
    if [ "$size" -lt 1000000 ]; then
        fail "src/engine/core/libsimlin.wasm is too small ($size bytes; expected >1MB)"
    else
        pass "src/engine/core/libsimlin.wasm exists ($size bytes)"
    fi
fi

# 6. The compiled server bundle exists. GAE runs `node src/server/lib`
#    on the instance; an empty lib/ would crash-loop without a useful
#    error.
if [ ! -f src/server/lib/index.js ]; then
    fail "src/server/lib/index.js missing (server build did not run)"
else
    pass "src/server/lib/index.js exists"
fi

echo ""
if [ "$errors" -gt 0 ]; then
    echo "verify-deploy-build: FAILED ($errors error(s))" >&2
    exit 1
fi
echo "verify-deploy-build: OK"
