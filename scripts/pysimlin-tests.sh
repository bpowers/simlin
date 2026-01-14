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

echo "Installing pysimlin with dev dependencies..."
cd src/pysimlin
uv sync --extra dev
cd "$REPO_ROOT"

echo "Running pysimlin tests..."
uv run --directory src/pysimlin pytest -q --no-cov tests/

echo "Running pysimlin examples..."
uv run --directory src/pysimlin python examples/edit_existing_model.py
uv run --directory src/pysimlin python examples/population_model.py

echo "Building wheel..."
uv pip install --directory src/pysimlin build
uv run --directory src/pysimlin python -m build -w .
