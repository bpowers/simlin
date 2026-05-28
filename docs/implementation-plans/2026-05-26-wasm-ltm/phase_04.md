# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 4: Arrayed/cross-element LTM + Unsupported hardening

**Goal:** Arrayed (apply-to-all / subscripted / cross-element) LTM models that lower match the VM element-for-element, and a genuinely-unlowerable LTM model returns an explicit `WasmGenError::Unsupported` end-to-end (no panic, no silently-wrong blob, no silent VM fallback); the floor ratchets up to include the supported arrayed models.

**Architecture:** The Phase 1 harness already compares the *whole* result slab element-wise (`assert_ltm_slabs_match`), which inherently covers per-element arrayed LTM scores (an arrayed LTM var occupies contiguous slots, read by striding `base + elem`, not separate names). So arrayed parity (AC2.4) is just adding the arrayed corpus to that harness. For the Unsupported path, rather than building a 65,536-element fixture to trip `MAX_UNROLL_UNITS` (forbidden by the test-time-budget rule), this phase uses a tiny model that is Unsupported at the *production* threshold via an unlowerable opcode (`ViewRangeDynamic` — a non-constant subscript range, GH #612), combined with a small feedback loop so LTM is genuinely enabled. One committed XMILE fixture serves both the Rust (AC3.1) and TS (AC3.2) assertions.

**Tech Stack:** Rust (`wasmgen`, `simlin-engine` tests, `libsimlin` FFI tests), TypeScript (`@simlin/engine` jest). DLR-FT interpreter for blob execution. XMILE fixture.

**Scope:** Phase 4 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC2: Analytic outputs match the VM within tolerance
- **wasm-ltm.AC2.4 Success:** arrayed / cross-element LTM models that lower match the VM element-for-element (per-element link and loop scores).

### wasm-ltm.AC3: No silent fallback for unlowerable LTM models
- **wasm-ltm.AC3.1 Failure:** an arrayed LTM model that exceeds `MAX_UNROLL_UNITS` (or uses an opcode the backend cannot lower) returns `WasmGenError::Unsupported` from the compile path — never a panic, never a silently-wrong blob.
- **wasm-ltm.AC3.2 Failure:** that unsupported case surfaces to the TS caller as a `SimlinError`/`Error` from `Model.simulate({ engine: 'wasm', enableLtm: true })`; the same model still simulates via `engine: 'vm'`.

### wasm-ltm.AC4: Parity harness with a ratcheting floor
- **wasm-ltm.AC4.2 Failure:** a regression that drops a previously-supported LTM model below the floor (or any `Unsupported` at the end-state gate) fails the suite.

---

## Background: what exists today (verified 2026-05-27)

**Arrayed LTM fixtures (both exist, no expected-output files):**
- `test/arrayed_population_ltm/arrayed_population.stmx` (3,742 bytes) — A2A same-element loops over `Region = {NYC, Boston, LA}` (N=3).
- `test/cross_element_ltm/cross_element.stmx` (4,476 bytes) — cross-element loops (`migration_pressure[NYC] = (population[NYC]-population[Boston])*0.01`) + a `SUM(population[*])` reducer (agg-node path), `Region = {NYC, Boston}` (N=2).
- Both are exercised VM-only today by `tests/simulate_ltm.rs` (`test_arrayed_population_ltm_exhaustive` `:5008-5124`, `test_cross_element_ltm_exhaustive` `:5261`), so VM-as-oracle is established. Both are far under `MAX_UNROLL_UNITS` (N=2/3), so they **lower** to wasm.

**Per-element score layout (the key subtlety):** a per-element LTM score is **not** a separate named column. Each edge/loop is ONE synthetic var occupying N contiguous result slots (one per A2A element), read by striding `base_offset + elem`. `test_arrayed_population_ltm_exhaustive` filters `Results.offsets` by the prefix `$⁚ltm⁚link_score⁚` then reads `base_offset + elem`. Name forms (`ltm_augment.rs:1105-1135`):
- Bare A2A (same-element): `$⁚ltm⁚link_score⁚{from}→{to}` — element in the slot stride.
- FixedIndex (cross-element): `$⁚ltm⁚link_score⁚{from}[{elems}]→{to}` — element(s) in the name brackets (single slot).
- Scalar→arrayed: `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`.
- Loop scores: prefix `$⁚ltm⁚loop_score⁚`.

> Because Phase 1's `assert_ltm_slabs_match` compares the **whole slab** (both `Results` share the identical `var_offsets`), it transparently covers all three forms (strided and name-baked elements) with no per-var slot-span logic. Phase 4 reuses it.

**The Unsupported path (`MAX_UNROLL_UNITS` is a hard const; no override needed):**
- `wasmgen/lower.rs:1106` `const MAX_UNROLL_UNITS: usize = 65_536;`; `charge_unroll` (`:1115-1124`) returns `WasmGenError::Unsupported("...unrolling exceeds...")`. Existing unit tests trip it *cheaply* via a `dense_view(0, &[300,300])` (`size()` = 90,000) at `lower_tests.rs:3585-3641` — the cost is in `size()`, not materialization. (If a production-XMILE-path override is ever needed, replicate the `AggLoopBudgetGuard` RAII `thread_local` idiom at `db_ltm.rs:3104-3158`. **Not needed here.**)
- The TS-reachable Unsupported trigger is `ViewRangeDynamic` (GH #612): a subscript with non-constant range bounds, e.g. `SUM(source[lo:hi])`. The existing libsimlin graceful-failure test `compile_to_wasm_unsupported_model_surfaces_error` (`libsimlin/tests/wasm.rs:350-404`) builds exactly this via `TestProject` (`indexed_dimension("A",5)`, `array_aux("source[A]","A")`, `scalar_aux("lo","2")`, `scalar_aux("hi","4")`, `scalar_aux("total","SUM(source[lo:hi])")`) and asserts `msg.contains("ViewRangeDynamic") || msg.contains("code generation failed")` with both buffers NULL. This is Unsupported at the **production** threshold — usable identically from Rust and TS.
- Error propagation: `wasmgen` `Unsupported(String)` → `compile_datamodel_to_artifact` (`module.rs:114-125`) → `simlin_model_compile_to_wasm` (`model.rs:157-170`: `SimlinErrorCode::Generic`, message `"wasm code generation failed: {err}"`, buffers NULL, **never panics**) → TS `wasmgen.ts:112-118` (`throw new SimlinError(...)`, **no VM fallback** per `src/engine/CLAUDE.md`).

**The VM still handles `ViewRangeDynamic`** (it is a wasmgen-only limitation), so the same fixture simulates fine via `engine:'vm'` (AC3.2's second clause).

**Floor gate from Phase 1:** `MIN_LTM_MODELS_LOWERED` + `ltm_corpus_floor_gate` in `tests/simulate_ltm_wasm.rs` (scalar corpus, `lowered >= MIN`, reports `Unsupported`).

**Divergence from the design doc:** (1) the design implies a `MAX_UNROLL_UNITS`-tripping fixture; per the test-budget rule we instead trip Unsupported via `ViewRangeDynamic` with a tiny fixture (the engine-internal unroll-cap path is already unit-tested cheaply at `lower_tests.rs:3585`). (2) No combined "LTM loop + dynamic-range" fixture exists yet — Phase 4 creates one.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Arrayed / cross-element LTM series parity (element-for-element)

**Verifies:** wasm-ltm.AC2.4

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs`

**Implementation / Testing:**
1. Add the arrayed corpus paths: `arrayed_population_ltm/arrayed_population.stmx`, `cross_element_ltm/cross_element.stmx`.
2. `#[test] fn series_arrayed_population_matches_vm()` → `assert_ltm_series_match("arrayed_population_ltm/arrayed_population.stmx")`; `#[test] fn series_cross_element_matches_vm()` → `assert_ltm_series_match("cross_element_ltm/cross_element.stmx")`. The Phase 1 `assert_ltm_series_match` + `assert_ltm_slabs_match` whole-slab comparator already covers every element of every arrayed/cross-element `$⁚ltm⁚*` var (strided and name-baked forms alike) plus the agg-node (`$⁚ltm⁚agg⁚*`) columns — no new comparison logic needed.
3. Add a focused assertion that at least one **arrayed** (multi-slot) LTM var is present (e.g. a `$⁚ltm⁚loop_score⁚*` whose element count > 1, or simply that the model's `Region` dimension is reflected) so the test cannot pass vacuously on a scalar reduction.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm series_arrayed series_cross`
Expected: both arrayed parity tests pass (per-element match within `LTM_SERIES_TOLERANCE`).

**Commit:** `engine: verify arrayed and cross-element LTM series match the VM`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Ratchet and tighten the floor gate to the end-state

**Verifies:** wasm-ltm.AC4.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs`

**Implementation / Testing:**
1. Introduce `const EXPECTED_SUPPORTED_LTM_MODELS: &[&str] = &[ ...scalar from Phase 1... , "arrayed_population_ltm/arrayed_population.stmx", "cross_element_ltm/cross_element.stmx" ];` and bump `MIN_LTM_MODELS_LOWERED` to its length (the monotonically-risen floor).
2. Rewrite `ltm_corpus_floor_gate` to the **end-state** form: iterate `EXPECTED_SUPPORTED_LTM_MODELS`, assert each returns `Ok` from `wasm_results_for_ltm` (any `Unsupported` among the expected-supported set **fails** the suite — AC4.2), and assert `lowered >= MIN_LTM_MODELS_LOWERED`. Keep an `eprintln!` of any failure detail. A regression that drops a previously-supported model now fails (it both falls below the floor and breaks the per-model `Ok` assertion).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm ltm_corpus_floor_gate`
Expected: passes with the raised floor; flipping any expected-supported model to non-lowering would fail it.

**Commit:** `engine: ratchet LTM-on-wasm floor to include arrayed models`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Unsupported LTM fixture + Rust clean-error assertions

**Verifies:** wasm-ltm.AC3.1

**Files:**
- Add: `test/ltm_dynamic_range_unsupported/model.stmx`
- Modify: `src/simlin-engine/tests/simulate_ltm_wasm.rs` (engine-level assertion)
- Modify: `src/libsimlin/tests/wasm.rs` (FFI-level assertion)

**Implementation:**
1. Author `test/ltm_dynamic_range_unsupported/model.stmx`: a minimal LTM model — one stock with an inflow that reads the stock back (a feedback loop, so LTM emits link/loop scores) — PLUS an arrayed aux with a non-constant subscript range that wasmgen cannot lower, mirroring `wasm.rs:350-404`'s `TestProject`: a dimension `A` (size ~5), `source[A]`, scalar auxes `lo`/`hi`, and `total = SUM(source[lo:hi])`. (Cross-check the model parses and simulates on the VM with LTM on before asserting wasm-Unsupported.)
2. Engine-level test `unsupported_ltm_model_returns_wasmgen_error` in `simulate_ltm_wasm.rs`: load the fixture; assert `compile_datamodel_to_artifact(&project, "main", true, false)` returns `Err(WasmGenError::Unsupported(_))` (no panic); and assert `vm_results_for_ltm(&project, "main")` succeeds (the model is fine on the VM — proving it's a wasm-only limitation).
3. FFI-level test in `wasm.rs` (mirror `compile_to_wasm_unsupported_model_surfaces_error` `:350-404`, but LTM on): `simlin_model_compile_to_wasm(model, /*ltm*/ true, false, &mut wasm, &mut wasm_len, &mut layout, &mut layout_len, &mut err)` leaves both output buffers NULL and stores a non-null `SimlinError`; never panics.

**Testing:** the two Rust tests are the AC3.1 deliverable (engine-level `WasmGenError::Unsupported` + FFI-level clean `SimlinError`, no panic, no blob).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm_wasm unsupported_ltm && cargo test -p libsimlin --test wasm unsupported`
Expected: both assert a clean error with no panic.

**Commit:** `engine: clean WasmGenError for an unlowerable LTM model`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Unsupported surfaces to TS as an Error; VM still works

**Verifies:** wasm-ltm.AC3.2

**Files:**
- Modify: `src/engine/tests/wasm-ltm.test.ts`

**Implementation / Testing:**
1. `'unsupported LTM model rejects on wasm but runs on vm'` (AC3.2): load `test/ltm_dynamic_range_unsupported/model.stmx` (via the Phase 3 fixture loader, path `../../../test/ltm_dynamic_range_unsupported/model.stmx`).
   - `await expect(model.simulate({}, { engine: 'wasm', enableLtm: true })).rejects.toThrow()` — assert it rejects with a `SimlinError`/`Error` (no silent fallback, no silently-wrong result).
   - `const vmSim = await model.simulate({}, { engine: 'vm', enableLtm: true }); await vmSim.runToEnd();` resolves and produces results (the same model simulates via the VM). Optionally assert `vmSim.getLinks()` returns links.

**Verification:**
Run: `cd src/engine && npx jest tests/wasm-ltm.test.ts -t 'unsupported'`
Then: `pnpm --filter @simlin/engine test`
Expected: wasm path rejects; vm path resolves; suite green.

**Commit:** `engine: surface unlowerable LTM wasm compile as an error (no VM fallback)`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

---

## Phase 4 Done When

- Arrayed (`arrayed_population`) and cross-element (`cross_element`) LTM models lower to wasm and match the VM **element-for-element** (per-element link, loop, and agg scores) within `LTM_SERIES_TOLERANCE` (**wasm-ltm.AC2.4**).
- An LTM model using an unlowerable construct returns `WasmGenError::Unsupported` from the engine compile path and a clean `SimlinError` (NULL buffers, no panic) from the FFI (**wasm-ltm.AC3.1**); it surfaces to the TS caller as a thrown `Error` from `Model.simulate({engine:'wasm', enableLtm:true})` while `engine:'vm'` still simulates it (**wasm-ltm.AC3.2**).
- The floor gate is ratcheted to the arrayed-inclusive expected-supported set and fails on any regression or end-state `Unsupported` (**wasm-ltm.AC4.2**).
- `cargo test --workspace` and `pnpm --filter @simlin/engine test` are green.
