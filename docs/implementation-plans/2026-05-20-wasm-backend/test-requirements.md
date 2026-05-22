# WebAssembly Simulation Backend — Test Requirements

This document maps every acceptance criterion from the design plan
([`docs/design-plans/2026-05-20-wasm-backend.md`](../../design-plans/2026-05-20-wasm-backend.md),
the authoritative AC list) to its verification. There are 8 AC groups and 22
individual cases (AC1.1–1.5, AC2.1–2.2, AC3.1–3.3, AC4.1–4.2, AC5.1–5.2,
AC6.1–6.2, AC7.1–7.4, AC8.1–8.2). The phase mappings come from the
`**Verifies:**` lines in [`phase_01.md`](phase_01.md) … [`phase_08.md`](phase_08.md).

## Verification conventions

This backend is engine-internal and is validated against the bytecode VM as the
correctness oracle, so verification is almost entirely automated. Two test
surfaces recur throughout:

- **Unit (inline `#[cfg(test)] mod tests`)** in the relevant
  `src/simlin-engine/src/wasmgen/*.rs` file. Each unit test hand-builds a tiny
  `ByteCode`/`CompiledSimulation`, emits a wasm module with `wasm-encoder`,
  validates it (`wasm::validate`), instantiates it under the DLR-FT
  `wasm-interpreter` via the `checked` crate's `Store`, invokes the export, and
  asserts on linear memory / return values against the VM's matching handler
  (the executable spec). Files: `wasmgen/lower.rs`, `wasmgen/module.rs`,
  `wasmgen/math.rs`, `wasmgen/lookup.rs` (if split out; otherwise in
  `lower.rs`), `wasmgen/views.rs`, `wasmgen/vector.rs`, `wasmgen/alloc.rs`.
- **Integration / corpus** in `src/simlin-engine/tests/simulate.rs` (and
  `src/simlin-engine/tests/simulate_systems.rs`). The `ensure_wasm_matches`
  hook runs each supported corpus model through the wasm backend after the VM
  run and feeds its results through the model's existing comparator; the
  `wasm_parity_floor` gate enforces a monotonically rising count of
  wasm-supported models; the `#[ignore]`d `simulates_clearn_wasm` twin checks
  C-LEARN against `Ref.vdf`.

**The correctness bar is the existing comparators, not a separate
wasm-vs-VM threshold.** A model's wasm output must clear the same
`ensure_results` (abs `2e-3` / Vensim-relative `5e-6`) or `ensure_vdf_results`
(1% `VDF_RTOL` + the `EXPECTED_VDF_RESIDUAL` carve-out) check the VM clears,
against the same expected outputs. "wasm-vs-VM parity" is achieved because both
backends clear the same comparator against the same expected outputs — there is
no tighter backend-equivalence tolerance (design "Validation bar"; reflected in
AC1.1, AC1.3, and AC7.4 below).

---

## AC1: The wasm backend reproduces the VM's simulation results

| AC | Literal text | Verification |
|---|---|---|
| **AC1.1** (Success) | A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` against the model's expected outputs at those tests' existing tolerances. (No separate, tighter wasm-vs-VM threshold.) | **Automated — integration.** The `ensure_wasm_matches` hook in `src/simlin-engine/tests/simulate.rs` (`simulate_path_with_excluding` + the `.mdl` path) runs each supported model through the backend and asserts via the existing `ensure_results_excluding` comparator (the same check the VM passes; no separate threshold). The supported set widens each phase: scalar/Euler (Phase 1), full scalar builtins (Phase 2), graphical functions (Phase 3), RK + PREVIOUS/INIT (Phase 4), arrays (Phase 5), vector ops/allocation (Phase 6), modules + systems format (Phase 7). The per-phase floor raise in `wasm_parity_floor` records the widening. Per-opcode correctness is also covered by the unit tests under each AC below. |
| **AC1.2** (Success) | Arrayed/subscripted models (apply-to-all, subscripts, vector operations) match the VM element-for-element. | **Automated — unit + integration.** Unit: reducer/iteration/subscript parity vs the VM in `wasmgen/views.rs` and `wasmgen/lower.rs` (Phase 5: subscript OOB→NaN, broadcast, each reducer, iteration loops) and the vector-op/allocation parity tests in `wasmgen/vector.rs` and `wasmgen/alloc.rs` (Phase 6: VectorSelect/ElmMap/SortOrder/Rank/LookupArray/Allocate). Integration: arrayed corpus models clear `ensure_results` via `ensure_wasm_matches` and raise the floor (Phase 5 Task 5, Phase 6 Task 5). A2A variables are unrolled to scalar bytecode by the compiler, so they are additionally covered by the Phase 1/2 scalar path. |
| **AC1.3** (Success) | C-LEARN runs through the wasm backend and matches `Ref.vdf` / the VM under the existing VDF tolerance and the `EXPECTED_VDF_RESIDUAL` carve-out. | **Automated — integration (`#[ignore]`d).** Phase 8 Task 2 adds `#[test] #[ignore] fn simulates_clearn_wasm()` in `src/simlin-engine/tests/simulate.rs`, reusing `run_clearn_vs_vdf()`'s compile path, running the blob under DLR-FT, and asserting `ensure_vdf_results_excluding(&vdf, &wasm_results, EXPECTED_VDF_RESIDUAL)` — the same check `simulates_clearn` uses. `#[ignore]`d for runtime (interpreter is not a JIT); run via `cargo test --release --features file_io -- --ignored simulates_clearn_wasm`. |
| **AC1.4** (Failure) | A model using a not-yet-supported construct returns `WasmGenError::Unsupported` — a clean error, never a panic or a silently wrong result. | **Automated — unit + integration (negative path).** Unit: Phase 1 Task 1 asserts unsupported opcodes (`Op2::Eq`/`Op2::Mod`/`Apply`/`Lookup`/an array opcode at that point) return `WasmGenError::Unsupported` rather than panicking, in `wasmgen/lower.rs`. Integration end state: Phase 8 Task 1 flips the `simulate.rs` hook so any `Unsupported` for a VM-simulated core model is a hard failure (never a silent wrong result); a deliberately-introduced `Unsupported` must fail the suite. |
| **AC1.5** (Edge) | Empty-view reducers, out-of-bounds subscripts, and division-by-zero produce the same NaN / finite-`:NA:` / Inf values the VM produces. | **Automated — unit (edge path), split across phases.** Phase 1 Task 1: raw `Op2::Div` by zero (`x/0`→±Inf, `0/0`→NaN, IEEE-identical to the VM) in `wasmgen/lower.rs`. Phase 2 Task 1: the finite `:NA:` sentinel (`crate::float::NA`) vs genuine IEEE NaN, kept distinct by the `approx_eq` helper (curated sample incl. `(NA,NA)`/`(NA,0.0)`/`(NaN,NaN)`) in `wasmgen/lower.rs`. Phase 5 Task 2 + Task 4: empty-but-valid reducers (`ArraySum`→0.0; Max/Min/Mean/Stddev→NaN) and invalid-view→NaN for all reducers; out-of-bounds subscripts→NaN (pinned against `array_tests.rs` cases) in `wasmgen/lower.rs`/`wasmgen/views.rs`. |

## AC2: The backend consumes the salsa compiled bytecode

| AC | Literal text | Verification |
|---|---|---|
| **AC2.1** (Success) | The wasm module is produced from `compile_project_incremental(...) -> CompiledSimulation`, not from the `Expr` IR or the monolithic `compiler::Module`. | **Automated — unit (Phase 1).** Task 1: each scalar-core opcode lowers from `ByteCode.code` (bytecode, not `Expr`), unit-tested in `wasmgen/lower.rs`. Task 2: `compile_simulation(&CompiledSimulation)` builds the module from a `CompiledSimulation` produced by `compile_project_incremental`, unit-tested in `wasmgen/module.rs` against `Vm::new(sim).run_to_end()`. Task 3: `compile_datamodel_to_wasm` is rerouted through the salsa pipeline, and `wasmgen/expr.rs` (the `Expr`-tree path) is deleted — verified by `cargo test -p simlin-engine --features file_io wasmgen` plus the structural check that no `crate::compiler::Module` references remain in `wasmgen/`. |
| **AC2.2** (Success) | The POC's `#[cfg(test)]` un-gating of the monolithic builder is reverted; the crate builds with `Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map` test-only again. | **Automated — build state (Phase 1 Task 4).** Operational verification (a visibility/gating revert, no new behavior): `cargo build -p simlin-engine` builds with the four items `#[cfg(test)]`-gated again; `cargo test -p simlin-engine --features file_io` still compiles and passes (test code reaches the now-test-only builder); `git diff main -- src/simlin-engine/src/compiler/mod.rs` shows only the re-gating. |

## AC3: simulate.rs runs the corpus through both backends

| AC | Literal text | Verification |
|---|---|---|
| **AC3.1** (Success) | During rollout, each corpus model runs through the VM and (when supported) the wasm backend, comparing wasm-vs-VM; unsupported models are skipped (not failed) and counted against a monotonically rising floor. | **Automated — integration (Phase 1 Tasks 5–6).** `ensure_wasm_matches` returns `WasmRunOutcome::Ran | Skipped(msg)` (`src/simlin-engine/tests/test_helpers.rs`); the inline hook in `simulate.rs` records `Skipped` rather than failing; `const WASM_SUPPORTED_FLOOR` + `#[test] fn wasm_parity_floor()` count `Ran` models and assert `ran >= WASM_SUPPORTED_FLOOR`. The floor is raised in every subsequent functionality phase (Phases 2–7 each have a "raise the floor" task). |
| **AC3.2** (Success) | End state — no core-simulation model is skipped: every XMILE, MDL, and systems-format model in the corpus runs through both backends. | **Automated — integration (Phase 8 Task 1).** The harness flips: the skip-counting branch is removed and the gate asserts every VM-simulated core-simulation model in the default suite runs through wasm with zero `Unsupported`, across `src/simlin-engine/tests/simulate.rs` (XMILE + `.mdl`) and `src/simlin-engine/tests/simulate_systems.rs` (systems format). |
| **AC3.3** (Failure) | A regression that makes a previously-supported model unsupported (dropping below the floor, or any `Unsupported` at the end-state gate) fails the test suite. | **Automated — integration (Phase 8 Task 1); the gate itself is the test.** During rollout, dropping below `WASM_SUPPORTED_FLOOR` fails `wasm_parity_floor` (Phase 1). At the end state, any `Unsupported` for a VM-simulated core model is a hard `panic!` in the hook + the closed gate. Confirmed by temporarily introducing an `Unsupported` and observing the suite fail. |

## AC4: Self-describing results + efficient by-name retrieval

| AC | Literal text | Verification |
|---|---|---|
| **AC4.1** (Success) | The blob exports `n_slots`/`n_chunks`/`results_offset` and writes step-major snapshots; a host locates and strides the results with no external metadata. | **Automated — unit (Phase 1 Task 2; reaffirmed Phase 7 Task 3).** A dedicated test in `wasmgen/module.rs` reads the three exported i32 globals from the instantiated module (`instance_export(inst, "n_slots").as_global()`, etc.), asserts they equal the `WasmLayout` values, then uses only the module-exported geometry to stride to one variable's series and confirms it matches the VM. Phase 7 Task 3 reaffirms geometry-from-globals matches the layout alongside the FFI test. |
| **AC4.2** (Success) | Reading one variable's series via the name→offset layout copies only that variable's `n_chunks` values (never the whole `n_chunks × n_slots` slab) and equals the VM's series for that variable. | **Automated — unit / integration (Phase 7 Task 3).** A `wasmgen`/libsimlin test reads one variable's `n_chunks`-long series via `WasmLayout.var_offsets` (striding `results[results_offset + (c*n_slots + off)*8]`), asserts it equals the VM's `get_series` for that variable, and asserts only `n_chunks` values were copied (not the whole slab). |

## AC5: Override + reset

| AC | Literal text | Verification |
|---|---|---|
| **AC5.1** (Success) | Overriding a constant via `set_value`, then `reset`, then `run`, yields the same series the VM produces under the same override (matching `simlin_sim_set_value`/`reset` semantics). | **Automated — unit (Phase 7 Task 2).** A test in `wasmgen/module.rs` calls `set_value(off_of_a_constant, v); reset(); run();` on the blob and compares the full series to a VM run with `vm.set_value(ident, v)` under the same override. A `set_value` on a non-constant offset is asserted to return the error code with no write. |
| **AC5.2** (Success) | `reset` with no override restores the compiled-default results. | **Automated — unit (Phase 7 Task 2).** A test in `wasmgen/module.rs` calls `reset(); run()` with no override and asserts the blob reproduces the compiled-default series. |

## AC6: libsimlin FFI

| AC | Literal text | Verification |
|---|---|---|
| **AC6.1** (Success) | `simlin_model_compile_to_wasm` returns a valid wasm blob plus the name→offset layout via the malloc-return convention; both buffers are freeable with `simlin_free`; it works before any `SimlinSim` exists. | **Automated — integration FFI (Phase 7 Task 3).** A Rust integration test in `src/libsimlin/` compiles a model to wasm + serialized layout, asserts the wasm validates, the layout deserializes to the expected geometry + name→offset map, both buffers free with `simlin_free`, and the call works from only a `SimlinModel` (no `SimlinSim`). |
| **AC6.2** (Failure) | A model that cannot be compiled to wasm surfaces a `SimlinError` rather than panicking across the FFI boundary. | **Automated — integration FFI (negative path, Phase 7 Task 3).** A `src/libsimlin/` test feeds a model that fails codegen and asserts the `out_error` (`*mut *mut SimlinError`) is set via `store_error`/`store_anyhow_error` with no panic across the boundary. |

## AC7: Numeric-parity specifics

| AC | Literal text | Verification |
|---|---|---|
| **AC7.1** (Success) | Math wasm provides natively (`sqrt`, `abs`, `floor`/`ceil`/`trunc`/`nearest`, `min`/`max`, arithmetic) uses wasm instructions; the transcendentals wasm lacks (`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`exp`/`ln`/`log10`/`pow`) and the allocation `erfc` are open-coded as self-contained wasm helper functions (range reduction + polynomial). Each open-coded helper has a unit test comparing its output to Rust `f64` over a sampled range; results need not be bit-identical to the VM's libm — only close enough that the existing tests pass. | **Automated — unit, split across phases.** Phase 2 Task 2: each scalar transcendental helper (`exp`/`ln`/`sin`/`cos`/`atan` kernels + `tan`/`acos`/`log10`/`pow`/`asin` composed) emitted in `wasmgen/math.rs` gets a unit test comparing wasm output to Rust `f64` over a dense sampled domain + NaN/inf edges, with a documented tolerance comfortably inside the `simulate.rs` tolerances. Phase 2 Task 4 confirms native instructions are used for `Abs`/`Sqrt`/`Int`/`Min`/`Max`. Phase 3 Task 2: the lookup kernels tested against the VM's `lookup`/`lookup_forward`/`lookup_backward`. Phase 6 Task 4: `erfc_approx`/`normal_cdf` (in `wasmgen/alloc.rs`) unit-tested against the Rust `alloc::erfc_approx`/`normal_cdf`. |
| **AC7.2** (Success) | Equality and truthiness (`Eq`/`Neq`/`And`/`Or`/`If` condition) use ULP-based `approx_eq` matching the VM. | **Automated — unit (Phase 2 Task 1).** An `approx_eq(a,b)->i32` wasm helper reproduces `float_cmp::approx_eq!(f64, …)` (epsilon + 4-ulp ordered-integer algorithm) bit-faithfully; a unit test in `wasmgen/lower.rs` runs it under DLR-FT over a curated + randomized sample (exact-equal, far, 1–4 ULP, EPSILON-apart, subnormals, `(NaN,NaN)`, `(NA,NA)`, `(NA,0.0)`, `(±0)`, `(±inf)`) and asserts equality with `crate::float::approx_eq`. Further tests confirm `Op2::Eq`, `Op2::And`, `Op2::Or`, `Not`, and `SetCond`+`If` match the VM for near-zero/ULP-adjacent operands where raw `==`/`!=0.0` would diverge. `Neq` lowers to `Eq`+`Not`, so it is covered transitively. |
| **AC7.3** (Edge) | `Mod` matches the VM's `rem_euclid` semantics (computed via wasm `floor`). `Max`/`Min` use the wasm `f64.max`/`f64.min` instructions; if a corpus test surfaces a NaN/±0 difference from the VM's compare-based form, fall back to explicit compare-and-select for that case. | **Automated — unit (Phase 2 Tasks 3–4; reaffirmed Phase 5 Task 2).** Phase 2 Task 3: `Op2::Mod` asserted to match `l.rem_euclid(r)` over the four sign combinations + non-integer operands, result always in `[0,|r|)`, in `wasmgen/lower.rs`. Phase 2 Task 4: `Min`/`Max` use `f64.min`/`f64.max`, with the documented compare-and-select fallback if a corpus test surfaces a NaN/±0 divergence. Phase 5 Task 2 reaffirms for the array reducers `ArrayMax`/`ArrayMin` (which use the VM's compare form on the reduce path, since their empty-view→NaN semantics differ from the binary builtins). |
| **AC7.4** (Success) | Euler, RK2, and RK4 each match the VM's saved samples (cadence and values); `PREVIOUS`/`INIT` match via the snapshot regions. | **Automated — unit + integration (Phase 1 Euler; Phase 4 RK2/RK4 + PREVIOUS/INIT).** Phase 1 Task 2: the Euler `run` loop's cadence and per-step values asserted against `Vm::new(sim).run_to_end()` in `wasmgen/module.rs` (`step_count == n_chunks`, save cadence matches). Phase 4 Task 1: `PREVIOUS`/`INIT` models (incl. `PREVIOUS` at t0/after-first-step and `INIT` from a flow vs from an initial) match the VM via the `prev_values`/`initial_values` snapshot regions. Phase 4 Task 2: RK2 (Heun) and RK4 scalar models match the VM's saved samples (cadence and values), incl. the snapshot timing under RK. Integration: RK + PREVIOUS/INIT corpus models clear `ensure_results` via `ensure_wasm_matches` and raise the floor (Phase 4 Task 3) — checked against expected outputs at the existing tolerances, not a separate threshold. |

## AC8: Engineering quality (cross-cutting)

These two criteria are not satisfied by a single test; they are properties of the
test *structure* established uniformly across every phase, and they map to the
unit-test suite as a whole.

| AC | Literal text | Verification |
|---|---|---|
| **AC8.1** | New code reaches ≥95% test coverage via unit tests that execute emitted wasm under the DLR-FT interpreter, with each opcode/feature group individually tested. | **Automated — the unit-test suite as a whole (all phases).** Satisfied cross-cuttingly: every functionality task in Phases 1–7 is TDD'd with inline `#[cfg(test)] mod tests` in its `wasmgen/*.rs` file (`lower.rs`, `module.rs`, `math.rs`, `lookup.rs`/`lower.rs`, `views.rs`, `vector.rs`, `alloc.rs`), each test building and executing a wasm module under the DLR-FT interpreter and asserting against the VM. Each opcode/feature group (scalar core, builtins, transcendentals, lookups, RK/PREVIOUS/INIT, view ops, reducers, vector ops, allocation, modules, override/reset) is individually tested. Coverage ≥95% is a `wasmgen`-wide property of this suite, not one named test. |
| **AC8.2** | Each functionality phase ends with passing tests for the acceptance criteria it claims to cover. | **Automated — per-phase "Done When" gates (all phases).** Each phase file ends with a "Done When" section enumerating the ACs it claims and the passing tests/commands that demonstrate them; the per-phase floor raise and the `cargo test -p simlin-engine --features file_io wasmgen` / `--test simulate` verifications gate each phase. This is a process/structure criterion satisfied by the phase boundaries themselves. |

---

## Human verification: none required, and why

Every one of the 22 acceptance criteria is automatable, and the plan automates
all of them. This backend has no human-verification surface:

- It is **engine-internal** — there is no UI, rendering, animation, copy, or
  interactive UX to inspect. (The `@simlin/engine` TypeScript API, browser
  worker, and live-graph/diagram UX are explicitly out of scope per the design;
  the in-scope override/reset and by-name retrieval are engine-side mechanisms
  validated programmatically.)
- Its correctness oracle is the **bytecode VM**, an in-repo executable
  specification. Every numeric/behavioral claim is a diff against the VM (or, for
  C-LEARN, against `Ref.vdf`) under the existing comparators — fully
  programmatic.
- Even the criteria that look qualitative reduce to automated checks:
  "self-describing" (AC4.1) is asserted by reading exported globals with no
  external metadata; "clean error, never a panic" (AC1.4) and "surfaces a
  `SimlinError` rather than panicking" (AC6.2) are negative-path tests; the
  cross-cutting engineering-quality criteria (AC8.1/AC8.2) are satisfied by the
  per-opcode TDD + DLR-FT unit-test structure and the per-phase "Done When"
  gates.

The only non-test deliverable is Phase 8 Task 3 (documentation), which carries no
AC and is verified by review.
