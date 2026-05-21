# WebAssembly Simulation Backend Design

## Summary

The Simlin engine compiles a system dynamics model into a stack-based bytecode program and runs it on an interpreting VM (`src/simlin-engine/src/vm.rs`). This work adds a second execution path: a code-generation backend that translates that same bytecode into a self-contained WebAssembly module which runs the whole simulation in one exported call. The bytecode VM is kept as the authoritative correctness oracle, and the entire effort is a correctness exercise — every model the VM can run must produce identical results through the wasm backend, validated to within ~1e-9. Performance of the generated wasm is explicitly out of scope; naive-but-correct lowering is the goal.

The architectural key is *where* the backend taps into the pipeline. Rather than re-deriving anything from the engine's `Expr` intermediate representation or the older monolithic compiler, it consumes the public output of the incremental (salsa) compile, `compile_project_incremental(...) -> CompiledSimulation` — the exact value the VM consumes. By translating the already-compiled bytecode opcode-for-opcode, the backend inherits all of the engine's hard-won assembly (dependency ordering, memory-offset resolution, recurrence-cycle handling, graphical-function layout, module instantiation, implicit SMOOTH/DELAY variables) unchanged, and the VM's opcode handlers serve as an executable specification for each lowering. The bytecode's control flow is already structured, so no general control-flow reconstruction is needed: conditionals become eager selects, array iteration becomes bounded loops, and submodels become function calls. For bit-identical numeric parity, transcendental math (`sin`, `pow`, `ln`, etc.) is lowered to host-function imports backed by the *same* Rust `f64` routines the VM uses, rather than an embedded approximation. The emitted module writes step-major result snapshots into linear memory and exports its own geometry plus a variable-name→memory-offset "listing", so a host can read a single variable's time series by striding memory directly. A small libsimlin FFI surfaces the blob and that listing to callers, and the generated modules are validated by executing them natively under the DLR-FT `wasm-interpreter` and diffing against the VM. Work is staged across eight TDD phases (scalar core → builtins → graphical functions → RK integration → arrays → vector/allocation ops → modules + FFI → full-corpus + C-LEARN), with a `simulate.rs` parity harness that ratchets the supported-model count upward each phase until no core-simulation model is skipped.

## Definition of Done

Turn the validated proof-of-concept (branch `wasm-backend-poc`) into a full, correctness-first implementation of a WebAssembly code-generation backend for the Simlin engine, validated to full parity with the bytecode VM.

### Deliverables

1. A wasm code-generation backend in `simlin-engine` (building on `src/simlin-engine/src/wasmgen/`) that lowers a model into a **self-contained WebAssembly module** running the entire simulation in an exported call.
2. The backend translates the **compiled bytecode** produced by the salsa incremental pipeline (`compile_project_incremental` → `CompiledSimulation`, the exact program the bytecode VM runs), **not** the `Expr` IR and **not** the monolithic `compiler::Module`. It thereby inherits all of the engine's assembly — dependency ordering, offset resolution, recurrence-SCC handling, graphical-function layout, module instantiation, implicit SMOOTH/DELAY vars — unchanged. The POC's `#[cfg(test)]` un-gating of `Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map` is reverted.
3. The emitted module writes **step-major full-slab results** (simulation time in column 0; each saved timestep a contiguous row) and is **self-describing** (exports its results geometry). This mirrors the sd.js results format.
4. The host can **efficiently retrieve a specific variable's time series by name**, mirroring `src/engine`'s existing `Sim.getSeries(name)` API shape. A variable name→slot-offset layout (a "model listing", à la sd.js's offsets map) is exposed alongside the blob, so a caller reads one variable's series by striding the blob's memory directly — copying only that variable's `n_chunks` values, never the whole `n_chunks × n_slots` slab, and with no libsimlin round-trip.
5. The blob supports **constant-value overrides and reset** (override a constant, reset, re-run from t0), validated for correctness against the VM's override/reset semantics.
6. A **libsimlin FFI** surfaces the compiled blob and the variable-offset layout to callers, using the existing malloc-and-return-buffer convention.

### Success criteria

1. **Full parity with the bytecode VM** for core simulation across the `simulate.rs` corpus. Every model the VM simulates — including C-LEARN, which exercises arrays/subscripts, modules + stdlib macros (e.g. SMOOTH/DELAY), graphical functions, the full builtin set, and Euler + RK2 + RK4 integration — also runs through the wasm backend and matches the expected outputs / the VM within the established tolerance.
2. `simulate.rs` (and the systems-format simulation tests where applicable) is **wired to run supported models through BOTH the VM and the wasm backend** (executed natively under the DLR-FT `wasm-interpreter`). End state: no "unsupported feature" skips remain for core simulation.
3. New code follows **strict TDD with 95%+ coverage**, idiomatic Rust, good factoring, and real unit tests. The existing integration suite complements but does not replace unit tests.

### Out of scope

1. **Performance** of the generated wasm: no ms/throughput targets, no benchmarks, no codegen micro-optimization (naive-but-correct lowering is acceptable). Correctness is the sole quality bar.
2. **LTM / loop-score** synthetic variables — `simulate_ltm.rs` stays VM-only. (LTM synthetic variables are just more equations, so the backend may incidentally support them, but parity is not required.)
3. The **`@simlin/engine` TypeScript API, browser Web Worker, and live-graph/diagram interactive UX** — a separate, later design. The blob's override/reset and by-name variable-retrieval capabilities ARE in scope as engine-side mechanisms, but the end-to-end interactive experience is not.

### Inherited decisions / constraints

- Emit wasm with the `wasm-encoder` crate; validate by executing generated modules under the DLR-FT `wasm-interpreter` as a native test oracle.
- Direct-drive architecture: the host instantiates the model blob and drives it; libsimlin is not on the per-run hot path (no trampoline). A wasm module cannot instantiate another, so the model blob is always host-instantiated.
- The bytecode VM **coexists** as the correctness oracle; it is not replaced.
- Because performance is out of scope, transcendental / non-primitive builtins (e.g. `pow`, `sin`, `ln`) may be lowered to **host imports backed by the same Rust `f64` math** the VM uses, yielding bit-identical parity rather than an embedded-libm approximation.

## Acceptance Criteria

### wasm-backend.AC1: The wasm backend reproduces the VM's simulation results
- **wasm-backend.AC1.1 Success:** A model within the supported feature set produces, through the wasm backend, the same time series as the bytecode VM for every shared variable (max abs/relative difference ≤ ~1e-9).
- **wasm-backend.AC1.2 Success:** Arrayed/subscripted models (apply-to-all, subscripts, vector operations) match the VM element-for-element.
- **wasm-backend.AC1.3 Success:** C-LEARN runs through the wasm backend and matches `Ref.vdf` / the VM under the existing VDF tolerance and the `EXPECTED_VDF_RESIDUAL` carve-out.
- **wasm-backend.AC1.4 Failure:** A model using a not-yet-supported construct returns `WasmGenError::Unsupported` — a clean error, never a panic or a silently wrong result.
- **wasm-backend.AC1.5 Edge:** Empty-view reducers, out-of-bounds subscripts, and division-by-zero produce the same NaN / finite-`:NA:` / Inf values the VM produces.

### wasm-backend.AC2: The backend consumes the salsa compiled bytecode
- **wasm-backend.AC2.1 Success:** The wasm module is produced from `compile_project_incremental(...) -> CompiledSimulation`, not from the `Expr` IR or the monolithic `compiler::Module`.
- **wasm-backend.AC2.2 Success:** The POC's `#[cfg(test)]` un-gating of the monolithic builder is reverted; the crate builds with `Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map` test-only again.

### wasm-backend.AC3: simulate.rs runs the corpus through both backends
- **wasm-backend.AC3.1 Success:** During rollout, each corpus model runs through the VM and (when supported) the wasm backend, comparing wasm-vs-VM; unsupported models are skipped (not failed) and counted against a monotonically rising floor.
- **wasm-backend.AC3.2 Success:** End state — no core-simulation model is skipped: every XMILE, MDL, and systems-format model in the corpus runs through both backends.
- **wasm-backend.AC3.3 Failure:** A regression that makes a previously-supported model unsupported (dropping below the floor, or any `Unsupported` at the end-state gate) fails the test suite.

### wasm-backend.AC4: Self-describing results + efficient by-name retrieval
- **wasm-backend.AC4.1 Success:** The blob exports `n_slots`/`n_chunks`/`results_offset` and writes step-major snapshots; a host locates and strides the results with no external metadata.
- **wasm-backend.AC4.2 Success:** Reading one variable's series via the name→offset layout copies only that variable's `n_chunks` values (never the whole `n_chunks × n_slots` slab) and equals the VM's series for that variable.

### wasm-backend.AC5: Override + reset
- **wasm-backend.AC5.1 Success:** Overriding a constant via `set_value`, then `reset`, then `run`, yields the same series the VM produces under the same override (matching `simlin_sim_set_value`/`reset` semantics).
- **wasm-backend.AC5.2 Success:** `reset` with no override restores the compiled-default results.

### wasm-backend.AC6: libsimlin FFI
- **wasm-backend.AC6.1 Success:** `simlin_model_compile_to_wasm` returns a valid wasm blob plus the name→offset layout via the malloc-return convention; both buffers are freeable with `simlin_free`; it works before any `SimlinSim` exists.
- **wasm-backend.AC6.2 Failure:** A model that cannot be compiled to wasm surfaces a `SimlinError` rather than panicking across the FFI boundary.

### wasm-backend.AC7: Numeric-parity specifics
- **wasm-backend.AC7.1 Success:** Transcendental builtins (`pow`/`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`exp`/`ln`/`log10`) and the allocation `erfc` polynomial produce bit-identical results to the VM (host imports backed by Rust `f64`).
- **wasm-backend.AC7.2 Success:** Equality and truthiness (`Eq`/`Neq`/`And`/`Or`/`If` condition) use ULP-based `approx_eq` matching the VM.
- **wasm-backend.AC7.3 Edge:** `Max`/`Min` reproduce the VM's NaN/±0 behavior (explicit compare-and-select, not `f64.max`/`f64.min`); `Mod` uses `rem_euclid`.
- **wasm-backend.AC7.4 Success:** Euler, RK2, and RK4 each match the VM's saved samples (cadence and values); `PREVIOUS`/`INIT` match via the snapshot regions.

### wasm-backend.AC8: Engineering quality (cross-cutting)
- **wasm-backend.AC8.1:** New code reaches ≥95% test coverage via unit tests that execute emitted wasm under the DLR-FT interpreter, with each opcode/feature group individually tested.
- **wasm-backend.AC8.2:** Each functionality phase ends with passing tests for the acceptance criteria it claims to cover.

## Glossary

### System dynamics / simulation
- **System dynamics (SD)**: A modeling discipline for simulating dynamic systems as networks of stocks (accumulations), flows (rates of change), and feedback loops, governed by integration over time.
- **Stock / flow**: A stock is a state variable that accumulates; a flow is a rate that adds to or drains a stock each timestep. Stocks are integrated; flows and auxiliaries are recomputed each step.
- **Integration method (Euler / RK2 / RK4)**: Numerical schemes for advancing stocks over a timestep `dt`. Euler is a single first-order step; RK2 (Heun's method) and RK4 (classic Runge-Kutta) are multi-stage methods that re-evaluate flows at intermediate points. Each produces a distinct integration loop the backend must reproduce.
- **Graphical function (lookup)**: A piecewise-linear table function `y = f(x)` defined by `(x, y)` knot points, evaluated by binary search + linear interpolation with edge clamping. Drives the `Lookup`/`LookupForward`/`LookupBackward` opcodes.
- **Apply-to-all (A2A) / subscript / arrayed variable**: An arrayed variable indexed over named dimensions. "Apply-to-all" means one equation applies element-wise across every element; subscripting selects specific elements. The backend must match the VM element-for-element.
- **Reducer**: An array builtin that collapses a vector to a scalar — `SUM`, `MEAN`, `MIN`, `MAX`, `STDDEV`, `SIZE`. Empty-view semantics differ by reducer (`SUM`→0.0; `MAX`/`MIN`/`MEAN`/`STDDEV`→NaN).
- **Vector operations**: Array builtins that produce arrays — `VECTOR SELECT`, `VECTOR ELM MAP`, `VECTOR SORT ORDER`, `RANK`, and the `ALLOCATE AVAILABLE`/`ALLOCATE BY PRIORITY` market-clearing allocators (the latter use a bisection solve over per-requester allocation curves and an `erfc`/`normal_cdf` polynomial).
- **PREVIOUS / INIT**: Lag intrinsics. `PREVIOUS(x)` reads `x`'s value from the prior timestep; `INIT(x)` reads `x`'s value at `t0`. Served by snapshot buffers (`prev_values`/`initial_values`) captured at specific points in the integration loop.
- **SMOOTH / DELAY**: Standard-library macros (exponential smoothing, material delays) the engine expands into implicit hidden stock variables. The backend inherits these expanded variables from the compiled bytecode without special handling.
- **C-LEARN**: A large real-world climate-policy SD model (~53k lines / 1.4 MB) used as the most demanding integration test. It exercises arrays, modules, stdlib macros, graphical functions, the full builtin set, and all three integration methods, so passing it is the strongest parity signal.
- **`:NA:` sentinel (`crate::float::NA`)**: Vensim's "missing data" marker, the *finite* value `-2^109` — deliberately not IEEE NaN, so `IF x = :NA:` works and `:NA:` arithmetic stays finite. The backend must keep this distinct from genuine (absorbing) NaN, which the engine still produces for out-of-bounds array reads and empty reducers.
- **`approx_eq`**: The engine's ULP-based floating-point equality (`float_cmp` crate). Used for all equality/inequality tests and truthiness (`Eq`/`Neq`/`And`/`Or`/`If` conditions), so the backend routes comparisons through the same sequence rather than raw `==`.
- **VDF (Vensim Data File)**: Vensim's proprietary binary simulation-output format. C-LEARN's expected results live in `Ref.vdf`, compared under a 1% cross-simulator tolerance with an `EXPECTED_VDF_RESIDUAL` carve-out for a few separately-tracked residual variables.

### This codebase
- **Bytecode VM (`vm.rs`)**: The stack-based interpreter that executes compiled bytecode. It is the in-repo ground truth; each opcode's wasm lowering reproduces the matching VM handler arm. It coexists with the new backend as the correctness oracle, not as something being replaced.
- **`Expr` IR**: The engine's expression-tree intermediate representation (progressively lowered `Expr0`→`Expr3`). The backend deliberately does *not* consume this — it consumes bytecode, one stage later.
- **salsa**: An incremental-computation framework (the `salsa` crate) the engine uses to cache compilation at per-variable granularity. `compile_project_incremental` is its public production entry point; the backend consumes its output without touching any salsa-internal query.
- **`CompiledSimulation`**: The public salsa output the VM consumes — `{ modules, specs, root, offsets }`. Bundles every compiled module, the time specs, the root module key, and the global variable-name→slot-offset map. The backend's single input.
- **`CompiledModule`**: One model instance's compiled form: three opcode programs (`compiled_initials`, `compiled_flows`, `compiled_stocks`), per-program literals, and a shared `ByteCodeContext`.
- **`ByteCodeContext`**: The shared side tables a module's opcodes reference by index — graphical-function tables, nested-module declarations, dimension info, interned names, pre-computed static array views, and temp-array offsets/sizes.
- **opcode / `eval_bytecode` / `apply` / `lookup`**: An opcode is one bytecode instruction. `eval_bytecode` is the VM's main dispatch loop; `apply` handles builtin-function opcodes; `lookup` handles graphical-function evaluation. These are the reference implementations the lowering mirrors.
- **`fuse_three_address`**: A late VM optimization that fuses adjacent opcodes into 3-address forms. It runs inside `Vm::new`, *after* `CompiledSimulation` is produced — so the backend, consuming the un-fused `CompiledSimulation`, only ever sees the plain opcode set.
- **`EvalModule` / `LoadModuleInput` / `ModuleInput`**: The opcodes implementing nested submodels. `EvalModule` packs inputs and recurses into a child module at a base memory offset (`module_off`); the backend lowers this to a wasm function call.
- **Model "listing" (name→offset layout / `WasmLayout`)**: A map from canonical variable name to its slot offset in the results slab, exported alongside the blob. Modeled on sd.js's offsets map, it lets a host read one variable's series by striding memory directly. `WasmLayout` carries `n_slots`, `n_chunks`, `results_offset`, and `var_offsets`.
- **Step-major full slab / `n_slots` / `n_chunks` / `results_offset`**: The results memory layout — one contiguous row per saved timestep (time in column 0), `n_slots` values wide, `n_chunks` rows tall, beginning at byte `results_offset`. "Step-major" means timesteps are the major axis (mirrors sd.js). Reading one variable copies only its `n_chunks` values, not the whole `n_chunks × n_slots` slab.
- **`Sim.getSeries(name)`**: The existing `src/engine` (TypeScript) API the by-name retrieval mirrors — fetch one variable's `Float64Array` time series by name.
- **`compiler::Module` (monolithic builder)**: The engine's older, non-incremental compile path (`Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map`). The proof-of-concept temporarily exposed it under `#[cfg(test)]`; this design reverts that, keeping the dead monolithic path out of production builds since `CompiledSimulation` makes it unnecessary.
- **libsimlin**: The crate exposing a flat C FFI to the engine (used by WASM, CGo, C/C++ callers). The new `simlin_model_compile_to_wasm` entry point lives here.
- **Malloc-and-return-buffer convention**: libsimlin's pattern for returning variable-length data across the FFI — the callee allocates a buffer and returns a pointer + length through out-params; the caller later frees it with `simlin_free`. Used to surface the wasm blob and the layout.
- **LTM (Loops That Matter)**: The engine's feedback-loop-scoring feature, which synthesizes extra "link/loop score" equations. Explicitly out of scope here — `simulate_ltm.rs` stays VM-only.
- **`simulate.rs` corpus / parity harness / floor gate**: The end-to-end integration suite that runs a corpus of XMILE/MDL/systems models against expected outputs. The new harness additionally runs each supported model through the wasm backend and diffs against the VM, enforcing a monotonically rising "floor" on how many models pass — so a regression that drops a previously-supported model fails the build.
- **`WasmGenError::Unsupported`**: The clean error the backend returns for a not-yet-supported construct — never a panic or a silently wrong result. During rollout it counts as a skip; at the end state it is a hard failure.

### Third-party / external concepts
- **WebAssembly (wasm) / linear memory / `f64` instructions**: A portable bytecode/VM target. The generated module computes over a single flat `f64` array ("linear memory") using wasm's native floating-point instructions, which match Rust `f64` semantics for primitive arithmetic.
- **`wasm-encoder` crate**: A Rust library for programmatically emitting (encoding) a `.wasm` binary module. The backend builds the module with this.
- **DLR-FT `wasm-interpreter` (+ `checked` crate)**: A pure-Rust WebAssembly interpreter from the German Aerospace Center (DLR) flight-software group, pinned by git rev in `Cargo.toml`. It runs the emitted module natively in tests (validate → instantiate via the `checked` crate's `Store` API → invoke → read memory) as the parity oracle. It is an interpreter, not a JIT, so long/large models run slowly — hence heavy tests are `#[ignore]`d.
- **Host import / host function**: A function the wasm module imports rather than defining itself, supplied by the embedder at instantiation. The backend imports transcendental math this way, bound to Rust closures in the native oracle (and, in a future browser host, to JS) — making the blob intentionally *not* import-free.
- **`rem_euclid` / explicit compare-and-select**: Rust's Euclidean remainder, used for the `Mod` operator to match the VM (instead of wasm's `f64` remainder). Likewise `Max`/`Min` are lowered as explicit IEEE comparisons rather than `f64.max`/`f64.min`, which differ on NaN and signed-zero inputs.
- **Relooper**: A general algorithm for reconstructing structured control flow from arbitrary jumps when targeting wasm. Noted as *not needed* here because the source bytecode is already structured.

## Architecture

The backend translates the engine's compiled bytecode into an equivalent WebAssembly module, mirroring the bytecode VM (`src/simlin-engine/src/vm.rs`) opcode-for-opcode. It consumes the public salsa output `compile_project_incremental(db, project, model) -> CompiledSimulation` (`vm.rs:134`) — the same value `Vm::new` consumes — so no salsa-internal queries are touched and all engine assembly (dependency ordering, model-global offset resolution, recurrence-SCC handling, graphical-function layout, module instantiation, implicit SMOOTH/DELAY variables) is inherited unchanged.

`CompiledSimulation` is `{ modules: HashMap<ModuleKey, CompiledModule>, specs: Specs, root: ModuleKey, offsets: HashMap<Ident, usize> }`. Each `CompiledModule` (`bytecode.rs:4616`) holds three opcode programs (`compiled_initials`, `compiled_flows`, `compiled_stocks`), per-program `literals`, and a shared `ByteCodeContext` (`bytecode.rs:1585`: graphical-function tables, module declarations, dimensions, temp-array sizes, static array views). It is the *un-fused* form — the 3-address `fuse_three_address` pass runs later in `Vm::new` — so the backend translates the plain opcode set only.

**New entry point** (in `src/simlin-engine/src/wasmgen/`), the contract other layers depend on:

```rust
pub fn compile_simulation(sim: &CompiledSimulation) -> Result<WasmArtifact, WasmGenError>;

pub struct WasmArtifact {
    pub wasm: Vec<u8>,          // a complete, self-contained wasm module
    pub layout: WasmLayout,     // the "model listing" for host-side reads
}

pub struct WasmLayout {
    pub n_slots: usize,
    pub n_chunks: usize,
    pub results_offset: usize,               // byte offset of the results region
    pub var_offsets: Vec<(String, usize)>,   // canonical variable name -> slot offset
}
```

**Emitted module shape.** For each module instance `(model, input_set)`, the three opcode programs become three wasm functions over a shared linear-memory f64 slab. A generated `run` function seeds the reserved globals + initials, then drives the integration loop (Euler/RK2/RK4), calling the flows and stocks functions each step and writing step-major snapshots into a results region. `EvalModule` becomes a call into the child instance's functions with a base-offset argument (`module_off`), mirroring the VM's recursion (`vm.rs:1379`).

Because the bytecode's control flow is structured, no general control-flow reconstruction (relooper) is needed: `If` is an eager select (both arms pre-evaluated, then chosen), array iteration is a bounded `BeginIter…NextIterOrJump…EndIter` loop, and modules are calls.

**Numeric strategy.** Transcendental and other non-wasm-primitive operations (`powf`, `sin`/`cos`/`tan`, `asin`/`acos`/`atan`, `exp`, `ln`, `log10`, `rem_euclid`, and the `erfc` polynomial used by priority allocation) are lowered to **host-function imports backed by the same Rust `f64` methods the VM's `apply` uses** (`vm.rs:2938`), yielding bit-identical results rather than an embedded-libm approximation. The native test oracle supplies these imports as Rust closures; a browser host would supply them in JS (out of scope here, but the import signatures are fixed by this design). This makes the blob *not* import-free — an intentional change from the POC, justified because performance is out of scope and parity is paramount. All other arithmetic uses wasm f64 instructions directly, matching the VM exactly: `approx_eq` (ULP-based) for equality and truthiness, genuine IEEE NaN distinct from the finite `:NA:` sentinel (`crate::float::NA`), and `Max`/`Min` via explicit compare-and-select rather than `f64.max`/`f64.min` (which differ on NaN/±0).

**Host interface.** The emitted module exports its `memory`, the `run` / `set_value` / `reset` functions, and self-describing geometry globals (`n_slots`, `n_chunks`, `results_offset`). The `WasmLayout` (returned alongside the blob and surfaced through libsimlin) provides the name→offset map, so a host reads one variable's series by striding the blob's memory directly — copying only that variable's `n_chunks` values, never the whole `n_chunks × n_slots` slab, with no libsimlin round-trip — mirroring `src/engine`'s `Sim.getSeries(name)`.

## Existing Patterns

Investigation found a working proof-of-concept at `src/simlin-engine/src/wasmgen/` plus established engine conventions this design follows:

- **The VM is the executable specification.** Each opcode's wasm lowering reproduces the matching arm of `eval_bytecode` (`vm.rs:1257`), `apply` (`vm.rs:2938`), `lookup` (`vm.rs:3056`), the array opcodes (`vm.rs:1739`–`2794`, plus `vm_vector_elm_map.rs`, `vm_vector_sort_order.rs`, `alloc.rs`), and the integration loop (`vm.rs:631`–`860`). Parity is checked against the VM, the in-repo ground truth.
- **Consume the public salsa output.** `compile_project_incremental` (`db.rs:5886`) is the production compile entry; the backend consumes its `CompiledSimulation`, exactly as `Vm::new` (`vm.rs:559`) does.
- **DLR-FT oracle.** The POC's `population_wasm_matches_vm` (`wasmgen/module.rs`) is the validation pattern: emit → `wasm::validate` → `checked::Store` instantiate → invoke → read linear memory → compare to VM `Results`. `wasm-encoder` and the DLR-FT `wasm-interpreter` + `checked` crates are already wired in `Cargo.toml`.
- **libsimlin FFI convention.** `simlin_model_compile_to_wasm` (`libsimlin/src/model.rs`) follows the malloc-and-return-buffer pattern of `simlin_project_serialize_*` (`serialization.rs:40`).
- **Test conventions.** Strict TDD with inline `#[cfg(test)]` unit modules, plus the `tests/simulate.rs` corpus compared via `ensure_results` (`tests/test_helpers.rs:62`) and `ensure_vdf_results` (`simulate.rs:349`).

**Divergence:** the POC's `#[cfg(test)]` un-gating of the monolithic `compiler::Module` builder (`compiler/mod.rs`: `Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map`) is reverted — consuming `CompiledSimulation` removes any need for that path, and re-gating keeps the dead monolithic path out of production builds.

## Implementation Phases

Eight phases. The `simulate.rs` parity harness is introduced in Phase 1 and ratchets upward each phase (treating unsupported features as skip-not-fail until the end state). Every functionality phase is TDD'd to 95%+ coverage with unit tests mapped to the acceptance criteria it covers, executed under the DLR-FT interpreter.

<!-- START_PHASE_1 -->
### Phase 1: Bytecode-to-wasm scalar core + parity harness
**Goal:** Translate `CompiledSimulation` (scalar variables, Euler) to a self-contained wasm module, and stand up the dual VM-vs-wasm parity gate.

**Components:**
- `src/simlin-engine/src/wasmgen/` — restructure to consume `CompiledSimulation`: `compile_simulation(&CompiledSimulation) -> WasmArtifact`; per-opcode emitter for the scalar core (`LoadVar`/`LoadGlobalVar`/`LoadConstant`/`Op2`/`Not`/`If` (select)/`AssignCurr`/`AssignNext`/`Ret`); the Euler `run` loop + step-major results region + self-describing globals + `WasmLayout`.
- `compiler/mod.rs` — revert the POC `#[cfg(test)]` un-gating.
- `tests/test_helpers.rs` (or a `pub(crate)` util promoted from `wasmgen`) — `ensure_wasm_matches(datamodel, model_name, &vm_results, excluded)` returning `Ran | Skipped(WasmGenError::Unsupported)`; reads the blob's results from linear memory using the exported geometry; compares to VM `Results` at ~1e-9.
- `tests/simulate.rs` — call the helper after the existing VM run; maintain a monotonically rising floor on the count of corpus models that run through wasm.

**Dependencies:** None (first phase; builds on existing POC scaffolding).

**Done when:** scalar models in the corpus match the VM through wasm; the floor gate is active; the monolithic builder is re-gated and the crate builds. Covers `wasm-backend.AC1.1`, `wasm-backend.AC2.1`, `wasm-backend.AC2.2`, `wasm-backend.AC3.1`, `wasm-backend.AC4.1`, `wasm-backend.AC7.4`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Full scalar builtins + numeric parity
**Goal:** Every scalar `BuiltinFn` and operator reaches VM parity.

**Components:**
- `wasmgen/` — `Apply` lowering: direct f64 ops (`Abs`/`Sqrt`/`Int`/`Sign`/`Quantum`/`SafeDiv`/`Max`/`Min` via explicit compare); composed builtins matching the VM's exact f64 sequences (`Step`/`Pulse`/`Ramp`/`Sshape`/scalar `Mean`); `Op2::Exp`→`powf`, `Op2::Mod`→`rem_euclid`, `And`/`Or` truthiness; constants `Inf`/`Pi`/`IsModuleInput`.
- Host-import math module: imported functions backed by Rust `f64` methods; the native oracle binds them; signatures fixed.
- `Eq`/`Neq`/comparison-to-bool/`If` condition/`is_truthy` routed through a `approx_eq` sequence matching `crate::float::approx_eq`.

**Dependencies:** Phase 1.

**Done when:** all scalar-only corpus models match the VM; unit tests cover each builtin and the approx_eq/NaN/`:NA:` edge cases. Covers `wasm-backend.AC1.1`, `wasm-backend.AC7.1`, `wasm-backend.AC7.2`, `wasm-backend.AC7.3`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Graphical functions (lookups)
**Goal:** `Lookup`/`LookupForward`/`LookupBackward` parity.

**Components:**
- `wasmgen/` — emit the `ByteCodeContext.graphical_functions` tables into a wasm data segment (or initialized memory region); a shared lookup helper (binary search + linear interpolation + edge clamp + `approx_eq`-at-knot) mirroring `vm.rs:3056`–`3186`; `Lookup` opcode → element-offset + index + call.

**Dependencies:** Phase 2.

**Done when:** corpus models using graphical functions match the VM; unit tests cover interpolate/forward/backward, edge clamping, and out-of-range element. Covers `wasm-backend.AC1.1`, `wasm-backend.AC7.1`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: RK2/RK4 integration + PREVIOUS/INIT
**Goal:** Non-Euler methods and the lag intrinsics.

**Components:**
- `wasmgen/` — generate the RK2 (Heun) and RK4 multi-stage loops (per-stock scratch region, in-place stage mutation, time juggling, end-of-step flows re-eval) matching `vm.rs:712`–`860`; `LoadPrev`/`LoadInitial` with `prev_values`/`initial_values` snapshot regions captured at the correct loop points (`vm.rs:1066`, Euler/RK snapshot timing).

**Dependencies:** Phase 2 (loop structure), Phase 3 not required.

**Done when:** RK2/RK4 models and PREVIOUS/INIT models match the VM; unit tests cover each method and the snapshot timing. Covers `wasm-backend.AC1.1`, `wasm-backend.AC7.4`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Arrays — subscripts, iteration, reducers
**Goal:** Core array machinery using statically-resolved `ArrayView`s (no runtime view-stack).

**Components:**
- `wasmgen/` — a temp-array region in linear memory (`temp_offsets`/`temp_total_size`); `Subscript`/`StaticSubscript`/`TempArray`/`TempArrayElement`/`AssignTemp` lowering; `BeginIter…NextIterOrJump…EndIter` (and broadcast iteration) translated to wasm bounded loops with compile-time stride arithmetic from `ArrayView`; array reducers `ArraySum`/`ArrayMax`/`ArrayMin`/`ArrayMean`/`ArrayStddev`/`ArraySize` (empty-view NaN vs 0.0 semantics).

**Dependencies:** Phase 2.

**Done when:** arrayed (A2A/subscript) corpus models match the VM; unit tests cover subscript OOB→NaN, broadcast, and each reducer. Covers `wasm-backend.AC1.1`, `wasm-backend.AC1.2`, `wasm-backend.AC7.3`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Arrays — vector operations and allocation
**Goal:** The helper-heavy array builtins.

**Components:**
- `wasmgen/` — `VectorSelect`, `VectorElmMap` (full-source projection, no modulo, OOB→NaN), `VectorSortOrder` (per-row 0-based stable), `Rank` (whole-view 1-based stable), `LookupArray`, and `AllocateAvailable`/`AllocateByPriority` (the bisection market-clearing + curve types + `normal_cdf`/`erfc` polynomial), each as a wasm helper mirroring `vm_vector_elm_map.rs`, `vm_vector_sort_order.rs`, and `alloc.rs`.

**Dependencies:** Phase 5.

**Done when:** corpus models using vector ops/allocation match the VM; unit tests cover each op including the allocation bisection. Covers `wasm-backend.AC1.2`.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Modules + host interface (FFI, layout, override/reset)
**Goal:** Submodels run, and the host can drive the blob and read variables efficiently.

**Components:**
- `wasmgen/` — per-instance module functions; `EvalModule` (pack inputs, call child at `module_off`) and `ModuleInput`/`LoadModuleInput`; the module tree over `(model, input_set)` instances.
- `wasmgen/` — the blob's `set_value(offset, val)` / `reset` mechanism (overrides recorded and re-applied after initials, matching `simlin_sim_set_value`/`reset` semantics).
- `libsimlin/src/model.rs` — `simlin_model_compile_to_wasm` returns the blob *and* the `WasmLayout` (name→offset listing) via the malloc-return convention; the host reads one variable's series by striding the blob's memory.

**Dependencies:** Phases 2–6 (module bodies may use any feature).

**Done when:** module-bearing and systems-format (`simulate_systems.rs`) and metasd-simulation models match the VM; override-then-reset-then-run matches the VM with the same override; by-name series read copies only `n_chunks` values. Covers `wasm-backend.AC1.1`, `wasm-backend.AC4.1`, `wasm-backend.AC4.2`, `wasm-backend.AC5.1`, `wasm-backend.AC5.2`, `wasm-backend.AC6.1`.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Full-corpus parity + C-LEARN
**Goal:** Close the gate — no unsupported-feature skips for core simulation.

**Components:**
- `tests/simulate.rs` — flip the harness so any `WasmGenError::Unsupported` is a hard failure for core-simulation models; remove the skip branch; the floor becomes the full corpus.
- `tests/simulate.rs` — a `simulates_clearn_wasm` twin reusing `run_clearn_vs_vdf()` and the `EXPECTED_VDF_RESIDUAL` carve-out, comparing wasm-vs-VM (and wasm-vs-`Ref.vdf`), `#[ignore]`d for runtime like `simulates_clearn`.
- `src/simlin-engine/CLAUDE.md`, `docs/` — document the backend and its coverage.

**Dependencies:** Phases 1–7.

**Done when:** every core-simulation corpus model (XMILE, MDL, systems) runs through both VM and wasm with no skips; C-LEARN matches under the existing tolerance/residual; docs updated. Covers `wasm-backend.AC1.3`, `wasm-backend.AC3.2`, `wasm-backend.AC3.3`.
<!-- END_PHASE_8 -->

## Additional Considerations

**Float parity.** The target is bit-identical to the VM where the math is shared (host-imported transcendentals backed by Rust `f64`); the VM-vs-wasm comparison tolerance is ~1e-9 (tighter than the VM-vs-expected tolerances in `ensure_results`, because this is a backend-equivalence check). The parity-sensitive details — `approx_eq` for equality/truthiness, genuine NaN vs the finite `:NA:` sentinel, `Max`/`Min` NaN/±0 behavior, and `rem_euclid` for `Mod` — are explicit acceptance criteria, not incidental.

**The blob imports a math module.** Unlike the POC, the emitted module is not import-free; it imports the transcendental helpers. The native oracle binds them to Rust closures (DLR-FT supports host functions); a future browser host binds them in JS. The import signatures are part of the contract this design fixes.

**Test-suite time budget.** The wasm path adds a compile + an interpreted run per corpus model on top of the VM run (XMILE models already compile+run three times for round-trips). The DLR-FT interpreter is not a JIT, so long-horizon or large models can be slow; keep heavy models (C-LEARN) `#[ignore]`d and run via `--release -- --ignored`, and watch the default suite against the 3-minute cap — push borderline models into `#[ignore]`d wasm-specific tests rather than inlining the assertion everywhere.

**Out of scope (reaffirmed).** Generated-wasm performance and benchmarks; LTM synthetic variables (`simulate_ltm.rs` stays VM-only); the `@simlin/engine` TypeScript API, browser worker, and live-graph UX. The override/reset and by-name retrieval *mechanisms* are in scope as engine-side capabilities; the interactive end-to-end experience is a separate, later design.
