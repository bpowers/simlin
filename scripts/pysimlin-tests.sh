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
CARGO_TARGET_DIR_RESOLVED="$("$REPO_ROOT/scripts/cargo-target-dir.sh")"

cd src/pysimlin

# Only rebuild the CFFI extension if the static library, header, or build
# script is newer than the .so (or the .so doesn't exist yet).
# Resolved rather than assumed -- see scripts/cargo-target-dir.sh. A stale path
# here is quieter than the wasm one: the staleness check below simply never
# fires, so the CFFI extension is silently not rebuilt against a changed
# library.
LIBSIMLIN_A="$CARGO_TARGET_DIR_RESOLVED/release/libsimlin.a"
SIMLIN_H="$REPO_ROOT/src/libsimlin/simlin.h"
CFFI_SO=$(find simlin -maxdepth 1 -name '_clib*.so' -print -quit 2>/dev/null || true)
if [ -z "$CFFI_SO" ] || [ "$LIBSIMLIN_A" -nt "$CFFI_SO" ] || [ "$SIMLIN_H" -nt "$CFFI_SO" ] || [ simlin/_ffi_build.py -nt "$CFFI_SO" ]; then
  echo "Rebuilding CFFI extension..."
  rm -f simlin/_clib*.so
  rm -rf build/
  uv sync --extra dev
  uv pip install setuptools
  # Pin the archive rather than letting `_ffi_build.py::_get_library_path`
  # search: its candidate list covers the workspace and crate-local `target/`
  # directories only, so under CARGO_TARGET_DIR it would either link a stale
  # default-target archive or fail -- and its own docs say guessing wrong here
  # silently links a stale engine into the extension (GH #682). This is the
  # same archive the freshness check above compared against.
  #
  # Failures are NOT suppressed: a build error here leaves no extension for the
  # suite to import, and the import error that follows says nothing about why.
  SIMLIN_STATIC_LIB="$LIBSIMLIN_A" uv run python setup.py build_ext --inplace
else
  # Ensure deps are up to date (uv fast-paths when nothing changed)
  uv sync --extra dev
fi

cd "$REPO_ROOT"

echo "Running pysimlin linting..."
uv run --directory src/pysimlin ruff check .

# ruff check is the linter only; this is what actually enforces
# formatting (the Python analogue of cargo fmt --check / prettier -l).
echo "Checking pysimlin formatting..."
if ! uv run --directory src/pysimlin ruff format --check .; then
  echo "The files listed above are not ruff-formatted."
  echo "Run: uv run --directory src/pysimlin ruff format ."
  exit 1
fi

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
