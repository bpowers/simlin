#!/bin/bash
set -e

# Project-specific lint rules.
# Only includes rules with near-zero baseline violations or ratchet mechanisms.
# See doc/tech-debt.md for items tracked by measurement commands.

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
while IFS= read -r file; do
    lines=$(wc -l < "$file" | tr -d ' ')
    if [ "$lines" -gt "$MAX_LINES" ]; then
        echo "ERROR: $file has $lines lines (threshold: $MAX_LINES)."
        echo "  Fix: Consider splitting this file into smaller modules."
        ERRORS=$((ERRORS + 1))
    fi
done < <(find src -name '*.rs' -not -path '*/target/*' -not -path '*/.git/*' \
    -not -name '*.gen.rs' -not -path '*/tests/*')

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

if [ "$ERRORS" -gt 0 ]; then
    echo ""
    echo "Project lint check failed with $ERRORS error(s)."
    exit 1
fi

echo "Project lint check passed."
