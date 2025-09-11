#!/bin/bash
set -ex

# Test ASAN locally on macOS or Linux
echo "Testing ASAN build locally..."

# Detect platform
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "Running on macOS"
    RUST_TARGET="aarch64-apple-darwin"
    # macOS uses different sanitizer setup
    export RUSTFLAGS="-Z sanitizer=address"
    ASAN_LIB=""  # macOS handles this differently
else
    echo "Running on Linux"
    RUST_TARGET="x86_64-unknown-linux-gnu"
    export RUSTFLAGS="-Z sanitizer=address"
    ASAN_LIB=$(gcc -print-file-name=libasan.so)
fi

# Build libsimlin with ASAN
cd src/libsimlin
cargo +nightly build --release --target "$RUST_TARGET"

# Copy library to expected location
mkdir -p target/release
cp ../../target/"$RUST_TARGET"/release/libsimlin.a target/release/

# Install pysimlin with ASAN
cd ../pysimlin
export ASAN=1
pip install -e .

# Run the specific test that's failing
echo "Running problematic test..."
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS
    RUST_BACKTRACE=1 \
    ASAN_OPTIONS="detect_leaks=0:verbosity=3:halt_on_error=0" \
    python -m pytest tests/test_memory.py::TestErrorPathMemoryLeaks::test_import_error_no_leak -vvs
else
    # Linux
    RUST_BACKTRACE=1 \
    ASAN_OPTIONS="detect_leaks=1:verbosity=3:halt_on_error=0" \
    LD_PRELOAD=$ASAN_LIB \
    PYTHONMALLOC=malloc \
    python -m pytest tests/test_memory.py::TestErrorPathMemoryLeaks::test_import_error_no_leak -vvs
fi

echo "Test complete!"