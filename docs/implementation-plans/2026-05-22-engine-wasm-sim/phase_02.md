# @simlin/engine WebAssembly Simulation Backend — Phase 2: @simlin/engine node core (TS)

**Goal:** Make `Model.simulate({ engine: 'wasm' })` (and `Model.run(..., { engine: 'wasm' })`) run under Node with VM parity. All vm-vs-wasm branching lives in `DirectBackend`; the public `Sim`/`Model`/`Run` classes stay structurally unchanged; `'vm'` remains the default so existing callers are untouched.

**Architecture:** A new functional-core module `internal/wasmgen.ts` wraps the libsimlin FFI `simlin_model_compile_to_wasm` (returns the per-model wasm blob bytes + a serialized `WasmLayout`) and provides two pure functions: `parseWasmLayout` (decode the little-endian layout wire format) and `readStridedSeries` (strided f64 read of one variable's series out of the blob's linear memory into a single `Float64Array`). A complete, Rust-matching `canonicalize` resolves caller variable names to the layout's canonical keys. `DirectBackend.simNew` gains an optional `engine` param: for `'wasm'` it compiles the blob, instantiates it as its own import-free `WebAssembly.Instance` (synchronously), parses the layout, captures the model's stop time, and stores instance+layout+exports on the handle entry; every sim op then branches on the entry's recorded engine. `Sim.create` threads `engine` through `backend.simNew`; `getRun` fetches `getLinks` only when LTM is enabled (so a wasm sim — which never enables LTM — returns empty links and `Model.run` works). Explicit errors (no VM fallback) for unsupported models, `enableLtm` on wasm, and `getLinks` on wasm.

**Tech Stack:** TypeScript (engine package, `lib: ["es2020", "dom"]` so `WebAssembly.*` types compile); jest + ts-jest (`testEnvironment: node`, `tests/**/*.test.ts`, `internal/wasm` mapped to the node loader); functional-core / imperative-shell pattern; the synchronous `WebAssembly.Module`/`Instance` constructors (the blob is import-free; `DirectBackend` runs off the browser main thread, so sync compile is allowed everywhere it runs).

**Scope:** Phase 2 of 4. Depends on Phase 1 (the blob's `run_to`/`run_initials`/resumable `reset` ABI). Phase 3 wires the browser worker; Phase 4 is the benchmark.

**Codebase verified:** 2026-05-22

---

## Acceptance Criteria Coverage

### engine-wasm-sim.AC1: Engine selection via `Model.simulate`/`run`
- **engine-wasm-sim.AC1.1 Success:** `Model.simulate({engine:'wasm'})` returns a `Sim` driven by the blob; `simulate()` / `{engine:'vm'}` returns the VM-backed `Sim`.
- **engine-wasm-sim.AC1.2 Success:** `Model.run({engine:'wasm'})` returns a `Run` whose series match `Model.run({engine:'vm'})` within tolerance.
- **engine-wasm-sim.AC1.3 Success:** existing callers passing no `engine` get today's VM behavior (default unchanged).

### engine-wasm-sim.AC2: `runToEnd`/`runTo` parity (resumable)
- **engine-wasm-sim.AC2.1 Success:** `runToEnd()` (wasm) series equal the VM within tolerance.
- **engine-wasm-sim.AC2.2 Success:** `runTo(t)` then `getValue(name)` (wasm) equals the VM's value at `t`.
- **engine-wasm-sim.AC2.3 Success:** segmented `runTo(t1)` then `runTo(t2)` (`t1<t2`) equals a single `runTo(t2)` and the VM.
- **engine-wasm-sim.AC2.4 Edge:** `runTo(t)` past FINAL_TIME clamps to the end, matching the VM.

### engine-wasm-sim.AC3: `reset` parity (facade-level; the blob behavior is proven in Phase 1)
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
- **engine-wasm-sim.AC7.1 Failure:** `Model.simulate({engine:'wasm'})` on a wasm-unsupported model throws the explicit error, never silently using the VM.
- **engine-wasm-sim.AC7.2 Success:** that same model runs fine via `engine:'vm'`.

---

## Background: what exists today (verified)

All paths absolute from `/home/bpowers/src/simlin`. The design's cited line numbers all matched current code.

**`DirectBackend`** (`src/engine/src/direct-backend.ts`):
- `HandleEntry` (`:116-124`): `{ kind: 'project'|'model'|'sim'; ptr: number; disposed: boolean; projectHandle?: number }`. Backing store `_handles: Map<number, HandleEntry>` (`:128`), `_nextHandle = 1` (`:127`), `_projectChildren` (`:129`).
- `allocHandle(kind, ptr, extra?: { projectHandle? }): number` (`:131-145`) — the single handle-creation chokepoint.
- `getEntry(handle, expectedKind): HandleEntry` (`:147-159`), `getSimPtr` (`:169`), `getModelPtr` (`:163`), `getProjectPtr` (`:159`).
- Sim ops (exact lines): `simNew(modelHandle, enableLtm): SimHandle` (`:378-384`), `simDispose` (`:386-396`), `simRunTo(handle, time)` (`:398-400`), `simRunToEnd` (`:402-404`), `simReset` (`:406-408`), `simGetTime` (`:410-412`), `simGetStepCount` (`:414-416`), `simGetValue(handle, name)` (`:418-420`), `simSetValue(handle, name, value)` (`:422-424`), `simGetSeries(handle, name): Float64Array` (`:426-429`, computes `stepCount` then calls FFI), `simGetVarNames` (`:431-433`), `simGetLinks` (`:435-438`), `modelGetSimSpecsJson(handle): Uint8Array` (`:372`).
- `simDispose` calls `simlin_sim_unref(entry.ptr)` (`:395`); `projectDispose` (`:235`) and `reset` (`:183-192`) also unref child/all sim ptrs. **All three must skip the unref for wasm entries** (no native alloc; the `WebAssembly.Instance` is GC'd when the entry is dropped).

**`EngineBackend`** (`src/engine/src/backend.ts`): full interface `:41-97`; `simNew(modelHandle: ModelHandle, enableLtm: boolean): MaybePromise<SimHandle>` at `:85` (the **only** interface change). `MaybePromise<T> = T | Promise<T>` (`:39`) — `DirectBackend` returns `T` synchronously; the wasm path stays synchronous.

**`Sim`** (`src/engine/src/sim.ts`): `static async create(model, overrides = {}, enableLtm = false): Promise<Sim>` (`:44-57`) → `await backend.simNew(model.handle, enableLtm)`; fields `_handle/_model/_overrides/_disposed/_enableLtm`; getter `ltmEnabled` (`:82-84`). `getRun` (`:198-222`) currently fetches links unconditionally at `:212` (`const [loops, links, stepCount] = await Promise.all([this._model.loops(), this.getLinks(), this.getStepCount()])`). `reset` (`:127`) calls `backend.simReset` then re-applies `_overrides` via `simSetValue`.

**`Model`** (`src/engine/src/model.ts`): `simulate(overrides = {}, options: { enableLtm?: boolean } = {})` (`:430-434`); `run(overrides = {}, options: { analyzeLtm?: boolean } = {})` (`:448-456`, maps `analyzeLtm` → `enableLtm`); `loops()` (`:314-317`, model-level, engine-agnostic); `timeSpec()` (`:297`) already parses `modelGetSimSpecsJson` → `{ start, stop: simSpecs.endTime, dt, units }` (with a defensive `simSpecs.endTime ?? 10`).

**`Run`** (`src/engine/src/run.ts`): pure data holder, no WASM. `RunData` (`:18-25`): `{ varNames; results: Map<string, Float64Array>; loops; links: readonly Link[]; stepCount; overrides }`. `convertLinks(0)` returns `[]` (`direct-backend.ts:97-114`), so LTM-off VM runs already carry `links: []`. No change needed to `Run`.

**FFI marshalling** (`src/engine/src/internal/`):
- `memory.ts`: `malloc(size)` (`:19`, wraps `simlin_malloc`), `free(ptr)` (`:33`, wraps `simlin_free`), `stringToWasm` (`:57`), `copyFromWasm(ptr, len): Uint8Array` (`:132`), `allocOutPtr` (`:142`), `readOutPtr` (`:152`), `allocOutUsize` (`:162`), `readOutUsize` (`:172`), `readFloat64Array(ptr, count): Float64Array` (`:197`, element-wise `DataView.getFloat64(..., true)`).
- `model.ts:349-376` `callBufferReturningFn` — the single-buffer error-ptr + out-buffer template (reads via `copyFromWasm`, frees the returned `bufPtr` with `free()`). **The new `wasmgen.ts` wrapper is this shape with two out-buffers.**
- `internal/index.ts:13-21` aggregates the wrappers (`export * from './sim'`, etc.). Add `export * from './wasmgen';`.
- The raw FFI functions are accessed dynamically via `getExports().simlin_X as (...) => ...` (no generated `.d.ts`). `simlin_model_compile_to_wasm` has **no TS binding yet** — `wasmgen.ts` creates it. Rust signature (`src/libsimlin/src/model.rs:117`): `(model, out_wasm, out_wasm_len, out_layout, out_layout_len, out_error)`; both buffers freed with `simlin_free` (== `free()`); failure stores `SimlinErrorCode::Generic` ("wasm code generation failed: ...").
- WASM singleton loaders: `internal/wasm.node.ts` (async instantiate; `getExports()`/`getMemory()`), `internal/wasm.browser.ts`. The singleton's `memory` can grow (`maximum: 16384`). The **blob is a separate `WebAssembly.Instance`** with its own non-growing memory.

**Errors** (`src/engine/src/internal/error.ts:152-161`): `class SimlinError extends Error { code; details }` — the only Error subclass. Facade guards throw plain `Error`. FFI errors surface as `SimlinError` via the error-ptr idiom.

**Name canonicalization** (the correctness crux): `WasmLayout.var_offsets` keys are **canonical** idents (`module.rs:746`, from `CompiledSimulation.offsets`). The VM path passes raw names to the FFI, which applies `Ident::new` = Rust `canonicalize` (`common.rs:364`). The TS `src/core/canonicalize.ts` is **incomplete** (whole-string quote check; no unquoted-dot → middle-dot, no quoted-inner-dot → sentinel, not quote-aware per-part), so it gives wrong keys for dotted/module/quoted names. A complete, Rust-matching canonicalizer is required. The Rust rules (per `common.rs:364`, verified; sentinel `LITERAL_PERIOD_SENTINEL = '\u{2024}'` at `common.rs:281`): split the trimmed name into `.`-separated parts with a quote-aware iterator (`IdentifierPartIterator`, `common.rs:1534`); for each part, if wrapped in `"..."`, take the inner text and replace each inner `.` with `'\u{2024}'` (U+2024 ONE DOT LEADER), else replace each `.` with `'\u{00B7}'` (U+00B7 MIDDLE DOT, the module separator); then replace `\\` → `\`, collapse whitespace runs (space, `\t`, `\r`, `\n`, U+00A0, and the literal escape sequences `\n`/`\r`) into a single `_`, and lowercase; concatenate parts. Verified vectors (as TS string literals with escapes, never bare glyphs — U+2024/U+2025/U+00B7 are visually indistinguishable in many fonts): `canonicalizeIdent('a.b') === 'a\u{00B7}b'`, `canonicalizeIdent('"a.b"') === 'a\u{2024}b'`, `canonicalizeIdent('a."b c"') === 'a\u{00B7}b_c'`.

**Layout var-name shapes:** keys include reserved `time`/`dt`/`initial_time`/`final_time` (`db.rs:5367`); arrayed vars are per-element `base[e1,e2]` (canonical base + comma-joined canonical element names, `db.rs:5409`); module outputs `module\u{00B7}subvar`; lookup-only tables excluded; `$`-prefixed LTM vars only when LTM enabled (never for wasm). VM `simlin_sim_get_var_names` (`src/libsimlin/src/simulation.rs:726`) returns the same canonical keys, filters **only** `$`-prefixed names (`is_internal_var`, `simulation.rs:29` = `name.starts_with('$')` — it does **not** filter the reserved names), and `sort()`s by Rust byte order. `simlin_sim_get_stepcount` (`simulation.rs:270`) returns `results.step_count` = saved-row count = `n_chunks`. VM `simlin_sim_get_value` → `vm.get_value_now(off)` reads `data[curr_chunk * n_slots + off]` (`vm.rs:880-887`, the live current chunk) under a `debug_assert!(did_initials)` precondition.

**Test conventions:** jest (`pnpm test` from `src/engine/`; `jest.config.js` maps `@simlin/engine/internal/wasm`→`wasm.node.ts`). Representative end-to-end test: `src/engine/tests/direct-backend.test.ts` — `new DirectBackend(); backend.reset(); backend.configureWasm({ source: loadWasmBuffer() }); await backend.init();` then `projectOpenXmile → projectGetModel → simNew → simRunToEnd → simGetSeries`. Fixtures by `__dirname` path: `pysimlin/tests/fixtures/teacup.stmx`; `test/test-models/samples/teacup/teacup.mdl`. A wasm-**unsupported** construct: a runtime view range `[start:end]` (`ViewRangeDynamic`, `wasmgen/lower.rs:1530`); author it as a tiny fixture or via `TestProject`-equivalent. Target 95%+ coverage; functional core / imperative shell.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
Subcomponent A: the functional core — pure parsers + FFI wrapper + a correct canonicalizer. These have no `Sim`/`Model` coupling and are unit-tested in isolation.

<!-- START_TASK_1 -->
### Task 1: `internal/wasmgen.ts` — compile FFI wrapper + pure layout parser + strided read

**Verifies:** engine-wasm-sim.AC4.3 (single-`Float64Array` strided read, at the unit level)

**Files:**
- Create: `src/engine/src/internal/wasmgen.ts`
- Modify: `src/engine/src/internal/index.ts:13-21` (add `export * from './wasmgen';`)
- Test: `src/engine/tests/wasmgen.test.ts` (unit)

**Implementation:**
Define the types and three functions:
- `interface WasmLayout { nSlots: number; nChunks: number; resultsOffset: number; varOffsets: Map<string, number> }`.
- `interface WasmBlobExports { memory: WebAssembly.Memory; run(): void; run_to(time: number): void; run_initials(): void; reset(): void; set_value(offset: number, value: number): number; clear_values(): void; n_slots: WebAssembly.Global; n_chunks: WebAssembly.Global; results_offset: WebAssembly.Global }`.
- `simlin_model_compile_to_wasm(model: SimlinModelPtr): { wasm: Uint8Array; layout: Uint8Array }` — imperative shell. Mirror `internal/model.ts:349 callBufferReturningFn` but with **two** out-buffer/out-len pairs plus one error out-ptr: `allocOutPtr()` ×2 for the buffers, `allocOutUsize()` ×2 for the lengths, `allocOutPtr()` for the error. Call `getExports().simlin_model_compile_to_wasm(model, outWasm, outWasmLen, outLayout, outLayoutLen, outErr)`. If `readOutPtr(outErr) !== 0`, read code/message/details, `simlin_error_free`, and `throw new SimlinError(...)` (this is the unsupported-model path, AC7.1). On success, `copyFromWasm` both buffers, `free()` both returned `bufPtr`s, and `free()` every out-ptr in `finally`.
- `parseWasmLayout(bytes: Uint8Array): WasmLayout` — **pure**. Port the POC parser (`src/engine/wasm-backend-poc.mjs:130-155`): little-endian `DataView`; read `nSlots`/`nChunks`/`resultsOffset` as u64 (`getBigUint64(p, true)` → `Number`), `count` as u32; then `count` entries of `{ nameLen: u32, name: utf8[nameLen], offset: u64 }` into a `Map`. Use `TextDecoder` for names.
- `readStridedSeries(memory: ArrayBufferLike, layout: WasmLayout, slot: number): Float64Array` — **pure** (takes an `ArrayBuffer`, not the instance, so it is unit-testable). Allocate exactly one `Float64Array(layout.nChunks)`; fill via `new DataView(memory).getFloat64(layout.resultsOffset + (c * layout.nSlots + slot) * 8, true)` for `c in 0..nChunks`. No intermediate arrays (AC4.3).

Register the module in `internal/index.ts`.

**Testing:**
- `parseWasmLayout`: hand-build a byte buffer (known nSlots/nChunks/resultsOffset + two named entries at known offsets) and assert the parsed struct + map. Round-trip against the documented wire format.
- `readStridedSeries`: build a fake `ArrayBuffer` laid out step-major (nChunks×nSlots f64 at a known resultsOffset), and assert the function extracts a known variable's column exactly, returns a `Float64Array` of length `nChunks`, and allocates nothing else (AC4.3).

These are pure-function unit tests; they need no WASM instance or libsimlin.

**Verification:**
Run: `pnpm -C src/engine exec jest tests/wasmgen.test.ts 2>&1 | tail -20`
Expected: all unit tests pass.
Run: `pnpm -C src/engine exec tsc --noEmit 2>&1 | tail -5`
Expected: typechecks (the `WebAssembly.*` types resolve via `lib: dom`).

**Commit:** `engine: add wasmgen FFI wrapper + pure layout parser and strided read`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Complete, Rust-matching `canonicalize` for name→slot resolution

**Verifies:** supports engine-wasm-sim.AC4.1 / AC4.4 (correct name resolution into the canonical layout keys)

**Files:**
- Create: `src/engine/src/internal/canonicalize.ts`
- Test: `src/engine/tests/canonicalize.test.ts` (unit)

**Implementation:**
Implement `canonicalizeIdent(name: string): string` as a **pure** function reproducing Rust `simlin-engine/src/common.rs:364 canonicalize` exactly:
1. `trim()`.
2. Split into parts on `.` using a **quote-aware** scan (a `.` inside a `"..."` segment does not split). Mirror Rust's `IdentifierPartIterator` (`common.rs:1534`).
3. For each part: if it is wrapped in double quotes, take the inner text and replace each inner `.` with `'\u{2024}'` (U+2024 ONE DOT LEADER, the literal-period sentinel = `common.rs:281`); otherwise replace each `.` with `'\u{00B7}'` (U+00B7 MIDDLE DOT, the module separator).
4. Then on each part: replace `\\` with `\`; collapse any run of whitespace (space, `\t`, `\r`, `\n`, U+00A0) **and** the two-char escape sequences `\n`/`\r` into a single `_`; `toLowerCase()`.
5. Concatenate the parts (no separator — the `'\u{2024}'` / `'\u{00B7}'` substitutions already carry the join).

> **Why a new module, not `src/core/canonicalize.ts`:** the core helper is incomplete (no dot/quote handling) and is shared by other consumers whose behavior must not shift mid-feature. This module is the engine-local, fully-correct canonicalizer the wasm name lookup needs.

**Testing:**
Assert the Rust test vectors (from `common.rs` tests at `:412` and `:489-509`), expressed as **TS string literals with explicit `\uXXXX` escapes — never bare glyphs** (U+2024 one-dot-leader, U+2025 two-dot-leader, and U+00B7 middle-dot are visually indistinguishable in many fonts; a copied glyph would silently assert the wrong codepoint and corrupt name resolution):
- `canonicalizeIdent('Hello World') === 'hello_world'`
- `canonicalizeIdent('a.b') === 'a\u{00B7}b'`  — unquoted dot → U+00B7 middle dot
- `canonicalizeIdent('"a.b"') === 'a\u{2024}b'`  — quoted-inner dot → U+2024 one dot leader
- `canonicalizeIdent('a."b c"') === 'a\u{00B7}b_c'`
- `canonicalizeIdent('model.variable') === 'model\u{00B7}variable'`
- `canonicalizeIdent('"a/d"."b c"') === 'a/d\u{00B7}b_c'`
- `canonicalizeIdent('"a/d".b') === 'a/d\u{00B7}b'`
- `canonicalizeIdent('"quoted"') === 'quoted'`
- `canonicalizeIdent('"b c"') === 'b_c'`
- `canonicalizeIdent('café') === 'café'`  — non-ASCII passes through, lowercased
- `canonicalizeIdent('Å\nb') === 'å_b'`  — non-ASCII lowercase + literal `\n` escape → underscore
- `canonicalizeIdent('   a b') === 'a_b'`
- idempotency on already-canonical input: `canonicalizeIdent('room_temperature') === 'room_temperature'`

Include a property test: `canonicalizeIdent` is idempotent (`canonicalizeIdent(canonicalizeIdent(x)) === canonicalizeIdent(x)`).

**Verification:**
Run: `pnpm -C src/engine exec jest tests/canonicalize.test.ts 2>&1 | tail -20`
Expected: all vectors pass.

**Then file the canonicalizer-unification debt:** dispatch the `track-issue` agent (Task tool, `subagent_type: "track-issue"`) describing that `src/engine/src/internal/canonicalize.ts` duplicates the incomplete `src/core/canonicalize.ts`, and that the two should later be unified into one Rust-faithful canonicalizer (so we do not silently keep two diverging copies). Confirm the agent reports the issue filed or already-tracked.

**Commit:** `engine: add Rust-faithful canonicalize for wasm name resolution`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
Subcomponent B: the `DirectBackend` demux — sim creation/disposal, then per-op branching. Tested directly through `DirectBackend` (not yet through `Model`/`Sim`), so each task ends green before the facade is wired.

<!-- START_TASK_3 -->
### Task 3: `EngineBackend.simNew` engine param + `DirectBackend` wasm sim creation/disposal

**Verifies:** engine-wasm-sim.AC1.1, engine-wasm-sim.AC6.2, engine-wasm-sim.AC7.1, engine-wasm-sim.AC7.2, engine-wasm-sim.AC5.4 (instance owned once on the entry)

**Files:**
- Modify: `src/engine/src/backend.ts:85` (interface `simNew`)
- Modify: `src/engine/src/direct-backend.ts` (`HandleEntry` `:116-124`; `allocHandle` `:131`; `simNew` `:378-384`; `simDispose` `:386-396`; `projectDispose` `:235`; `reset` `:183-192`)
- Test: `src/engine/tests/wasm-backend.test.ts` (integration via `DirectBackend`)

**Implementation:**
1. **Interface:** change `backend.ts:85` to `simNew(modelHandle: ModelHandle, enableLtm: boolean, engine?: 'vm' | 'wasm'): MaybePromise<SimHandle>;`. Because `engine` is optional, `WorkerBackend` (which implements the interface and is untouched until Phase 3) still satisfies it. Add a shared `type SimEngine = 'vm' | 'wasm';` (export it from `backend.ts`).
2. **Widen `HandleEntry`** with wasm-only fields: `engine?: SimEngine; wasmInstance?: WebAssembly.Instance; wasmLayout?: WasmLayout; wasmExports?: WasmBlobExports; wasmStopTime?: number;`. Widen `allocHandle`'s `extra` param to carry them (or set them on the entry immediately after `allocHandle` returns).
3. **`simNew` demux:**
   - `engine` defaults to `'vm'`. For `'vm'`: unchanged path (`simlin_sim_new(modelPtr, enableLtm)`, `allocHandle('sim', ptr, { projectHandle, engine: 'vm' })`).
   - For `'wasm'`: **reject `enableLtm`** first with a clear `Error` ("LTM is not supported on the wasm engine; use engine:'vm'") — AC6.2, before any compile. Then: `const { wasm, layout } = simlin_model_compile_to_wasm(modelEntry.ptr)` (throws `SimlinError` on an unsupported model → AC7.1, no fallback). `const parsed = parseWasmLayout(layout)`. Capture stop time by parsing the model's sim specs and reading `endTime`, mirroring `Model.timeSpec()` (`model.ts:297`): `JSON.parse(new TextDecoder().decode(this.modelGetSimSpecsJson(modelHandle))).endTime`. `endTime` is a required field of the serialized `SimSpecs`, so it is effectively always present for a compiled model and no magic-number fallback is embedded here; if you choose to mirror `timeSpec`'s defensive `?? 10` (`model.ts:297`), reuse that exact expression rather than introducing a divergent constant. Instantiate synchronously and import-free: `const instance = new WebAssembly.Instance(new WebAssembly.Module(wasm), {})`. Build `wasmExports` from `instance.exports`. `allocHandle('sim', 0, { projectHandle, engine: 'wasm', wasmInstance: instance, wasmLayout: parsed, wasmExports, wasmStopTime })`. (`ptr` is `0`/unused for wasm.)
4. **Dispose guards:** in `simDispose`, `projectDispose`, and `reset`, only call `simlin_sim_unref(entry.ptr)` when `entry.engine !== 'wasm'` (a wasm entry has no native sim; dropping the entry lets the `WebAssembly.Instance` be GC'd). Keep the rest of the disposal bookkeeping unchanged.

**Testing** (via `DirectBackend` directly, mirroring `direct-backend.test.ts` setup):
- AC1.1: `simNew(modelHandle, false, 'wasm')` returns a sim handle; the entry records `engine: 'wasm'` and holds an instance (assert a subsequent `simRunToEnd` + `simGetSeries` works once Task 4 lands — for this task, assert the handle is created and `simNew(..., 'vm')` / `simNew(...)` still create VM sims).
- AC6.2: `simNew(modelHandle, true, 'wasm')` throws a clear error and creates no sim.
- AC7.1/AC7.2: build a wasm-unsupported model (runtime view range), assert `simNew(modelHandle, false, 'wasm')` throws (a `SimlinError`/`Error`, no VM fallback), and that `simNew(modelHandle, false, 'vm')` on the same model succeeds.
- AC5.4 (creation half): assert the instance is created exactly once and stored on the entry (a later `simReset`/`simSetValue` in Task 4 reuses it).
- Disposal: `simDispose` on a wasm sim does not throw and does not call `simlin_sim_unref` (e.g. spy/verify no native unref on a 0 ptr); `projectDispose` cleans up wasm child sims.

**Verification:**
Run: `pnpm -C src/engine exec jest tests/wasm-backend.test.ts 2>&1 | tail -25`
Expected: creation/error/dispose tests pass.
Run: `pnpm -C src/engine exec jest tests/direct-backend.test.ts 2>&1 | tail -10`
Expected: existing VM tests still pass (default path unchanged).

**Commit:** `engine: wasm-engine sim creation and disposal in DirectBackend`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Per-op demux (`run`/`reset`/reads/`setValue`) + `getLinks` rejection

**Verifies:** engine-wasm-sim.AC2.1, AC2.2, AC2.3, AC2.4, AC3.1, AC3.2, AC4.1, AC4.2, AC4.3, AC4.4, AC5.1, AC5.2, AC5.3, AC6.1

**Files:**
- Modify: `src/engine/src/direct-backend.ts` (the sim ops `:398-438`)
- Test: `src/engine/tests/wasm-backend.test.ts` (extend)

**Implementation:** each sim op fetches the entry via `getEntry(handle, 'sim')` and branches on `entry.engine`. For `'vm'`, the existing FFI call is unchanged. For `'wasm'`, drive the blob (`entry.wasmExports` / `entry.wasmLayout` / `entry.wasmStopTime`):
- `simRunToEnd`: `entry.wasmExports.run_to(entry.wasmStopTime)` (the blob's `run_to` runs `run_initials` internally and is resumable; using the real stop time mirrors the VM's `run_to(specs.stop)`).
- `simRunTo(time)`: `entry.wasmExports.run_to(time)` (resumable; segments accumulate — AC2.3; a `time` past stop is clamped by the blob — AC2.4).
- `simReset`: `entry.wasmExports.reset()` (Phase-1 reset: clears the cursor, keeps constant overrides — AC3.2). The `Sim` facade re-applies overrides after `simReset`, which is harmless/consistent.
- `simSetValue(name, value)`: resolve `slot = entry.wasmLayout.varOffsets.get(canonicalizeIdent(name))`; if absent → throw "unknown variable" (parity with the VM's not-found error). Else `const rc = entry.wasmExports.set_value(slot, value)`; if `rc !== 0` → throw a `SimlinError`/`Error` ("cannot set value of '<name>': not a simple constant"), matching the VM's `BadOverride` (AC5.2). `rc === 0` succeeds (AC5.1, AC5.3).
- `simGetSeries(name): Float64Array`: resolve slot (canonicalize → `varOffsets`); if absent → throw the same not-found error as the VM (AC4.4). Else `readStridedSeries(entry.wasmExports.memory.buffer, entry.wasmLayout, slot)` — one `Float64Array(nChunks)` (AC4.1, AC4.3).
- `simGetValue(name): number`: resolve slot; read the variable's **current value** from the blob's live `curr` chunk (linear-memory base 0): `new DataView(entry.wasmExports.memory.buffer).getFloat64(slot * 8, true)`. This mirrors the VM oracle exactly — `simlin_sim_get_value` → `vm.get_value_now(off)` reads `data[curr_chunk * n_slots + off]` (`vm.rs:880-887`), the live current chunk. **Precondition (same as the VM):** `get_value_now` carries `debug_assert!(did_initials)`; a `getValue` before any `run_to`/`runToEnd` reads stale base-0 memory, which matches the VM's undefined-before-initials behavior — so pre-run `getValue` is out of scope (callers run first, as the AC2.2 test does). The base-0 `curr`-chunk read is the **determined** source of truth (not a guess); the AC2.2 parity test guards it, and only if a one-step offset against the VM nonetheless surfaces should you reconcile against `vm.rs:880` (the VM is the oracle).
- `simGetStepCount(): number`: `entry.wasmLayout.nChunks` (= the VM's saved-row count — AC4.2).
- `simGetVarNames(): string[]`: from `entry.wasmLayout.varOffsets` keys, **filter only `$`-prefixed keys** (matching the VM's `is_internal_var`, `src/libsimlin/src/simulation.rs:29` = `name.starts_with('$')`) and sort by Unicode **code point** (to match Rust's byte-order `sort()`; do **not** use the default JS UTF-16 `Array.sort` for non-ASCII names). **Do NOT filter the reserved names** `time`/`dt`/`initial_time`/`final_time`: the VM's `simlin_sim_get_var_names` (`simulation.rs:726`) filters only `is_internal_var`, so those reserved names DO appear in the VM's output and the wasm path must include them too. The AC4.2 parity test guards this equality — it does not decide it; the behavior is pinned here.
- `simGetTime(): number`: `new DataView(entry.wasmExports.memory.buffer).getFloat64(0, true)` (the `time` slot is slot 0 of the live `curr` chunk at base 0).
- `simGetLinks`: for `'wasm'`, **throw** a clear `Error` ("getLinks is not supported on the wasm engine; use engine:'vm'") — AC6.1.

> Read the blob's `memory.buffer` freshly per call (the blob's memory does not grow, but reading fresh keeps the pattern uniform with the singleton helpers and avoids a stale-buffer footgun).

**Testing** (VM-vs-wasm parity through `DirectBackend`; the VM is the oracle, compared within the engine's existing tolerance — use a small supported fixture like teacup, plus at least one fixture with a constant the test overrides):
- AC2.1: `simRunToEnd` then `simGetSeries(name)` (wasm) equals the VM for the model's variables.
- AC2.2: `simRunTo(t)` then `simGetValue(name)` (wasm) equals the VM after the same `simRunTo(t)`.
- AC2.3: `simRunTo(t1)`+`simRunTo(t2)` equals a single `simRunTo(t2)` and the VM.
- AC2.4: `simRunTo(stop*2)` equals `simRunToEnd` and the VM.
- AC3.1/AC3.2: `simReset` then re-run reproduces defaults; with a prior `simSetValue(const)`, reset preserves the override (matches VM).
- AC4.1/AC4.2/AC4.4: `simGetSeries` for every var equals VM; `simGetVarNames()`/`simGetStepCount()` equal VM (the VM `getVarNames` includes the reserved time vars — assert exact array equality); `simGetSeries('definitely_not_a_var')` throws like the VM.
- AC4.3: `simGetSeries` returns one `Float64Array` of length `nChunks` (assert `instanceof Float64Array` and `.length === stepCount`).
- AC5.1/AC5.2/AC5.3: `simSetValue(const, v)` then run matches VM; `simSetValue(nonConstant, v)` throws; mid-run `simRunTo(t1)`+`simSetValue(const,v)`+`simRunTo(t2)` affects only post-`t1` steps (matches VM driven identically).
- AC6.1: `simGetLinks` on a wasm sim throws.

**Verification:**
Run: `pnpm -C src/engine exec jest tests/wasm-backend.test.ts 2>&1 | tail -30`
Expected: all parity + error tests pass.

**Commit:** `engine: per-op vm/wasm demux in DirectBackend with VM parity`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (task 5) -->
Subcomponent C: thread `engine` through the public facade and gate link-fetching, then test through `Model`/`Sim`.

<!-- START_TASK_5 -->
### Task 5: `Sim.create`/`Model.simulate`/`Model.run` engine threading + `getRun` LTM gating

**Verifies:** engine-wasm-sim.AC1.1, AC1.2, AC1.3, AC6.3

**Files:**
- Modify: `src/engine/src/sim.ts` (`create` `:44-57`; `getRun` `:198-222`)
- Modify: `src/engine/src/model.ts` (`simulate` `:430-434`; `run` `:448-456`)
- Test: `src/engine/tests/wasm-model.test.ts` (public-API parity)

**Implementation:**
- `Sim.create(model, overrides = {}, enableLtm = false, engine: SimEngine = 'vm')`: pass `engine` to `await backend.simNew(model.handle, enableLtm, engine)`. Store `engine` on the `Sim` (a private field) so `getRun`/diagnostics can see it; expose nothing new publicly.
- `Model.simulate(overrides = {}, options: { enableLtm?: boolean; engine?: SimEngine } = {})`: forward `engine` to `Sim.create(this, overrides, enableLtm, engine)`. (The `enableLtm:true && engine:'wasm'` rejection is enforced authoritatively in `DirectBackend.simNew` from Task 3, covering the worker path too; this method just forwards.)
- `Model.run(overrides = {}, options: { analyzeLtm?: boolean; engine?: SimEngine } = {})`: forward `{ enableLtm: analyzeLtm, engine }` to `simulate`. Preserve the existing `analyzeLtm`→`enableLtm` naming asymmetry.
- `getRun` (`sim.ts:212`): gate link-fetching on LTM — replace the unconditional `this.getLinks()` with `this.ltmEnabled ? this.getLinks() : Promise.resolve([])`. This makes `Model.run({engine:'wasm'})` work (a wasm sim never enables LTM, so `getLinks` is never called on it → returns `[]`), and is harmless for the VM path (which already returns `[]` with LTM off). `this._model.loops()` stays unconditional (model-level, engine-agnostic).

**Testing** (through the public `Model`/`Sim` API; VM is the oracle):
- AC1.1: `model.simulate({ engine: 'wasm' })` returns a `Sim` whose `runToEnd()`+`getSeries()` match the VM; `model.simulate()` and `model.simulate({ engine: 'vm' })` return VM-backed sims.
- AC1.2: `model.run({ engine: 'wasm' })` series equal `model.run({ engine: 'vm' })` within tolerance.
- AC1.3: existing `model.simulate(overrides)` / `model.run(overrides)` calls with no `engine` behave exactly as before (VM); confirm a representative existing test path is unaffected.
- AC6.3: `model.run({ engine: 'wasm' })` resolves to a `Run` with `links` empty (`[]`), and does not throw (no `getLinks` call on the wasm sim).

**Verification:**
Run: `pnpm -C src/engine exec jest tests/wasm-model.test.ts 2>&1 | tail -25`
Expected: public-API parity + empty-links tests pass.
Run: `pnpm -C src/engine test 2>&1 | tail -15`
Expected: the full engine suite is green (default behavior unchanged).
Run: `pnpm -C src/engine exec tsc --noEmit 2>&1 | tail -5`
Expected: typechecks (incl. `WorkerBackend` still satisfying the widened `EngineBackend`).

**Commit:** `engine: thread engine selection through Model/Sim and gate getRun links`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase 2 Done When

- `Model.simulate({ engine: 'wasm' })` and `Model.run({ engine: 'wasm' })` run supported models under a node `DirectBackend` with series matching the VM within the engine's existing tolerance; `engine: 'vm'` / no `engine` is unchanged (AC1.*).
- `runToEnd`/`runTo` (incl. segmented and clamped), `reset` (defaults + override-preserving), by-name reads (`getSeries`/`getVarNames`/`getStepCount`, single `Float64Array`, unknown-name error), and constants-only `setValue` (incl. mid-run) all match the VM (AC2.*, AC3.*, AC4.*, AC5.*).
- `getLinks()` on a wasm sim throws; `Model.simulate({engine:'wasm', enableLtm:true})` is rejected up front; an unsupported model throws (no VM fallback) yet runs via `engine:'vm'`; `Model.run({engine:'wasm'})` returns empty links (AC6.*, AC7.*).
- The only `EngineBackend` change is the optional `engine` param on `simNew`; `Sim`/`Model`/`Run` are otherwise structurally unchanged; `WorkerBackend` still compiles (Phase 3 wires it). `pnpm -C src/engine test` and `tsc --noEmit` pass.
- The `track-issue` agent has been dispatched (in Task 2) to file the debt of unifying the engine-local `canonicalizeIdent` with `src/core/canonicalize.ts`.
