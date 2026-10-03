# Rust Development Standards

## Error Handling

- **Strongly** prefer idiomatic use of `Result`/`Option` rather than `.unwrap()`. Avoid `.unwrap_or_default()` when it would silently mask an error condition; use it when the default is genuinely the correct value (e.g. `map.get(&key).unwrap_or_default()` for missing keys).
- If a case (e.g. match arm) is expected to be unreachable, use `unreachable!()`, not a comment.

## Testing

- Do NOT write one-off Rust files compiled with `rustc` to test hypotheses. Write unit tests close to the source of the problem instead -- they serve as both verification and documentation.
- Tests should err on the side of brittleness: if a required test file is missing, fail loudly rather than skipping.

### Reading test output

Do NOT preemptively pipe `cargo test` (or `cargo build`) through `head` or `tail` to bound the output. `head` closes the pipe once it has its lines; Rust tooling ignores SIGPIPE, so the producer sees EPIPE and can stop early while still exiting 0 (checked in this repo: `cargo tree -e all --workspace | head -2` drops ~178KB of output and `PIPESTATUS` is `0 0`), which makes a truncated run indistinguishable from a clean one. `tail` lets the run finish but discards the earlier output a late failure often depends on. Either way you lose the evidence, and the exit status will not warn you. Run the full command once, let it finish, and read the failures from the complete output -- rerun a single failing test with a name filter (`cargo test -p <crate> <name>`) when you need a tight loop, rather than truncating the evidence from the broad run.

### One integration-test harness per crate

Add new integration tests as a `mod` in the crate's `tests/integration/main.rs`, NOT as a new top-level `tests/*.rs` file. Cargo builds every top-level `tests/*.rs` file as its own binary that statically links the crate's full dependency graph (~40-110MB each in debug). Beyond the link time and disk cost, macOS imposes a first-exec security scan on every freshly built binary -- roughly 1-3s per binary, proportional to size, and serialized system-wide -- so per-file test binaries made fresh `cargo test` runs pay minutes of scan wait and blew the pre-commit cap (GH #706; consolidating 80 binaries down to ~11 cut a fresh-link workspace test run from ~290s to ~85s on macOS).

Conventions inside a harness:

- Feature-gated modules use `#[cfg(feature = ...)]` on the `mod` declaration in `main.rs` (equivalent to the old per-target `required-features`, without skipping the whole harness).
- A test that mutates process-global state (e.g. installs a `#[global_allocator]`, like `simlin-engine/tests/vm_alloc.rs`) is the one valid reason for a separate top-level `tests/*.rs` binary; document why in the file.
- Tests from different former files now share one process and interleave on libtest threads -- don't add tests that set env vars, change the working directory, or bind fixed ports.
- Run one module's tests with a name filter: `cargo test -p <crate> --test integration -- <module>::`.

### Test time budgets

`cargo test --workspace` is wrapped in a 3-minute wall-clock cap in both `scripts/pre-commit` (via `timeout(1)` from GNU coreutils) and `.github/workflows/ci.yaml` (via the step-level `timeout-minutes` field). A run that trips it fails the build. If the whole suite legitimately grows past the cap, raise both call sites in the same commit -- do not bypass the hook with `--no-verify`.

Pre-commit needs `timeout(1)` on PATH. Linux distros ship it as `timeout`; on macOS install via `brew install coreutils` (the binary is named `gtimeout` there, and the pre-commit hook picks up whichever is present).

**The budget is CPU-seconds, not seconds.** The cap is wall-clock on a CI runner with four cores, and cargo runs the test binaries one after another, so every CPU-second a test spends on any thread is a quarter of a second of that runner's wall clock, or more on a slow one. A test that fans out over rayon finishes in a second on a 32-core desktop and still costs what it costs: libtest's per-test time and a desktop's wall clock both hide it. The suite has little room left (the capped step takes 56 to 127 s across CI runners for one and the same commit, and the slow end is the one that matters), so:

- A test in the default suite is small: well under one CPU-second on a debug build. Measure CPU, not wall, with the test alone in its process:

  ```bash
  # from the crate directory (src/simlin-engine), where fixture paths resolve
  /usr/bin/time -f '%e s wall, %U s cpu' ../../target/debug/deps/<binary> --exact <module>::<test>
  ```

- Anything heavier is a gate (below): a sweep of the model corpus, C-LEARN or World3 compiled or run, a large proptest. Parallelism inside the test does not make it cheap.
- Compare the whole suite before and after a change by its CPU on four cores, which is what a runner has:

  ```bash
  cargo test --workspace --no-run
  RUST_TEST_THREADS=4 taskset -c 0-3 /usr/bin/time -v cargo test --workspace   # read "User time"
  ```

  CI's own wall clock is no measure of a change: runner to runner it varies by more than a factor of two.

To find what is slow, grep the per-binary durations from a regular run:

```bash
cargo test --workspace 2>&1 | grep 'finished in'
```

For PER-TEST durations, run the compiled test binary directly with libtest's
(nightly-gated, but stable-toolchain-accessible) report-time flag:

```bash
# run from the crate directory (src/simlin-engine), NOT the repo root --
# several packages have a tests/integration/main.rs with the same file name,
# and fixture paths resolve relative to the crate
RUSTC_BOOTSTRAP=1 ../../target/debug/deps/<binary> -Z unstable-options --report-time \
  2>&1 | grep 'ok <' | sort -t'<' -k2 -rn | head -20
```

That is wall time under contention, so it ranks single-threaded tests and says nothing about one that fans out; use the CPU measurement above for those. A binary's parallel wall clock is `max(longest single test, total CPU/threads)`, so one serial mega-test sets the floor no matter how many cores are available: prefer one `#[test]` per fixture over a single test that loops a fixture list serially.

#### Gates: the tests the default suite cannot run

A gate is a test that is `#[ignore]`d because a debug build takes too long over it. `cargo test` and the pre-commit hook skip it; `scripts/gates.sh` runs it, under the `gates` cargo profile (optimized, with debug assertions and overflow checks), and CI's `gates` job runs that script on every push to main and every pull request to main. An ignored test nothing runs holds nothing -- a pin a later change moves is found by whoever next runs it by hand -- so the job is what makes an ignored test a check.

```rust
/// Every corpus model's save keeps what the model simulates.
#[test]
#[ignore = "every corpus model saved, read back and simulated; run under the gates profile"]
fn every_corpus_save_keeps_what_the_model_simulates() { ... }
```

```bash
scripts/gates.sh                              # every gate
scripts/gates.sh every_corpus_save            # the tests a filter selects, ignored or not
scripts/gates.sh --nocapture clearn           # libtest flags pass through
```

The rules:

- **`#[ignore]` means "needs an optimized build", and nothing else.** The reason string says what the test sweeps and ends `; run under the gates profile`. Say what makes it heavy in a way that stays true ("C-LEARN under the wasm interpreter"), not how long it took one day.
- **Every ignored test passes in the gates job.** A test that records a known defect is not ignored: it asserts the defect (or lists it in an allowlist that fails when a listed entry stops reproducing) and runs in the default suite, so it fails when the defect is fixed. An ignored test that is expected to fail is a test nobody reads.
- **A gate fails when its property breaks.** Check that the same way as any other test: make the obvious wrong change and see it fail under `scripts/gates.sh`.
- **An instrument** -- an ignored test that prints a measurement or writes files for a person, and asserts nothing about them -- runs in the job too, so it has to run to completion; its reason string says it is one.
- Where it is cheap, keep a small default-suite test beside a gate that exercises the same code on one or two models, so the hook sees a break before CI does.
- `scripts/gates.sh` also runs the rest of each suite it covers, with the `ext_data` feature on: the only run of the suite with the Excel data provider compiled in. To give another crate gates, add it to `PACKAGES` in that script.

The simlin-serve smoke test is ignored for a different reason (it spawns the built binary with its embedded SPA) and has a CI job of its own, `serve-smoke`.

**`#[ignore]` for runtime is a judgement about today's engine, so re-take it after the engine gets faster.** Time the ignored set on a debug build and un-ignore what now fits the default suite's budget:

```bash
cargo test -p simlin-engine --test integration --no-run
RUSTC_BOOTSTRAP=1 cargo test -p simlin-engine --test integration -- --ignored \
  -Z unstable-options --report-time 2>&1 | grep 'ok <' | sort -t'<' -k2 -rn
```

#### Testing threshold gates without building giant fixtures

If you have a production gate like `MAX_FOO = 10_000`, do NOT test it by constructing a fixture with 10,001 items -- that ties test runtime to the production constant and makes every test run pay the full gate cost. PR #461 was reverted for exactly this: a test built 10,001 disjoint 3-cycles (~30k variables) so that `model_ltm_variables` would trip `MAX_LTM_TOTAL_CIRCUITS`, and the binary took 44 minutes.

Instead:

- Expose a test-only constant (e.g. a `#[cfg(test)] const` or a field threaded through the API) that the test can set to a tiny value (5, 10) and trip with a correspondingly tiny fixture.
- Or pick a gate whose shape is cheap to exercise (e.g. the `MAX_LTM_SCC_NODES = 50` structural gate at the checkpoint needed a 51-node SCC to trip -- that's 51 variables, not 30,000).

## Code Quality

- No placeholder comments ("this is a placeholder"). Use `todo!()` or `unimplemented!()` macros for stubbed-out code, but generally continue working until the implementation is complete.
- Target 95%+ code coverage for new code.
