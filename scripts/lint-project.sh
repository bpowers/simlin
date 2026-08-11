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
COPYRIGHT_OUTPUT=$(mktemp)
if ! python3 scripts/check-copyright.py > "$COPYRIGHT_OUTPUT"; then
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        echo "ERROR: copyright header: $line"
        ERRORS=$((ERRORS + 1))
    done < "$COPYRIGHT_OUTPUT"
fi
rm -f "$COPYRIGHT_OUTPUT"

# Rule 4: a path-filtered workflow's push and pull_request `paths` lists must
# match. They are maintained by hand and read as one filter, so a path added to
# only one of them silently means "runs on merge but not on the PR" (or the
# reverse) -- a gap that looks like coverage. `.github/workflows/wasm-opt.yml`
# is the only such workflow today; the loop covers any future one.
PATHS_OUTPUT=$(mktemp)
if ! python3 - > "$PATHS_OUTPUT" <<'PYEOF'; then
import glob, sys
import yaml

status = 0
for path in sorted(glob.glob(".github/workflows/*.y*ml")):
    with open(path) as fh:
        wf = yaml.safe_load(fh)
    triggers = (wf or {}).get(True) or (wf or {}).get("on") or {}
    if not isinstance(triggers, dict):
        continue
    push = (triggers.get("push") or {}).get("paths")
    pull = (triggers.get("pull_request") or {}).get("paths")
    if push is None and pull is None:
        continue
    if push != pull:
        only_push = [p for p in (push or []) if p not in (pull or [])]
        only_pull = [p for p in (pull or []) if p not in (push or [])]
        print(f"{path}: push and pull_request `paths` differ; "
              f"push-only={only_push} pull_request-only={only_pull}")
        status = 1
sys.exit(status)
PYEOF
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        echo "ERROR: workflow paths: $line"
        ERRORS=$((ERRORS + 1))
    done < "$PATHS_OUTPUT"
fi
rm -f "$PATHS_OUTPUT"

if [ "$ERRORS" -gt 0 ]; then
    echo ""
    echo "Project lint check failed with $ERRORS error(s)."
    exit 1
fi

echo "Project lint check passed."
