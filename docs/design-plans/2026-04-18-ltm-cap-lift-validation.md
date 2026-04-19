# 2026-04-18 -- LTM cap-lift validation

**Role**: adversarial reviewer on team `ltm-perf-unleash` (task #6).
**Scope**: validate that Options A (auto-flip on large element-level SCCs)
and B (post-sim relative loop score) are safe to merge before task #7
raises or removes `MAX_LTM_CIRCUITS`.

Companion documents:

- `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md` -- motivation
  (Cliff A: ~17 GB in `build_element_level_loops`; Cliff B: ~140 TB of
  `rel_loop_score` equation text at WRLD3 scale).
- `docs/design-plans/2026-04-18-ltm-cap-lift-options.md` -- architect's
  decision tree, including rationale for accepting A + B while shelving
  C/E/F/G.

Commits covered:

- `c11f4851` engine: auto-flip LTM to discovery on large SCCs
- `aff5c56f` doc: note LTM auto-flip in divergences-from-papers
- `26475018` engine: move rel_loop_score from compile-time to post-sim
- `4959706c` doc: describe post-sim rel_loop_score computation

Plus two commits added during this validation:

- `e79cf3c9` engine: surface LTM auto-flip warning via collect_all_diagnostics
- `69e7ba14` engine: add adversarial LTM auto-flip and rel_loop_score corner tests

## 1. Test-suite outcomes

All existing suites pass on top of the above commits.  The pre-commit
hook (`scripts/pre-commit`) was exercised with both of my validation
commits (`e79cf3c9` and `69e7ba14`) and reported green across every
gate:

| Check                                                   | Result |
|---------------------------------------------------------|--------|
| Dependency policy                                       | pass   |
| Documentation links                                     | pass   |
| Project lint rules                                      | pass   |
| Rust (cargo fmt + cbindgen + clippy + cargo test)       | pass   |
| TypeScript (lint + build + tsc + pnpm test)             | pass   |
| Pysimlin tests (Python 3.11+)                           | pass   |

Explicitly re-run targeted suites:

| Suite                                            | Result |
|--------------------------------------------------|--------|
| `cargo test -p simlin-engine --lib`              | 3041 passed, 2 ignored |
| `cargo test -p simlin-engine --features file_io --test simulate_ltm` | 45 passed, 1 ignored |
| `cargo test -p simlin-engine --features file_io --test wrld3_ltm_panic` | 1 passed in 0.52s |
| `cargo test -p simlin-engine --lib db_ltm_unified_tests` | 17 passed |
| `cargo test -p simlin-engine --lib ltm_post`     | 7 passed |

The `simulate_ltm` suite doubles as the semantic cross-check for Option
B (see section 3); the 45 passing tests include the ones that assert
exact relative-loop-score values against known-good fixtures.

## 2. WRLD3 end-to-end numbers

Binary: `cargo run --release --features file_io --example ltm_full_bench`
on `test/metasd/WRLD3-03/wrld3-03.mdl`.
Abort ceiling: 15 GiB VmPeak (default).
Machine baseline at startup: ~6.1 MiB VmPeak / ~3.3 MiB VmHWM.

Scenarios were run with three circuit-cap settings to cover the
pre-lift production cap (100_000), the diagnosis plan's proposed cap
(2,000,000), and an unbounded run to confirm the pipeline is
independent of the cap once Option A is in place.

| cap                     | circuits    | ltm_vars                          | peak MiB | VmHWM MiB | wall ms | compile |
|-------------------------|-------------|-----------------------------------|----------|-----------|---------|---------|
| 100,000 (current)       | 0 (capped)  | 483 (link=483, loop/path/comp=0) | 32.88    | 29.16     | 189.5   | ok      |
| 2,000,000               | 1,863,803   | 483 (link=483, loop/path/comp=0) | 467.08   | 458.15    | 1597.1  | ok      |
| usize::MAX (uncapped)   | 1,863,803   | 483 (link=483, loop/path/comp=0) | 467.07   | 457.09    | 1581.2  | ok      |

Observations:

- **Auto-flip fires on WRLD3 as intended** -- all three runs produce
  `loop_score=0`, `path=0`, `composite=0`, and the single-partition
  166-node SCC exceeds `MAX_LTM_SCC_NODES = 50`.  The 483 link scores
  are the full causal-edge set (same as discovery mode).
- **cap=2M and cap=usize::MAX are indistinguishable** -- peak memory
  within 0.01 MiB, wall time within 16 ms noise.  The cap stops
  mattering once Option A pre-empts `build_element_level_loops`.
- **Equation-text footprint (`eq_bytes`) is 324,274 bytes** -- a
  ~430,000,000x reduction from the ~140 TB the old compile-time
  `rel_loop_score` emitter would have produced.
- **The path that previously dominated (`build_element_level_loops`
  grouping 1.86M circuits) is never entered.**  In production, when
  Option A auto-flips, `model_element_loop_circuits` is never called
  either (the element-level SCC check at `db_ltm.rs:2087-2091` short-
  circuits before Johnson's runs).  The bench's stage-6 row does call
  it for instrumentation, which is why the 2M-circuit row shows
  circuits=1,863,803 -- but `model_ltm_variables` at stage 7 is unaware
  because its internal call to `model_element_loop_circuits` is gated
  off by `is_discovery`.

## 3. Before/after semantic cross-check

The cross-check of Option B is the passing `simulate_ltm` suite.  45
integration tests assert exact `loop_score` and relative-loop-score
timeseries against fixture expectations; they exercised the old
compile-time SAFEDIV equation path and, after `26475018`, exercise the
new post-sim `compute_rel_loop_scores` path with no fixture
modifications.  Specifically relevant tests:

- `test_independent_subsystems_partitioned_relative_scores` -- proves
  that relative scores are normalized within each cycle partition and
  not across the whole model.
- `test_a2a_two_loop_relative_scores_sum_to_100` -- an explicit
  end-to-end assertion against the summed-to-100 property of the
  within-partition relative scores.
- `test_coupled_two_stock_single_partition`,
  `test_arms_race_single_partition`,
  `test_discovery_independent_subsystems` -- exercise discovery mode
  relative scores including multi-stock partitions.
- `hero_culture_loop_sign_continuity`,
  `discovery_arms_race_3party`, `discovery_decoupled_stocks`,
  `discovery_logistic_growth_finds_both_loops` -- cover the five models
  called out explicitly in the task (logistic_growth_ltm,
  arms_race_3party, decoupled_stocks, cross_element_ltm,
  hero_culture_ltm); the remaining two (arrayed_population_ltm,
  modules_hares_and_foxes) are covered by `test_a2a_*` and the smooth-
  instance suite respectively.

The `ltm_post::tests::matches_reference_formula` proptest runs 128
cases of random (loops, partitions, timesteps) and compares the post-
sim computation to an inlined reference SAFEDIV formula.  It would
catch any numeric divergence from the old compile-time behaviour to
within 1e-10.

## 4. Corner cases added

Committed in `69e7ba14`:

1. **`test_auto_flip_keys_on_largest_scc_not_total_nodes`** -- two
   disjoint 40-node cycles (80 nodes total) must stay on the exhaustive
   path.  A regression that accidentally summed SCC sizes would auto-
   flip every mid-sized model with a handful of independent feedback
   subsystems.

2. **`test_user_discovery_mode_does_not_emit_auto_flip_warning`** --
   when the caller sets `ltm_discovery_mode = true`, the warning is
   suppressed, so diagnostics do not become noise on the path that was
   deliberately chosen.

3. **`test_auto_flip_uses_element_level_scc_for_arrayed_models`** -- a
   60-element A2A stock-flow loop has an element-level SCC size of 2
   (same-element edges only), so auto-flip does not fire.  Pins the
   invariant that the gate reads the element-level graph; a future
   refactor that accidentally re-measured the variable-level graph
   would be caught.

4. **`nan_loop_score_propagates_without_panic`** in `ltm_post.rs` --
   pins the documented IEEE-754 propagation contract for non-finite
   upstream values.  Matches the behaviour of the removed compile-time
   SAFEDIV equation (arithmetic on NaN, never asserting).

Two negative tests also added in `e79cf3c9` to guard the diagnostic
surfacing path (section 5).  The existing positive tests from
`c11f4851` (51-node auto-flip, 49-node stays exhaustive, warning-
diagnostic-exists) are retained.

## 5. Findings

### 5.1 Findings fixed inline

- **Diagnostic visibility gap** -- `model_ltm_variables` accumulated a
  `CompilationDiagnostic::Warning` on auto-flip, but
  `model_all_diagnostics` never invoked `model_ltm_variables`.  Salsa's
  accumulator only propagates through transitively-called tracked
  functions, so the warning never reached `collect_all_diagnostics`
  (the exact API that both `libsimlin` -- via `patch.rs:gather_error_details_with_db`
  -- and `simlin-mcp` -- via `read_model.rs:64`, `edit_model.rs:247,279` --
  use to hand diagnostics to end users).  From the caller's
  perspective, auto-flip was silent: an LTM-enabled simulation of a
  large WRLD3-class model would finish without ever telling the user
  why every loop score was absent.

  Fixed in `e79cf3c9` by adding a one-line gated call to
  `model_ltm_variables` from inside `model_all_diagnostics` (guarded by
  `if project.ltm_enabled(db)` so LTM-disabled projects pay no
  synthesis cost).  Two new tests guard this:
  - `test_auto_flip_warning_surfaces_via_collect_model_diagnostics`
    (positive: 51-node SCC model with LTM enabled must emit the
    warning through the user-facing collector).
  - `test_ltm_disabled_does_not_surface_auto_flip_warning`
    (negative: 51-node SCC model with LTM disabled must *not* emit the
    warning, proving the gate works).

  The fix is small (17 lines of new code, including the doc comment
  update describing the new third diagnostic source) and verifiable by
  reverting the one-line call and re-running the surfacing test -- it
  would fail, which is exactly the invariant we want a test to pin.

### 5.2 Findings not fixed (none blocking)

No additional regressions found.  I looked specifically for:

- **Salsa invalidation cost of the new `collect_all_diagnostics` call**
  -- `model_ltm_variables` is already a salsa tracked function, so
  non-LTM-enabled projects short-circuit at the `if project.ltm_enabled(db)`
  check and LTM-enabled projects reuse the same cached result the later
  `compile_project_incremental` path would have computed anyway.  There
  is no double-work for typical MCP flows (diagnose-then-compile).

- **`MAX_LTM_SCC_NODES = 50` choice** -- the threshold is a strict `>`
  and is not user-tunable.  The diagnosis plan section 3.1 documents
  why 50 is safe (keeps all production test fixtures exhaustive; see
  `test_model_ltm_variables_stays_exhaustive_below_scc_threshold` at
  49 and the 40-element arrayed test).  Task #7 will revisit once
  `MAX_LTM_CIRCUITS` is raised.

- **Absence of an explicit bench at cap=50_000** (half the current
  cap) -- behaviourally identical to cap=100_000 since the auto-flip
  short-circuits the cap-gated Johnson enumeration entirely.  Skipping
  saves bench time without losing signal.

- **Rel-loop-score post-sim computation under empty results** --
  handled by `compute_rel_loop_scores` iterating an empty
  `results.iter()` (0 rows), producing zero-length Vecs.  Already
  implicitly exercised by the smoke path; not worth a dedicated test.

- **VDF work-in-progress branch** (`vdf-model-guided-mapping`) -- no
  interaction with LTM compilation; confirmed by reviewing the VDF
  module's salsa surface (none).

### 5.3 Findings filed for follow-up

None.  Every issue I found was trivially fixable inline, and the two
fixes are isolated, test-guarded, and reviewed.

## 6. Sign-off

**Option A (auto-flip on large SCCs) is safe to merge.**

- Gate keys on the element-level largest SCC, which correctly rejects
  WRLD3-scale models (166-node SCC) while keeping mid-sized arrayed and
  multi-subsystem models on the exhaustive path.
- Failure mode is conservative: on a false-positive (SCC > 50 but the
  cycle population is small enough to compile), the user loses per-loop
  scores but gets discovery-mode link scores and a clear diagnostic.
  The reverse failure -- a 17 GB allocation on a dense model -- is what
  Option A prevents.
- Diagnostic is now visible to the user through the same channels the
  rest of the compiler's warnings use.

**Option B (post-sim `compute_rel_loop_scores`) is safe to merge.**

- 7 unit tests + 128-case proptest + 45 end-to-end `simulate_ltm`
  fixtures all pass against the new post-sim path with no fixture
  updates; semantic behaviour is unchanged to within 1e-10 vs. the old
  compile-time SAFEDIV emission.
- WRLD3 equation-text footprint shrinks from the projected ~140 TB to
  324,274 bytes.
- SAFEDIV-0 behaviour (zero denominator -> zero result) and IEEE-754
  NaN propagation are both pinned by tests.

**Task #7 (cap raise) is unblocked.**

With Option A in place, the cap no longer determines whether WRLD3
compiles; it only determines whether the bench's instrumentation stage
6 shows a non-zero circuit count.  Task #7 can raise `MAX_LTM_CIRCUITS`
(the task describes raising or removing the cap) and the WRLD3 pipeline
will continue to succeed at ~470 MiB peak and ~1.6 s wall.  No further
Rust changes from validation are required.

I recommend task #7 additionally:

1. Remove the cap entirely (or set it to `usize::MAX`) rather than
   raising it to an arbitrary larger finite value -- Option A now
   supplies the actual backstop, and an arbitrary cap would become
   confusing vestigial state.
2. Run the bench at cap=usize::MAX on both WRLD3 and at least one
   mid-sized arrayed production model (e.g., the hero-culture model)
   before merging, to confirm the auto-flip threshold is not silently
   kicking in on the smaller models.
3. Keep the existing `MAX_LTM_SCC_NODES = 50` constant as-is; it is
   the right backstop for Cliff A regardless of what happens to the
   circuit cap.

-- adversary, 2026-04-18

## Resolution

Task #7 completed 2026-04-18: `MAX_LTM_CIRCUITS` plus its runtime
override (`set_max_ltm_circuits`, `default_max_ltm_circuits`) and the
three default-budget enumeration wrappers (`find_loops`,
`find_circuit_node_lists`, `find_indexed_circuits`) were removed from
`src/simlin-engine/src/ltm.rs`.  The three salsa call sites in
`db_analysis.rs` plus `detect_loops` and `ltm_full_bench.rs` now pass
`usize::MAX` to the `_with_limit` APIs.  `MAX_LTM_SCC_NODES = 50`
remains as-is per recommendation #3.  The recommendation-#2 re-run on
`wrld3-03.mdl`, `arrayed_population.stmx`, and `cross_element.stmx`
matches this table: 467 MiB / 1.6 s for WRLD3 (1,863,803 circuits),
10.5 MiB for each arrayed model, with auto-flip behaviour unchanged.
