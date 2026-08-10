#!/usr/bin/env bash
# Print the absolute path of this workspace's cargo target directory.
#
# Scripts that stage a built artifact need to know where cargo actually put it,
# and that is NOT always `<repo>/target`: `CARGO_TARGET_DIR`, `--target-dir`, and
# `build.target-dir` in any applicable cargo config all move it. Hardcoding the
# default turns a moved target directory into a `cp: cannot stat` at the staging
# step, which reads as a broken build rather than as a path mismatch -- and has
# cost more than one debugging session.
#
# `cargo metadata` is the only thing that accounts for every way the directory
# can be set, so this asks cargo rather than reconstructing its rules. Keep this
# the single copy: a second, hand-maintained resolution drifts exactly where the
# real one is non-trivial.
#
# Usage:  TARGET_DIR="$(scripts/cargo-target-dir.sh)"
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1 && pwd)"

cargo metadata --format-version 1 --no-deps \
  --manifest-path "$REPO_ROOT/Cargo.toml" \
  | jq -r '.target_directory'
