#!/bin/bash
set -ex

# Test the wheel build process locally
echo "Testing wheel build locally..."

# Clean up previous builds
rm -rf src/pysimlin/dist src/pysimlin/build src/pysimlin/*.egg-info

# Build libsimlin first
echo "Building libsimlin..."
cargo build --release -p simlin

# Copy library to expected location
mkdir -p src/libsimlin/target/release
cp target/release/libsimlin.a src/libsimlin/target/release/

# Stage the notebook widget assets (widget.js + wasm + ASSETS.json) from the
# frontend build outputs; setup.py refuses to build a wheel without them.
echo "Staging widget assets..."
python3 src/pysimlin/scripts/stage_widget_assets.py --no-build

# Build the wheel
echo "Building wheel..."
cd src/pysimlin
python -m pip install --upgrade pip build
python -m build --wheel

# Test the wheel
echo "Testing wheel installation..."
python -m venv test_env
source test_env/bin/activate
pip install dist/*.whl
pip install pytest pytest-cov numpy pandas

# Run tests
echo "Running tests..."
python -m pytest tests/ -v

deactivate
rm -rf test_env

echo "Local test complete!"