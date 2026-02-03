#!/bin/bash
set -ex

# Test full Rust suite with ASAN on Linux
echo "Running full test suite with ASAN..."

# Ensure we're on Linux
if [[ "$OSTYPE" != "linux-gnu"* ]]; then
    echo "Error: ASAN testing is only supported on Linux"
    exit 1
fi

# Install nightly toolchain if not present
if ! rustup toolchain list | grep -q "nightly"; then
    echo "Installing nightly toolchain with rust-src..."
    rustup toolchain install nightly --component rust-src
fi

# Set up ASAN environment
export RUSTFLAGS="-Zsanitizer=address"
export RUSTDOCFLAGS="-Zsanitizer=address"
export ASAN=1

# Determine target triple for current platform
TARGET=$(rustc -vV | sed -n 's/^host: //p')
echo "Using target: $TARGET"

# Run tests with ASAN - testing xmutil directly and simlin-engine
# Include xmutil feature to ensure xmutil C/C++ code is exercised
# Use -Zbuild-std to rebuild std library with ASAN
# Log to file for analysis
echo "Testing xmutil package..."
RUST_BACKTRACE=1 \
ASAN_OPTIONS="detect_leaks=1:check_initialization_order=1:strict_init_order=1:verbosity=0:print_stats=1:halt_on_error=0" \
cargo +nightly test -Zbuild-std --target "$TARGET" -p xmutil 2>&1 | tee asan-test.log

echo ""
echo "Testing simlin-engine with xmutil feature..."
RUST_BACKTRACE=1 \
ASAN_OPTIONS="detect_leaks=1:check_initialization_order=1:strict_init_order=1:verbosity=0:print_stats=1:halt_on_error=0" \
cargo +nightly test -Zbuild-std --target "$TARGET" -p simlin-engine --features "xmutil,file_io" 2>&1 | tee -a asan-test.log

echo ""
echo "=== ASAN Summary ==="
grep -A5 "SUMMARY: AddressSanitizer" asan-test.log || echo "No ASAN summary found"

echo ""
echo "=== Memory Leaks Detected ==="
grep "Direct leak" asan-test.log | head -10 || echo "No direct leaks found"

echo ""
echo "ASAN test complete! Full log saved to asan-test.log"