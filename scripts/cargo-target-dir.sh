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
# The JSON is parsed with python3, NOT jq, and that is deliberate. This script
# is on the primary build path -- `src/engine/build.sh` and
# `scripts/pysimlin-tests.sh` both call it, so it runs on every `pnpm build`
# and so on every pre-commit and in CI. jq is otherwise used only by release
# and CI-support scripts, no workflow installs it (the GitHub runner images
# happen to ship it), and `scripts/dev-init.sh` does not check for it.
# Depending on it here would turn a missing jq into `jq: command not found`
# under `set -e` -- trading the `cp: cannot stat` this script exists to
# prevent for an equally opaque failure one step earlier. python3 adds
# nothing: `scripts/pre-commit` already shells to it in phase 1, before any
# build runs, and `scripts/pysimlin-tests.sh` is python by definition.
#
# There is deliberately NO fallback to `<repo>/target` when the lookup fails.
# A fallback would be silent and wrong in exactly the case this script exists
# for -- a genuinely moved target directory -- reintroducing the original
# `cp: cannot stat` for the only callers who need the resolution at all.
#
# Usage:  TARGET_DIR="$(scripts/cargo-target-dir.sh)"
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." >/dev/null 2>&1 && pwd)"

cargo metadata --format-version 1 --no-deps \
  --manifest-path "$REPO_ROOT/Cargo.toml" \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
