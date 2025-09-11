#!/bin/bash
set -ex

# Debug wheel contents locally
echo "Building and inspecting wheel..."

# Clean up
rm -rf src/pysimlin/dist src/pysimlin/build src/pysimlin/*.egg-info

# Build libsimlin
echo "Building libsimlin..."
cargo build --release -p simlin
mkdir -p src/libsimlin/target/release
cp target/release/libsimlin.a src/libsimlin/target/release/

# Build wheel
echo "Building wheel..."
cd src/pysimlin
python -m pip install --upgrade pip build wheel
python -m build --wheel

# Inspect wheel contents
echo "=== Wheel contents ==="
unzip -l dist/*.whl | grep -E "\.(so|dylib|pyd|dll|a)" || echo "No compiled extensions found!"
echo ""

# Try to install and test
echo "=== Testing wheel installation ==="
python -m venv test_wheel_env
source test_wheel_env/bin/activate

pip install dist/*.whl
echo ""
echo "=== Checking installed files ==="
SITE_PACKAGES=$(python -c "import site; print(site.getsitepackages()[0])")
ls -la "$SITE_PACKAGES"/simlin*

echo ""
echo "=== Trying to import ==="
python -c "from simlin._ffi import ffi, lib; print('_ffi imported successfully')" || {
    echo "Failed to import _ffi"
    python -c "import simlin; print(dir(simlin))"
}

deactivate
rm -rf test_wheel_env

echo "Debug complete!"