# Human Test Plan: WebAssembly Simulation Backend

Companion to [design-plans/2026-05-20-wasm-backend.md](../design-plans/2026-05-20-wasm-backend.md) and [implementation-plans/2026-05-20-wasm-backend/](../implementation-plans/2026-05-20-wasm-backend/).

The wasm backend is engine-internal with the bytecode VM as its automated correctness oracle, so nearly everything is machine-verified: all 22 acceptance criteria map to genuine, non-vacuous automated tests that execute the emitted wasm under the DLR-FT interpreter and compare against the VM (or `crate::float::approx_eq` / `crate::vm::lookup*` / `crate::alloc::*` / `Ref.vdf`). The steps below cover the residual surface automation can't fully stand in for: the heavy `#[ignore]`d parity twins, the FFI driven from a real (non-Rust-test) host, the AC3.3 deliberate-regression confidence check, and an optional line-coverage measurement for AC8.1.

## Prerequisites

- `./scripts/dev-init.sh` has been run (idempotent).
- The default suites are green (re-run if the tree changed):
  - `cargo test -p simlin-engine --features file_io --lib wasmgen` (~259 tests)
  - `cargo test -p simlin-engine --features file_io --test simulate` (incl. `wasm_parity_floor`)
  - `cargo test -p simlin-engine --features file_io --test simulate_systems` (incl. `wasm_systems_parity_floor`)
  - `cargo test -p simlin --test wasm` (FFI)

## Phase A: Heavy parity twins (AC1.3; AC1.1/AC7.4 at scale)

These are `#[ignore]`d for runtime (the DLR-FT interpreter is not a JIT) and never run in the default suite, so they are the only automated coverage of C-LEARN-against-`Ref.vdf` and WORLD3-at-scale through wasm. Run in release.

| Step | Action | Expected |
|------|--------|----------|
| A1 | `cargo test -p simlin-engine --release --features file_io --test simulate -- --ignored simulates_clearn_wasm` | Passes. C-LEARN compiles to wasm, runs under the interpreter, and clears the 1% VDF gate + `EXPECTED_VDF_RESIDUAL` carve-out -- the same gate the VM clears (~3358 vars matched / 84 excluded across 251 steps). |
| A2 | `cargo test -p simlin-engine --release --features file_io --test simulate -- --ignored simulates_wrld3_03_wasm` | Passes. WORLD3 wasm output matches the VM element-for-element. |

## Phase B: FFI from a real host (AC6.1, AC6.2, AC4.1, AC4.2)

`src/libsimlin/tests/wasm.rs` drives the FFI in-process; this exercises the same entry point from outside the Rust harness (how TS/WASM, CGo, C/C++ consumers reach it) -- the cross-boundary contract automation can't fully represent.

| Step | Action | Expected |
|------|--------|----------|
| B1 | Build the cbindgen header + lib (per [src/libsimlin/CLAUDE.md](../../src/libsimlin/CLAUDE.md)). | `simlin_model_compile_to_wasm` is declared in `simlin.h` with five out-params + `out_error`. |
| B2 | From a small C/Go driver (or `node` over the WASM build): open a model, `simlin_project_get_model("main")`, then `simlin_model_compile_to_wasm(...)` **without ever calling `simlin_sim_new`**. | Non-NULL `out_wasm`/`out_layout` with non-zero lengths, `out_error == NULL` (AC6.1: works pre-sim). |
| B3 | Parse the layout per the documented little-endian wire format (`n_slots`/`n_chunks`/`results_offset` as u64; `count` u32; then per entry `name_len` u32 + UTF-8 + `offset` u64). Instantiate the blob, read the exported globals, call `run`, and stride one variable using only the layout. | The strided series matches `simlin_sim_get_series` for that variable; only `n_chunks` values are read per variable (AC4.1/AC4.2). |
| B4 | `simlin_free(out_wasm); simlin_free(out_layout);` | No crash/leak/double-free. |
| B5 | Feed an unsupported model (e.g. a true runtime-range subscript `SUM(source[lo:hi])` with variable `lo`/`hi` -> `ViewRangeDynamic`) to `simlin_model_compile_to_wasm`. | `out_error != NULL` with a descriptive message, both buffers NULL, **no panic across the boundary** (AC6.2). |
| B6 | Pass a NULL `out_layout` pointer. | `out_error` set, no crash. |

## Phase C: AC3.3 deliberate-regression confidence check

The gate is automated; the deliberate break is a manual confidence step. **Do not commit the edit.**

| Step | Action | Expected |
|------|--------|----------|
| C1 | Temporarily edit `src/simlin-engine/src/wasmgen/lower.rs` so a common opcode (e.g. the `Op2::Add` arm) returns `WasmGenError::Unsupported(...)`. | -- |
| C2 | `cargo test -p simlin-engine --features file_io --test simulate` | **Fails**: `wasm_parity_floor` and the per-model `wasm_parity_hook` panic, listing the now-unsupported models (AC3.2/AC3.3). |
| C3 | `cargo test -p simlin-engine --features file_io --test simulate_systems` | **Fails**: `wasm_systems_parity_floor` panics. |
| C4 | `git checkout -- src/simlin-engine/src/wasmgen/lower.rs`; re-run C2/C3. | Back to green. |

## Phase D (optional): AC8.1 coverage measurement

| Step | Action | Expected |
|------|--------|----------|
| D1 | `cargo llvm-cov -p simlin-engine --features file_io --lib -- wasmgen` (or the repo's configured coverage command); read `src/wasmgen/*` line/region coverage. | `wasmgen/` aggregate >=95%. Pins the AC8.1 number the suite establishes structurally (per-opcode TDD) but does not assert in CI. |

## Traceability

Every acceptance criterion is covered by an automated test (see the test-analysis mapping); the manual steps above add real-host / heavy-model / deliberate-regression confidence on top:

| AC | Manual step(s) | AC | Manual step(s) |
|----|----------------|----|----------------|
| AC1.3 | A1 | AC6.1 | B1-B4 |
| AC1.1/AC7.4 (scale) | A2 | AC6.2 | B5-B6 |
| AC1.4 | B5 | AC3.3 | C1-C4 |
| AC4.1/AC4.2 | B3 | AC8.1 | D1 (optional) |

All other ACs (AC1.2, AC1.5, AC2.1, AC2.2, AC3.1, AC3.2, AC5.1, AC5.2, AC7.1, AC7.2, AC7.3, AC8.2) are fully covered by automated tests and need no manual step.
