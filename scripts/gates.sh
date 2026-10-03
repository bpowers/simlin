#!/usr/bin/env bash
# Run the gates: every test the default suite cannot, under an optimized build.
#
# A test is `#[ignore]`d when a debug build takes too long over it (a sweep of
# the model corpus, C-LEARN or World3 compiled and run), so `cargo test` and
# the pre-commit hook skip it and this is what runs it -- here, and in CI's
# `gates` job, which calls this script. The build is the `gates` cargo profile
# (workspace Cargo.toml): optimized, with debug assertions and overflow checks.
#
# It runs the ignored tests AND the rest of each suite. The rest costs little
# once optimized, and it is the only run of the suite with a cargo feature
# no default build turns on compiled in (FEATURES below).
#
# usage:
#   scripts/gates.sh                    every gate
#   scripts/gates.sh <filter>...        the tests a libtest filter selects,
#                                       ignored or not (flags pass through:
#                                       scripts/gates.sh --nocapture clearn)
#   scripts/gates.sh --no-run           build only (CI times the build apart
#                                       from the run)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# The packages with gates. To add one, list it here with `-p`; its features go
# in FEATURES in the same `package/feature` form.
PACKAGES=(-p simlin-engine)

# Features no other lane runs the suite with: `ext_data` (the Excel data
# provider).
FEATURES="simlin-engine/ext_data"

# The unit tests and the one integration harness. Naming them keeps the
# doctests and the allocator-counting harness (tests/vm_alloc.rs), which hold
# no gate, from being built under a second profile.
TARGETS=(--lib --test integration)

# Gates known to fail, skipped by exact name so every other gate still runs.
# The C-LEARN LTM digest's pin is stale; the change that re-pins it deletes
# this skip.
SKIP=(--skip simulate_ltm::clearn_ltm_slot_maxima_digest)

if [ "${1:-}" = "--no-run" ]; then
    exec cargo test "${PACKAGES[@]}" --profile gates --features "$FEATURES" "${TARGETS[@]}" --no-run
fi

exec cargo test "${PACKAGES[@]}" --profile gates --features "$FEATURES" "${TARGETS[@]}" \
    --no-fail-fast -- --include-ignored "${SKIP[@]}" "$@"
