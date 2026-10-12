#!/bin/bash
set -e

# Project-specific lint rules.
# Only includes rules with near-zero baseline violations or ratchet mechanisms.
# See docs/tech-debt.md for items tracked by measurement commands.

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# Fail fast if rg is not installed (required for ratchet checks)
if ! command -v rg > /dev/null 2>&1; then
    echo "ERROR: ripgrep (rg) is required but not installed."
    echo "  Install: cargo install ripgrep, or brew install ripgrep"
    exit 1
fi

ERRORS=0

# Run a check that writes one error per line to stdout, and count those lines.
# A check that FAILS TO RUN counts as an error in its own right: without that,
# a crashed script writes its traceback to stderr, contributes zero lines here,
# and the lint reports success -- a rule that silently stopped running looks
# exactly like a rule that found nothing.
run_line_check() {
    local label="$1"
    shift
    local out err rc
    out=$(mktemp)
    err=$(mktemp)
    set +e
    "$@" > "$out" 2> "$err"
    rc=$?
    set -e
    # Only a FAILING check's stdout is error lines; a passing one may print a
    # summary there.
    local found=0
    if [ "$rc" -ne 0 ]; then
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            echo "ERROR: $label: $line"
            ERRORS=$((ERRORS + 1))
            found=1
        done < "$out"
    fi
    if [ "$rc" -ne 0 ] && [ "$found" -eq 0 ]; then
        echo "ERROR: $label: check failed to run (exit $rc):"
        sed 's/^/    /' < "$err" >&2
        ERRORS=$((ERRORS + 1))
    fi
    rm -f "$out" "$err"
}

# Rule 1: No --no-verify in any script or config file (excluding this lint script itself).
# This should always have zero occurrences.
NOVERIFY_PATTERN='--no-verify'
NO_VERIFY_COUNT=$(grep -r --include='*.sh' --include='*.yaml' --include='*.yml' \
    --include='*.json' --include='*.toml' --include='*.js' --include='*.ts' \
    -l "$NOVERIFY_PATTERN" scripts/ .github/ 2>/dev/null | \
    grep -v 'lint-project\.sh' | wc -l | tr -d ' ')
if [ "$NO_VERIFY_COUNT" -gt 0 ]; then
    echo "ERROR: Found $NOVERIFY_PATTERN in scripts or config files:"
    grep -r --include='*.sh' --include='*.yaml' --include='*.yml' \
        --include='*.json' --include='*.toml' --include='*.js' --include='*.ts' \
        -n "$NOVERIFY_PATTERN" scripts/ .github/ 2>/dev/null | \
        grep -v 'lint-project\.sh'
    echo "  Fix: Remove $NOVERIFY_PATTERN flags. Pre-commit hooks must not be bypassed."
    echo "  See CLAUDE.md for the policy."
    ERRORS=$((ERRORS + 1))
fi

# Rule 2: Rust source file size warning
# Threshold set just above the current maximum (vm.rs at ~5513 lines).
MAX_LINES=6000
RS_FILES=$(mktemp)
find src -name '*.rs' -not -path '*/target/*' -not -path '*/.git/*' \
    -not -name '*.gen.rs' -not -path '*/tests/*' > "$RS_FILES"
while IFS= read -r file; do
    lines=$(wc -l < "$file" | tr -d ' ')
    if [ "$lines" -gt "$MAX_LINES" ]; then
        echo "ERROR: $file has $lines lines (threshold: $MAX_LINES)."
        echo "  Fix: Consider splitting this file into smaller modules."
        ERRORS=$((ERRORS + 1))
    fi
done < "$RS_FILES"
rm -f "$RS_FILES"

# Rule 3: Copyright headers on all Rust and TypeScript source files
# check-copyright.py writes one error per line to stdout; summary to stderr.
run_line_check "copyright header" python3 scripts/check-copyright.py

# Rule 4: a path-filtered workflow's push and pull_request `paths` lists must
# match. They are maintained by hand and read as one filter, so a path added to
# only one of them silently means "runs on merge but not on the PR" (or the
# reverse) -- a gap that looks like coverage. `.github/workflows/wasm-opt.yml`
# is the only such workflow today; the loop covers any future one.
run_line_check "workflow paths" python3 scripts/check-workflow-paths.py
rm -f "$PATHS_OUTPUT"

# Rule 5: a map or set keyed by an interned canonical identifier is spelled
# `IdentMap` / `IdentSet` (simlin_engine::common), never `HashMap`, `HashSet`
# or their `Fx` twins. The aliases are the one statement of how such a key is
# hashed, so a second spelling is a second answer. The pattern reads the FIRST
# type parameter, through a reference, a lifetime, a path prefix or a tuple,
# and across a line break; the key types are the three interned identifier
# types and the engine's keys built from them. It sees only a type that is
# written down: a map whose key type is inferred is caught where it meets an
# IdentMap in a signature (the hasher is part of the type), or not at all.
IDENT_KEYED_PATTERN='\b(Fx)?Hash(Map|Set)<\s*\(?\s*&?\s*('"'"'\w+\s+)?((crate::|simlin_engine::)?common::)?(Ident|CanonicalElementName|CanonicalDimensionName|VarRef|ModuleKey|ModuleEdgeKey)\b'
IDENT_KEYED=$(rg -U -n --glob '*.rs' --glob '!*.gen.rs' "$IDENT_KEYED_PATTERN" src/ || true)
if [ -n "$IDENT_KEYED" ]; then
    echo "ERROR: std/Fx hash collections keyed by a canonical identifier:"
    echo "$IDENT_KEYED"
    echo "  Fix: use simlin_engine::common::IdentMap / IdentSet."
    echo "  See the rustdoc on IdentMap for why there is one spelling."
    ERRORS=$((ERRORS + 1))
fi

if [ "$ERRORS" -gt 0 ]; then
    echo ""
    echo "Project lint check failed with $ERRORS error(s)."
    exit 1
fi

echo "Project lint check passed."
