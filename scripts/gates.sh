#!/usr/bin/env bash
# Run the gates: every test the default suite cannot, under an optimized build.
#
# A test is `#[ignore]`d when a debug build takes too long over it (a sweep of
# the model corpus, C-LEARN or World3 compiled and run), so `cargo test` and
# the pre-commit hook skip it and this is what runs it -- here, and in CI's
# `gates` job, which calls this script. The build is the `gates` cargo profile
# (workspace Cargo.toml): optimized, with debug assertions and overflow checks.
#
# It runs the ignored tests only: the default suite runs the rest, and running
# them twice buys nothing. The one exception is the tests of a cargo feature no
# default build turns on (FEATURES and FEATURE_TESTS below), which no other
# run compiles.
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
# provider). FEATURE_TESTS is the libtest filter that selects their tests,
# which are not ignored and run here because nothing else compiles them.
FEATURES="simlin-engine/ext_data"
FEATURE_TESTS=(data_provider::)

# The unit tests and the one integration harness. Naming them keeps the
# doctests and the allocator-counting harness (tests/vm_alloc.rs), which hold
# no gate, from being built under a second profile.
TARGETS=(--lib --test integration)

if [ "${1:-}" = "--no-run" ]; then
    exec cargo test "${PACKAGES[@]}" --profile gates --features "$FEATURES" "${TARGETS[@]}" --no-run
fi

run() {
    cargo test "${PACKAGES[@]}" --profile gates --features "$FEATURES" "${TARGETS[@]}" \
        --no-fail-fast -- "$@"
}

# A filter names the tests it wants, ignored or not.
if [ "$#" -gt 0 ]; then
    run --include-ignored "$@"
    exit
fi

# Both runs happen even when the first fails, so one failure does not hide
# another; the script fails if either did.
status=0
run --ignored || status=$?
run "${FEATURE_TESTS[@]}" || status=$?
exit "$status"
