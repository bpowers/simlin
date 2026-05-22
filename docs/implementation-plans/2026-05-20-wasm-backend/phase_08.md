# WebAssembly Simulation Backend — Phase 8: Full-corpus parity + C-LEARN

**Goal:** Close the gate — make any `WasmGenError::Unsupported` for a VM-simulated core model a hard failure (no skips remain for core simulation), add the `#[ignore]`d C-LEARN wasm twin against `Ref.vdf`, and document the backend and its coverage.

**Architecture:** The parity harness flips from "skip-not-fail" to "fail" for core-simulation models: every XMILE/MDL/systems model the VM simulates in the default suite must also run through the wasm backend and clear the same comparator. The heavy `#[ignore]`d models (C-LEARN, WORLD3, COVID/metasd) get `#[ignore]`d wasm twins so they don't blow the 3-minute default-suite cap under the (interpreted, non-JIT) DLR-FT oracle.

**Tech Stack:** the `tests/simulate.rs` corpus harness, `run_clearn_vs_vdf()`, `ensure_vdf_results` + `EXPECTED_VDF_RESIDUAL`; docs.

**Scope:** Phase 8 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.3 Success:** C-LEARN runs through the wasm backend and matches `Ref.vdf` / the VM under the existing VDF tolerance and the `EXPECTED_VDF_RESIDUAL` carve-out.
- **wasm-backend.AC1.4 Failure:** A model using a not-yet-supported construct returns `WasmGenError::Unsupported` — a clean error, never a panic or a silently wrong result. *(Phase 8 is the end-state expression of this AC: the flipped gate turns any `Unsupported` for a VM-simulated core model into a hard failure — never a silent wrong result.)*

### wasm-backend.AC3
- **wasm-backend.AC3.2 Success:** End state — no core-simulation model is skipped: every XMILE, MDL, and systems-format model in the corpus runs through both backends.
- **wasm-backend.AC3.3 Failure:** A regression that makes a previously-supported model unsupported (dropping below the floor, or any `Unsupported` at the end-state gate) fails the test suite.

---

## Notes for the implementer (read first)

- **The end-state gate applies to models the VM actually simulates in the default suite.** Models the VM itself does not simulate (the unsupported-feature `#[ignore]`s: DELAY FIXED `simulate.rs:1534-1552`, GET DATA `simulate.rs:1595-1609`) stay VM-only and are out of scope — the wasm hook runs *after* the VM run, so a model the VM `#[ignore]`s never reaches it. LTM (`simulate_ltm.rs`) stays VM-only (out of scope).
- **C-LEARN harness** (confirmed): `run_clearn_vs_vdf() -> (Results, Results)` at `simulate.rs:1865-1893` (VM results + parsed `Ref.vdf`); `ensure_vdf_results`/`ensure_vdf_results_excluding` at `simulate.rs:309/349` (1% `VDF_RTOL` + matched-floor); `EXPECTED_VDF_RESIDUAL` at `simulate.rs:1746`; `simulates_clearn` at `simulate.rs:1849` (`#[ignore]`, `// Run with: cargo test --release -- --ignored simulates_clearn`). The wasm twin compares the **wasm** output to `Ref.vdf` with the **same** `ensure_vdf_results_excluding(&vdf, &wasm_results, EXPECTED_VDF_RESIDUAL)` check.
- **Test-suite time budget** (`docs/dev/rust.md:13-17`): default suite under a 3-minute wall-clock cap; the DLR-FT interpreter is not a JIT, so heavy models run slowly under it. Keep the heavy models' wasm twins `#[ignore]`d (run via `cargo test --release -- --ignored <name>`), exactly like their VM counterparts.
- **Building C-LEARN's `CompiledSimulation` for the wasm twin:** reuse the C-LEARN compile path from `run_clearn_vs_vdf` (open the `.mdl`, sync, `compile_project_incremental`), then `compile_simulation` → run the blob under DLR-FT → build `Results` from the slab (`is_vensim` consistent with the VDF comparison) → `ensure_vdf_results_excluding`.
- `pub(crate)`/`pub` latitude per the repo owner. Engine tests gated on `file_io`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Flip the harness — Unsupported is a hard failure; close the floor

**Verifies:** wasm-backend.AC3.2, wasm-backend.AC3.3.

**Files:** Modify `src/simlin-engine/tests/test_helpers.rs` (or `simulate.rs`) and `src/simlin-engine/tests/simulate.rs`, `src/simlin-engine/tests/simulate_systems.rs`.

**Implementation:**
1. Change the inline wasm hook in `simulate_path_with_excluding` (and the `.mdl` + systems paths) so a `WasmRunOutcome::Skipped(msg)` for a model the VM simulated is now a **hard failure** (`panic!`) for core-simulation models, not a silent skip. (Equivalently, `ensure_wasm_matches` returns `()` and panics on `Unsupported`.)
2. Replace the monotonic floor with the end-state assertion: the `wasm_parity_floor`/equivalent gate now requires that **every** VM-simulated core-simulation model in the default suite runs through wasm (zero `Unsupported`). Remove the skip-counting branch. Keep the gate's runtime within the cap (it only covers the small/medium default corpus; heavy models are `#[ignore]`d twins, Task 2).
3. If Task 1 surfaces any remaining `Unsupported` for a VM-simulated core model, close that lowering gap (a small addition to the relevant phase's emitter) — the design's end state is full core-simulation coverage. (A genuinely VM-unsupported feature stays out of scope and must not reach the hook.)

**Testing:** the flipped gate is the test: it fails if any VM-simulated core model is `Unsupported` (AC3.3) and passes only at full coverage (AC3.2). Confirm a deliberately-introduced `Unsupported` (temporarily) fails the suite.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate` and `--test simulate_systems`

**Commit:** `engine: close the wasm parity gate (Unsupported is a hard failure)`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: C-LEARN (and heavy-model) wasm twins

**Verifies:** wasm-backend.AC1.3.

**Files:** Modify `src/simlin-engine/tests/simulate.rs`.

**Implementation:**
Add `#[test] #[ignore] fn simulates_clearn_wasm()` (with the `// Run with: cargo test --release -- --ignored simulates_clearn_wasm` comment) that: builds C-LEARN's `CompiledSimulation` (reusing the compile path inside `run_clearn_vs_vdf`), compiles it via `compile_simulation`, runs the blob under DLR-FT, builds a `Results` from the slab, and asserts `ensure_vdf_results_excluding(&vdf_results, &wasm_results, EXPECTED_VDF_RESIDUAL)` — the same check `simulates_clearn` uses. Add similarly-`#[ignore]`d wasm twins for the other heavy models that have VM equivalents (WORLD3 `simulates_wrld3_03`, the COVID/metasd SSTATS model) if they exercise wasm-supported features, mirroring their existing VM tests' comparators.

**Testing:** `simulates_clearn_wasm` (run on demand): C-LEARN's wasm output matches `Ref.vdf` under the existing tolerance + residual carve-out.

**Verification:** `cargo test -p simlin-engine --release --features file_io -- --ignored simulates_clearn_wasm`
Expected: passes (matches `Ref.vdf` within the VDF tolerance and `EXPECTED_VDF_RESIDUAL`).

**Commit:** `engine: C-LEARN wasm parity twin against Ref.vdf`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Documentation

**Verifies:** (none — documentation; supports AC3.2 reporting.)

**Files:** Modify `src/simlin-engine/CLAUDE.md`; update `docs/` (and `docs/README.md` if adding a doc file, per `docs/CLAUDE.md`).

**Implementation:**
- Add a `wasmgen` entry to `src/simlin-engine/CLAUDE.md`'s module map: the backend lowers `CompiledSimulation` bytecode to a self-contained wasm module (alternative execution path to the VM, validated against the VM via the DLR-FT interpreter), its file layout (`mod.rs`/`module.rs`/`lower.rs`/`math.rs`/`views.rs`/`vector.rs`/`alloc.rs` as built), the `compile_simulation`/`WasmArtifact`/`WasmLayout` contract, and the supported-feature coverage (full core simulation: scalar + arrays + lookups + Euler/RK2/RK4 + modules; LTM out of scope).
- Document how to run the wasm parity tests (default suite runs small/medium corpus through wasm; heavy twins via `cargo test --release -- --ignored <name>`), and that the bytecode VM remains the correctness oracle.
- Note the `libsimlin` `simlin_model_compile_to_wasm` entry (blob + `WasmLayout`).

**Testing:** n/a (docs). Verify links/freshness; keep the `**Last updated:**` date current in `simlin-engine/CLAUDE.md`.

**Verification:** `pnpm lint` / a docs build if applicable; manual review.

**Commit:** `doc: document the wasm simulation backend and its coverage`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 8 Done When
- Every core-simulation corpus model (XMILE, MDL, systems) runs through both VM and wasm with no skips; an `Unsupported` for a VM-simulated core model fails the suite.
- C-LEARN matches `Ref.vdf` through wasm under the existing tolerance + `EXPECTED_VDF_RESIDUAL` (`#[ignore]`d twin).
- The backend and its coverage are documented in `simlin-engine/CLAUDE.md` and `docs/`.
