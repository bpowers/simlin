# Element-Level Cycle Resolution — Phase 6 Implementation Plan

**Goal:** C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`)
compiles via the incremental path and runs to FINAL TIME via the VM with no
panic and no all-NaN core series; issue #363 (incremental-compiler panic on
C-LEARN) is re-verified now that the cycle gate no longer masks the deeper
pipeline; the residual test-only `catch_unwind` is retired. (This is the
explicit mid-plan value-locking checkpoint.)

**Architecture:** Add a new `#[ignore]`d structural-gate test in
`tests/simulate.rs` that parses C-LEARN, compiles it via the incremental path
(`compile_vm`), asserts a clean `Result` (no panic), runs the VM to FINAL
TIME, and asserts no core series is entirely NaN. Re-point the existing
`clearn_ltm_discovery_blocked_by_macro_expansion` test (whose contract is
currently *inverted* — a clean compile is a `panic!`) to expect a clean
compile and remove its `catch_unwind`. If a post-gate panic surfaces, it is a
hard, root-caused failure (AC7.5 / #363's prescribed fix), never caught.

**Tech Stack:** Rust (`simlin-engine`) integration tests; the incremental
salsa compile path; `gh` for issue #363 status.

**Scope:** Phase 6 of 7. **Depends on Phases 2, 3, 5** (C-LEARN needs the
multi-variable + init combined SCC, the synthetic-helper safety net, and
correct VECTOR ops to avoid NaN propagation).

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`).

---

## Design deviations (verified — these override the design doc)

1. **`compile_vm` IS the incremental path** (`tests/simulate.rs:103-111`:
   `SimlinDb::default()` → `sync_from_datamodel_incremental` →
   `compile_project_incremental(&db, sync.project, "main").unwrap()`). The
   monolithic path was deleted (#375). `simulates_clearn`
   (`tests/simulate.rs:949`) calls `compile_vm` at `simulate.rs:960`. AC7.1
   "compiles via the incremental path" is correct. The "monolithic vs
   incremental" question is resolved — there is only the incremental path.
2. **AC7.4 is more invasive than "retire catch_unwind."**
   `clearn_ltm_discovery_blocked_by_macro_expansion`
   (`tests/ltm_discovery_large_models.rs:624-693`) currently asserts the
   **inverse** of the goal: `Ok(Ok(()))` (clean compile) ⇒
   `panic!("C-LEARN unexpectedly compiled...")`; `Ok(Err(_))`/`Err(_)` ⇒ pass.
   Re-pointing requires: invert the `match` arms (a clean compile must
   *pass*), remove the `catch_unwind` at `:670`, rewrite the stale docstring
   (`:624-647`, "blocked by macro expansion GH #349" — macros are complete),
   update the stale `CLEARN_MDL` const docstring (`:127-130`), and rename the
   function (its name encodes a no-longer-true premise). It runs with **LTM
   discovery enabled** (`set_project_ltm_enabled(true)` +
   `set_project_ltm_discovery_mode(true)`, `:673-674`) — a heavier config than
   the new plain-`compile_vm` structural test.
3. **`catch_unwind` at `ltm_discovery_large_models.rs:670` is the ONLY one in
   the engine test suite.** #363's other historically-listed sites
   (`benches/compiler.rs`, `src/analysis.rs`, `src/layout/mod.rs`) were
   already removed on this branch; `db.rs:106/115` are comments, not calls.
   AC7.4 is satisfied by deleting the single line-670 wrapper.
4. **Issue #363 is OPEN on GitHub** (no `tech-debt.md` duplicate). "The cycle
   gate masks #363" is the **design's thesis, not a codebase-recorded fact** —
   AC7.2/AC7.5 are a genuine re-verification; #363's panic may or may not
   still reproduce once the gate passes. #363's prescribed fix: capture the
   panic backtrace under a debug build, convert the panic site(s) to return
   `Result::Err`, then remove the `catch_unwind`.
5. **There is NO `Results` series accessor and NO "core C-LEARN series"
   enumeration anywhere.** `Results` (`results.rs:75-84`) exposes flat
   `data: Box<[f64]>` + `offsets` + `iter()`; the read idiom is
   `data[step*step_size+offsets[ident]]` (see `macro_test_value_at`,
   `tests/simulate.rs:1827-1841`). The plan must **define** "core series": use
   the matched set `Ref.vdf.offsets ∩ results.offsets` and assert each matched
   series is not entirely NaN (this also dovetails with Phase 7's AC8.2 NaN
   guard).
6. **The new structural-gate test MUST be `#[ignore]`d** with a
   `// Run with: cargo test --release -- --ignored <name>` comment (C-LEARN
   parse alone ~4-5s release / longer debug; full compile+run far more; the
   3-minute `cargo test` cap forbids it in the default set). All four sibling
   C-LEARN tests follow this convention. The closest skeleton is
   `simulates_wrld3_03` (`tests/simulate.rs:873-910`) — same
   read→`open_vensim`→`compile_vm`→`Vm::new`→`run_to_end`→`into_results`
   shape — but it is NOT `#[ignore]`d and asserts only VDF structural
   properties; the new test is `#[ignore]`d and adds the not-all-NaN check.

---

## Acceptance Criteria Coverage

### element-cycle-resolution.AC7: C-LEARN structural gate + #363
- **element-cycle-resolution.AC7.1 Success:** C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) compiles via the incremental path with no fatal `ModelError` (no `circular_dependency`; non-fatal unit-inference warnings allowed).
- **element-cycle-resolution.AC7.2 Success:** The C-LEARN VM runs to FINAL TIME with no panic.
- **element-cycle-resolution.AC7.3 Success:** No core C-LEARN series is entirely NaN after the run.
- **element-cycle-resolution.AC7.4 Success:** The residual test-only `catch_unwind` for C-LEARN (`tests/ltm_discovery_large_models.rs:670`) is removed and its `clearn_*` test expects a clean compile result.
- **element-cycle-resolution.AC7.5 Failure:** If a post-gate panic surfaces, it is a hard test failure (root-caused), not caught/ignored.

---

## Testing conventions

Heavy C-LEARN tests are `#[test] #[ignore]` with a `// Run with: cargo test
--release -- --ignored <name>` comment (per `docs/dev/rust.md:38-47`); they
are run explicitly, not in the capped default `cargo test` set, so they do
NOT count against the 180s pre-commit cap. The pre-commit hook still runs
(fmt/clippy/non-ignored tests) on every commit — never `--no-verify`. Run the
new/changed C-LEARN tests explicitly with `--release -- --ignored` to verify.

---

<!-- START_TASK_1 -->
### Task 1: New C-LEARN incremental structural-gate test

**Verifies:** element-cycle-resolution.AC7.1, element-cycle-resolution.AC7.2, element-cycle-resolution.AC7.3, element-cycle-resolution.AC7.5

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` — add a new `#[test] #[ignore]` function (e.g. `compiles_and_runs_clearn_structural`), modeled on `simulates_wrld3_03` (`tests/simulate.rs:873-910`).

**Implementation:**
- Add the `// Run with: cargo test --release -- --ignored compiles_and_runs_clearn_structural` comment directly above the attributes (matching the `simulates_clearn` convention at `tests/simulate.rs:946`).
- Body: `std::fs::read_to_string("../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl")` → `open_vensim` → compile via the incremental path.
  - **AC7.1:** assert the compile `Result` is `Ok` — **no fatal `ModelError`,
    specifically no `circular_dependency`**. Non-fatal unit-inference warnings
    are explicitly allowed (out of scope). Compile by calling
    `compile_project_incremental` directly (not the `compile_vm` `.unwrap()`
    wrapper) so the `Result` can be asserted clean rather than panicking; if a
    diagnostic is present, fail with the collected diagnostics in the message
    (mirror how `corpus_clearn_macros_import` inspects collected diagnostics)
    so a regression is legible.
  - **AC7.2:** `Vm::new(compiled).unwrap()` then `vm.run_to_end().unwrap()`
    (runs to FINAL TIME) — no panic. Do **NOT** wrap in `catch_unwind`
    (AC7.5: a post-gate panic must be a hard, root-caused failure — let it
    propagate as a test failure with backtrace).
  - **AC7.3:** `let results = vm.into_results();` then define "core series"
    as `Ref.vdf.offsets ∩ results.offsets`: parse
    `../../test/xmutil_test_models/Ref.vdf` via
    `simlin_engine::vdf::VdfFile::parse(...).to_results_via_records()` (as
    `simulates_clearn` does, `tests/simulate.rs:967-974`), intersect the
    offset keysets, and assert that **for each matched ident, at least one
    step is non-NaN** (`(0..step_count).any(|s| !data[s*step_size+off].is_nan())`,
    using the `macro_test_value_at`/flat-index idiom). Fail listing any
    entirely-NaN matched idents. (This intentionally dovetails Phase 7's
    AC8.2 NaN guard.)
- This is the **structural value-locking checkpoint**: it locks "C-LEARN
  compiles + runs + not-all-NaN" before Phase 7's numeric tail.

**Testing:**
The test IS the AC7.1/7.2/7.3/7.5 verification. It must be runnable and
**pass** after Phases 2/3/5 are in (the cycle gate resolved, VECTOR ops
correct). If it surfaces a post-gate panic, that is #363 reproducing — Task 3
addresses it; do not mask it.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
— passes (clean compile, runs to FINAL TIME, no all-NaN core series).
Then `git commit` (pre-commit; the new test is `#[ignore]`d so it does not
run in the capped default set, but fmt/clippy/non-ignored tests must be
green).
**Commit:** `engine: C-LEARN incremental structural-gate test (AC7.1-7.3, AC7.5)`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Re-point `clearn_ltm_discovery_*` and retire the `catch_unwind`

**Verifies:** element-cycle-resolution.AC7.4

**Files:**
- Modify: `src/simlin-engine/tests/ltm_discovery_large_models.rs:624-693` (the test, its docstring, remove `catch_unwind` at `:670`, rename the fn) and `:127-131` (the stale `CLEARN_MDL` const docstring)

**Implementation:**
- Remove the `std::panic::catch_unwind(move || { ... })` wrapper at
  `ltm_discovery_large_models.rs:670` — call the compile directly so a panic
  is a hard test failure (AC7.5).
- Invert the contract: the test now **expects a clean compile**. With LTM
  discovery enabled (`set_project_ltm_enabled(true)` +
  `set_project_ltm_discovery_mode(true)`, `:673-674` — keep this config; it is
  the test's identity), assert `compile_project_incremental(...)` returns
  `Ok`. The old `Ok(Ok(())) ⇒ panic!("unexpectedly compiled")` arm becomes
  the success path.
- Rename the function away from the now-false premise (e.g.
  `clearn_ltm_discovery_compiles` — it no longer is "blocked_by_macro_expansion").
- Rewrite the test docstring (`:624-647`) and the `CLEARN_MDL` const
  docstring (`:127-130`): drop the stale "blocked by macro expansion GH #349"
  framing (macros are complete per `corpus_clearn_macros_import`); state that
  C-LEARN compiles via the incremental path with LTM discovery enabled, and
  reference #363 as re-verified (Task 3).
- Keep `#[test] #[ignore]` and its `// Run with: ...` comment (C-LEARN is
  heavy; runtime-class). Scope note: AC7.4 requires only "expects a clean
  compile result" — expanding to full discovery coverage
  (`discover_loops_with_graph` tractability/structural-sanity assertions, as
  the old panic message suggested) is **not** required by AC7.4; if a clean
  one-line `Ok` assertion is insufficient or such expansion is desired,
  file/track it via `track-issue` rather than scope-creeping Phase 6.

**Testing:**
Run the renamed test explicitly: it must pass (clean compile, no
`catch_unwind`, no panic). If it panics, that is #363 → Task 3 (do not
re-add `catch_unwind`).

**Verification:**
Run: `cargo test -p simlin-engine --features xmutil --release -- --ignored clearn_ltm_discovery --nocapture` — passes.
Confirm no `catch_unwind` remains in `src/simlin-engine/tests/`
(`rg catch_unwind src/simlin-engine/tests` ⇒ no hits).
**Commit:** `engine: re-point clearn LTM-discovery test to clean compile; retire catch_unwind (AC7.4)`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3 (EXPANDED — element-level lagged-read strip; then re-verify #363)

**Verifies:** element-cycle-resolution.AC7.1, element-cycle-resolution.AC7.2, element-cycle-resolution.AC7.5

**Why expanded (Task 1 outcome (b), verified):** Task 1's structural gate
surfaced a genuine **Phase-1-3 cycle-resolution gap** (NOT a regression, NOT
numeric, NOT yet #363) blocking the C-LEARN compile: `compile_project_incremental`
returns `Err` with 2 `CircularDependency` diagnostics on
`main.previous_emissions_intensity_vs_refyr`, a member of a 22-member
multi-variable recurrence SCC. `symbolic_phase_element_order` returns `None`
via the `self_loop` branch because the induced element graph has 105
element-self-loops, **every one a `SymLoadPrev` (PREVIOUS) read, never a
`LoadVar`**. Mechanism: C-LEARN's `SAMPLE IF TRUE(cond,input,init)` expands
(`xmile_compat.rs:359`) to `(IF cond THEN input ELSE PREVIOUS(SELF,init))` — a
PREVIOUS-wrapped same-element self-reference. `build_var_info` correctly
strips the *whole-variable* PREVIOUS self-edge (so it is not a `dt_cycle_sccs`
self-loop), but the 22-member SCC is still legitimately identified via the
*un-lagged* cross-element chain, so `symbolic_phase_element_order` IS reached,
and its read-opcode arm (`db_dep_graph.rs:814-821`) lumps `SymLoadPrev` /
`SymLoadInitial` in with current-value reads — over-collecting the lagged
self-read into a spurious element-self-loop ⇒ false `CircularDependency`. The
Phase-1 "loud-safe over-approximation" rustdoc (`db_dep_graph.rs:717-733`)
deemed this acceptable because it "only forces a conservative
`CircularDependency`" — but for C-LEARN's legitimately-identified
multi-variable SCC containing a PREVIOUS-self-ref it is **over-conservative
and blocks the plan's payoff**.

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` — `symbolic_phase_element_order`
  read-opcode arm (`~814-821`); the `phase_element_order` PREVIOUS-safety
  rustdoc (`~717-733`); the `db.rs` `var_phase_symbolic_fragment_prod`
  loud-safe contract comment (`~3260-3274`) — all in the same commit
  (CLAUDE.md comment-freshness).
- Add: a minimized `#[cfg(test)]` fixture/test in `db_dep_graph_tests.rs`.
- (Conditional, the original Task 3) Modify whichever incremental-pipeline
  source panics on C-LEARN *post-gate* (convert to typed `Result::Err`).

**Implementation — Part A (the element-level lagged-read strip, the C-LEARN
blocker fix):**
Make the symbolic element graph **inherit `build_var_info`'s per-phase
PREVIOUS/INIT strip** (the element-level analogue of the variable-level strip
at `db_dep_graph.rs:261-264` / `:283-287`). The element graph models
*current-(phase-)timestep evaluation order*; a lagged/snapshot read is not a
current-timestep ordering edge. Mirror the variable-level strip **exactly and
phase-awarely** (verify the precise opcode↔strip correspondence against
`build_var_info` before coding — do not assume):
- **`SymbolicOpcode::SymLoadPrev`** (PREVIOUS — `prev_values` snapshot, prior
  timestep): never contributes an element-graph edge, in **either** phase
  (element-level analogue of `lagged_dt_previous` / `lagged_initial_previous`,
  both stripped).
- **`SymbolicOpcode::SymLoadInitial`** (INIT — `initial_values` snapshot):
  **phase-aware** — in the **`SccPhase::Dt`** graph it contributes **no** edge
  (analogue of `init_only_dt` / `dt_init_only_referenced_vars` being stripped
  from `dt_deps`); in the **`SccPhase::Initial`** graph it **DOES** contribute
  an edge (INIT(x) during init is a genuine init-phase dependency — `build_var_info`
  strips ONLY `lagged_initial_previous` from `initial_deps`, NOT INIT-refs).
  Confirm this exact asymmetry against `build_var_info` and document it.
- `LoadVar` / `LoadSubscript` / `PushVarView` / `PushVarViewDirect` /
  `PushStaticView`(Var base): unchanged — these are the current-value reads a
  genuine cycle is made of.
- **AC4 soundness argument (must be airtight, stated in the rustdoc):** a
  genuine current-timestep element cycle is a cycle of *current-value* reads;
  `SymLoadPrev`/`SymLoadInitial`(dt) read a prior/initial snapshot, never the
  current timestep's value, so excluding them **cannot drop a genuine-cycle
  edge** (it can only remove a spurious lagged edge). This is the *correct*
  element relation, not a new over/under-approximation: it makes the element
  graph match the engine's actual per-phase relation (`build_var_info`),
  exactly as Phase 1/2 made the SCC relation match the engine's.
- Rewrite the `db_dep_graph.rs:717-733` rustdoc and the `db.rs:3260-3274`
  loud-safe contract: the prior "PREVIOUS is over-collected; loud-safe because
  it only forces conservative `CircularDependency`" claim is **superseded** —
  state that the element graph now inherits `build_var_info`'s per-phase
  PREVIOUS/INIT strip (cite the exact `build_var_info` lines), why that is the
  correct relation, and the AC4 argument.

**Implementation — Part B (re-verify #363 post-gate):** with Part A landed and
the gate passing, run C-LEARN through the incremental pipeline (Task 1's test)
under a **debug** build (`RUST_BACKTRACE=1`). If **no panic** reproduces: #363
was masked-or-already-fixed; record the re-verification via the `track-issue`
agent (comment on #363; do not close unless the user directs). If **a panic
reproduces**: it is now a hard failure (AC7.5) — root-cause it, convert the
panic site to a typed `Result::Err` through `NotSimulatable`/the diagnostic
path, add a focused unit test at the converted site.

**Testing (TDD, mandatory):**
- RED-first: a minimized `#[cfg(test)]` fixture in `db_dep_graph_tests.rs` —
  a multi-variable SCC where one member is `SAMPLE IF TRUE`-shaped (i.e.
  `x[tNext] = IF c THEN y[tPrev] ELSE PREVIOUS(x[tNext], init)` with `y`
  closing the cluster, the C-LEARN shape minimized) — currently RED with a
  spurious `CircularDependency`; GREEN after Part A. Cover both a dt-phase and
  (if constructible) an init-phase variant to exercise the `SymLoadInitial`
  phase-asymmetry.
- **MANDATORY soundness pins (must stay GREEN unchanged — prove the strip
  does not mask a genuine cycle):** `genuine_cycles_still_rejected`,
  `resolve_dt_genuine_element_two_cycle_is_unresolved`,
  `resolve_dt_scalar_two_cycle_is_unresolved`, the `x[dimA]=x[dimA]+1`
  same-element self-cycle (AC4.2), `self_recurrence_resolves_and_no_self_token_leak`,
  `previous_self_reference_still_resolves`, the `ref`/`interleaved`/
  `init_recurrence`/`helper_recurrence` end-to-end gates, and the full
  `db_dep_graph` suite. If any requires modification to stay green, the strip
  is unsound — STOP and report (do not edit a guard to pass).
- Part B's integration proof is Task 1's `compiles_and_runs_clearn_structural`
  (left uncommitted in the working tree by the Task 1 dispatch — do **not**
  commit `simulate.rs` in this Task; use it to verify Part A makes C-LEARN
  compile+run+not-all-NaN, then the orchestrator sequences the Task 1 commit).

**Verification:**
Run the minimized fixture (RED→GREEN) + the full soundness-pin set in the
default suite. Then `cargo test -p simlin-engine --features file_io --release
-- --ignored compiles_and_runs_clearn_structural --nocapture` — C-LEARN now
compiles, runs to FINAL TIME, no all-NaN core series (Part A), no post-gate
panic (Part B; or the panic is converted to a typed `Err`). Commit only the
engine fix + unit fixture + rustdoc updates (NOT `simulate.rs`) via
`git commit` (pre-commit fmt/clippy/non-ignored cargo test 180s cap; NEVER
`--no-verify`).
**Commit:** `engine: element-level lagged-read strip in symbolic SCC graph; re-verify #363 (AC7.1, AC7.2, AC7.5)`
<!-- END_TASK_3 -->

---

## Phase 6 Done When

- C-LEARN compiles via the incremental path with no fatal `ModelError` (no
  `circular_dependency`; unit-inference warnings allowed), runs to FINAL TIME
  with no panic, and no core series (matched `Ref.vdf ∩ results`) is entirely
  NaN (Task 1 — AC7.1, AC7.2, AC7.3).
- The `catch_unwind` at `ltm_discovery_large_models.rs:670` is removed; the
  renamed `clearn_*` test expects a clean compile result; no `catch_unwind`
  remains in the engine test suite for C-LEARN (Task 2 — AC7.4).
- Any post-gate panic is root-caused and converted to a typed error (not
  caught); #363 status re-verified and recorded (Task 3 — AC7.5).
- The default engine suite stays green under the 3-minute `cargo test` cap
  (the new C-LEARN test is `#[ignore]`d / runtime-class).
<!-- END_PHASE_6 -->
