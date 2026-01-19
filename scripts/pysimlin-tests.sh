#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
export RUST_BACKTRACE=1
cd "$REPO_ROOT"

if ! command -v uv >/dev/null 2>&1; then
  echo "uv not found. Install with: curl -LsSf https://astral.sh/uv/install.sh | sh" >&2
  exit 1
fi

echo "Building libsimlin (release)..."
cargo build --release --manifest-path src/libsimlin/Cargo.toml

echo "Cleaning stale pysimlin build artifacts..."
cd src/pysimlin
# Remove any cached cffi extension modules to ensure they're rebuilt with new declarations
rm -f simlin/_clib*.so
rm -rf build/
# Clear Python bytecode caches
find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
find . -type f -name "*.pyc" -delete 2>/dev/null || true
cd "$REPO_ROOT"

echo "Installing pysimlin with dev dependencies..."
cd src/pysimlin
uv sync --extra dev
# Build the cffi extension in-place for editable installs
uv pip install setuptools
uv run python setup.py build_ext --inplace 2>/dev/null || true
cd "$REPO_ROOT"

echo "Running pysimlin tests..."
uv run --directory src/pysimlin pytest -q --no-cov tests/

echo "Running pysimlin examples..."
uv run --directory src/pysimlin python examples/edit_existing_model.py
uv run --directory src/pysimlin python examples/population_model.py

echo "Building wheel..."
uv run --directory src/pysimlin python -m build -w .
