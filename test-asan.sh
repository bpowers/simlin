#!/bin/bash
# `pipefail` is what makes this a test: the suite below is piped into `tee`,
# and without it the pipeline reports tee's status, so a failing test or an
# ASan report leaves the script -- and the scheduled job that runs it -- green.
set -exo pipefail

# Test full Rust suite with ASAN on Linux
echo "Running full test suite with ASAN..."

# Ensure we're on Linux
if [[ "$OSTYPE" != "linux-gnu"* ]]; then
    echo "Error: ASAN testing is only supported on Linux"
    exit 1
fi

# Pin nightly version to avoid type-check failures in dependencies
NIGHTLY_TOOLCHAIN="${NIGHTLY_TOOLCHAIN:-nightly-2026-02-21}"

# Install nightly toolchain if not present
if ! rustup toolchain list | grep -q "$NIGHTLY_TOOLCHAIN"; then
    echo "Installing $NIGHTLY_TOOLCHAIN toolchain with rust-src..."
    rustup toolchain install "$NIGHTLY_TOOLCHAIN" --component rust-src
fi

# Set up ASAN environment
export RUSTFLAGS="-Zsanitizer=address"
export RUSTDOCFLAGS="-Zsanitizer=address"
export ASAN=1

# Determine target triple for current platform
TARGET=$(rustc -vV | sed -n 's/^host: //p')
echo "Using target: $TARGET"

# Run the engine's suite under ASan: the VM, the bytecode it runs, and the
# hashing it leans on are where the engine's `unsafe` is.
# Use -Zbuild-std to rebuild std library with ASAN
# Log to file for analysis
# Every test binary runs and the summaries below print before the script
# reports a failure: `--no-fail-fast` keeps a failing unit-test binary from
# hiding the integration harness, and the status is collected rather than left
# to `set -e`, which would stop before the summaries.
# ASAN_OPTIONS deliberately leaves `halt_on_error` at its default: with
# `halt_on_error=0` a test binary that leaks prints its leak report and exits
# 0, so a leak would be a line in the log of a green job.
# `allocator_may_return_null=1` keeps the allocator's contract: a request no
# allocator can meet returns null, which `try_reserve` reports as an error the
# engine refuses a run with. ASan's default aborts the process instead.
STATUS=0

echo "Testing simlin-engine..."
RUST_BACKTRACE=1 \
ASAN_OPTIONS="allocator_may_return_null=1:detect_leaks=1:check_initialization_order=1:strict_init_order=1:verbosity=0:print_stats=1" \
cargo +"$NIGHTLY_TOOLCHAIN" test -Zbuild-std --target "$TARGET" -p simlin-engine --features file_io --no-fail-fast 2>&1 | tee asan-test.log || STATUS=$?

echo ""
echo "=== ASAN Summary ==="
grep -A5 "SUMMARY: AddressSanitizer" asan-test.log || echo "No ASAN summary found"

echo ""
echo "=== Memory Leaks Detected ==="
grep "Direct leak" asan-test.log | head -10 || echo "No direct leaks found"

echo ""
if [ "$STATUS" -ne 0 ]; then
    echo "ASAN test FAILED (cargo exit $STATUS). Full log saved to asan-test.log"
    exit "$STATUS"
fi
echo "ASAN test complete! Full log saved to asan-test.log"