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

Individual tests should finish in a few seconds on a debug build. Target is under 2s per test; 5s is the soft ceiling. Slow tests compound: we have thousands of them and they run on every pre-commit and every CI push.

`cargo test --workspace` is wrapped in a 3-minute wall-clock cap in both `scripts/pre-commit` (via `timeout(1)` from GNU coreutils) and `.github/workflows/ci.yaml` (via the step-level `timeout-minutes` field). CI baseline is ~60s, so the cap is ~3x headroom; a run that trips it means something has regressed and the build will fail. If the whole suite legitimately grows past the cap, raise both call sites in the same commit -- do not bypass the hook with `--no-verify`.

Pre-commit needs `timeout(1)` on PATH. Linux distros ship it as `timeout`; on macOS install via `brew install coreutils` (the binary is named `gtimeout` there, and the pre-commit hook picks up whichever is present).

To find slow tests, grep the per-binary durations from a regular run:

```bash
cargo test --workspace 2>&1 | grep 'finished in'
```

Anything over a few seconds is worth looking at.

For PER-TEST durations, run the compiled test binary directly with libtest's
(nightly-gated, but stable-toolchain-accessible) report-time flag:

```bash
# run from the crate directory (src/simlin-engine), NOT the repo root --
# several packages have a tests/integration/main.rs with the same file name,
# and fixture paths resolve relative to the crate
RUSTC_BOOTSTRAP=1 ../../target/debug/deps/<binary> -Z unstable-options --report-time \
  2>&1 | grep 'ok <' | sort -t'<' -k2 -rn | head -20
```

A binary's parallel wall clock is `max(longest single test, total/threads)`, so
one serial mega-test sets the floor no matter how many cores are available.
Prefer one `#[test]` per fixture (or a rayon `par_iter` inside a corpus test)
over a single test that loops a fixture list serially.

#### Testing threshold gates without building giant fixtures

If you have a production gate like `MAX_FOO = 10_000`, do NOT test it by constructing a fixture with 10,001 items -- that ties test runtime to the production constant and makes every test run pay the full gate cost. PR #461 was reverted for exactly this: a test built 10,001 disjoint 3-cycles (~30k variables) so that `model_ltm_variables` would trip `MAX_LTM_TOTAL_CIRCUITS`, and the binary took 44 minutes.

Instead:

- Expose a test-only constant (e.g. a `#[cfg(test)] const` or a field threaded through the API) that the test can set to a tiny value (5, 10) and trip with a correspondingly tiny fixture.
- Or pick a gate whose shape is cheap to exercise (e.g. the `MAX_LTM_SCC_NODES = 50` structural gate at the checkpoint needed a 51-node SCC to trip -- that's 51 variables, not 30,000).

If a test MUST do expensive work (full compilation of a real-world model, enumeration over a large graph for a correctness claim), gate it with `#[ignore]` and document the opt-in command next to the test, for example:

```rust
// Run with: cargo test --release -- --ignored my_expensive_test
#[test]
#[ignore]
fn my_expensive_test() { ... }
```

Prefer `--release` for expensive tests -- enumeration, simulation, and layout code can be 10-50x faster than debug.

**`#[ignore]` for runtime is a judgement about today's engine, so re-take it after the engine gets faster.** An ignored gate does not run in pre-commit or CI, which means it catches nothing until someone remembers to ask for it -- `clearn_ltm_var_count_guardrail`'s own doc comment recorded a regression that slipped through for exactly that reason. After a compile-time improvement, time the ignored set and un-ignore what now fits:

```bash
cargo test -p simlin-engine --test integration --no-run
RUSTC_BOOTSTRAP=1 cargo test -p simlin-engine --test integration -- --ignored \
  -Z unstable-options --report-time 2>&1 | grep 'ok <' | sort -t'<' -k2 -rn
```

Then check the whole suite against CI's budget rather than trusting a developer machine, which has far more cores than a runner:

```bash
RUST_TEST_THREADS=4 taskset -c 0-3 cargo test --workspace   # approximates a CI runner
```

When a test stays ignored, say WHY in the attribute or the doc comment, and make the reason falsifiable: "runtime class" goes stale, while "executing C-LEARN under the non-JIT wasm interpreter, which no compiler speedup touches" or "a strict subset of a test that now runs by default" does not.

## Code Quality

- No placeholder comments ("this is a placeholder"). Use `todo!()` or `unimplemented!()` macros for stubbed-out code, but generally continue working until the implementation is complete.
- Target 95%+ code coverage for new code.
