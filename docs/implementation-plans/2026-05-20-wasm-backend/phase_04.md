# WebAssembly Simulation Backend — Phase 4: RK2/RK4 integration + PREVIOUS/INIT

**Goal:** Generate the RK2 (Heun) and RK4 multi-stage integration loops, and serve `PREVIOUS`/`INIT` via `prev_values`/`initial_values` snapshot regions captured at the same loop points the VM uses.

**Architecture:** `compile_simulation` selects the run-loop shape from `sim.specs.method` (Euler from Phase 1; RK2/RK4 added here). The RK loops mirror `vm.rs:712-838`: per-stock scratch (`saved`/`accum` in a linear-memory region), trial-point mutation of `curr`, time juggling across stages, a final flows-only re-evaluation with restored state, then the `prev_values` snapshot. `LoadPrev`/`LoadInitial` read the two snapshot regions; the `use_prev_fallback` gate is a mutable wasm global (not a time comparison). Because the emitter knows which program it is lowering, `LoadInitial`'s "during Initials read `curr`, else read `initial_values`" branch is resolved at compile time.

**Tech Stack:** `wasm-encoder` (loops/blocks, mutable global, multi-region memory); the VM integration loops + `run_initials` + `LoadPrev`/`LoadInitial` arms as spec.

**Scope:** Phase 4 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` at those tests' existing tolerances. (No separate, tighter wasm-vs-VM threshold.)

### wasm-backend.AC7
- **wasm-backend.AC7.4 Success:** Euler, RK2, and RK4 each match the VM's saved samples (cadence and values); `PREVIOUS`/`INIT` match via the snapshot regions. *(Phase 1 established Euler; Phase 4 completes RK2/RK4 and PREVIOUS/INIT.)*

---

## Notes for the implementer (read first)

- **Reserved globals**: `TIME_OFF=0`, `DT_OFF=1`, `INITIAL_TIME_OFF=2`, `FINAL_TIME_OFF=3` (`vm.rs:83-86`). `LoadGlobalVar` reads these absolutely (no `module_off`).
- **Stock offsets**: the set of stock data-buffer offsets is the `AssignNext { off }` targets in `root.compiled_stocks` (Phase 1 already collects these for the Euler copy-back). They are module-relative `off` (root `module_off=0`, so absolute here). The VM's `stock_offsets` (`vm.rs:265`) are absolute and include submodule stocks via `EvalModule` recursion — Phase 4 is root-only; Phase 7 generalizes.
- **RK4 loop** (`vm.rs:712-787`), reproduce per timestep:
  - `saved_time = curr[TIME_OFF]`.
  - Stage 1: `eval_step` (flows then stocks). For each stock `off`: `s1 = next[off]-curr[off]; saved[i]=curr[off]; accum[i]=s1; curr[off]=saved[i]+s1*0.5`. Then `curr[TIME_OFF]=saved_time+dt*0.5`.
  - Stage 2: `eval_step`. `s2=next[off]-curr[off]; accum[i]+=2*s2; curr[off]=saved[i]+s2*0.5`.
  - Stage 3: `eval_step`. `s3=next[off]-curr[off]; accum[i]+=2*s3; curr[off]=saved[i]+s3`. Then `curr[TIME_OFF]=saved_time+dt`.
  - Stage 4: `eval_step`. `s4=next[off]-curr[off]; accum[i]+=s4; next[off]=saved[i]+accum[i]/6.0; curr[off]=saved[i]`.
  - `curr[TIME_OFF]=saved_time; next[TIME_OFF]=saved_time+dt`.
  - **Final flows-only re-eval** with restored `curr` (`eval(StepPart::Flows)`), so `curr`'s aux/flow slots hold time-`t` values (stages 2-4 clobbered them). **Load-bearing** for both saved output and PREVIOUS.
  - `prev_values := curr`; `use_prev_fallback := 0`; `save_advance!`.
- **RK2 (Heun) loop** (`vm.rs:788-838`): Stage 1 `eval_step`, `s1=next-curr; saved=curr; accum=s1; curr=saved+s1`, `curr[TIME]=saved_time+dt`. Stage 2 `eval_step`, `s2=next-curr; accum+=s2; next=saved+accum/2.0; curr=saved`. `curr[TIME]=saved_time; next[TIME]=saved_time+dt`. Final flows re-eval; `prev_values:=curr`; `use_prev_fallback:=0`; `save_advance!`.
- **`eval_step` = flows() then stocks()**; the stocks program writes `next[off]` via `AssignNext`. So per stage: `call flows(0); call stocks(0)`; then read `next[off]`/`curr[off]`. The final re-eval calls **only** `flows(0)`.
- **`run_initials`** (`vm.rs:1066-1135`): seed `curr[TIME/DT/INITIAL_TIME/FINAL_TIME]`, set `use_prev_fallback=1`, run initials once, then **capture `initial_values := curr` (whole `n_slots` chunk)** exactly once. (`prev_values` is not written during initials.)
- **`prev_values`/`initial_values`** are each `n_slots` wide (`vm.rs:617-618`). Address with `module_off + off` (root: `module_off=0`).
- **`LoadPrev { off }`** (`vm.rs:1320-1328`): pops a fallback; pushes `if use_prev_fallback { fallback } else { prev_values[module_off+off] }`. **Gate on the flag, never a `TIME==INITIAL_TIME` check** (RK moves TIME to trial points).
- **`LoadInitial { off }`** (`vm.rs:1332-1340`): `if part==Initials { curr[module_off+off] } else { initial_values[module_off+off] }`. Since the emitter knows the program (`StepPart`), pick the branch at compile time: in the initials function emit a `curr` read, in flows/stocks emit an `initial_values` read.
- **Memory layout additions:** `prev_values` (n_slots), `initial_values` (n_slots), and (RK only) `rk_scratch` = `saved`(n_stocks)+`accum`(n_stocks). Append after the Phase-3 GF region; grow `pages`. Add a mutable i32 global `use_prev_fallback` (init 1).
- `pub(crate)`/`pub` latitude per the repo owner. TDD, inline `#[cfg(test)] mod tests`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: PREVIOUS/INIT snapshot regions + LoadPrev/LoadInitial

**Verifies:** wasm-backend.AC7.4 (PREVIOUS/INIT), wasm-backend.AC1.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (layout: prev/initial regions + `use_prev_fallback` global; `run_initials` capture; Euler-loop `prev_values` snapshot), `src/simlin-engine/src/wasmgen/lower.rs` (`LoadPrev`/`LoadInitial` arms).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
1. Reserve `initial_values` + `prev_values` regions (each `n_slots*8` bytes) and a mutable i32 global `use_prev_fallback` (init 1). Thread their bases into `EmitCtx` + the `StepPart` being emitted.
2. In the `run`/initials sequence: after seeding globals and calling the initials function, copy the `curr` chunk into `initial_values` (an unrolled per-slot copy or a small copy loop). Leave `use_prev_fallback=1`.
3. In the Euler loop (Phase 1's loop), after `flows`+`stocks` and before advancing time, copy `curr → prev_values` and set `use_prev_fallback=0` (mirroring `vm.rs:705-707`).
4. `LoadPrev { off }`: pop fallback into a scratch local; `global.get use_prev_fallback`; `if` → push fallback, `else` → push `prev_values[module_off+off]` (use `select` after loading both, or an `if/else` producing f64).
5. `LoadInitial { off }`: in the **initials** program emit `curr[module_off+off]`; in **flows/stocks** programs emit `initial_values[module_off+off]`.

**Testing:**
- Euler models using `PREVIOUS(x)` and `INIT(x)` (build via `TestProject`/XMILE), assert wasm matches the VM series. Include: `PREVIOUS` at t0 (returns the fallback), `PREVIOUS` after the first step, `INIT(x)` referenced from a flow (reads `initial_values`), and `INIT(x)` referenced from another initial equation (reads `curr` during Initials).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen PREVIOUS/INIT snapshot regions + LoadPrev/LoadInitial`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: RK2 + RK4 run-loop generation

**Verifies:** wasm-backend.AC7.4 (RK2/RK4), wasm-backend.AC1.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (method dispatch in `compile_simulation`; emit RK2/RK4 loops).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Remove Phase 1's `Method::Euler`-only guard; dispatch on `sim.specs.method`. Emit the RK4 and RK2 loops per the Notes above, unrolling the per-stock stage math over the compile-time-known stock offsets, using the `rk_scratch` region for `saved[i]`/`accum[i]`. Each stage does `call flows(0); call stocks(0)` then the per-stock arithmetic; the end-of-step does a **flows-only** `call flows(0)` with restored `curr`, then the `prev_values` snapshot (Task 1), then `save_advance!`. Mind the time juggling (`curr[TIME_OFF]` set to `saved_time + dt*0.5`, `+dt`, restored to `saved_time`; `next[TIME_OFF]=saved_time+dt`).

**Testing:**
- RK2 and RK4 scalar models (e.g. a logistic-growth or SIR model run under each method): assert wasm matches the VM's saved samples (cadence and values). Include a model with `PREVIOUS`/`INIT` under RK to confirm the snapshot timing (prev captured after the final flows re-eval).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen RK2/RK4 integration loops`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Raise floor; RK + PREVIOUS/INIT corpus parity

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC7.4.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`).

**Implementation:**
Corpus models using RK2/RK4 and/or PREVIOUS/INIT now run through wasm. Re-observe the `Ran` count and raise `WASM_SUPPORTED_FLOOR`.

**Testing:** the raised floor gate; note in the commit which RK/PREVIOUS models flipped from `Skipped` to `Ran`.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate`

**Commit:** `engine: raise wasm parity floor after RK + PREVIOUS/INIT`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 4 Done When
- RK2/RK4 models and PREVIOUS/INIT models match the VM through wasm.
- Unit tests cover each integration method and the snapshot timing (Euler post-step; RK after the end-of-step flows re-eval; initial_values once after initials; the `use_prev_fallback` gate).
- The floor gate is raised.
