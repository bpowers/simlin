#!/bin/bash
# Regenerate stdlib.gen.rs from stdlib/*.stmx files
# Usage: scripts/gen-stdlib.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$ROOT_DIR"

# Build the CLI tool
cargo build -p simlin-cli --release

# Generate the stdlib Rust code
target/release/simlin gen-stdlib \
    --stdlib-dir stdlib \
    --output src/simlin-engine/src/stdlib.gen.rs
