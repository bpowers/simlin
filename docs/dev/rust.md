# Rust Development Standards

## Error Handling

- **Strongly** prefer idiomatic use of `Result`/`Option` rather than `.unwrap()`. Avoid `.unwrap_or_default()` when it would silently mask an error condition; use it when the default is genuinely the correct value (e.g. `map.get(&key).unwrap_or_default()` for missing keys).
- If a case (e.g. match arm) is expected to be unreachable, use `unreachable!()`, not a comment.

## Testing

- Do NOT write one-off Rust files compiled with `rustc` to test hypotheses. Write unit tests close to the source of the problem instead -- they serve as both verification and documentation.
- Tests should err on the side of brittleness: if a required test file is missing, fail loudly rather than skipping.

### Test time budgets

Individual tests should finish in a few seconds on a debug build. Target is under 2s per test; 5s is the soft ceiling. Slow tests compound: we have thousands of them and they run on every pre-commit and every CI push.

`cargo test --workspace` is wrapped in a 180s wall-clock cap in both `scripts/pre-commit` and `.github/workflows/ci.yaml`. CI baseline is ~60s, so the cap is ~3x headroom; a run that trips the cap means something has regressed and the build will fail. If the whole suite legitimately grows past the cap, raise both call sites in the same commit -- do not bypass the hook with `--no-verify`.

To find slow tests, grep the per-binary durations from a regular run:

```bash
cargo test --workspace 2>&1 | grep 'finished in'
```

Anything over a few seconds is worth looking at.

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

## Code Quality

- No placeholder comments ("this is a placeholder"). Use `todo!()` or `unimplemented!()` macros for stubbed-out code, but generally continue working until the implementation is complete.
- Target 95%+ code coverage for new code.
