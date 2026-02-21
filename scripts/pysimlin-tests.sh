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

cd src/pysimlin

# Only rebuild the CFFI extension if the static library is newer than the .so,
# or if the .so doesn't exist yet.
LIBSIMLIN_A="$REPO_ROOT/target/release/libsimlin.a"
CFFI_SO=$(find simlin -maxdepth 1 -name '_clib*.so' -print -quit 2>/dev/null || true)
if [ -z "$CFFI_SO" ] || [ "$LIBSIMLIN_A" -nt "$CFFI_SO" ] || [ simlin/_ffi_build.py -nt "$CFFI_SO" ]; then
  echo "Rebuilding CFFI extension..."
  rm -f simlin/_clib*.so
  rm -rf build/
  uv sync --extra dev
  uv pip install setuptools
  uv run python setup.py build_ext --inplace 2>/dev/null || true
else
  # Ensure deps are up to date (uv fast-paths when nothing changed)
  uv sync --extra dev
fi

cd "$REPO_ROOT"

echo "Running pysimlin type checking..."
uv run --directory src/pysimlin mypy simlin/

echo "Running pysimlin tests..."
uv run --directory src/pysimlin pytest -n auto -q --no-cov tests/

echo "Running pysimlin examples..."
uv run --directory src/pysimlin python examples/edit_existing_model.py
uv run --directory src/pysimlin python examples/population_model.py

# Build wheel only in CI or when explicitly requested (not needed for pre-commit)
if [ "${BUILD_WHEEL:-0}" = "1" ] || [ -n "${CI:-}" ]; then
  echo "Building wheel..."
  uv run --directory src/pysimlin python -m build -w .
fi
