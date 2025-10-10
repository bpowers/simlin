#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
export RUST_BACKTRACE=1
cd "$REPO_ROOT"

PY=${PYTHON_BIN:-/opt/homebrew/bin/python3.13}
if ! command -v "$PY" >/dev/null 2>&1; then
  echo "Python 3.13 not found at $PY; falling back to python3" >&2
  PY=python3
fi

PYV="$($PY -c 'import sys;print("%d.%d"%sys.version_info[:2])')"
min="3.13"
if [ "$(printf '%s\n' "$min" "$PYV" | sort -V | head -n1)" != "$min" ]; then
  echo "Python $PYV detected; require >= $min for pysimlin" >&2
  exit 1
fi

echo "Building libsimlin (release)..."
cargo build --release --manifest-path src/libsimlin/Cargo.toml

VENV=".venv-pysimlin"
if [ ! -d "$VENV" ]; then
  "$PY" -m venv "$VENV"
fi
source "$VENV/bin/activate"
python -m pip install -U pip
python -m pip install -e src/pysimlin[dev]

echo "Running pysimlin tests..."
pytest -q --no-cov src/pysimlin/tests

echo "Running pysimlin examples..."
python src/pysimlin/examples/edit_existing_model.py
python src/pysimlin/examples/population_model.py

echo "Building wheel..."
python -m pip install build
python -m build -w src/pysimlin
