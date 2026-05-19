# Element-Level Cycle Resolution — Phase 6 Implementation Plan

**Goal:** C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`)
compiles via the incremental path and runs to FINAL TIME via the VM with no
panic and no all-NaN core series; issue #363 (incremental-compiler panic on
C-LEARN) is re-verified now that the cycle gate no longer masks the deeper
pipeline; the residual test-only `catch_unwind` is retired. (This is the
explicit mid-plan value-locking checkpoint.) **Tasks 4-5 (added during
execution — see "Why Tasks 4-5 added"):** clearing the cycle gate (Task 3)
unmasked **two pre-existing, latent compile bugs orthogonal to cycle
resolution** (filed as GH #580) that block AC7.1 — fixed here as general engine
fixes (no model-specific hacks) so Task 1's structural gate can be committed
passing; the user was surfaced the corrected #575-class scope picture and chose
"drive both fixes now". **Task 6 (added after Tasks 4-5):** fixing Bug B then
unmasked a 3rd distinct, orthogonal pre-existing layer (GH #582 — a
`GraphicalFunctionId` overflow from missing cross-fragment GF de-duplication in
`concatenate_fragments`) that still blocks AC7.1; fixed in Task 6 under the
user's follow-on "drive #582, checkpoint per layer" directive (each further
unmasked layer is surfaced to the user before being driven). **Task 7 (added
after a user-authorized forward sweep):** the sweep established that #583 (the
`TempId` overflow Task 6 unmasked) is the *sole* remaining assembly-path ceiling
(only two `u8` index types exist; everything else is already `u16`) and is a
#582-class concat-vs-monolithic **divergence**, NOT genuine-capacity — the
monolithic path recycles temps to ~21 while the incremental path sums to 243/3233
and widening produces a runtime OOB. Task 7 fixes it by matching the monolithic
recycle (not widening). **Tasks 8-9 (AC7.3 NaN layer):** with C-LEARN
compiling+running (Tasks 4-7; AC7.1/7.2 met), AC7.3 (no all-NaN core series)
fails on two SPURIOUS engine bugs (Ref.vdf is finite everywhere Simlin is
NaN/inf) — a macro-`INITIAL()` module output omitted from the initials runlist
(Task 8, Cluster A) and arrayed `VECTOR SORT ORDER` returning flat global
indices instead of per-iterated-slice 0-based ranks (Task 9, Cluster B,
completing the AC5 multi-row case Phase 4 missed) — fixed as general engine fixes.
**Task 10 (root-cause `:NA:` fix):** the 3rd, deepest NaN cause — the user
corrected (domain-authoritative) that Vensim `:NA:` is a **finite sentinel
`-2^109`, NOT NaN**; Simlin's `:NA:`→IEEE-NaN (`xmile_compat.rs:109`/
`parser/mod.rs:529`) is the engine bug behind the residual all-NaN cascade.
Task 10 represents `:NA:` as `-2^109` (clearing AC7.3 — the series become finite,
existence tests gate correctly), keeping the structurally-distinct OOB→NaN /
empty-reducer→NaN paths untouched; the `-2^109`↔`0` VDF reconciliation for
AC8.1's 1% match is Phase 7.

**Architecture:** Add a new `#[ignore]`d structural-gate test in
`tests/simulate.rs` that parses C-LEARN, compiles it via the incremental path
(`compile_vm`), asserts a clean `Result` (no panic), runs the VM to FINAL
TIME, and asserts no core series is entirely NaN. Re-point the existing
`clearn_ltm_discovery_blocked_by_macro_expansion` test (whose contract is
currently *inverted* — a clean compile is a `panic!`) to expect a clean
compile and remove its `catch_unwind`. If a post-gate panic surfaces, it is a
hard, root-caused failure (AC7.5 / #363's prescribed fix), never caught.
Tasks 4-5 fix the two #580 latent compile bugs that the cleared gate exposes:
a new additive `DimensionsContext` group-mapping resolver wired into
`substitute_dimension_refs` (Bug A — temp-arg-helper extraction) and an
arrayed-GF dependency-stub reconstruction in the isolated per-variable recompile
(Bug B), each with its own fast model-agnostic unit test, the shared
`dimensions.rs` mapping surface and the SCC element-graph compile path
regression-pinned.

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

## Why Tasks 4-5 added (Task 3 / GH #580 outcome — verified, user-approved to drive)

Task 3 Part A fully dissolved C-LEARN's false `CircularDependency`
(`has_circular_dependency = false`; `model_dependency_graph: has_cycle=false`;
the 22-member recurrence SCC resolves in both the dt and init phases) and Part
B re-verified #363 (the incremental-compiler panic does **not** reproduce — a
clean typed `Err`, recorded on #363, not closed). With the cycle gate clean,
the C-LEARN incremental compile now reaches assembly and surfaces a **distinct,
previously-masked** failure: `compile_project_incremental` returns a clean
`Err(NotSimulatable, "failed to compile fragments for variables: ...")` on ≈150
names. A deep root-cause investigation (filed as **GH #580**) established this
is **two independent, pre-existing, latent bugs in the per-variable /
per-helper isolated-recompile machinery — NOT a Task 3 regression, NOT caused
by the resolved SCC**: all 150 failing names are *outside* every resolved SCC,
and the Phase-2 combined-fragment path is never reached for any of them. They
were masked because the false `CircularDependency` short-circuited
`compile_project_incremental` *before* `assemble_module`'s fragment loop ever
ran — so the design's "cycle gate masks the deeper pipeline" thesis held, but
the deeper failure is these two bugs, not #363's panic.

They block the plan's committed **AC7.1** (C-LEARN compiles `Ok`) and therefore
AC7.2/AC7.3/AC8.1. The user was surfaced the corrected picture (a #575-class
decision — these are orthogonal pre-existing bugs, not the re-architecture) and
**chose "Drive both fixes now"**: fix them as general engine fixes
(root-cause-first, RED-TDD, no model-specific hacks, per-phase code review),
then commit Task 1's structural gate passing. Each fix carries its own fast,
model-agnostic unit test so coverage does not depend on the heavy `#[ignore]`d
C-LEARN test (mirroring phase_07 Task 4's "any general bug found gets a focused
unit test" rule). Tasks 4-5 **unblock** AC7.1's test
(`compiles_and_runs_clearn_structural`); they are newly-surfaced prerequisites,
not design ACs — test-requirements.md's AC7 row is reconciled at finalization
with a note (exactly as AC2.4/AC3.1's empirically-built fixtures are).

The two bugs (verified, concrete trace in #580):
- **Bug A — ≈140 synthetic temp-arg helpers** (`$⁚<var>⁚0⁚arg0⁚<element>`):
  when `make_temp_arg` lifts a per-element scalar helper out of an
  `INITIAL(...)`-wrapped **A2A** parent (`FF change start year[COP] =
  INITIAL(... ff_change_start_year_aggregated[Aggregated Regions] ...)`),
  `substitute_dimension_refs` (`builtins_visitor.rs:298-403`) fails to
  translate a **group-mapped** cross-dimension subscript: it tries
  `translate_via_mapping` (`:345`) and `find_mapping_parent_of` +
  `translate_to_source_via_mapping` (`:354-365`), but **both bail to `None` on
  a group / unequal-cardinality mapping** (`translate_to_source_via_mapping`
  `dimensions.rs:455-457` and `translate_via_mapping` `:513-515` both
  `return None` when `source_named.elements.len() != target_named.elements.len()`,
  and a group mapping `Aggregated Regions[Developing B] → COP[{3-element
  subgroup}]` has no 1:1 `element_map` row for a base COP element inside a
  subgroup target). So `substitute_dimension_refs` returns the expr
  **unchanged** (`:368`), the bare `[aggregated_regions]` full-dimension
  subscript survives into a **scalar** `Aux`, and `lower_variable` → `lower_ast`
  returns `EquationError{code: DimensionInScalarContext}`, which `lower_variable`
  (`model.rs`) records into `errors` and **discards the AST** (sets `ast`/
  `init_ast` to `None`). `lower_implicit_var` (`db.rs:3878`) **only checks the
  *pre*-lowering `equation_errors()` (`:3908-3913`), never the
  post-`lower_variable` ones**, so it returns `Some((name, var_with_None_ast))`;
  `compile_implicit_var_phase_bytecodes` → `compiler::Var::new` then trips
  `EmptyEquation` → all-`None` bytecodes → `missing_vars`. The originating
  `DimensionInScalarContext` is **silently swallowed** (no
  `CompilationDiagnostic`, unlike the real-var path's
  `accumulate_var_compile_error` at `db.rs:3782`).
- **Bug B — 6 real arrayed vars** (`global_rs_co2_ff`, `rs_global_ch4/n2o/pfc/sf6`,
  `rs_ff_co2_ff_aggregated`): the var lowers to a valid non-empty `Expr` AST,
  but the **isolated minimal-`Module` recompile** fails. `global_rs_co2_ff` &
  the four `rs_global_*` are `SUM(RS X[COP!](Time/One year))` — a `SUM` reducer
  over an **arrayed graphical function applied with a dynamic argument** —
  failing with `Err(Generic, "Cannot push view for expression type ... expected
  array expression")`; `rs_ff_co2_ff_aggregated` is a `VECTOR SELECT(...)`
  failing with `Err(BadTable, "range subscripts not supported in lookup
  tables")`. The dependency-stub / dep-table construction in
  `db_var_fragment.rs` (`build_stub_variable` `:344`; dep-table collection via
  `extract_tables_from_source_var` `:946-951`) does not reconstruct the
  arrayed-GF dependency as a full array-shaped, per-element-table-bearing stub,
  so `RS X[COP!](x)` does not lower to an array view in the mini-layout the way
  it does in the whole-model compile. (`compile_phase_to_per_var_bytecodes`
  `db.rs:3114` shares the same `Var::new` + minimal-`Module` shape — Task 5
  root-cause-confirms the exact failing call site before fixing, since the 6
  vars are outside every resolved SCC.)

---

<!-- START_TASK_4 -->
### Task 4: Bug A — group-mapped subscript translation in temp-arg-helper extraction

**Verifies:** unblocks element-cycle-resolution.AC7.1 (newly-surfaced
prerequisite — see "Why Tasks 4-5 added"); directly verified by its own focused
unit tests.

> **AS-BUILT root-cause correction (verified during execution, commit
> `b25dc06d` — supersedes the diagnosis below).** The root cause prescribed
> below (`substitute_dimension_refs` + `DimensionsContext::translate_via_mapping`
> bailing on group cardinality, fixed by a new `translate_via_group_mapping`)
> was **empirically disproved** before coding: the group `element_map` IS fully
> expanded and `translate_via_mapping` resolves it correctly *once the mapped
> dimension is in the per-variable `DimensionsContext`*. The real bug is in
> **`db.rs::expand_maps_to_chains`** (the salsa-scoping pass): `SourceDimension.name`
> keeps display casing (`"Aggregated Regions"`) while `maps_to`/`mappings[].target`
> are stored **canonical** (`"cop"`), and the reachability passes compared them
> with a raw `==`, so the reverse-mapping pass never pulled the mapped source
> dimension into the context → `has_mapping_to == false` → bare subscript →
> `DimensionInScalarContext`. **As-built Part A:** `expand_maps_to_chains`
> canonicalizes every reachability comparison and resolves each canonical target
> back to display name before insertion (fixes the reverse direction *and* a
> latent forward-direction bug for importer-sourced models); `substitute_dimension_refs`
> / `dimensions.rs` are left **byte-identical**, so the "LTM/mapping leak" worry
> is structurally impossible. **As-built Part B:** the `lower_implicit_var`
> post-lower re-check is routed through `try_accumulate_diagnostic` (the bare
> `.accumulate(db)` the spec names panics outside a `#[salsa::tracked]` frame —
> the assembly chain is untracked; this also surfaced a distinct pre-existing
> observability gap, `IN_TRACKED_CONTEXT` never set `true`, tracked separately).
> The `dimensions.rs` `translate_via_group_mapping` unit test below was replaced
> by `db_dimension_invalidation_tests.rs::expand_maps_to_chains_tests` (4 tests
> on the actually-fixed function); the RED→GREEN fixture and the loud-safe Part B
> test landed as specced. GH #580's root-cause section was corrected accordingly;
> test-requirements.md's AC7 row is reconciled at finalization. The text below is
> the original (investigation-path) diagnosis, retained for provenance.

**Files:**
- Add: `src/simlin-engine/src/dimensions.rs` — a new
  `DimensionsContext::translate_via_group_mapping` method + its `#[cfg(test)]`
  unit tests (in the existing `mod tests`).
- Modify: `src/simlin-engine/src/builtins_visitor.rs:339-367`
  (`substitute_dimension_refs`) — call the new resolver as a THIRD fallback.
- Modify: `src/simlin-engine/src/db.rs:3878-3963` (`lower_implicit_var`) — the
  loud-safe post-lower error re-check (Part B).
- Add: a focused fixture/test (in `src/simlin-engine/src/array_tests.rs` or a
  new `#[cfg(test)]` test reachable in the default capped suite).

**Implementation — Part A (the fix; general, no model-specific hack):**
Add `DimensionsContext::translate_via_group_mapping(active_dim, active_element,
ref_dim) -> Option<CanonicalElementName>`: given the iterated element
`active_element` of the active (parent) dimension `active_dim`, and a subscript
reference to `ref_dim` where `ref_dim` is related to `active_dim` by a **group
mapping** (one `ref_dim` element maps to a *group* — a subdimension, or a
multi-element subset — of `active_dim`), return the `ref_dim` element whose
target group **contains** `active_element`. Resolve the mapping in **either
direction** (`ref_dim.maps_to == active_dim` or vice-versa), reusing
`find_mapping_info` / `element_map` (the group rows) and
`get_subdimension_relation` (`dimensions.rs:541`) to locate the containing
group — do NOT assume equal cardinality (that is exactly the case the existing
`translate_via_mapping` / `translate_to_source_via_mapping` reject at
`dimensions.rs:455-457` / `:513-515`). Wire it into `substitute_dimension_refs`
as a **third** fallback, AFTER the existing `translate_via_mapping` (`:345`) and
`find_mapping_parent_of` (`:354-365`) attempts and BEFORE the unchanged-`expr`
return at `:368`. **Do NOT alter `translate_via_mapping` /
`translate_to_source_via_mapping` / `find_mapping_parent_of` semantics** — they
are shared by the LTM mapping consumers (`db_ltm_ir.rs`
`classify_iterated_dim_shape`'s mapped branch, `ltm_agg.rs`'s mapped-dimension
carve-out, the `Expr2Context::has_mapping_to` A2A-lowering path); a *new
additive* method that fires only where the old ones returned `None` keeps that
surface byte-stable.

**Implementation — Part B (loud-safe companion — no silent miscompile):**
In `lower_implicit_var` (`db.rs:3878`), AFTER `crate::model::lower_variable`
(`:3959`), re-check the *lowered* variable's `equation_errors()` (the current
check at `:3908-3913` only inspects the *pre*-lowering `parsed_implicit`). If
the lowered var carries errors (its AST was discarded), accumulate a precise
`CompilationDiagnostic` carrying the real error + span (mirror the real-var
path's `accumulate_var_compile_error`, `db.rs:3782`) and return `None`. This
does not itself fix a miscompile; it converts a *residual* un-translatable
mapping shape from the opaque aggregate `missing_vars` string into a legible
per-variable `DimensionInScalarContext` diagnostic — so if Part A is incomplete
for some shape, the failure is loud and actionable (AC7.5 / the "no silent
miscompile" hard rule), never a silent all-`None` fragment. Verify Part B is a
no-op when Part A succeeds (the lowered var has no errors).

**Testing (TDD, mandatory):**
- **RED-first fixture** (exact shape empirically determined during execution —
  plan convention 4: bounded ≈4-5 attempts + `track-issue` escalation, the AC
  is not weakened): a minimal model with a **group / unequal-cardinality**
  dimension mapping and an `INITIAL(...)`-wrapped A2A var whose argument
  references the mapped dimension by its **full dimension name** (#580
  investigator sketch — widen the group/cardinality mismatch until it
  reproduces):
  ```
  Big : e1, e2, e3, e4 ~~|
  Small : s1, s2 -> (Big: BigGroupA, BigGroupB) ~~|
  BigGroupA : e1, e2 ~~|   BigGroupB : e3, e4 ~~|
  src[Small] = 1 ~~|
  out[Big] = INITIAL( src[Small] ) ~~|
  ```
  RED: `compile_project_incremental` returns `Err` whose message contains
  `$⁚out⁚0⁚arg0⁚` (or, with Part B, a `DimensionInScalarContext` on that
  helper). If it compiles, the mapping is too simple — widen per #580's note
  (the genuine C-LEARN mapping is heterogeneous: a single base element vs
  3-element subgroups, `len(ref_dim) ≠ len(active_dim)`); verify by checking
  the minted helper's printed equation still contains the bare `[small]`
  subscript. GREEN after Part A: it compiles and simulates to the hand-computed
  per-`Big`-element series (each `out[e]` = the `src` value of the `Small`
  element whose group contains `e`).
- A focused **`dimensions.rs` `#[cfg(test)]`** unit test for
  `translate_via_group_mapping` directly: group containment in both mapping
  directions; the equal-cardinality case still delegates unchanged; an
  unrelated dimension ⇒ `None`.
- A focused **`lower_implicit_var` Part B** test: a synthetic
  implicit-var-bearing model whose helper genuinely cannot be element-resolved
  ⇒ the compile `Err` now names the specific helper + `DimensionInScalarContext`
  (loud), not only the aggregate `missing_vars`.
- **MANDATORY soundness pins (must stay GREEN unchanged):** the full
  `db_dep_graph` suite; `self_recurrence`, `ref`, `interleaved`,
  `init_recurrence`, `helper_recurrence`, `vector_simple` end-to-end;
  `genuine_cycles_still_rejected`; `incremental_compilation_covers_all_models`
  (AC2.6, the 22-model corpus); and — because Part A touches shared
  `dimensions.rs` — the LTM suites that exercise mapping
  (`db_ltm_unified_tests`, `db_ltm_module_tests`, `db_element_graph_tests`,
  `db_ltm_ir_tests`) plus `array_tests` / `compiler_vector`. If ANY LTM /
  mapping test changes behavior, the new method leaked into a shared path —
  **STOP and report** (do not edit a guard to pass).

**Verification:**
Run the RED→GREEN fixture + the focused unit tests + the full soundness-pin set
in the default capped suite. Then confirm the C-LEARN structural gate progresses
(the ≈140 synthetic-helper names are gone from the `Err`):
`cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
(Bug B's 6 real vars may still fail until Task 5 — expected; Task 4's
done-criterion is the ≈140 synthetic-helper names removed from the `Err`).
`git commit` (pre-commit fmt/clippy/non-ignored cargo test 180s cap; NEVER
`--no-verify`).
**Commit:** `engine: group-mapped subscript translation in temp-arg-helper extraction (#580 Bug A)`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Bug B — arrayed-GF dependency stub in the isolated per-variable recompile

**Verifies:** unblocks element-cycle-resolution.AC7.1 (newly-surfaced
prerequisite — see "Why Tasks 4-5 added"); directly verified by its own focused
unit tests.

> **AS-BUILT root-cause correction (verified during execution, commit
> `ad432fbe` — supersedes the diagnosis below).** The root-cause-confirm step
> (which this task mandates) **disproved** the prescribed location: `Var::new`
> succeeds for all 6 vars, the dep's per-element `tables` and dims ARE correctly
> reconstructed in the mini-layout, and `db_var_fragment.rs::build_stub_variable`
> / `extract_tables_from_source_var` are NOT the gap. The real bug is a **general
> engine codegen gap**: a per-element arrayed graphical function applied with an
> index (`SUM(g[D!](x))`, the `VECTOR SELECT` form) lowers to
> `App(Lookup(<full array view>, scalar-index))`, and **no codegen path ever
> materialized that as an array view** (`walk_expr_as_view` fell through →
> `Cannot push view`; `extract_table_info` rejected the multi-element table base
> → `BadTable`). Same root cause, two symptoms; never implemented, not
> cycle-gate-masked. **As-built fix** (the spec's `(Conditional) ... compiler
> array-view / lookup-range handling` clause): a new layout-independent
> `Opcode::LookupArray` (mirrors `VectorSortOrder`; symbolize/desymbolize/
> `renumber_opcode` arms; `symbolic_phase_element_order`'s `_ => {}` catch-all →
> no SCC verdict can shift, exactly like `Lookup`) + a single `Pass1Context::transform`
> (`Expr3`) decomposition of the apply into `AssignTemp`/`TempArray` so the
> surrounding reducer/op consumes an ordinary array view. Files actually touched:
> `ast/expr3.rs`, `bytecode.rs`, `vm.rs`, `compiler/codegen.rs`, `compiler/symbolic.rs`,
> `array_tests.rs` (NOT `db_var_fragment.rs` / `db.rs`). The two fixtures and the
> view-equivalence intent landed as specced. **AC7.1 is still blocked** after
> this task by a *6th* distinct latent bug this fix unmasked — a
> `GraphicalFunctionId = u8` overflow in `concatenate_fragments::absorb` (no
> cross-fragment GF de-duplication; ~165 distinct C-LEARN GF tables duplicate
> per consumer fragment past 255), tracked separately; **Task 1's `simulate.rs`
> gate commit is NOT sequenced until that is resolved.** The text below is the
> original (investigation-path) diagnosis, retained for provenance.

**Files:**
- Modify: `src/simlin-engine/src/db_var_fragment.rs` — the dependency-stub
  construction (`build_stub_variable` call site `:344`) and/or the dep-table
  collection (`:910-952`, via `extract_tables_from_source_var`).
- (Conditional) Modify: `src/simlin-engine/src/db.rs:3114`
  (`compile_phase_to_per_var_bytecodes`) and/or the `compiler` array-view /
  lookup-range handling — only if the root-cause-confirm step points there.
- Add: a focused fixture/test in `src/simlin-engine/src/array_tests.rs` (or
  `tests/compiler_vector.rs`).

**Implementation — root-cause-confirm FIRST (RED), then fix (general):**
1. **Reproduce + isolate** (RED): build the minimal fixture (below), confirm it
   reproduces `Err(Generic, "Cannot push view for expression type ... expected
   array expression")` for the `SUM(arrayed-GF(...))` shape AND
   `Err(BadTable, "range subscripts not supported in lookup tables")` for the
   `VECTOR SELECT(...)` shape, **via the incremental compile path**. Confirm
   the **exact** failing call site: #580 attributes it to
   `compile_phase_to_per_var_bytecodes`'s `module.compile()` (`db.rs:3174`),
   but the 6 real C-LEARN vars are *outside* every resolved SCC — establish
   whether the failure is in that path (and how it is reached for non-SCC vars)
   or in the main `compile_var_fragment` → `lower_var_fragment` → `build_var` →
   `Var::new` path (which shares the same minimal-`Module` shape). Identify the
   precise gap vs the whole-model compile: (i) `build_stub_variable` (`:344`)
   giving the arrayed-GF dependency the wrong dimensions/shape, (ii)
   `extract_tables_from_source_var` (`:947`) not returning the dep's per-element
   tables, and/or (iii) the GF dep needing to enter the mini-layout as a
   genuine array-shaped, per-element-`tables`-bearing `Variable::Var` so a
   `dep[D!](x)` reference lowers to an **array view** (not a scalar).
2. **Fix** (no model-specific hack): make the isolated per-variable recompile
   **view-equivalent to the whole-model compile** for an arrayed graphical
   function applied with an index inside a reducer — reconstruct the
   dependency's dimensions AND per-element `tables` in the mini-layout, and
   handle `VECTOR SELECT` / range-subscript lookup forms instead of rejecting
   with `BadTable`. General contract: for ANY dependency that is an arrayed
   graphical function, the stub carries enough shape + table data that
   `dep[D!](x)` compiles identically to the monolithic path.

**Testing (TDD, mandatory):**
- **RED-first fixture** (exact shape empirically determined — plan convention
  4: bounded ≈4-5 attempts + `track-issue` escalation):
  ```
  D : a, b, c ~~|
  g[D]( (0,0),(1,10),(2,20) ) ~~|        ' a genuine per-element arrayed GF
  drive = TIME ~~|                       ' non-constant index
  total = SUM( g[D!](drive) ) ~~|
  ```
  RED: `compile_project_incremental` returns the `Cannot push view ...` `Err`.
  Add a `VECTOR SELECT` variant for the `BadTable` case. If the sketch
  compiles, widen per #580's note (`g` a genuine per-element-`tables` arrayed
  GF, `drive` strictly non-constant). GREEN after the fix: compiles and
  simulates to the hand-computed `total` series (`SUM` of the per-element GF
  outputs at `drive = TIME`).
- A focused unit test pinning the **isolated-recompile view equivalence**: the
  arrayed-GF dependency's mini-`Module` produces the same array view as the
  whole-model compile (assert the compiled bytecode pushes a view, not a
  scalar).
- **MANDATORY soundness pins (must stay GREEN unchanged):** `array_tests`,
  `compiler_vector`, the full `db_dep_graph` suite **and** the recurrence
  end-to-end gates (`compile_phase_to_per_var_bytecodes` is ALSO the SCC
  element-graph builder's compile path — a behavior shift there could perturb
  `symbolic_phase_element_order` verdicts), `genuine_cycles_still_rejected`,
  and `incremental_compilation_covers_all_models` (AC2.6, the 22-model corpus).
  If ANY SCC verdict or corpus model changes behavior — **STOP and report**.

**Verification:**
Run the RED→GREEN fixture + the focused unit test + the full soundness-pin set
in the default capped suite. Then the C-LEARN structural gate should now be
fully GREEN (both Bug A and Bug B cleared):
`cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
— C-LEARN compiles `Ok`, runs to FINAL TIME, no all-NaN core series. `git
commit` (pre-commit; NEVER `--no-verify`).
**Commit:** `engine: arrayed-GF dependency stub in isolated per-variable recompile (#580 Bug B)`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: #582 — cross-fragment graphical-function de-duplication in `concatenate_fragments`

**Verifies:** unblocks element-cycle-resolution.AC7.1 (3rd layer unmasked by
Tasks 4-5); directly verified by its own focused unit test. Added under the
user's "drive #582, checkpoint per layer" directive after Tasks 4-5 landed.

**Why added (Task 5 outcome, verified — GH #582):** with Bug B fixed
(`ad432fbe`), C-LEARN's ~6 arrayed-GF-in-reducer fragments now compile and
their dependency GF tables enter fragment assembly for the first time. That
exposes a pre-existing **fragment-concatenation-vs-monolithic divergence**:
`concatenate_fragments`'s `absorb` (`compiler/symbolic.rs:1349`,
`self.merged_gf.extend_from_slice(&frag.graphical_functions)`) appends every
fragment's `graphical_functions` with **no de-duplication**, so a dependency
arrayed GF referenced by N consumer fragments is duplicated N times. C-LEARN
has only ~165 *distinct* GF tables, but the duplication accumulates past 255 and
`renumber_opcode`'s `gf_off > u8::MAX` guard (`symbolic.rs:1546-1551`) trips:
`NotSimulatable: "graphical function offset 493 exceeds GraphicalFunctionId
capacity (u8::MAX = 255)"` (`GraphicalFunctionId = u8`, `bytecode.rs:21`;
`Opcode::Lookup`/`LookupArray` `base_gf` renumbered via `checked_add_u8` at
`symbolic.rs:1566`/`:1642`). The monolithic `Module::compile` **de-duplicates**
GF tables — so the incremental path is incorrect-by-omission here, not merely
capacity-limited. 3rd distinct pre-existing latent bug the cleared cycle gate
exposed (Bug A, Bug B, then this); orthogonal to cycle resolution.

**Files:**
- Modify: `src/simlin-engine/src/compiler/symbolic.rs` — `concatenate_fragments`
  / `absorb` (the GF-table merge ~`:1349`) and the GF renumber path
  (`renumber_opcode` / `renumber_fragment_code`, the `gf_off` arithmetic
  ~`:1546-1642`).
- Add: a focused `#[cfg(test)]` unit test (in `compiler/symbolic.rs`'s test
  module, or `array_tests.rs`).

**Implementation — root-cause-confirm FIRST (the plan's per-bug diagnosis has
been wrong 3x — verify, do not trust), then fix (general, monolithic-matching):**
1. **Confirm** (RED): reproduce the overflow with a minimal input — either a
   synthetic model where one dependency arrayed GF is referenced by enough
   consumer fragments that the *duplicated* count exceeds `u8::MAX` while the
   *distinct* count stays well under it, OR a direct `#[cfg(test)]`
   `concatenate_fragments`/`absorb` unit test with hand-built fragments sharing
   GF tables (whichever is the cheaper, clearer RED — plan convention 4, bounded
   ~4-5 attempts + `track-issue` escalation). Confirm the EXACT dedup key the
   monolithic `Module::compile` uses (GF `Table` value identity? the originating
   variable ident? read the monolithic path and match it — do not invent a key)
   and the exact `base_gf`/`gf_off` remap arithmetic the current flat-offset
   renumber assumes.
2. **Fix:** de-duplicate GF tables across fragments in `absorb` /
   `concatenate_fragments`, keyed by the SAME identity the monolithic path uses,
   and remap each fragment's local `base_gf` references to the deduped global
   index (the flat running `gf_off` is replaced by a per-fragment local→global
   GF index map threaded through `renumber_opcode` / `renumber_fragment_code`).
   After dedup, C-LEARN's ~165 distinct GFs are well under `u8::MAX`, so
   `GraphicalFunctionId = u8` is retained — **do NOT widen to u16** (the weaker
   band-aid; if some model has >255 *genuinely distinct* GFs even after dedup,
   that is a separate concern — file via `track-issue`, do not scope-creep here).

**Loud-safe / no silent miscompile (load-bearing):** the dedup MUST be
value-exact — two GF tables that are genuinely different must NEVER merge to one
index (that would silently make a `Lookup`/`LookupArray` read the wrong table).
The identity key must be the table's full content (or a key the monolithic path
already proves sufficient). If two fragments carry the same *name* but different
table content, they must stay distinct (or it is a pre-existing name-collision
bug — STOP and report, do not paper over it).

**Testing (TDD, mandatory):**
- RED→GREEN: the minimal overflow reproduction above — RED `... exceeds
  GraphicalFunctionId capacity ...`; GREEN compiles and (if a runnable model)
  simulates correctly, with the deduped `merged_gf` length == the distinct GF
  count and every `Lookup`/`LookupArray` resolving to the correct table.
- A focused unit test asserting `absorb` / `concatenate_fragments` dedups
  identical GF tables and remaps `base_gf` correctly, and KEEPS distinct tables
  distinct (the value-exactness guard).
- **MANDATORY soundness pins (must stay GREEN unchanged):** `concatenate_fragments`
  is the SAME machinery the Phase 2 GH #575 combined-SCC-fragment lowering uses
  (`FragmentMerger` / `renumber_fragment_code`) — so pin the full recurrence /
  cycle suite (`self_recurrence`, `ref`, `interleaved`, `init_recurrence`,
  `helper_recurrence`, `genuine_cycles_still_rejected`), the full `db_dep_graph`
  + `db_combined_fragment_tests` suites, `incremental_compilation_covers_all_models`
  (AC2.6, the 22-model corpus — the strongest no-regression gate for a GF
  renumber change), `array_tests` / `compiler_vector` (GF / Lookup coverage),
  and the full engine lib. If ANY combined-fragment / SCC-verdict / corpus-model
  / GF-lookup test changes behavior — **STOP and report** (a remap bug is a
  silent miscompile risk).

**Verification:**
Run the RED→GREEN reproduction + the focused unit test + the full soundness-pin
set in the default capped suite. Then run the C-LEARN structural gate:
`cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
— report the EXACT outcome (compile `Result`, run-to-FINAL-TIME, the
matched-series not-all-NaN check). If C-LEARN now compiles `Ok` + runs +
not-all-NaN, AC7.1/7.2/7.3 are met (the orchestrator then sequences the Task 1
commit). If a further latent layer surfaces, report it PRECISELY (root cause,
the `Err`/panic, the site) — do NOT mask it; the orchestrator checkpoints with
the user per the "checkpoint per layer" directive. `git commit` (pre-commit
fmt/clippy/non-ignored cargo test 180s cap; NEVER `--no-verify`).
**Commit:** `engine: cross-fragment graphical-function de-duplication (#582)`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: #583 — match monolithic temp recycling in fragment concatenation (supersedes "widen TempId")

**Verifies:** unblocks element-cycle-resolution.AC7.1/AC7.2/AC7.3 (the sole
remaining assembly-path ceiling — the forward sweep confirmed there is no "stack
of u8 ceilings"); directly verified by its own focused unit tests + the C-LEARN
structural gate.

**Why added (user-authorized forward-sweep outcome — overturns #583's premise):**
the user chose to "sweep all remaining assembly-path ceilings at once". The sweep
(an all-narrow-types-widened-to-u32 + peak-count probe build, fully reverted)
established: **(1)** only TWO `u8` index types exist — `GraphicalFunctionId`
(already bounded to ~162 by #582's dedup) and `TempId`; every other index
(`LiteralId`/`ModuleId`/`ViewId`/`DimId`/`DimListId`/`NameId`/…) is already `u16`
and nowhere near its limit. **(2)** None serialize to protobuf (bytecode is never
persisted — widening would be back-compat-safe, but is the WRONG fix). **(3)**
With everything widened, C-LEARN **compiles `Ok`** and `Vm::new` succeeds — but
the VM then **panics at `vm.rs:2372`** (`VectorSortOrder` reads `temp_offsets[347]`
when the table has 243 entries). So widening converts a clean compile `Err` into
a runtime OOB — proof the temp numbering is **incoherent**, not capacity-limited.
**(4)** `TempId` is a **concat-vs-monolithic divergence, NOT genuine-capacity**
(this REFUTES #583's stated premise): monolithic `Module::compile` recycles temps
via a `HashMap<temp_id,size>` keyed max-merge (`compiler/mod.rs:2720-2740`) and
needs only **21** slots for C-LEARN; the incremental path SUMS and allocates
**243** (the `merged` table) / **3233** (`flows_concat` — they DISAGREE, which is
the runtime OOB). Same class as #582 (incorrect-by-omission in
`concatenate_fragments`), now for temps.

Two compounding manifestations (both in `compiler/symbolic.rs`, both fixed here):
- **(1a) the divergence:** `ContextResourceCounts::from_fragments` (`:1256-1272`)
  explicitly SUMS each fragment's `(max_temp_id+1)` (its own comment at
  `:1261-1262` says "the total is the sum ... not the global max"); `absorb_non_gf`
  advances `merged_temp_sizes.len()` per temp-bearing fragment — never recycling.
  Monolithic max-merges to 21. → 243.
- **(1b) a plain arithmetic bug:** `absorb_non_gf:1523` is
  `temp_offset = self.merged_temp_sizes.len() as u32 + self.ctx_base.temps` — the
  `+ ctx_base.temps` is **re-added on every temp-bearing fragment** (flows
  `ctx_base.temps=115` × ~28 fragments + Σmaxes ≈ **3232**, the observed
  `flows_concat` max). The SAME `+ ctx_base.X` re-add is on `:1521`/`:1522`/`:1524`
  (`mod`/`view`/`dim_list`) — latent only because C-LEARN fragments carry ~0 of
  each. This is why `flows_concat` (3233, `db.rs:5280 compiled_flows`) ≠ `merged`
  (243, `db.rs:5414 temp_offsets`) — the two tables the VM consumes disagree.

**Files:**
- Modify: `src/simlin-engine/src/compiler/symbolic.rs` —
  `ContextResourceCounts::from_fragments` (`:1256`), `FragmentMerger::absorb_non_gf`
  (`:1518`, the `:1521-1524` offset arithmetic), `renumber_opcode` /
  `renumber_fragment_code` (`:1700`/`:1870`, the `temp_off` path + the removable
  `temp_off > u8::MAX` guard at `:1879`), `into_concatenated` /
  `into_per_var_bytecodes`.
- Modify: `src/simlin-engine/src/db.rs` — `assemble_module` per-phase concat
  arithmetic (`:5081-5305`, esp. phase bases `:5086-5096` and the `merged` vs
  `flows_concat`/`stocks_concat` reconciliation `:5219`/`:5280`/`:5414`).
- Add: focused `#[cfg(test)]` unit tests.

**Implementation — root-cause-confirm FIRST (the plan's per-bug diagnosis has been
wrong 3x; this sweep's monolithic evidence is strong but VERIFY), then fix
(general, monolithic-matching — do NOT widen):**
1. **Confirm** the monolithic recycle (`compiler/mod.rs:2720-2740` keyed
   max-merge → `n_temps=21`) and that a plain-phase concatenated stream's
   per-fragment temp live ranges are **sequential / non-overlapping** (a fragment's
   temps are dead once its runlist segment completes — the property monolithic
   relies on to recycle). Confirm `combine_scc_fragment` (`db.rs:3600-3685`) is
   DIFFERENT: its members' per-element segments **interleave** per `element_order`,
   so their temp live ranges **overlap** and must NOT share slots.
2. **Fix (1b):** `absorb_non_gf` must not re-add `ctx_base.X` per fragment — seed
   the merger's `merged_X` accounting with `ctx_base.X` ONCE (or compute
   `offset = ctx_base.X + cumulative_merged`), uniformly for temps AND the
   `mod`/`view`/`dim_list` siblings (`:1521-1524`), so `flows_concat` ≡ `merged`.
3. **Fix (1a):** in the **plain-phase** concatenation path
   (`concatenate_fragments_with_gf` / the `assemble_module` flows/stocks/init
   concat), **max-merge (recycle)** fragment temps into a shared keyed pool
   matching `Module::compile`'s `temp_sizes_map` (→ ~21, not 243). **KEEP
   `combine_scc_fragment` on the disjoint-range (sum) path** — its interleaved
   per-element segments need non-overlapping temp ranges (a naive shared-slot-0
   recycle there would miscompile a multi-member recurrence SCC). Thread the
   distinction explicitly (a flag/param, or two paths). Remove the
   `temp_off > u8::MAX` guard; **KEEP `TempId = u8`** (after recycle it is ~21; do
   NOT widen — the probe proved widen yields a runtime OOB; `GraphicalFunctionId`
   stays `u8` too, bounded by #582's dedup).

**Loud-safe / no silent miscompile (load-bearing):** two temps that are
simultaneously **live** must NEVER share a slot. In the sequential phase concat
they are not (segments are ordered; a fragment's temps die at its segment end);
in `combine_scc_fragment` they ARE (interleaved per `element_order`) — so that
path stays disjoint. If the sequential-liveness property does NOT hold for some
phase concat (e.g. a fragment reads a *prior* fragment's temp), STOP and report —
recycling would be a silent miscompile.

**Testing (TDD, mandatory):**
- A focused unit test: for a multi-temp-bearing-fragment fixture, the incremental
  plain-phase temp count **equals** the monolithic `Module::compile` `n_temps`
  (the recycle is exact), and every renumbered temp opcode resolves in-range.
- A test pinning `combine_scc_fragment`: a multi-member recurrence SCC whose
  members each bear a temp still gets **disjoint** temp ranges (no shared slot
  across interleaved segments) — the SCC-path-stays-summed guard.
- RED→GREEN: the C-LEARN structural gate goes from `Err(... temp offset 347 ...)`
  to compiling `Ok` + running to FINAL TIME + not-all-NaN (or the next layer —
  see Verification).
- **MANDATORY soundness pins (must stay GREEN unchanged):** `concatenate_fragments`
  / `FragmentMerger` / `renumber_opcode` are shared with the Phase-2 GH #575
  `combine_scc_fragment` — so pin the FULL recurrence/cycle suite
  (`self_recurrence`, `ref`, `interleaved`, `init_recurrence`, `helper_recurrence`,
  `genuine_cycles_still_rejected`), the full `db_dep_graph` +
  `db_combined_fragment_tests` suites, `incremental_compilation_covers_all_models`
  (AC2.6, the 22-model corpus — the strongest gate for a temp-renumber change),
  `array_tests` / `compiler_vector` (temp-bearing `VectorSortOrder`/`LookupArray`/
  `AssignTemp`), `cargo test --workspace`, and the full engine lib. If ANY
  combined-fragment / SCC-verdict / corpus-model / vector-op test changes behavior
  — **STOP and report** (a temp-recycle collision is a silent miscompile).

**Verification:**
Run the unit tests + the full soundness-pin set, then:
`cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
— report the EXACT outcome (compile `Result`, run-to-FINAL-TIME, the
matched-series not-all-NaN check). **Sweep caveat:** under the probe the VM
panicked on the incoherent temp table BEFORE `run_to_end` completed, so AC7.2/7.3
were never exercised — once the temp tables are coherent the VM runs further and
**may surface a runtime-numeric/NaN layer** (Phase 7's AC8 territory; `MEMORY.md`
flags VECTOR ELM MAP OOB→NaN and SORT ORDER as risk points). If C-LEARN now
compiles `Ok` + runs to FINAL TIME + not-all-NaN, AC7.1/7.2/7.3 are MET (the
orchestrator sequences the Task 1 commit). If a runtime layer surfaces, report it
PRECISELY (root cause, the `Err`/panic, the site) — do NOT mask it; the
orchestrator checkpoints with the user per the "checkpoint per layer" directive.
`git commit` (pre-commit; NEVER `--no-verify`).
**Commit:** `engine: match monolithic temp recycling in fragment concatenation (#583)`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: AC7.3 NaN Cluster A — INITIAL-backed module/macro primary output omitted from the initials runlist

**Verifies:** element-cycle-resolution.AC7.3 (no core C-LEARN series entirely
NaN); directly verified by its own focused test.

**Why added (root-caused after Task 7 — C-LEARN runs but ~177 climate vars are
inf/NaN):** with C-LEARN compiling+running (Tasks 4-7; AC7.1/7.2 met), AC7.3
fails — 708-of-3482 matched core series (939-of-5726 offsets) are entirely NaN.
A read-only investigation found TWO independent **SPURIOUS** engine bugs (Ref.vdf
is FINITE at every offset Simlin reports NaN/inf — not genuine model NaN). Cluster
A (this task, the majority): C-LEARN defines a user macro `:MACRO: INIT(x) → INIT
= INITIAL(x)` (the MDL importer renames Vensim `INITIAL`→`init`; the macro SHADOWS
the built-in `init` by macro precedence); ~177 call sites (e.g.
`volumetric_heat_capacity = INITIAL(...)`, line 12698) compile to invoke this one
shared macro module. **The bug:** that macro module's compiled **initials runlist
OMITS its own `INITIAL()` primary output** (`init = INITIAL(x)`, data offset 0) —
the output is compiled ONLY into the flows phase. During the PARENT's initials
phase the parent reads the module-output slot, which is **never written during
initials** → uninitialized garbage (inf), frozen into the `initial_values`
snapshot (`vm.rs` `run_initials` `copy_from_slice`) and served forever by
`LoadInitial`. `volumetric_heat_capacity` = **inf** (Simlin) vs **0.1327**
(Ref.vdf); the input arg evaluates correctly to 0.1327. **(The prior
runlist-ORDERING hypothesis is DISPROVEN — the outer initials order is correct;
the defect is runlist INCLUSION: the slot is never written in initials at all.)**

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` — `runlist_initials` (~`:2182-2229`,
  the `needed`-set inclusion predicate ~`:2182-2193`).
- (Likely) Modify: `src/simlin-engine/src/db.rs` — the initials-phase gate (`:3828`
  and `:4084`, `if dep_graph.runlist_initials.contains(&var_ident_str) { ... } else
  { None }`).
- Add: a focused `#[cfg(test)]` / integration test.

**Implementation — root-cause-confirm FIRST (plan diagnoses have been wrong;
verify), then fix (general):**
1. **Confirm** the mechanism: probe that the macro module's `INITIAL()`-backed
   primary output (compiles to `LoadInitial`) is absent from `runlist_initials`,
   so its slot is unwritten during the parent's initials phase and holds garbage.
   Confirm the current inclusion predicate's criteria and that the macro-output
   aux falls through.
2. **Fix:** extend the initials-runlist inclusion predicate so a (module/macro)
   primary-output variable whose equation compiles to `LoadInitial` (is
   `INITIAL(...)`) — and is read during a parent's initials phase — is INCLUDED in
   `runlist_initials` (and the `db.rs` gate admits its initial bytecodes). General:
   ANY variable read during initials whose value comes from `INITIAL()` must be
   evaluated in the initials phase. No model-specific / macro-name-specific hack.

**Loud-safe:** do not broaden the initials runlist beyond what is genuinely read
during initials (over-inclusion could change init ordering/costs). Precise: an
`INITIAL()`-backed output that IS read in initials.

**Testing (TDD, mandatory):**
- RED-first fixture (shape empirically determined — plan convention 4, bounded
  ~4-5 attempts + `track-issue` escalation): a minimal model with a user macro
  `:MACRO: MYINIT(x) MYINIT = INITIAL(x) :END OF MACRO:` and a parent
  `y = MYINIT(<expr>)` (or the equivalent module whose primary output is
  `INITIAL(...)`), asserting `y` simulates to `<expr>`'s initial value (NOT
  inf/NaN). RED before the fix (inf/NaN), GREEN after.
- **MANDATORY soundness pins (must stay GREEN unchanged):** the macro suites
  (`macro_expansion_tests`, `metasd_macros`), the INIT/PREVIOUS + init-recurrence
  suites (`init_recurrence`, `helper_recurrence`, `previous_self_reference_still_resolves`),
  the full recurrence/cycle gates, `incremental_compilation_covers_all_models`
  (AC2.6, 22-model corpus), and the full engine lib. If ANY init-ordering / macro
  / corpus test changes behavior — STOP and report.

**Verification:**
Run the fixture + soundness pins. Then re-run the C-LEARN structural gate and
report how many of the all-NaN offsets are cleared by Cluster A alone (the
Cluster B / COP-target family remains until Task 9 — expected). `git commit`
(NEVER `--no-verify`).
**Commit:** `engine: include INITIAL-backed module outputs in the initials runlist (AC7.3 Cluster A)`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: AC7.3 NaN Cluster B — arrayed VECTOR SORT ORDER per-iterated-slice 0-based ranks (completes AC5)

**Verifies:** element-cycle-resolution.AC7.3 (no core C-LEARN series entirely
NaN) + completes element-cycle-resolution.AC5 (VECTOR SORT ORDER genuine-Vensim
semantics — the multi-row case Phase 4 did not cover); directly verified by its
own focused unit test.

**Why added (root-caused after Task 7 — the COP-target NaN family):** the second
spurious-engine-bug cluster (Ref.vdf finite). C-LEARN: `Target Order[COP,Target] =
VECTOR SORT ORDER(Effective Target Year[COP,Target], ASCENDING)` (line 19565), then
`sorted target year[COP,Target] = VECTOR ELM MAP(Effective Target Year[COP,t1],
Target Order[COP,Target])` (line 19486). `Target` = 3 elements, `COP` = 7 rows.
**The bug:** `Opcode::VectorSortOrder` (`vm.rs` ~`:2334-2377`) returns **GLOBAL
FLAT indices into the entire [COP,Target] array** instead of **per-iterated-row
0-based ranks** — for the 7th COP row it emits [18,19,20] instead of [0,1,2]. Then
`VECTOR ELM MAP` (`vm_vector_elm_map.rs:107`) uses [18,19,20] to index the
7-element single-column slice `Effective Target Year[COP,t1]` → out-of-bounds →
NaN. (The ELM MAP OOB→NaN is the CORRECT Phase 5 semantics — the bug is the bad
indices fed to it.) `target_order[cop_developing_b]` = **[18,19,20]** (Simlin) vs
**[0,1,2]** (Ref.vdf); `sorted_target_year[cop_developing_b]` = **NaN** vs
**4000.0**. This is the **multi-row / A2A** VECTOR SORT ORDER case — Phase 4's AC5
fix corrected the 1-based→0-based output for the single-row case but did not cover
per-iterated-slice ranks for a multi-dimensional source.

**Files:**
- Modify: `src/simlin-engine/src/vm.rs` — `Opcode::VectorSortOrder` dispatch
  (~`:2334-2377`).
- Add: focused unit tests in `src/simlin-engine/src/array_tests.rs` (and/or
  `tests/compiler_vector.rs`).

**Implementation — root-cause-confirm FIRST, then fix (general, genuine-Vensim):**
1. **Confirm** that for a multi-row arrayed source `VectorSortOrder` currently
   emits flat whole-array offsets rather than per-row 0-based ranks (probe a
   `[D1,D2]` fixture). Confirm genuine-Vensim semantics: VECTOR SORT ORDER ranks
   within the **iterated slice** (per-row), 0-based (as Phase 4 established for the
   single-row case and `MEMORY.md` records).
2. **Fix:** `VectorSortOrder` must compute ranks relative to the
   **currently-iterated source slice** (each row's worth of elements), 0-based, so
   the result is a valid per-row index permutation. General correctness fix for ALL
   arrayed VSO callers; the single-row case (Phase 4 / AC5) must stay byte-identical.

**Loud-safe:** the result must be a valid per-slice index permutation (every index
in `[0, slice_len)`), so a downstream `VECTOR ELM MAP` cannot read OOB on a
well-formed model.

**Testing (TDD, mandatory):**
- RED-first: a multi-row `order[D1,D2] = VECTOR SORT ORDER(vals[D1,D2], ascending)`
  fixture where each D1-row's sort must be 0-based WITHIN the row — RED (flat global
  indices), GREEN (per-row 0-based). Plus the C-LEARN downstream shape
  (`VECTOR ELM MAP(src[D1,e1], order[D1,D2])`) proving no OOB NaN. Bounded attempts
  + `track-issue` escalation.
- **MANDATORY soundness pins (must stay GREEN unchanged — esp. the AC5 single-row
  suite Phase 4 established):** `array_tests` `vso_*` / `mod flag_split_tests` /
  `mod dimension_dependent_scalar_arg_tests`, the five `tests/compiler_vector.rs`
  VSO tests (`vector_sort_order_a2a_*`, `nested_vector_sort_order_inside_sum_*`),
  `simulates_vector_simple_mdl` (the `l`/`m` columns), `incremental_compilation_covers_all_models`
  (22-model corpus), and the full engine lib. If ANY single-row VSO / corpus test
  changes behavior — STOP and report (the fix must not regress Phase 4's AC5).

**Verification:**
Run the fixtures + soundness pins. Then re-run the C-LEARN structural gate — with
BOTH Task 8 (Cluster A) and Task 9 (Cluster B) in, report whether AC7.3 now PASSES
(no matched core series entirely NaN). If a residual NaN tail remains (a 3rd
cause), report it precisely — do NOT mask; the orchestrator checkpoints with the
user. `git commit` (NEVER `--no-verify`).
**Commit:** `engine: arrayed VECTOR SORT ORDER per-slice 0-based ranks (AC5/AC7.3 Cluster B)`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: represent Vensim `:NA:` as the finite sentinel -2^109, not NaN (root-cause `:NA:` fix)

**Verifies:** element-cycle-resolution.AC7.3 (no core C-LEARN series entirely
NaN) — the 3rd, deepest NaN cause; directly verified by focused unit tests + the
C-LEARN gate.

**Why added (USER CORRECTION — domain-authoritative — overturns the prior
"Simlin is correct" verdict):** the prior investigation concluded the residual
all-NaN series were a comparison artifact (Simlin correct). **The user corrected
this:** per the Vensim docs, `:NA:` is **NOT** IEEE NaN — it is a **finite
sentinel value `-2^109`** (≈ -6.49e32) "used to test for the existence of data."
So Simlin's `:NA:`→NaN IS an engine bug. A deep combined research + audit
(web-cited + raw-VDF-byte probe) confirmed:
- **Vensim `:NA:` = `-2^109`**, an ordinary finite number (NOT absorbing):
  `IF THEN ELSE(X = :NA:, ...)` is the canonical existence test (ordinary `=`
  equality-to-the-sentinel, which only works because it's finite); arithmetic
  computes on `-2^109`; aggregations include it; `:NA:` ≠ NaN/FP-error. (Sources:
  vensim.com/documentation/na.html, dataequations.html; Ventana forum t=4707.)
- **Vensim saves `:NA:` → `0.0` to the VDF** (empirical: an exhaustive raw-byte
  scan of all 2641 `Ref.vdf` data blocks found `-2^109` **0 times**, NaN **0
  times**; the `:NA:` series are literal `0x00000000`). So Vensim's RUNTIME value
  is `-2^109` but the SAVED value is `0`.
- **Simlin is 3-way inconsistent, none correct:** expression `:NA:` → `"NAN"`
  string (`mdl/xmile_compat.rs:109`) → `Expr0::Const("NaN", f64::NAN)`
  (`parser/mod.rs:529-535`) → IEEE NaN (the primary bug); data-list `:NA:` →
  `NA_VALUE = -1e38` (`mdl/parser.rs:51`); Vensim's = `-2^109`. Under NaN, any
  unguarded arithmetic on a `:NA:` is irreversibly NaN → the 434-series cascade.

**This task (Layer 1) fixes the engine `:NA:` semantics and clears AC7.3**: once
`:NA:` = `-2^109`, the 434 series are FINITE (not NaN), so "no all-NaN core
series" passes and the `IF THEN ELSE(X = :NA:, ...)` existence tests gate
correctly via finite equality. The separate `-2^109` (Simlin runtime) ↔ `0`
(Vensim VDF) reconciliation needed for **AC8.1's 1% numeric match is Phase 7**
work (the VDF comparison), NOT this task.

**CRITICAL — keep distinct from legitimate NaN (must NOT change):** Simlin's
other NaN sources are STRUCTURALLY distinct from `:NA:` and triggered by
conditions that never involve a `:NA:` literal — empty-view reducers → NaN
(`vm.rs:2023/2040/2057/2076`, `size()==0`), VECTOR ELM MAP **OOB → NaN**
(`vm_vector_elm_map.rs:103-112`; the genuine Phase 5 semantics — **and C-LEARN
line 19481 feeds the non-`:NA:` `Target Value[COP,t1]` slice into ELM MAP, so
OOB→NaN and `:NA:`→sentinel INTERSECT in this model and MUST stay distinct**),
range/subscript OOB → NaN (`compiler/context.rs:1778/1795/1890`), lookup/divide
undefined → NaN. These STAY NaN. Only the `:NA:` literal changes.

**Files:**
- Add: a single canonical `NA` constant (e.g. `src/simlin-engine/src/common.rs`,
  `pub const NA: f64 = -6.490371073168535e32;` with a test pinning
  `NA == -(2.0_f64).powi(109)`).
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs:109` (`Expr::Na` → the `NA`
  numeric literal, not `"NAN"`) and/or `parser/mod.rs:529-535` + the `Expr0`
  lowering — route expression `:NA:` to `Const(NA)` (the faithful representation:
  Vensim `:NA:` IS just the number `-2^109`; a dedicated `Expr::Na` node is only
  warranted if root-cause-confirm shows `Const(NA)`+`approx_eq` mishandles the
  existence test — verify, prefer the faithful `Const`).
- Modify: `src/simlin-engine/src/mdl/parser.rs:51` (`NA_VALUE`: `-1e38` → `NA`) +
  the AST doc comment `mdl/ast.rs:153`.
- Update (these pin the OLD WRONG value — correcting them IS part of the fix,
  with a comment; NOT silencing a guard): `mdl/parser.rs:1948/1965/1981`,
  `mdl/reader.rs:1510/1529/1549` (assert `-1e38` → assert `NA`).
- Add: focused `#[cfg(test)]` tests.

**Implementation — root-cause-confirm FIRST, then fix (general, Vensim-faithful):**
1. **Confirm** with minimal probes that `Const(NA)` gives Vensim-correct behavior:
   `IF THEN ELSE(x = :NA:, a, b)` where x is set to `:NA:` → takes the `a` branch
   (existence test true via `approx_eq(NA, NA)`); a genuine value → `b`; `:NA:`
   arithmetic → finite; and that `approx_eq` at magnitude 6.49e32 does NOT
   spuriously equate `-2^109` with a contaminated `-2^110`. Confirm `Const(NA)` is
   sufficient (vs a dedicated node).
2. **Fix:** route BOTH `:NA:` paths (expression + data-list) to the single `NA`
   sentinel. Do NOT touch the OOB→NaN / empty-reducer→NaN paths.

**Loud-safe / no-regression:** OOB→NaN and empty-reducer→NaN STAY NaN
(structurally distinct). The `-1e38`/`NaN` → `-2^109` test updates correct the old
wrong value (legitimate, commented) — distinct from soundness pins that must stay
byte-identical.

**Testing (TDD, mandatory):**
- RED-first: a model with the Vensim existence-test idiom
  `out = IF THEN ELSE(probe = :NA:, fallback, probe)` and `:NA:` arithmetic,
  asserting Vensim-correct FINITE behavior (not NaN) — RED before (NaN poisons
  it), GREEN after. Bounded attempts + `track-issue` escalation.
- The `NA == -2^109` const-pinning test; the updated `-1e38`→`NA` data-list tests
  (corrected, commented).
- **MANDATORY soundness pins (must stay GREEN unchanged):** the OOB→NaN tests
  (`vector_elm_map_tests` `out_of_bounds_*`/`negative_offset_*`, range-subscript
  OOB), the empty-reducer→NaN tests, the macro/data suites (`macro_expansion_tests`,
  `metasd_macros`), `mdl/reader` + `mdl_equivalence`,
  `incremental_compilation_covers_all_models` (22-model corpus), and the full
  engine lib. If ANY OOB-NaN / empty-reducer-NaN / corpus test changes behavior —
  STOP and report (the `:NA:` change must NOT bleed into legitimate NaN).

**Verification:**
Run the tests + soundness pins. Then re-run the C-LEARN structural gate: report
AC7.1/AC7.2/AC7.3 status and the remaining all-NaN count — AC7.3 should now PASS
(the 434 `:NA:`-cascade series are finite). A C-LEARN series being `-2^109`
instead of Vensim's saved `0` is EXPECTED here and is NOT an AC7.3 failure (the
`-2^109`↔`0` VDF match is Phase 7/AC8.1). If a residual all-NaN tail remains (a
distinct cause), report it precisely — do NOT mask; the orchestrator checkpoints.
`git commit` (NEVER `--no-verify`).
**Commit:** `engine: represent Vensim :NA: as the finite sentinel -2^109, not NaN`
<!-- END_TASK_10 -->

<!-- START_TASK_11 -->
### Task 11: convert the codegen PREVIOUS-subscript panic to a typed Err (#363, AC7.5)

**Verifies:** element-cycle-resolution.AC7.5 (a post-gate panic is root-caused and
converted to a typed `Result::Err`, not caught) + unblocks AC7.4 (Task 2's
clean-compile re-point); directly verified by a focused unit test + the
`clearn_ltm_discovery_compiles` gate.

**Why added (Task 2 outcome — #363 reproduces on the LTM-discovery path):** Task 3
Part B re-verified #363 on the PLAIN incremental path and found the panic did not
reproduce. Task 2 (re-pointing `clearn_ltm_discovery_*` to expect a clean compile,
with LTM discovery ENABLED) surfaced that **#363 DOES reproduce on the
LTM-discovery synthetic-fragment path**: `compile_project_incremental` PANICS (not
a clean Err) at `compiler/codegen.rs:494` — `self.walk_expr(expr).unwrap().unwrap()`
on a recoverable `Err(NotSimulatable, "PREVIOUS requires a variable reference after
helper rewriting")` (a `PREVIOUS(...)` whose arg sits inside a `Subscript` index,
not reduced to a bare `Expr::Var`). The sibling arms at `codegen.rs:768/769/795`
already use `?`. The caller `db_ltm.rs:2083` is ALREADY `Err(_) => None` (it intends
to gracefully drop an un-compilable LTM synthetic fragment) — so the stray
`.unwrap().unwrap()` short-circuits the exact `Result` path the caller is poised to
absorb. Textbook #363 panic-site (a recoverable Err escalated to a process-killing
panic). The structural gate (no LTM discovery) is unaffected — it passes.

**Files:**
- Modify: `src/simlin-engine/src/compiler/codegen.rs:494` — `.unwrap().unwrap()` →
  `?` (match the sibling `:768/769/795` propagation).
- Add: a focused `#[cfg(test)]` unit test reproducing the converted condition (a
  `PREVIOUS` of a subscripted/expr arg reaching this codegen arm → a typed `Err`,
  not a panic).

**Implementation — root-cause-confirm FIRST, then fix (general):**
1. **Confirm** the exact `walk_expr` return shape + that the siblings (`:768/769/795`)
   use `?`, so the fix matches them; confirm the Err flows to `db_ltm.rs:2083`'s
   `Err(_) => None` (graceful drop); confirm no other caller of this arm relies on
   the panic.
2. **Fix:** convert the panic site to propagate via `?`. Strictly loud-safe (panic
   → typed Err; a success is unchanged). The un-scoreable LTM synthetic fragment is
   then gracefully dropped (the established `model_ltm_fragment_diagnostics` Warning
   path), so C-LEARN compiles with LTM discovery enabled.

**Loud-safe / no silent miscompile:** panic → typed `Err` is strictly safer (never
changes a success). The dropped LTM synthetic fragment is the established graceful
behavior; if it SHOULD have compiled (a deeper PREVIOUS-helper-rewrite gap for a
subscripted arg), file it via `track-issue` (do NOT expand this task to fix the
rewrite).

**Testing (TDD, mandatory):**
- RED-first: a minimal `#[cfg(test)]` fixture whose path hits the
  `PREVIOUS`-in-subscript arm — RED (panic) before, GREEN (typed Err, gracefully
  handled) after.
- **MANDATORY soundness pins (must stay GREEN unchanged):** `codegen.rs:494` is a
  SHARED codegen arm — pin the full compiler/codegen suites, `array_tests`,
  `compiler_vector`, the recurrence/cycle gates, `incremental_compilation_covers_all_models`
  (22-model corpus), `simulate_ltm`, and the full engine lib. If ANY changes
  behavior, STOP and report (the `?` must only convert panics to Errs, never alter
  a success).

**Verification:**
Run the unit test + soundness pins. Then run `clearn_ltm_discovery_compiles` (Task
2's re-point, uncommitted in the tree): `cargo test -p simlin-engine --features
xmutil --release -- --ignored clearn_ltm_discovery --nocapture` — must now PASS
(clean compile, no panic). Commit the codegen fix + unit test (AC7.5), THEN commit
Task 2's re-point (AC7.4). NEVER `--no-verify`.
**Commit (AC7.5):** `engine: convert codegen PREVIOUS-subscript panic to typed Err (#363, AC7.5)`
**Commit (AC7.4, Task 2 re-point):** `engine: re-point clearn LTM-discovery test to clean compile; retire catch_unwind (AC7.4)`
<!-- END_TASK_11 -->

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
  caught); #363 status re-verified and recorded (Task 3 — AC7.5). #363 does NOT
  reproduce on the plain incremental path (Task 3 Part B), but DOES reproduce on
  the LTM-discovery synthetic-fragment path (surfaced by Task 2): a recoverable
  `PREVIOUS`-in-subscript `Err` escalated to a panic by `.unwrap().unwrap()` at
  `codegen.rs:494` — converted to a propagated typed `Err` in Task 11, letting
  the existing `db_ltm.rs:2083` `Err(_) => None` gracefully drop the
  un-scoreable LTM synthetic fragment (AC7.5).
- The two latent compile bugs the cleared cycle gate unmasked (GH #580) are
  fixed as general engine fixes, each with its own fast model-agnostic unit
  test: group-mapped subscript translation in temp-arg-helper extraction
  (Task 4 — Bug A, as-built: `db.rs::expand_maps_to_chains` canonicalization)
  and the arrayed-GF-in-reducer codegen gap (Task 5 — Bug B, as-built: the
  `LookupArray` opcode + `Expr3` decomposition). With both in, no synthetic
  temp-arg helper or real arrayed var remains in `compile_project_incremental`'s
  `missing_vars`. The shared `dimensions.rs` mapping resolver and the SCC
  element-graph compile path are regression-pinned (no LTM / cycle-gate
  behavior change). #580 closes when both land.
- Cross-fragment graphical-function de-duplication in `concatenate_fragments`
  (Task 6 — GH #582) removes the `GraphicalFunctionId = u8` overflow that
  Bug B's fix unmasked, matching the monolithic `Module::compile` dedup, with
  the combined-SCC-fragment machinery (`FragmentMerger` / `renumber_fragment_code`)
  and the 22-model corpus regression-pinned and the dedup proven value-exact
  (no wrong-table miscompile). #582 closes when Task 6 lands. Task 6 unmasked
  the sole remaining assembly-path ceiling (#583), addressed in Task 7.
- Fragment concatenation matches the monolithic `Module::compile` temp recycling
  (Task 7 — GH #583): the `TempId` overflow is resolved by recycling per-fragment
  temps into a shared keyed pool (~21, matching monolithic) plus removing the
  per-fragment `+ ctx_base.temps` re-add — NOT by widening `TempId` (the
  user-authorized forward sweep proved widening yields a runtime OOB; only
  `TempId` and `GraphicalFunctionId` are `u8`, both stay `u8`). The interleaved
  `combine_scc_fragment` (GH #575) path keeps disjoint temp ranges; the 22-model
  corpus + combined-fragment suite are regression-pinned and the recycle proven
  collision-free (no silent miscompile). With Tasks 4-7 in, C-LEARN's incremental
  compile reaches `Ok` and runs to FINAL TIME (AC7.1, AC7.2 met). #583 closes when
  Task 7 lands.
- No core C-LEARN series is entirely NaN (AC7.3): the two SPURIOUS engine bugs
  the run exposed (Ref.vdf is finite where Simlin is NaN/inf) are fixed as general
  engine fixes — a macro-`INITIAL()` module output omitted from the initials
  runlist (Task 8 — Cluster A) and arrayed `VECTOR SORT ORDER` returning flat
  global indices instead of per-iterated-slice 0-based ranks (Task 9 — Cluster B,
  completing the AC5 multi-row case). The 3rd, deepest cause is the `:NA:`
  mishandling (Task 10 — GH): Vensim `:NA:` is a finite sentinel `-2^109` (NOT
  NaN, per the user's domain correction), so Simlin's `:NA:`→IEEE-NaN poisons the
  `IF THEN ELSE(X = :NA:, …)` cascade; Task 10 represents `:NA:` as `-2^109`
  (series become finite → AC7.3 passes; existence tests gate correctly), keeping
  the structurally-distinct OOB→NaN / empty-reducer→NaN paths untouched. The
  22-model corpus, the AC5 single-row VSO suite, the OOB-NaN tests, the macro/init
  suites, and the recurrence gates are regression-pinned. The `-2^109`↔`0` VDF
  reconciliation (Simlin runtime vs Vensim's saved value) for the full `Ref.vdf`
  1% match is Phase 7's AC8.1, not Phase 6.
- The default engine suite stays green under the 3-minute `cargo test` cap
  (the new C-LEARN test is `#[ignore]`d / runtime-class).
<!-- END_PHASE_6 -->
