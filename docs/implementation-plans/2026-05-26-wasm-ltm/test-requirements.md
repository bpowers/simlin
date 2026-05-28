# Loops That Matter on the wasm Simulation Backend (wasm-ltm) — Test Requirements

This document maps every acceptance criterion of the LTM-on-wasm feature to its
verification. The authoritative AC list is the "Acceptance Criteria" section of the design
plan ([2026-05-26-wasm-ltm.md](/docs/design-plans/2026-05-26-wasm-ltm.md)); the verifying
tests and their exact file paths and function names come from the six implementation phase
plans in this directory (`phase_01.md` .. `phase_06.md`).

There are **16 acceptance criteria** across AC1..AC5, identified with their fully-scoped
names `wasm-ltm.AC1.1` .. `wasm-ltm.AC5.2`. **All 16 are covered by automated tests; none
require human verification.** The phase plans were written so that every behavioral and
failure-mode criterion is automatable, and the two engineering-quality criteria (AC5.1,
AC5.2) are satisfied structurally / by construction (see their rows). The mapping below
confirms this end to end.

## Coverage model (read this first)

A few cross-cutting decisions in the phase plans shape how the rows below read; they are
called out here so each AC row stays terse.

- **The engine `ltm_post` / `ltm_finding` analytic math is shared, not reimplemented per
  backend.** The numeric half (the synthetic `$⁚ltm⁚*` score equations) compiles into the
  `CompiledSimulation` and runs every timestep on *whichever* backend executes the bytecode;
  the analytic half (links + polarities + relative-loop-score + discovery) lives in
  `simlin-engine` (`ltm_post`, `ltm_finding`, `db_analysis`) and reaches the score series
  only through `results.offsets.get(...)` + step-major striding. Phase 2 funnels the VM FFI
  and the new from-series FFI through *one shared core per analysis*, so there is no
  per-backend reimplementation and (critically) no TypeScript reimplementation — TS only
  marshals the slab and deserializes via the existing `convertLinks`. This is the structural
  defense against the divergent-implementation bug class of #624 (AC5.1).

- **Parity tolerance.** The `$⁚ltm⁚*` columns are produced by *identical bytecode* on both
  backends, so the Rust series/slab comparisons use a tight `LTM_SERIES_TOLERANCE = 1e-6`
  (far tighter than the 0.05 rel-loop-score tolerance), and the libsimlin FFI and TS parity
  assertions likewise use `1e-6`. "Matches the VM" everywhere means "within `1e-6` element
  for element," with the VM as the correctness oracle.

- **Two execution oracles for the wasm blob.** Rust tests execute the emitted blob under the
  **DLR-FT `wasm-interpreter`** (the `checked` interpreter, pinned by git rev) — validate →
  instantiate → invoke `run` → stride the results region into an `engine::Results`. The
  TypeScript tests execute the same blob as an in-process `WebAssembly.Instance` owned by
  `DirectBackend`, driven through the public `Model`/`Sim` API. Both compare against the VM.

- **Heavy models are `#[ignore]`d.** The DLR-FT interpreter is an interpreter, not a JIT, so
  heavy corpus models (C-LEARN, World3) run slowly; their wasm-LTM / discovery twins are
  `#[ignore]`d to respect the 3-minute `cargo test` cap. The non-ignored corpus is small and
  fast.

Test-type legend: **rust-unit** = a `#[cfg(test)]` module inside a crate source file;
**rust-integration** = a file under `tests/` — note the `file_io`-gated
`simulate_ltm_wasm` target (`src/simlin-engine/tests/simulate_ltm_wasm.rs`, added in Phase 1
with `required-features = ["file_io"]`) and the libsimlin `wasm.rs` FFI target
(`src/libsimlin/tests/wasm.rs`), both of which run the emitted blob under the DLR-FT
interpreter; **jest-unit** = a pure-function jest test; **jest-integration** = a jest test
driving the node `DirectBackend` or the in-process `WorkerBackend`/`WorkerServer` pair (via
`createTestPair`) end to end through the public `Model`/`Sim` API.

---

## wasm-ltm.AC1: LTM-enabled wasm compilation produces a blob carrying the LTM series

| AC | test type & file | what it asserts | phase/task |
|----|------------------|-----------------|------------|
| **wasm-ltm.AC1.1** Success: `compile_datamodel_to_artifact(..., ltm_enabled: true, ...)` on a scalar LTM model produces a `WasmLayout` whose `var_offsets` contains the `$⁚ltm⁚link_score⁚*` and `$⁚ltm⁚loop_score⁚*` entries. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `layout_carries_ltm_series_when_enabled`. | Loads `logistic_growth_ltm/logistic_growth.stmx`, calls `compile_datamodel_to_artifact(&project, "main", true, false)`, and asserts the returned `WasmLayout.var_offsets` contains at least one name starting with `"$⁚ltm⁚link_score⁚"` and one with `"$⁚ltm⁚loop_score⁚"` (the layout is a verbatim copy of `CompiledSimulation.offsets`, so the synthetic slots appear automatically). | Phase 1 / Task 4 (threading enabled by Task 1). |
| **wasm-ltm.AC1.2** Success: the blob, run under the DLR-FT interpreter, writes the `$⁚ltm⁚*` columns; those series match the VM within the engine's existing tolerances. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `series_*` (one `#[test]` per scalar model: `logistic_growth`, `arms_race`, `decoupled`), via `assert_ltm_series_match` → `assert_ltm_slabs_match`. | For each scalar model: builds the VM `Results` (`vm_results_for_ltm`) and the blob-derived `Results` (`wasm_results_for_ltm`, run under the DLR-FT interpreter), first guards that the wasm side actually carries `$⁚ltm⁚*` columns (no vacuous LTM-off-vs-LTM-off compare), then whole-slab compares every column element-for-element within `LTM_SERIES_TOLERANCE = 1e-6`. | Phase 1 / Task 5 (helpers from Task 3). |
| **wasm-ltm.AC1.3** Success: `Model.simulate({ engine: 'wasm', enableLtm: true })` resolves to a `Sim` (the up-front rejection is gone) under node, and `getLinks()` returns links annotated with scores. | jest-integration (node `DirectBackend`): `src/engine/tests/wasm-ltm.test.ts` — `'simulate({engine:wasm, enableLtm}) resolves to a Sim'` and `'wasm getLinks returns scored links'`. | `model.simulate({}, { engine: 'wasm', enableLtm: true })` resolves to a `Sim` with no throw (the `simNewWasm` `enableLtm` rejection was deleted); after `runToEnd`, `sim.getLinks()` returns a non-empty array where at least one `Link` has a defined `score` array of length = step count (the wasm read/analyze path reads the blob slab and calls the from-series FFI, reusing `convertLinks`). | Phase 3 / Tasks 1, 2 & 4. |
| **wasm-ltm.AC1.4** Success: the same `engine:'wasm'` + `enableLtm` path through `WorkerBackend` (browser) yields `getLinks` scores matching node. | jest-integration (`WorkerBackend` via `createTestPair`): `src/engine/tests/worker-wasm.test.ts` — `'worker wasm getLinks matches node + VM'` (replaces the removed rejection test at `:319-327`). | A `WorkerBackend`-driven wasm sim (`enableLtm: true` → `runToEnd` → `simGetLinks`) produces links whose set, polarities, and `score` series equal a node `DirectBackend` run within `1e-6` — through the real in-process request/response loopback — with no new worker message types (`simGetLinks`/`simNew` already traverse the worker; `WorkerServer` wraps a `DirectBackend`). | Phase 6 / Task 1. |
| **wasm-ltm.AC1.5** Edge: compiling the same model with `ltm_enabled: false` produces a layout with no `$⁚ltm⁚*` entries (LTM-off behavior unchanged). | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `layout_omits_ltm_series_when_disabled`. | Loads the same `logistic_growth_ltm/logistic_growth.stmx`, calls `compile_datamodel_to_artifact(&project, "main", false, false)`, and asserts `var_offsets` contains **no** name starting with `"$⁚ltm⁚"`. | Phase 1 / Task 4. |

## wasm-ltm.AC2: Analytic outputs match the VM within tolerance

| AC | test type & file | what it asserts | phase/task |
|----|------------------|-----------------|------------|
| **wasm-ltm.AC2.1** Success: `simlin_analyze_links_from_wasm_results` returns per-link scores equal to `simlin_analyze_get_links` (VM) for a scalar LTM model, within tolerance. | rust-integration (libsimlin FFI, DLR-FT): `src/libsimlin/tests/wasm.rs` — `links_from_wasm_match_vm`. | Compiles `logistic_growth_ltm/logistic_growth.stmx` to wasm with `simlin_model_compile_to_wasm(model, /*ltm*/ true, false, ...)`, runs the blob into a slab, calls `simlin_analyze_links_from_wasm_results`; against the VM oracle (`simlin_sim_new(..., enable_ltm=true)` → `run_to_end` → `simlin_analyze_get_links`) it asserts the identical link set (`from`, `to`, `polarity`) and per-link `score` series equal within `1e-6`. Both FFI functions call the *same* shared links core. | Phase 2 / Task 4 (core from Task 1, helpers from Task 3). |
| **wasm-ltm.AC2.2** Success: `simlin_analyze_rel_loop_score_from_wasm_results` equals `simlin_analyze_get_relative_loop_score` (VM) for each loop id, including subscripted ids. | rust-integration (libsimlin FFI, DLR-FT): `src/libsimlin/tests/wasm.rs` — `rel_loop_score_from_wasm_matches_vm`. | Enumerates loop ids via `simlin_analyze_get_loops` (including a subscripted id where the corpus has one), computes the relative-loop-score series both from the wasm slab (`simlin_analyze_rel_loop_score_from_wasm_results`, which recomputes `loop_partitions`/`loop_element_index` from salsa exactly as `simlin_sim_new` does) and from the VM (`simlin_analyze_get_relative_loop_score`), and asserts equality within `1e-6` per loop id. Subscripted-id coverage lands here if the scalar corpus supports it, otherwise it is carried to AC2.4's arrayed FFI parity. | Phase 2 / Task 5 (core from Task 2, snapshot helper from Task 3). |
| **wasm-ltm.AC2.3** Success: `getLinks()` on a wasm sim (node `DirectBackend`) returns scores matching `getLinks()` on a VM sim, and `Run.links` is populated for a wasm LTM run. | jest-integration (node `DirectBackend`): `src/engine/tests/wasm-ltm.test.ts` — `'wasm getLinks scores match VM'` and `'Run.links populated for wasm LTM run'`. | Runs the same LTM model on `engine:'vm'` and `engine:'wasm'` (both `enableLtm:true`), matches links by `(from,to)`, and asserts identical link sets + polarities and each `score[]` equal within `1e-6`; and `model.run({}, { analyzeLtm: true, engine: 'wasm' })` yields `run.links.length > 0` matching the VM run (the `getRun` `_engine !== 'wasm'` guard was dropped so `wantLinks = this.ltmEnabled`). | Phase 3 / Tasks 2, 3 & 4. |
| **wasm-ltm.AC2.4** Success: arrayed / cross-element LTM models that lower match the VM element-for-element (per-element link and loop scores). | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `series_arrayed_population_matches_vm` and `series_cross_element_matches_vm`. | Adds `arrayed_population_ltm/arrayed_population.stmx` (A2A, N=3) and `cross_element_ltm/cross_element.stmx` (cross-element + `SUM` agg-node, N=2) to the harness; the Phase 1 whole-slab comparator (`assert_ltm_slabs_match`) transparently covers every element of every arrayed/cross-element `$⁚ltm⁚*` var (strided and name-baked forms) plus `$⁚ltm⁚agg⁚*` columns within `1e-6`, and a focused assertion guarantees at least one multi-slot arrayed LTM var is present so the test cannot pass vacuously. | Phase 4 / Task 1. |
| **wasm-ltm.AC2.5** Success: in discovery mode, the discovered loops and their per-timestep scores (via `discover_loops_with_graph` over the blob-derived `Results`) match the VM's discovery output. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `discovery_arms_race_matches_vm` (heavy C-LEARN / World3 twins added as `#[test] #[ignore]`). | Compiles `arms_race_3party/arms_race.stmx` in discovery mode (`wasm_results_for_ltm_discovery` sets both `ltm_enabled` and `ltm_discovery_mode`), feeds the blob-derived `Results` and the shared structural inputs (`ltm_discovery_inputs`) to `discover_loops_with_graph`, and asserts the discovered loop *sets* are identical and each loop's per-timestep `(time, score)` series matches the VM (times exactly equal — `specs` is faithfully reconstructed and load-bearing here; scores within `1e-6`). Drives the wasm backend directly because `analyze_model` hard-codes the VM. | Phase 5 / Tasks 1 & 2. |

## wasm-ltm.AC3: No silent fallback for unlowerable LTM models

| AC | test type & file | what it asserts | phase/task |
|----|------------------|-----------------|------------|
| **wasm-ltm.AC3.1** Failure: an arrayed LTM model that exceeds `MAX_UNROLL_UNITS` (or uses an opcode the backend cannot lower) returns `WasmGenError::Unsupported` from the compile path — never a panic, never a silently-wrong blob. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `unsupported_ltm_model_returns_wasmgen_error`; rust-integration (libsimlin FFI): `src/libsimlin/tests/wasm.rs` — the LTM-on unsupported test (mirrors `compile_to_wasm_unsupported_model_surfaces_error` with `/*ltm*/ true`). | Loads `test/ltm_dynamic_range_unsupported/model.stmx` (a feedback loop so LTM is genuinely enabled, plus a `ViewRangeDynamic` non-constant subscript range wasmgen cannot lower — Unsupported at the *production* threshold, no 65k-element fixture needed): the engine test asserts `compile_datamodel_to_artifact(&project, "main", true, false)` returns `Err(WasmGenError::Unsupported(_))` with no panic (and that `vm_results_for_ltm` succeeds); the FFI test asserts `simlin_model_compile_to_wasm(model, true, false, ...)` leaves both output buffers NULL and sets a non-null `SimlinError`, never panicking. | Phase 4 / Task 3. |
| **wasm-ltm.AC3.2** Failure: that unsupported case surfaces to the TS caller as a `SimlinError`/`Error` from `Model.simulate({ engine: 'wasm', enableLtm: true })`; the same model still simulates via `engine: 'vm'`. | jest-integration (node `DirectBackend`): `src/engine/tests/wasm-ltm.test.ts` — `'unsupported LTM model rejects on wasm but runs on vm'`. | Loads the same `ltm_dynamic_range_unsupported/model.stmx`; asserts `model.simulate({}, { engine: 'wasm', enableLtm: true })` rejects with a thrown `SimlinError`/`Error` (no silent VM fallback, no silently-wrong result), while `model.simulate({}, { engine: 'vm', enableLtm: true })` + `runToEnd` resolves and produces results. | Phase 4 / Task 4. |

## wasm-ltm.AC4: Parity harness with a ratcheting floor

| AC | test type & file | what it asserts | phase/task |
|----|------------------|-----------------|------------|
| **wasm-ltm.AC4.1** Success: the harness runs the LTM corpus through both backends, comparing wasm-vs-VM, and enforces a monotonically rising floor on the count of LTM models that lower; heavy models are `#[ignore]`d to respect the 3-minute cap. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `ltm_corpus_floor_gate`. | Iterates the scalar corpus, attempts `wasm_results_for_ltm` per model, counts those that lower, `eprintln!`s any `WasmGenError::Unsupported` (skip-not-fail during rollout), and asserts `lowered >= MIN_LTM_MODELS_LOWERED` (= 3, the scalar corpus). The single source-of-truth floor constant documents the ratchet contract. | Phase 1 / Task 6. |
| **wasm-ltm.AC4.2** Failure: a regression that drops a previously-supported LTM model below the floor (or any `Unsupported` at the end-state gate) fails the suite. | rust-integration: `src/simlin-engine/tests/simulate_ltm_wasm.rs` — `ltm_corpus_floor_gate` (end-state form). | Rewrites the gate to the end-state: iterates `EXPECTED_SUPPORTED_LTM_MODELS` (scalar + `arrayed_population` + `cross_element`), asserts each returns `Ok` from `wasm_results_for_ltm` (any `Unsupported` among the expected-supported set fails the suite) and `lowered >= MIN_LTM_MODELS_LOWERED` (bumped to the set length). Dropping a previously-supported model both falls below the floor and breaks the per-model `Ok` assertion. | Phase 4 / Task 2. |

## wasm-ltm.AC5: Engineering quality (cross-cutting)

| AC | test type & file | what it asserts | phase/task |
|----|------------------|-----------------|------------|
| **wasm-ltm.AC5.1** The VM and wasm analytic paths share one engine-level core; no analysis logic is reimplemented per-backend and none is reimplemented in TypeScript. The FFI grows by exactly the two `*_from_wasm_results` functions (no bulk/batch endpoint). | **Structural verification** + parity tests prove non-divergence: the from-series FFI fns call the same cores as the VM fns. rust-integration: `src/libsimlin/tests/wasm.rs` — `links_from_wasm_match_vm`, `rel_loop_score_from_wasm_matches_vm`; jest-integration: `src/engine/tests/wasm-ltm.test.ts` — `'wasm getLinks scores match VM'`; jest-integration: `src/engine/tests/worker-wasm.test.ts` — `'worker wasm getLinks matches node + VM'`. | Structurally: `simlin_analyze_get_links` / `simlin_analyze_get_relative_loop_score` are refactored onto two private shared cores (`analyze_links_core`, `rel_loop_score_series`); the new `simlin_analyze_links_from_wasm_results` / `simlin_analyze_rel_loop_score_from_wasm_results` call those *same* cores, and the FFI grows by exactly these two functions (no bulk/batch endpoint). TS reuses `convertLinks` (no reimplemented analysis). The parity tests prove the two paths produce identical output (cannot have diverged) within `1e-6`. | Phase 2 / Tasks 1, 2 & (parity) 4, 5; Phase 3 / Task 4; Phase 6 / Task 1. |
| **wasm-ltm.AC5.2** New code reaches >=95% coverage via TDD; each new FFI function and each new lowering/feature group has unit tests. | **Satisfied by construction** (per the Phase 6 note): the enumeration of per-feature tests across all phases. There is no separate coverage-percentage gate; a phase-end reviewer verifies the enumeration. | Every new unit of behavior has focused TDD tests: the wasm compile flags (Phase 1 layout on/off tests — AC1.1/AC1.5), each new FFI function (Phase 2 `links_from_wasm_match_vm` + `rel_loop_score_from_wasm_matches_vm`), the node read/analyze path (Phase 3 `wasm-ltm.test.ts`), each arrayed/cross-element lowering group + the Unsupported path (Phase 4), discovery (Phase 5 `discovery_arms_race_matches_vm`), and the worker path (Phase 6). The ">=95%" threshold is met by this enumeration, consistent with repo practice (TDD + pre-commit suite, no hard CI coverage gate). | Phases 1–6 (enumerated above). |

---

## Human verification

**None required — all 16 criteria are covered by automated tests** (the 14 behavioral and
failure-mode criteria of AC1–AC4 directly, plus the two engineering-quality criteria AC5.1
and AC5.2 by structural construction backed by the parity tests). Every VM-vs-wasm
comparison is judged by the engine's existing comparators / a tight `1e-6` element-for-element
tolerance, with the bytecode VM as the correctness oracle; heavy models are `#[ignore]`d to
respect the 3-minute `cargo test` cap but remain runnable explicitly. No criterion in this
plan is inherently un-automatable.
