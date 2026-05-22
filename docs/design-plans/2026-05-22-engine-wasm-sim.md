# @simlin/engine WebAssembly Simulation Backend (WasmSim) Design

## Summary

The Simlin engine compiles each system-dynamics model two ways from one
`CompiledSimulation`: into bytecode for a stack-based virtual machine (`vm.rs`, the
established path), and — via the `wasmgen` backend — into a self-contained, import-free
WebAssembly module (a "blob") that reproduces the VM opcode-for-opcode. The blob is meant
for fast repeated re-simulation, such as interactive parameter scrubbing: a host
instantiates it once and re-runs it on every change. Today the JavaScript API
(`@simlin/engine`) only ever drives the bytecode VM through libsimlin's C FFI. This work
makes the wasm blob a second, *selectable* simulation engine while keeping the public API
identical: callers opt in with `Model.simulate({ engine: 'wasm' })` (or
`Model.run(..., { engine: 'wasm' })`), and everything defaults to `'vm'`, so existing code
is untouched.

The approach keeps the public `Sim`/`Model`/`Run` surface a single thin facade and pushes
all engine-specific behavior down into `DirectBackend`, the layer that already maps opaque
integer handles to native objects. `DirectBackend` records which engine a sim handle uses
and branches every operation on it: VM handles flow through the existing FFI calls, while
wasm handles compile the blob, instantiate it as its own `WebAssembly.Instance`, and drive
its exports directly — reading results out of the blob's linear memory by striding a single
`Float64Array`. Putting the demultiplexing here (rather than in `Sim` subclasses) is
deliberate: in the browser the blob lives inside the Web Worker that wraps a
`DirectBackend`, while `Sim` lives on the main thread, so only the backend can own and call
it. To give the wasm path the VM's genuine *incremental* run semantics (run to time T,
change a constant, continue — affecting only the remainder), the blob's ABI is extended in
Rust with a resumable run interface (`run_to` / `run_initials` / `reset`) mirroring `vm.rs`,
backed by a persistent step cursor stored in mutable wasm globals. The VM remains the
correctness oracle throughout: the wasm path is parity-tested against it within the engine's
existing tolerances. LTM (loop) analysis stays VM-only and raises explicit errors on the
wasm engine rather than silently falling back. Work is sequenced node-first (the Rust ABI,
then the TypeScript core and a VM-vs-wasm benchmark), with the browser path requiring only a
single additive `engine` field on the existing worker message.

## Definition of Done

This work integrates the engine's WebAssembly code-generation backend (the per-model
wasm blob produced by libsimlin's `simlin_model_compile_to_wasm`) into the
`@simlin/engine` TypeScript API as a selectable simulation engine, so callers can run a
model either on the bytecode VM (today's behavior) or on a JIT-compiled wasm blob.

1. **`Model.simulate({ engine?: 'vm' | 'wasm' })`** (and `Model.run(..., { engine? })`)
   selects the simulation backend, defaulting to `'vm'` so existing callers are
   unchanged. **`Sim` stays a single, unchanged facade** — no interface split, no
   `VmSim`/`WasmSim` classes. The vm-vs-wasm demultiplexing lives entirely **below the
   facade, in `DirectBackend`**: each sim op branches on the handle's recorded engine
   kind. The public `Sim` type and its method set are unchanged; `Sim.create` just
   threads the `engine` choice through to `backend.simNew`.

2. **The wasm engine path is a true semantic peer of the VM path** for `runToEnd`,
   `runTo(time)`, `reset`, `setValue` (constant overrides), `getValue`, `getSeries`,
   `getVarNames`, `getStepCount`, `getRun`, and `dispose` — matching the VM's results
   within the engine's existing tolerances. `setValue` is constrained to overridable
   constants on both backends (the VM and the blob both reject non-constant targets), so
   this is parity, not a new limitation.

3. **The wasm blob gains a resumable run ABI.** To support `runTo(time)` with the VM's
   genuine incremental semantics (run to T, change a constant, continue — the change
   affects only the remainder), the blob is extended with `run_to(time)` / `run_initials`
   / `reset` that mirror `vm.rs`'s `run_to`/`run_initials`/`reset` (a persistent step
   cursor; `set_value` between calls affects subsequent steps). This is built in the
   engine's `wasmgen` backend, surfaced through the existing libsimlin
   `simlin_model_compile_to_wasm` blob, and parity-tested against the VM (the VM remains
   the correctness oracle).

4. **`getLinks` / LTM throw an explicit error on the wasm engine.** A `getLinks()` call
   on a wasm-engine sim throws ("not supported on the wasm engine; use engine:'vm'") —
   rejected by the `DirectBackend` demux — and `Model.simulate({ engine: 'wasm',
   enableLtm: true })` is rejected up front. To keep `Model.run({ engine: 'wasm' })`
   working, `getRun()` fetches links only when LTM is enabled (a wasm sim never has LTM
   enabled, so it returns empty links; harmless for the VM path, which already returns
   `[]` with LTM off). No silent fallback.

5. **Unsupported model + `engine: 'wasm'` returns an explicit error** (no VM fallback): if
   `simlin_model_compile_to_wasm` reports the model uses a wasm-unsupported construct, the
   wasm-engine sim creation surfaces that error rather than silently using the VM.

6. **The blob + `WebAssembly.Instance` live in the backend isolate.** They are owned by
   `DirectBackend` (node's thread; and the browser Worker, since `WorkerServer` wraps a
   `DirectBackend`). `getSeries` reads the step-major linear-memory results into a
   **single `Float64Array`** with no intermediates. The blob is reused across constant
   `setValue` / `reset` / re-run and across diagram-only edits; **structural model edits
   require a new `WasmSim` (recompile)**.

7. **A node benchmark** compares VM vs wasm **simulation (eval) time** for fishbanks,
   WORLD3, and C-LEARN, with explicit warmup and median reporting, exercising the new
   public API path under node (`DirectBackend`).

8. **Both node and browser** are supported, sequenced node-first. **The node path and
   the benchmark require zero worker-protocol changes** (`DirectBackend` runs
   in-thread). The browser path keeps the postMessage interface structurally unchanged:
   the only delta is one **optional, additive `engine` field on the existing `simNew`
   message** (no new message types, no new response shapes; `getSeries` already transfers
   `Float64Array` zero-copy).

### Out of scope
- LTM / loop-score analysis over the wasm backend (`getLinks` stays VM-only).
- Changing the bytecode VM.
- The diagram/app interactive scrubbing UX end-to-end (the engine mechanisms here are in
  scope; wiring them into the app's live graphs is a separate effort).

## Acceptance Criteria

### engine-wasm-sim.AC1: Engine selection via `Model.simulate`/`run`
- **engine-wasm-sim.AC1.1 Success:** `Model.simulate({engine:'wasm'})` returns a `Sim` driven by the blob; `simulate()` / `{engine:'vm'}` returns the VM-backed `Sim`.
- **engine-wasm-sim.AC1.2 Success:** `Model.run({engine:'wasm'})` returns a `Run` whose series match `Model.run({engine:'vm'})` within tolerance.
- **engine-wasm-sim.AC1.3 Success:** existing callers passing no `engine` get today's VM behavior (default unchanged).

### engine-wasm-sim.AC2: `runToEnd`/`runTo` parity (resumable)
- **engine-wasm-sim.AC2.1 Success:** `runToEnd()` (wasm) series equal the VM within tolerance.
- **engine-wasm-sim.AC2.2 Success:** `runTo(t)` then `getValue(name)` (wasm) equals the VM's value at `t`.
- **engine-wasm-sim.AC2.3 Success:** segmented `runTo(t1)` then `runTo(t2)` (`t1<t2`) equals a single `runTo(t2)` and the VM.
- **engine-wasm-sim.AC2.4 Edge:** `runTo(t)` past FINAL_TIME clamps to the end, matching the VM.

### engine-wasm-sim.AC3: `reset` parity
- **engine-wasm-sim.AC3.1 Success:** `reset()` then `runToEnd()` (wasm) reproduces the compiled-default results, matching the VM.
- **engine-wasm-sim.AC3.2 Success:** `reset()` preserves constant overrides set via `setValue` (matching VM reset semantics).

### engine-wasm-sim.AC4: By-name reads parity + single allocation
- **engine-wasm-sim.AC4.1 Success:** `getSeries(name)` (wasm) equals the VM's series for every variable in the layout, within tolerance.
- **engine-wasm-sim.AC4.2 Success:** `getVarNames()` and `getStepCount()` (wasm) match the VM.
- **engine-wasm-sim.AC4.3 Success:** `getSeries` returns one `Float64Array` of length `n_chunks`, read strided from linear memory with no intermediate arrays.
- **engine-wasm-sim.AC4.4 Failure:** `getSeries(unknownName)` errors the same way as the VM path.

### engine-wasm-sim.AC5: `setValue` (constants) + mid-run + reuse
- **engine-wasm-sim.AC5.1 Success:** `setValue(const, v)` then run (wasm) matches the VM under the same override.
- **engine-wasm-sim.AC5.2 Failure:** `setValue(nonConstant, v)` (wasm) throws, matching the VM's constants-only rejection.
- **engine-wasm-sim.AC5.3 Success:** `runTo(t1)`, `setValue(const, v)`, `runTo(t2)` affects only steps after `t1` (incremental semantics, matches VM).
- **engine-wasm-sim.AC5.4 Success:** `setValue`/`reset`/re-run on an existing wasm `Sim` reuses the same blob instance (no recompile).

### engine-wasm-sim.AC6: `getLinks`/LTM explicit errors
- **engine-wasm-sim.AC6.1 Failure:** `getLinks()` on a wasm sim throws an explicit "not supported on the wasm engine" error.
- **engine-wasm-sim.AC6.2 Failure:** `Model.simulate({engine:'wasm', enableLtm:true})` is rejected up front with a clear error.
- **engine-wasm-sim.AC6.3 Success:** `Model.run({engine:'wasm'})` succeeds and returns a `Run` with empty `links`.

### engine-wasm-sim.AC7: Unsupported model → explicit error (no fallback)
- **engine-wasm-sim.AC7.1 Failure:** `Model.simulate({engine:'wasm'})` on a wasm-unsupported model throws the explicit `WasmGenError`, never silently using the VM.
- **engine-wasm-sim.AC7.2 Success:** that same model runs fine via `engine:'vm'`.

### engine-wasm-sim.AC8: Browser/worker parity + minimal protocol
- **engine-wasm-sim.AC8.1 Success:** through `WorkerBackend`, `engine:'wasm'` produces series matching node `DirectBackend` (and the VM).
- **engine-wasm-sim.AC8.2 Success:** the protocol delta is exactly one optional `engine` field on the existing `simNew` message — no new message types/response shapes; `getSeries` still transfers zero-copy.

### engine-wasm-sim.AC9: Node benchmark
- **engine-wasm-sim.AC9.1 Success:** a node benchmark reports warm-median simulation (eval) time for fishbanks, WORLD3, and C-LEARN on both engines, via `Model.simulate({engine})`, with explicit warmup.

## Glossary

- **System dynamics (SD)**: A modeling discipline for simulating how stocks, flows, and
  feedback loops in a system evolve over time; the domain Simlin's engine serves.
- **`@simlin/engine`**: The TypeScript package that exposes the (WASM-compiled) Rust
  simulation engine to JavaScript callers through a Promise-based API.
- **`simlin-engine`**: The core Rust crate that compiles, type-checks, and simulates SD
  models. It produces one `CompiledSimulation` that can be executed by either the bytecode
  VM or the wasm backend.
- **`CompiledSimulation`**: The engine's compiled, ready-to-run representation of a model
  (slot layout, runlists, bytecode), consumed by both execution backends.
- **Bytecode VM (`vm.rs`)**: The stack-based virtual machine that interprets the engine's
  compiled bytecode. It is today's default simulation path and the design's correctness
  oracle for the wasm engine.
- **`wasmgen` backend**: The engine's alternative code-generation path that lowers a
  `CompiledSimulation` into a single self-contained WebAssembly module mirroring the VM
  opcode-for-opcode, intended for fast repeated re-simulation.
- **wasm blob / wasm-engine path**: The per-model WebAssembly module emitted by `wasmgen`.
  The wasm-vs-VM branching lives entirely inside `DirectBackend`; there is no `WasmSim`
  class.
- **`simlin_model_compile_to_wasm`**: The libsimlin C FFI entry point that compiles a
  model's datamodel to the wasm blob and returns it alongside a serialized `WasmLayout`. Its
  signature is unchanged by this work; only the blob's exported function set grows.
- **libsimlin**: The flat C FFI wrapper around `simlin-engine`, exposing the engine to
  TypeScript (via WASM), Go, and C/C++ through opaque, reference-counted pointer handles.
- **`WasmLayout`**: The blob's result-layout descriptor: geometry (`n_slots`, `n_chunks`,
  `results_offset`) plus a canonical-variable-name -> slot-offset map (`var_offsets`) that
  lets a host read one variable's time series by striding the results region.
- **Geometry globals (`n_slots` / `n_chunks` / `results_offset`)**: Immutable i32 wasm
  globals the blob exports describing its result slab — slots per step, number of saved
  steps, and the byte offset where results begin.
- **Step-major results slab**: The blob's output region in linear memory: `n_chunks` rows of
  `n_slots` f64 values (time in column 0), so a variable's series is read at stride
  `n_slots`.
- **Blob ABI / exports (`run` / `set_value` / `reset` / `clear_values`)**: The contract the
  emitted module exposes. `run` simulates from the start; `set_value` applies a constant
  override; `clear_values` discards overrides; `reset` (extended here) clears run state
  while keeping overrides.
- **Resumable run ABI (`run_to` / `run_initials` / `reset`)**: The new blob exports this
  design adds, mirroring `vm.rs`, so a run can be advanced incrementally from a persistent
  step cursor — enabling `Sim.runTo(time)` with true mid-run-edit semantics on the wasm
  engine.
- **Step cursor**: The VM's resumable run state (`curr_chunk` / `next_chunk` / `step_accum`
  / current time / `did_initials`), replicated in the blob as mutable wasm globals so
  `run_to` resumes where it left off.
- **`use_prev_fallback`**: The single mutable wasm global the blob already carries (it gates
  `PREVIOUS()` fallback behavior); cited as precedent that the module can hold mutable state,
  which the resumable cursor extends.
- **Euler / RK2 / RK4**: Numerical integration methods (Euler and second-/fourth-order
  Runge-Kutta) the simulation supports; both backends implement all three.
- **LTM (Loops That Matter)**: The engine's feedback-loop scoring analysis (link/loop
  scores). It is VM-only here; requesting it on the wasm engine is an explicit error, not a
  fallback.
- **`getLinks` / `Link`**: API for retrieving a model's causal links, optionally annotated
  with LTM scores. On the wasm engine this throws.
- **`WasmGenError` / `Unsupported`**: The error the `wasmgen` backend returns for a model
  using a construct it cannot lower (e.g. a true runtime-range subscript, or array unrolling
  beyond the per-function budget). Surfaced to wasm-engine callers instead of silently using
  the VM.
- **`Sim` / `Model` / `Run`**: The public `@simlin/engine` classes. `Model` builds sims;
  `Sim` is the thin step-by-step simulation facade (run, reset, get/set values, read series);
  `Run` is an immutable, WASM-free holder of completed results. All three stay structurally
  unchanged.
- **`EngineBackend`**: The TypeScript interface abstracting engine operations; `simNew` gains
  one optional `engine` parameter (the only interface change).
- **`DirectBackend`**: The in-thread `EngineBackend` implementation (used by Node and inside
  the Worker). It owns the handle->pointer map and is where this design confines all
  wasm-vs-VM branching, including ownership of the blob's `WebAssembly.Instance`.
- **Handle / `HandleEntry`**: Opaque non-zero integers the backend hands out in place of raw
  WASM pointers, tracked in a `HandleEntry { kind, ptr, ... }` map (extended here with the
  recorded engine kind plus wasm instance/layout for `'sim'` entries).
- **`WorkerBackend` / `WorkerServer` / worker protocol**: The browser path. `WorkerBackend`
  (main thread) sends `postMessage` requests over a discriminated-union protocol to
  `WorkerServer` (in a Web Worker), which delegates to an internal `DirectBackend`. The only
  change is one optional `engine` field on the existing `simNew` message.
- **`MaybePromise<T>`**: The backend's `T | Promise<T>` return helper — `DirectBackend`
  resolves synchronously, `WorkerBackend` asynchronously.
- **`WebAssembly.Instance` / linear memory**: A standard instantiated WebAssembly module and
  its flat byte-addressable memory; the blob's instance is owned by `DirectBackend`, and its
  results are read straight out of this memory.
- **Zero-copy transfer / `Float64Array`**: Transferring an `ArrayBuffer` across the Worker
  boundary by moving ownership rather than copying. `getSeries` returns one `Float64Array`
  this way, so the wasm path needs no new worker response shapes.
- **DLR-FT `wasm-interpreter`**: A pure-Rust, no_std WebAssembly interpreter (from DLR-FT)
  pinned as a dev-dependency and used in Rust tests to execute the emitted blob and verify it
  against the VM. It is a test oracle, not part of the runtime path.
- **`set_value` / constant override**: Replacing an overridable *constant*'s value before or
  during a run. Both the VM and the blob accept only constants and reject non-constant
  targets, so the wasm path's constraint is parity, not a new limitation.
- **fishbanks / WORLD3 / C-LEARN**: The three SD models the node benchmark uses to compare
  VM-vs-wasm simulation time (a small game, a large classic global model, and a
  climate-policy model, respectively).
- **`wasm-backend-poc.mjs`**: The throwaway Node proof-of-concept that already drives the
  blob end-to-end (the "direct-drive" architecture), serving as the reference for the layout
  parser and strided result read.

## Architecture

The engine already compiles one `CompiledSimulation` two ways: the bytecode VM (`vm.rs`)
and the `wasmgen` backend, the latter surfaced to hosts as a self-contained wasm blob via
libsimlin's `simlin_model_compile_to_wasm`. `@simlin/engine` exposes only the VM today.
This work makes the blob a second, selectable engine behind the *existing* `Sim` facade,
with all engine-specific behavior confined to `DirectBackend`.

**Selection.** `Model.simulate({ engine?: 'vm' | 'wasm' })` and `Model.run(..., { engine? })`
carry the choice (default `'vm'`, so existing callers are unchanged). `Sim` is unchanged;
`Sim.create` threads `engine` into `backend.simNew(modelHandle, enableLtm, engine)`. That
third parameter is the only `EngineBackend` interface change.

**Demux below the facade.** `DirectBackend` records the engine on its `'sim'` handle entry
and branches every sim op on it. For `'vm'`, the existing libsimlin FFI calls are
unchanged. For `'wasm'`, `simNew` compiles the model to a blob, instantiates it as its own
import-free `WebAssembly.Instance`, parses the returned `WasmLayout`, and stores
instance + layout + exports on the entry; later ops drive the blob directly. `Sim`,
`getRun`, and `Run` are otherwise untouched — except `getRun` fetches `getLinks` only when
LTM is enabled (a wasm sim never has LTM, so it returns empty links; harmless for the VM,
which already returns `[]` with LTM off).

**Why the demux is in the backend, not in Sim subclasses.** In the browser the blob's
`WebAssembly.Instance` lives in the Web Worker (because `WorkerServer` wraps a
`DirectBackend`), while the `Sim` object lives on the main thread. A blob can only be
driven where it lives, so the per-engine execution must sit in `DirectBackend`; a
main-thread `Sim` subclass holding the blob could not reach it across the worker boundary. This keeps `Sim` a single thin
facade and confines all engine knowledge to one place.

**Blob ABI (the contract this design extends).** The emitted module exports `memory`,
`run`/`set_value`/`reset`/`clear_values`, and immutable i32 globals
`n_slots`/`n_chunks`/`results_offset`; results are a step-major f64 slab (`n_chunks` rows
of `n_slots`, time in column 0). Today `run()` computes the whole simulation from t0 in one
call. To give `Sim.runTo(time)` the VM's incremental semantics on the wasm engine, the blob
gains a **resumable run ABI** mirroring `vm.rs`:

```
run_initials()       // idempotent: seed initials at the cursor if not already done
run_to(time: f64)    // run_initials if needed, then step from the persistent cursor
                     //   until current time > target (clamped to FINAL_TIME)
reset()              // clear the step cursor + did_initials + prev-values flag;
                     //   KEEP constant overrides (clear_values still discards them)
// unchanged: run(), set_value(off,val)->i32, clear_values(), memory, the geometry globals
```

The step cursor (`curr_chunk` / `next_chunk` / `step_accum` / current time / `did_initials`
/ `prev_values_valid`) persists across calls as mutable wasm globals (the module already
carries one mutable global, `use_prev_fallback`). `set_value` between `run_to` calls
mutates the live override region, so a constant changed mid-run affects only subsequent
steps — matching the VM. `simlin_model_compile_to_wasm`'s FFI signature is unchanged; only
the blob's exported function set grows.

**Host read path.** `getSeries(name)` resolves `name` to a slot via the parsed
`WasmLayout.var_offsets`, then copies that variable's `n_chunks` values out of the blob's
linear memory into a *single* `Float64Array` (stride `n_slots`, no intermediates):
`value[c] = f64 @ results_offset + (c * n_slots + off) * 8`. The Worker already transfers
`Float64Array` results zero-copy, so no worker response shape changes.

**Recompile policy.** A blob is bound to a model's compiled structure and sim specs. The
caller owns recompilation: `simulate({engine:'wasm'})` compiles a fresh blob; constant
changes use `setValue` on the existing `Sim` (the blob's constants-only `set_value`);
diagram-only edits keep the blob; structural edits mean the caller makes a new `Sim`. The
engine does not auto-track staleness.

## Existing Patterns

Investigation grounded every touch point in current code; this design slots a second engine
into established seams with no pattern divergence.

- **Backend handle/demux:** `DirectBackend` (`src/engine/src/direct-backend.ts`) maps
  integer handles to native pointers via `HandleEntry { kind, ptr, disposed, projectHandle }`
  (:118-124), allocated through one chokepoint `allocHandle` (:131); sim ops read
  `getSimPtr(handle)` (e.g. `simRunToEnd` :402, `simGetSeries` :426). Adding
  `{ engine, wasmInstance, wasmLayout, wasmExports }` to the `'sim'` entry and branching each
  op follows this structure exactly.
- **FFI marshalling:** `internal/sim.ts` (`simlin_sim_get_series` / `simlin_sim_set_value`)
  is the error-pointer + malloc'd-out-buffer idiom, built on `internal/memory.ts`
  (`malloc`/`free`/`allocOutPtr`/`readOutPtr`/`allocOutUsize`/`readOutUsize`/`copyFromWasm`/
  `readFloat64Array`/`stringToWasm`). The new `internal/wasmgen.ts` follows it; the only
  difference is four out-pointers + an error pointer and two buffers to `copyFromWasm` then
  `simlin_free`.
- **Backend interface:** `EngineBackend` (`src/engine/src/backend.ts`) with `MaybePromise<T>`
  (DirectBackend sync, WorkerBackend async). The `simNew` signature is the one edit.
- **Worker:** `WorkerServer` (`src/engine/src/worker-server.ts`) is a `switch(request.type)`
  over a discriminated-union protocol (`worker-protocol.ts`) and is internally backed by a
  `DirectBackend` (:31,46) — so the blob lives in the Worker for free. `getSeries` already
  returns via `sendFloat64WithTransfer` + `detachable` (zero-copy). The `simNew` request
  variant (`worker-protocol.ts:83`) gains one optional field.
- **Sim/Model/Run:** `Sim` (`src/engine/src/sim.ts`) is a thin facade delegating to the
  backend; `Model.simulate`/`run`/`loops` (`src/engine/src/model.ts`); `Run`
  (`src/engine/src/run.ts`) is a pure data holder needing no change. The throwaway
  `src/engine/wasm-backend-poc.mjs` already proves the node blob path end-to-end (blob
  ~18.6x faster than the VM for population under V8) and is the reference for the layout
  parser + stride read.
- **Resumable-ABI spec:** `vm.rs` `run_to` / `run_initials` / `reset` is the exact behavior
  the blob's new exports mirror; the blob is generated in `src/simlin-engine/src/wasmgen/`
  (`module.rs` `assemble_simulation` exports at :1819-1828, plus the `run` driver). libsimlin
  `tests/wasm.rs` is the FFI parity-test template (validate + run blob, compare to the VM).

## Implementation Phases

Node-first: the Rust resumable ABI unblocks `runTo`; the TS node core + benchmark deliver
the comparison; the single browser field comes last.

<!-- START_PHASE_1 -->
### Phase 1: wasm resumable run ABI (Rust)
**Goal:** the emitted blob supports VM-parity incremental runs (`run_to(time)` resumable
from a persistent cursor), so the TS engine can implement `runTo`.

**Components:**
- `src/simlin-engine/src/wasmgen/module.rs` — the `run` driver gains `run_initials`,
  `run_to(time)`, and a resumable `reset`; the step cursor becomes mutable wasm globals; the
  existing self-resetting `run()` and the constant-override exports stay.
- `src/simlin-engine/src/wasmgen/lower.rs` — loop restructuring as needed for the resumable
  driver.
- `src/simlin-engine/tests/simulate.rs` — parity harness runs supported models through the
  blob's segmented `run_to` (including a mid-run `set_value`) and compares to the VM's
  segmented `run_to`, the existing oracle.
- `src/libsimlin/tests/wasm.rs` — exercise the new `run_to` / `reset` exports across the FFI.

**Dependencies:** none.

**Done when:** the blob's segmented `run_to` / `reset` (incl. a mid-run constant override)
match the VM within existing tolerances; the libsimlin FFI signature is unchanged; `cargo
test` passes. Covers `engine-wasm-sim.AC2.*`, `engine-wasm-sim.AC3.*`, `engine-wasm-sim.AC5.*`
at the blob/VM-parity level.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: @simlin/engine node core (TS)
**Goal:** `Model.simulate({ engine: 'wasm' })` runs under node with VM parity; engine
demux + the explicit error semantics live in `DirectBackend`.

**Components:**
- `src/engine/src/model.ts` — `simulate` / `run` accept `{ engine? }` and forward it.
- `src/engine/src/sim.ts` — `Sim.create` threads `engine` to `backend.simNew`; `getRun`
  fetches `getLinks` only when LTM is enabled.
- `src/engine/src/backend.ts` — `simNew(modelHandle, enableLtm, engine?)` (the only interface
  change).
- `src/engine/src/internal/wasmgen.ts` (new) + `internal/index.ts` — wrap
  `simlin_model_compile_to_wasm` (four out-pointers + error; `copyFromWasm` both buffers;
  `simlin_free` both) and parse the serialized `WasmLayout`.
- `src/engine/src/direct-backend.ts` — widen `HandleEntry` / `allocHandle`; `simNew` for
  `'wasm'` compiles + instantiates + parses (explicit error on an unsupported model or on
  `enableLtm`); per-op demux for `simRunToEnd` / `simRunTo` / `simReset` / `simGetValue` /
  `simGetSeries` / `simGetVarNames` / `simGetStepCount` / `simSetValue`; `simGetLinks` throws
  for wasm.
- `src/engine/tests/` — jest parity tests (VM vs wasm via `DirectBackend`), the error cases,
  and constants-only `setValue`.

**Dependencies:** Phase 1.

**Done when:** a node `DirectBackend` runs supported models through both engines with
matching series; unsupported-model / `enableLtm` / `getLinks` raise explicit errors;
`Model.run({engine:'wasm'})` works (empty links); tests pass. Covers `engine-wasm-sim.AC1.*`,
`engine-wasm-sim.AC2.*`, `engine-wasm-sim.AC4.*`, `engine-wasm-sim.AC5.*`,
`engine-wasm-sim.AC6.*`, `engine-wasm-sim.AC7.*`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: browser / worker
**Goal:** `engine: 'wasm'` works through the Web Worker with no structural protocol change.

**Components:**
- `src/engine/src/worker-protocol.ts` — add the optional `engine` field to the `simNew`
  request variant (:83).
- `src/engine/src/worker-server.ts` — forward `request.engine` in the `simNew` case (~:369).
- `src/engine/src/worker-backend.ts` — include `engine` in the `simNew` message builder
  (~:512).
- `src/engine/tests/` — a worker-backend parity test for `engine: 'wasm'`.

**Dependencies:** Phase 2.

**Done when:** a `WorkerBackend` runs a wasm-engine sim with results matching node;
`getSeries` still transfers zero-copy; no new message types or response shapes. Covers
`engine-wasm-sim.AC8.*`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: node benchmark
**Goal:** a repeatable VM-vs-wasm **simulation-time** comparison under node.

**Components:**
- A node benchmark (under `src/engine/`) that, through `Model.simulate({ engine })`, measures
  eval/simulation time for fishbanks, WORLD3, and C-LEARN with explicit warmup + median, and
  reports VM vs wasm per model.

**Dependencies:** Phase 2.

**Done when:** the benchmark runs all three models on both engines under node and reports a
warm median simulation time per engine. Covers `engine-wasm-sim.AC9.*`.
<!-- END_PHASE_4 -->

## Additional Considerations

**Error handling (explicit, no fallback).** Requesting `engine:'wasm'` for a model the
backend reports as wasm-unsupported, or with `enableLtm:true`, fails at `simNew` with a clear
error rather than silently using the VM. A `getLinks()` call on a wasm-engine sim throws.
These surface as the engine's existing `SimlinError`/`Error` types.

**Blob instance lifecycle.** The blob's `WebAssembly.Instance` and its linear memory are
owned by the `DirectBackend` handle entry and released on `simDispose` (GC); there is no
libsimlin allocation to free for the blob's own memory. The blob's memory is sized once at
compile time and does not grow during a run, so a strided `getSeries` read is never
invalidated mid-call (unlike the libsimlin singleton, whose memory can grow).

**Parity tolerance.** Wasm-vs-VM agreement is judged within the engine's existing comparator
tolerances (the wasm backend is not bit-identical to the VM's libm by design); this matches
how the wasm backend's own corpus parity is gated.

