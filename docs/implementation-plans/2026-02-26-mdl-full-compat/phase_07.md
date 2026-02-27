# MDL Full Compatibility -- Phase 7: C-LEARN Simulation and Full Test Enablement

**Goal:** Enable all SDEverywhere test models and add C-LEARN simulation validated against VDF reference data, confirming that phases 1-6 resolved all feature gaps.

**Architecture:** Three tracks: (1) uncomment all feature-dependent SDEverywhere models in `TEST_SDEVERYWHERE_MODELS` and remove `#[ignore]` annotations for tests that phases 1-6 should have fixed, (2) add a `simulates_clearn()` test that loads the C-LEARN MDL, simulates it, and validates against `Ref.vdf` using the existing `ensure_vdf_results()` infrastructure, (3) enable `test_clearn_equivalence()` in `mdl_equivalence.rs` and fix any remaining parser diffs. Models that are genuinely not simulatable (preprocessing files without `.dat` output) remain excluded with updated comments.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 7 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC1: SDEverywhere models simulate via MDL path
- **mdl-full-compat.AC1.1 Success:** All 47 sdeverywhere test models parse and convert to datamodel without errors
- **mdl-full-compat.AC1.2 Success:** All 47 sdeverywhere test models simulate with results matching expected output within tolerance (2e-3 absolute or 5e-6 relative)

### mdl-full-compat.AC2: C-LEARN model works end-to-end
- **mdl-full-compat.AC2.1 Success:** C-LEARN simulation completes without `not_simulatable` error
- **mdl-full-compat.AC2.2 Success:** C-LEARN simulation results match VDF reference data within cross-simulator tolerance (1% relative error)

Note: The design's Phase 7 "Done when" clause references `mdl-full-compat.AC2.3`, which does not exist in the acceptance criteria section (only AC2.1 and AC2.2 are defined). This is assumed to be a typo in the design. All actual AC2 criteria (AC2.1, AC2.2) are covered by this phase.

Note: The design references "47 sdeverywhere test models" but there are 42 model directories in `test/sdeverywhere/models/`. Of those, 6 entries are not standalone simulatable models (preprocessing files, nested model directories without `.dat` output). The actual count of models expected to pass simulation is **39** (14 already passing + 25 to be enabled by phases 1-6). This excludes `interleaved` (needs element-level dependency resolution), the `flatten` preprocessing files, `preprocess` files, and `sir/model/sir.xmile` duplicate.

---

## Reference Files

Test infrastructure:
- `/home/bpowers/src/simlin/src/simlin-engine/tests/simulate.rs` -- `TEST_SDEVERYWHERE_MODELS` (line 499), `ensure_vdf_results()` (line 129), `ensure_results()` (line 181), `simulates_wrld3_03()` (line 699, pattern for C-LEARN test), `simulates_except()` (line 458, `#[ignore]`), `simulates_except_interpreter_only()` (line 477, `#[ignore]`)
- `/home/bpowers/src/simlin/src/simlin-engine/tests/mdl_equivalence.rs` -- `test_clearn_equivalence()` (line 1003, `#[ignore]`), `collect_project_diffs()` (line 703)

C-LEARN model files:
- `/home/bpowers/src/simlin/test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` -- source model
- `/home/bpowers/src/simlin/test/xmutil_test_models/Ref.vdf` -- VDF reference data

VDF parser:
- `/home/bpowers/src/simlin/src/simlin-engine/src/vdf.rs` -- `VdfFile::parse()`, `to_results()` (empirical/correlation), `to_results_with_model()` (model-guided, preferred for C-LEARN)

SDEverywhere models without expected output (permanently excluded):
- `flatten/input1.xmile`, `flatten/input2.xmile` -- preprocessing test files, no `.dat`
- `preprocess/expected.xmile`, `preprocess/input.xmile` -- preprocessing test files, no `.dat`
- `sir/model/sir.xmile` -- nested model directory, no `.dat`

SDEverywhere models with expected output but currently commented out (to be enabled by phases 1-6):
- Phase 1 (tactical fixes): `directconst`
- Phase 3 (EXCEPT): `except`, `except2`, `longeqns`, `ref`
- Phase 4 (DataProvider): `arrays_cname`, `arrays_varname`, `directdata`, `directlookups`, `directsubs`, `extdata`, `prune` (uses external data file)
- Phase 5 (builtins): `delayfixed`, `delayfixed2`, `quantum`, `sample`, `npv`, `getdata`
- Phase 6 (array ops): `allocate`, `mapping`, `multimap`, `subscript`, `vector`, `sumif` (needs `If` expression support in array view codegen)
- Already passing: `sum` (standalone test passes; stale comment in `TEST_SDEVERYWHERE_MODELS`)
- Requires new work: `interleaved` (element-level dependency resolution in `model.rs`)

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: Enable SDEverywhere Test Models

<!-- START_TASK_1 -->
### Task 1: Enable all SDEverywhere models that phases 1-6 should have fixed

**Verifies:** mdl-full-compat.AC1.1, mdl-full-compat.AC1.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (lines 499-599, `TEST_SDEVERYWHERE_MODELS`)

**Implementation:**

Uncomment all SDEverywhere model entries in `TEST_SDEVERYWHERE_MODELS` that have `.dat` expected output files, EXCEPT the genuinely non-simulatable entries listed below.

Models to uncomment (move from commented section to passing section):
- `allocate/allocate.xmile`
- `arrays_cname/arrays_cname.xmile`
- `arrays_varname/arrays_varname.xmile`
- `delayfixed/delayfixed.xmile`
- `delayfixed2/delayfixed2.xmile`
- `directconst/directconst.xmile`
- `directdata/directdata.xmile`
- `directlookups/directlookups.xmile`
- `directsubs/directsubs.xmile`
- `except/except.xmile`
- `except2/except2.xmile`
- `extdata/extdata.xmile`
- `getdata/getdata.xmile`
- `longeqns/longeqns.xmile`
- `mapping/mapping.xmile`
- `multimap/multimap.xmile`
- `npv/npv.xmile`
- `prune/prune.xmile` (already simulates; was incorrectly labeled "No expected results" but `prune.dat` exists)
- `quantum/quantum.xmile`
- `ref/ref.xmile`
- `sample/sample.xmile`
- `subscript/subscript.xmile`
- `sum/sum.xmile` (standalone test `simulates_sum()` already passes; stale comment in `TEST_SDEVERYWHERE_MODELS`)
- `sumif/sumif.xmile` (requires Phase 6 fix: see note below)
- `vector/vector.xmile`

Models that require additional implementation work before enabling:
- `interleaved/interleaved.xmile` -- the `simulate.rs` comment says "EmptyEquation: uses INTEG with complex initialization" but the actual failure is `circular_dependency`. The model has per-element equations for `a[A1] = x` and `a[A2] = y` where `y = a[A1]`, creating a dependency cycle at the variable level (`a` depends on itself through `y`). Fixing this requires element-level dependency resolution in `model.rs` (resolving dependencies per array element rather than per whole variable). This is a compiler enhancement not covered by phases 1-6. Add as `#[ignore]` with updated comment explaining the element-level dependency issue, or implement the fix in Task 3.
- `sumif/sumif.xmile` -- the `simulate.rs` comment says "EmptyEquation: uses SUM OF builtin with condition" but the actual equation is `SUM(IF THEN ELSE(A_Values[*]=:NA:, 0, A_Values[*]))` -- standard SUM containing a conditional expression. The failure is that `walk_expr_as_view` in `codegen.rs:283` cannot create an array view from an `If` expression (Discriminant 12). This requires extending the codegen to handle `If` inside array iteration contexts. If Phase 6 addresses this as part of array operation compilation, it will work. Otherwise, add a targeted fix here.

Models that remain commented out (permanently excluded):
- `flatten/expected.xmile`, `flatten/input1.xmile`, `flatten/input2.xmile` -- SDEverywhere preprocessing test files; `expected.dat` contains only one variable and is not a full simulation output file
- `preprocess/expected.xmile`, `preprocess/input.xmile` -- SDEverywhere preprocessing test files, no `.dat`
- `sir/model/sir.xmile` -- nested model directory duplicate, no `.dat`

Update the comments on the remaining excluded models to explain they are permanently excluded (not feature-gap exclusions).

**Testing:**

Run the SDEverywhere model test. All uncommented models should parse, convert, and simulate with results matching their `.dat` files.

**Verification:**
Run: `cargo test --features file_io,ext_data --test simulate -- simulates_arrayed_models_correctly`
Expected: Test passes with all newly enabled models

**Commit:** `engine: enable all SDEverywhere models in simulation test suite`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove #[ignore] from except tests and clean up duplicate test functions

**Verifies:** mdl-full-compat.AC1.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (lines 456-480)

**Implementation:**

Remove the `#[ignore]` annotation from `simulates_except()` at line 457.

For `simulates_except_interpreter_only()` at line 477: this test exercises the interpreter-only path for except models. The comment at lines 472-475 explains the test was ignored because Vensim subscript mappings are not preserved in XMILE conversion. After phases 2-3 (dimension mappings + EXCEPT support), this should work via the MDL path. If it still fails via the XMILE path (because xmutil drops subscript mappings), update the comment to explain the XMILE-path limitation and keep `#[ignore]` with an updated explanation. If it passes, remove `#[ignore]`.

**Testing:**

Run the except tests specifically.

**Verification:**
Run: `cargo test --features file_io --test simulate -- except`
Expected: `simulates_except` passes. `simulates_except_interpreter_only` either passes or remains `#[ignore]` with updated comment.

**Commit:** `engine: remove #[ignore] from except simulation tests`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Run full SDEverywhere simulation suite and fix any failures

**Verifies:** mdl-full-compat.AC1.1, mdl-full-compat.AC1.2

**Files:**
- Possibly modify: `src/simlin-engine/tests/simulate.rs` (if any models need to be re-excluded with updated comments)
- Possibly modify: engine source files (if minor fixes are needed)

**Implementation:**

Run the full simulation test suite and address any failures:

1. If a model fails due to a feature that phases 1-6 were supposed to implement, investigate and fix the issue in the appropriate engine module.
2. If a model fails due to a genuinely unsupported feature not covered by phases 1-6, re-comment the model with an updated, specific comment explaining what feature is missing.
3. If a model fails with a tolerance issue, check whether the tolerance thresholds are appropriate for Vensim-converted models.

Known models requiring investigation:

**`interleaved`**: The `simulate.rs` comment says "EmptyEquation: uses INTEG with complex initialization" but investigation reveals the actual failure is `circular_dependency`. The model has per-element array equations (`a[A1] = x`, `a[A2] = y`, `y = a[A1]`) that create a variable-level dependency cycle because the dependency resolver treats `a` as a single unit. Fixing this requires element-level dependency resolution in `model.rs` -- resolving dependencies per array element rather than per whole variable. If the fix is tractable, implement it. If it requires significant compiler refactoring, keep the model `#[ignore]`d with an updated comment documenting the element-level dependency issue and file a tracking issue.

The goal is zero failures except for: permanently excluded preprocessing models, and any models requiring compiler enhancements documented with specific tracking issues.

**Testing:**

Full SDEverywhere model simulation.

**Verification:**
Run: `cargo test --features file_io,ext_data --test simulate -- simulates_arrayed_models_correctly`
Expected: All tests pass

Run: `cargo test --features file_io,ext_data --test simulate`
Expected: No regressions in any simulation test

**Commit:** `engine: fix remaining SDEverywhere model simulation issues`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
## Subcomponent B: C-LEARN Simulation

<!-- START_TASK_4 -->
### Task 4: Add C-LEARN simulation test

**Verifies:** mdl-full-compat.AC2.1, mdl-full-compat.AC2.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (add new test function after `simulates_wrld3_03()` at line 748)

**Implementation:**

Add a `simulates_clearn()` test function following the pattern of `simulates_wrld3_03()` (lines 699-748). The test should:

1. Load the C-LEARN MDL file from `../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`
2. Parse using `open_vensim_with_data()` (from Phase 4) with a `FilesystemDataProvider` pointing to the C-LEARN model directory, since C-LEARN uses external data files via GET DIRECT DATA
3. Create a `Simulation` and run with the interpreter (`run_to_end()`)
4. Compile and run with the VM, comparing interpreter vs VM results using `ensure_results()`
5. Load VDF reference data from `../../test/xmutil_test_models/Ref.vdf`
6. Parse VDF using `VdfFile::parse()`
7. Convert VDF to Results using `vdf_file.to_results_with_model(&datamodel_project, "main")` -- this uses the model-guided structural mapping which is more reliable for large models than the empirical correlation approach used by `simulates_wrld3_03()`
8. Compare using `ensure_vdf_results()` with 1% relative tolerance

The test must be gated on `#[cfg(feature = "file_io")]`.

If `to_results_with_model()` fails for C-LEARN (the model-guided mapping may not cover all variables in a model this large), fall back to `to_results()` (empirical correlation) as `simulates_wrld3_03()` does. Add a comment explaining the fallback.

**Testing:**

The test itself is the verification.

**Verification:**
Run: `cargo test --features file_io --test simulate -- simulates_clearn`
Expected: Test passes -- C-LEARN simulates and results match VDF within 1% tolerance

**Commit:** `engine: add C-LEARN simulation test with VDF validation`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Fix C-LEARN simulation issues

**Verifies:** mdl-full-compat.AC2.1, mdl-full-compat.AC2.2

**Files:**
- Possibly modify: various engine source files depending on what fails

**Implementation:**

If `simulates_clearn()` from Task 4 fails, diagnose and fix the issues. Common categories of C-LEARN failures (from the 26 equivalence diffs identified during design):

1. **Conversion errors**: Missing or incorrectly converted equations. Check `open_vensim()` output for error diagnostics.
2. **NotSimulatable errors**: Variables that the compiler cannot handle. Check which builtin or feature is missing.
3. **Simulation divergence**: Results outside tolerance. Compare time series to identify where divergence starts (often an integration issue or missing initial condition).

For each failure:
- If it's a bug in phases 1-6 implementation, fix it in the appropriate module
- If it's a new issue not covered by phases 1-6, implement the minimal fix
- If a few variables are outside tolerance but the overall model works, document which variables diverge and why (Vensim's integration may differ from Euler for certain equation patterns)

**Testing:**

The C-LEARN test itself validates the fix.

**Verification:**
Run: `cargo test --features file_io --test simulate -- simulates_clearn`
Expected: Test passes

**Commit:** `engine: fix C-LEARN simulation issues`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Enable C-LEARN equivalence test in mdl_equivalence.rs

**Verifies:** mdl-full-compat.AC2.1

**Files:**
- Modify: `src/simlin-engine/tests/mdl_equivalence.rs` (line 1003, remove `#[ignore]`)

**Implementation:**

Remove the `#[ignore]` annotation from `test_clearn_equivalence()` at line 1003. This test compares the native Rust MDL parser output against xmutil's output using `collect_project_diffs()`.

After phases 1-6, most equivalence diffs should be resolved. If some remain:
1. Diffs caused by intentional design differences (e.g., the native parser normalizes differently than xmutil) should be documented with comments in the test or handled by the `normalize_project()` function
2. Diffs caused by bugs should be fixed in the MDL converter
3. If xmutil produces incorrect output (known issue for some Vensim features), the test should be updated to accept the native parser's output as correct

If the test has too many remaining diffs to fix in this phase, keep it `#[ignore]` with an updated comment listing the specific remaining diff categories and a count.

**Testing:**

Run the equivalence test.

**Verification:**
Run: `cargo test --features file_io,xmutil -p simlin-engine --test mdl_equivalence -- test_clearn_equivalence`
Expected: Test passes or remains `#[ignore]` with specific documented reasons

**Commit:** `engine: enable C-LEARN MDL equivalence test`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-8) -->
## Subcomponent C: Final Validation

<!-- START_TASK_7 -->
### Task 7: Enable remaining mdl_equivalence models

**Verifies:** mdl-full-compat.AC1.1

**Files:**
- Modify: `src/simlin-engine/tests/mdl_equivalence.rs` (commented-out entries in `EQUIVALENT_MODELS`)

**Implementation:**

Review the commented-out entries in `EQUIVALENT_MODELS` (around lines 55-108). Some models are disabled due to xmutil segfaults (`smooth3`, `trend`) and others due to feature gaps.

For models disabled due to xmutil segfaults: these cannot be enabled since xmutil is the comparison target. Keep them commented with the existing explanation.

For models disabled due to feature gaps that phases 1-6 should have fixed: try enabling them. If the equivalence test passes, keep them enabled. If it fails due to intentional normalization differences between native and xmutil parsers, update the normalization logic or document the difference.

**Testing:**

Run the full equivalence test suite.

**Verification:**
Run: `cargo test --features file_io -p simlin-engine --test mdl_equivalence`
Expected: All tests pass (models disabled due to xmutil bugs remain excluded)

**Commit:** `engine: enable remaining mdl_equivalence models`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Full test suite verification

**Verifies:** mdl-full-compat.AC1.1, mdl-full-compat.AC1.2, mdl-full-compat.AC2.1, mdl-full-compat.AC2.2

**Files:** None (verification only)

**Implementation:**

Run the complete test suite to verify no regressions:

```bash
cargo test -p simlin-engine
cargo test --features file_io,ext_data --test simulate
cargo test --features file_io -p simlin-engine --test mdl_equivalence
cargo test --features file_io -p simlin-engine --test mdl_roundtrip
```

Note: `ext_data` feature flag (introduced in Phase 4) is required for models that use external data files (directdata, extdata, etc.). If Phase 4 makes `file_io` imply `ext_data` in Cargo.toml, then `--features file_io` alone suffices.

Verify:
- All SDEverywhere models with expected output pass simulation
- C-LEARN simulation completes and matches VDF reference within 1% tolerance
- MDL equivalence tests pass (except models excluded due to xmutil segfaults)
- MDL roundtrip tests pass
- No regressions in any existing tests

No commit (verification only).
<!-- END_TASK_8 -->
<!-- END_SUBCOMPONENT_C -->
