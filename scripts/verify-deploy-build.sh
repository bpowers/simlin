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

# 4. The web component bundle carries Rspack's runtime-publicPath logic.
#    `assetPrefix: 'auto'` in rsbuild.component.config.js makes the bundle
#    compute its base URL at load time from `document.currentScript.src`
#    instead of `document.baseURI`. That is the cross-origin embed
#    contract: a third-party page loads
#    `<script src="https://app.simlin.com/static/js/sd-component.js">`,
#    and the worker chunk under static/js/async/ must then resolve against
#    app.simlin.com, not the embedding page's origin. Drop `assetPrefix:
#    'auto'` (or the LimitChunkCountPlugin that merges the worker) and
#    every external embed 404s on first worker init -- the failure mode
#    fixed in commit dd9e449c.
#
#    This is a smoke check, not a proof of cross-origin correctness: it
#    asserts the bundle contains the script-URL detection at all. If it's
#    gone, `currentScript` disappears from the minified output entirely.
if [ -f public/static/js/sd-component.js ]; then
    if ! grep -q 'currentScript' public/static/js/sd-component.js; then
        fail "public/static/js/sd-component.js has no 'currentScript' reference -- assetPrefix: 'auto' (runtime publicPath) appears to be missing; cross-origin embeds will 404 on the worker chunk"
    else
        pass "public/static/js/sd-component.js carries the runtime publicPath (document.currentScript) logic"
    fi
fi

# 5. The web component stylesheet is shipped beside the SPA CSS. The
#    component renders inside a closed shadow root, so document-level CSS from
#    the embedding page cannot style it; sd-component.js links this exact file
#    into the shadow tree.
if [ ! -f public/static/css/sd-component.css ]; then
    fail "public/static/css/sd-component.css missing (closed-shadow web component would render without its stylesheet)"
else
    size=$(wc -c < public/static/css/sd-component.css)
    if [ "$size" -lt 1024 ]; then
        fail "public/static/css/sd-component.css is suspiciously small ($size bytes)"
    elif ! LC_ALL=C grep -q 'simlinCanvas' public/static/css/sd-component.css; then
        fail "public/static/css/sd-component.css does not contain expected diagram canvas styles"
    elif ! LC_ALL=C grep -q -- '--color-primary' public/static/css/sd-component.css; then
        fail "public/static/css/sd-component.css does not contain expected theme styles"
    elif ! LC_ALL=C grep -q 'KaTeX_Main' public/static/css/sd-component.css; then
        fail "public/static/css/sd-component.css does not contain expected KaTeX styles"
    else
        pass "public/static/css/sd-component.css exists and contains editor styles ($size bytes)"
    fi
fi

if [ -f public/static/js/sd-component.js ]; then
    if ! grep -q '/static/css/sd-component.css' public/static/js/sd-component.js; then
        fail "public/static/js/sd-component.js does not link /static/css/sd-component.css into the shadow root"
    else
        pass "public/static/js/sd-component.js links the component stylesheet"
    fi
fi

# 6. A hashed WASM blob exists somewhere under public/static/wasm/, and it
#    is the SLIM browser artifact. rsbuild emits the engine WASM via
#    Rspack's asset module with a content hash; the exact filename varies.
#    The browser bundle must come from libsimlin-browser.wasm (built
#    --no-default-features, no png_render): the rasterization stack is
#    ~28% of the full binary and shipping it to browsers is pure dead
#    weight. The export name appears as a literal string in the wasm
#    export section, so a binary grep is a reliable presence check.
if [ ! -d public/static/wasm ]; then
    fail "public/static/wasm/ directory missing"
else
    wasm_count=$(find public/static/wasm -name '*.wasm' -type f | wc -l)
    if [ "$wasm_count" -lt 1 ]; then
        fail "no *.wasm files found under public/static/wasm/"
    else
        pass "public/static/wasm/ contains $wasm_count WASM file(s)"
        while IFS= read -r wasm_file; do
            if LC_ALL=C grep -q 'simlin_project_render_png' "$wasm_file"; then
                fail "$wasm_file exports simlin_project_render_png -- the browser bundle picked up the full WASM instead of libsimlin-browser.wasm"
            else
                pass "$wasm_file is the slim browser artifact (no png_render)"
            fi
        done < <(find public/static/wasm -name '*.wasm' -type f)
    fi
fi

# 7. The engine package's source WASM was built, and it is the FULL
#    artifact. The server runtime loads this via require('@simlin/engine')
#    and its model-preview pipeline calls simlin_project_render_png; a
#    slim WASM here would 500 every preview render. A missing or empty
#    WASM means the Rust+WASM step was skipped or failed silently.
#    ~1MB minimum is well under any real build (release WASM is ~6MB;
#    DISABLE_WASM_OPT bumps it to ~12MB).
if [ ! -f src/engine/core/libsimlin.wasm ]; then
    fail "src/engine/core/libsimlin.wasm missing (engine WASM build skipped?)"
else
    size=$(wc -c < src/engine/core/libsimlin.wasm)
    if [ "$size" -lt 1000000 ]; then
        fail "src/engine/core/libsimlin.wasm is too small ($size bytes; expected >1MB)"
    elif ! LC_ALL=C grep -q 'simlin_project_render_png' src/engine/core/libsimlin.wasm; then
        fail "src/engine/core/libsimlin.wasm lacks the simlin_project_render_png export (server PNG previews would break) -- was it built --no-default-features?"
    else
        pass "src/engine/core/libsimlin.wasm exists and is the full artifact ($size bytes)"
    fi
fi

# 8. The compiled server bundle exists. GAE runs `node src/server/lib`
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
