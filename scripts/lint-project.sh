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

# Rule 3: Ratchet for unwrap_or_default() in simlin-engine
# Compares current per-file counts against the committed baseline in a single
# Python invocation (avoids spawning a process per file).
BASELINE_FILE="$REPO_ROOT/.lint-baseline.json"
if [ -f "$BASELINE_FILE" ]; then
    CURRENT_COUNTS=$(mktemp)
    rg 'unwrap_or_default\(\)' --type rust -c src/simlin-engine/ 2>/dev/null | \
        sort > "$CURRENT_COUNTS" || true

    RATCHET_OUTPUT=$(python3 -c "
import json, sys

baseline_path = sys.argv[1]
counts_path = sys.argv[2]

with open(baseline_path) as f:
    baseline = json.load(f).get('unwrap_or_default', {}).get('counts', {})

errors = []
with open(counts_path) as f:
    for line in f:
        line = line.strip()
        if not line or ':' not in line:
            continue
        file_path, count_str = line.rsplit(':', 1)
        count = int(count_str)
        baseline_count = baseline.get(file_path)
        if baseline_count is None:
            errors.append(f'New unwrap_or_default() usage in {file_path} ({count} occurrences)')
        elif count > baseline_count:
            errors.append(f'unwrap_or_default() count increased in {file_path}: {baseline_count} -> {count}')

for e in errors:
    print(e)
sys.exit(len(errors))
" "$BASELINE_FILE" "$CURRENT_COUNTS" 2>/dev/null) || true

    if [ -n "$RATCHET_OUTPUT" ]; then
        while IFS= read -r line; do
            echo "ERROR: $line"
            echo "  Fix: Use explicit Result/Option handling instead of unwrap_or_default()."
            echo "  See doc/dev/rust.md for error handling guidelines."
            ERRORS=$((ERRORS + 1))
        done <<< "$RATCHET_OUTPUT"
    fi

    rm -f "$CURRENT_COUNTS"
else
    echo "WARNING: No baseline file found at $BASELINE_FILE. Run scripts/generate-lint-baseline.py to create it."
fi

if [ "$ERRORS" -gt 0 ]; then
    echo ""
    echo "Project lint check failed with $ERRORS error(s)."
    exit 1
fi

echo "Project lint check passed."
