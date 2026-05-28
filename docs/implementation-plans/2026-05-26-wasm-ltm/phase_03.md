# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 3: @simlin/engine node wiring

**Goal:** `Model.simulate({ engine: 'wasm', enableLtm: true })` succeeds under node (the up-front rejection is gone), `getLinks()` on a wasm sim returns links annotated with scores matching the VM, and `Run.links` is populated for a wasm LTM run.

**Architecture:** `DirectBackend` already demuxes every sim op on the handle's `engine` kind and owns each wasm sim's `WebAssembly.Instance` + parsed layout. Phase 3 threads `enableLtm` into the wasm compile, removes the two guards that forbade LTM on wasm, and implements `getLinks` for wasm by copying the blob's result slab + the serialized layout into the libsimlin singleton's memory and calling the Phase 2 from-series FFI — then deserializing with the *existing* `convertLinks` (no TS reimplementation of the analysis). Two small `HandleEntry` additions (the model pointer and the raw layout bytes) close gaps the design under-specified.

**Tech Stack:** TypeScript (`@simlin/engine`), WebAssembly JS API, ts-jest (`testEnvironment: node`). Reuses the libsimlin singleton memory helpers (`malloc`/`free`/`copyToWasm`/`allocOutPtr`) and `convertLinks`/`readLinks`.

**Scope:** Phase 3 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC1: LTM-enabled wasm compilation produces a blob carrying the LTM series
- **wasm-ltm.AC1.3 Success:** `Model.simulate({ engine: 'wasm', enableLtm: true })` resolves to a `Sim` (the up-front rejection is gone) under node, and `getLinks()` returns links annotated with scores.

### wasm-ltm.AC2: Analytic outputs match the VM within tolerance
- **wasm-ltm.AC2.3 Success:** `getLinks()` on a wasm sim (node `DirectBackend`) returns scores matching `getLinks()` on a VM sim, and `Run.links` is populated for a wasm LTM run.

---

## Background: what exists today (verified 2026-05-27)

**The TS wasm-compile wrapper (`src/engine/src/internal/wasmgen.ts`):**
- `simlin_model_compile_to_wasm` (`:92-138`): currently `(model: SimlinModelPtr) => { wasm, layout }`. Resolves the FFI from the libsimlin singleton (`getExports()`), allocates 5 out-pointers, calls the 6-arg WASM export, copies out the two buffers (`copyFromWasm` + `free`), returns `{ wasm, layout }`. After Phase 1 the WASM export is **8-arg** (`model, ltm_enabled, ltm_discovery_mode, out_wasm, ...`) and the wrapper passes `0, 0`.
- `readStridedSeries` (`:207-220`): strides one variable column out of a blob `ArrayBufferLike` using `layout.resultsOffset + (c*layout.nSlots + slot)*8`. `WasmLayout` TS interface at `:38-47` (`nSlots`, `nChunks`, `resultsOffset`, `varOffsets`).

**`DirectBackend` (`src/engine/src/direct-backend.ts`):**
- `HandleEntry` (`:152-170`) carries `wasmInstance?`, `wasmLayout?: WasmLayout` (parsed), `wasmExports?: WasmBlobExports`, `engine?: SimEngine`, and a wasm sim's `ptr` is `0`.
- `simNew` (`:442-452`) forwards `enableLtm` to `simNewWasm` (`:445`). `simNewWasm` (`:460-496`): **rejects LTM up front** (`:460-465`):
  ```ts
  if (enableLtm) {
    throw new Error("LTM is not supported on the wasm engine; use engine:'vm'");
  }
  const { wasm, layout } = simlin_model_compile_to_wasm(modelEntry.ptr);
  ```
  Then instantiates the blob, `parseWasmLayout(layout)` (`:470`, the raw `layout: Uint8Array` is **discarded** afterward), and `allocHandle('sim', 0, {...})` with `ptr: 0` — **no model pointer is retained**.
- `simGetLinks` (`:668-676`): **throws for wasm** (`:670-673`), else `simlin_analyze_get_links(entry.ptr)` + `convertLinks` (`:131-148`).
- `convertLinks` (`:131-148`): `readLinks(ptr)` → map to `Link` → `simlin_free_links(ptr)` in `finally`. Reusable as-is for the from-series result (same `SimlinLinks*` shape).
- `releaseWasmSimState` (`:508-512`): nulls the heavy wasm refs on dispose.

**libsimlin-singleton memory helpers (`src/engine/src/internal/memory.ts`):** `malloc(size)` (`:19-27`), `free(ptr)` (`:33-38`), `copyToWasm(data: Uint8Array): Ptr` (`:115-124`, the copy-JS-buffer-into-linear-memory primitive), `allocOutPtr`/`readOutPtr` (`:142-156`). The existing `simlin_analyze_get_links` TS binding (returns-a-pointer pattern) is in `src/engine/src/internal/analysis.ts:82-104`; `readLinks` is `:277-311`.

**`Sim.getRun` guard (`src/engine/src/sim.ts:234-237`):**
```ts
const wantLinks = this.ltmEnabled && this._engine !== 'wasm';
```
`Run` (`src/engine/src/run.ts:33-116`) is a pure data holder; `Run.links` getter `:106-108`.

**Flow to mirror:** `Model.simulate(overrides, { enableLtm, engine })` (`model.ts:427-434`) → `Sim.create(...)` (`sim.ts:54-72`) → `backend.simNew(model.handle, enableLtm, engine)`. VM retains the model inside `simlin_sim_new`; the wasm path does not call `simlin_sim_new`, so it must retain the model handle itself. The concrete FFI pair is **`simlin_model_ref`** (`src/libsimlin/src/model.rs:217`) and **`simlin_model_unref`** (`model.rs:226`) — `SimlinModel` carries the `ref_count` they manage. Bump with `simlin_model_ref` when storing `wasmModelPtr` and release with `simlin_model_unref` on dispose. (These already have TS bindings; if not exposed in `internal/`, add the two bindings.)

**Test infra:** ts-jest, `testEnvironment: node`, `testMatch: tests/**/*.test.ts`. VM-vs-wasm comparisons exist in `tests/wasm-backend.test.ts` (`expectSeriesClose`) and `tests/wasm-model.test.ts`. Models load via `fs.readFileSync` of XMILE + the project-open path (e.g. `api.test.ts:43`). **No LTM fixture is loaded by any TS test today** — teacup is non-LTM; a new loader pointed at `test/logistic_growth_ltm/logistic_growth.stmx` is needed. New files need a `// pattern:` FCIS classification comment.

**Divergence from the design doc:** the design lists "the unsupported-model error case" among Phase 3 tests, but scalar LTM models always lower — the unsupported path is arrayed-only. That assertion (wasm-ltm.AC3.2) is therefore realized in Phase 4, not here. The design's `direct-backend.ts:463`/`:670` line refs are close; the actual blocks are `:460-465` and `:668-676`.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Thread `enableLtm` into the wasm compile; drop the rejection; retain model ptr + raw layout

**Verifies:** wasm-ltm.AC1.3 (rejection gone, `simulate` resolves)

**Files:**
- Modify: `src/engine/src/internal/wasmgen.ts:92-138` (`simlin_model_compile_to_wasm` wrapper)
- Modify: `src/engine/src/direct-backend.ts:152-170` (`HandleEntry`/extras), `:460-496` (`simNewWasm`), `:508-512` (`releaseWasmSimState`)

**Implementation:**
1. `wasmgen.ts`: add a parameter `enableLtm: boolean` to the wrapper; pass `enableLtm ? 1 : 0` for `ltm_enabled` and `0` for `ltm_discovery_mode` (discovery is not TS-exposed) to the 8-arg WASM export, immediately after `model`. **Also return the raw `layout` bytes** (the wrapper already has `layout: Uint8Array` before any parse) so the caller can retain them.
2. `direct-backend.ts` `HandleEntry`: add `wasmModelPtr?: number` and `wasmLayoutBytes?: Uint8Array`.
3. `simNewWasm`: **delete** the `if (enableLtm) throw` block (`:460-465`). Call `simlin_model_compile_to_wasm(modelEntry.ptr, enableLtm)`. Retain the model so `wasmModelPtr` stays valid for `getLinks`: call `simlin_model_ref(modelEntry.ptr)` (`model.rs:217`). Store `wasmModelPtr: modelEntry.ptr` and `wasmLayoutBytes: layout` on the handle entry alongside the existing `wasmLayout` (parsed).
4. `releaseWasmSimState`: call `simlin_model_unref(entry.wasmModelPtr)` (`model.rs:226`) to release the retained model, then clear `wasmModelPtr` / `wasmLayoutBytes`.

**Testing:** Covered by Task 4's `simulate({engine:'wasm', enableLtm:true})`-resolves test. No isolated test here (wiring/retention).

**Verification:**
Run: `cd src/engine && npx jest tests/wasm-model.test.ts`
Expected: existing wasm-model tests still pass (LTM-off path unchanged); build is clean (`pnpm --filter @simlin/engine build`).

**Commit:** `engine: enable LTM on the wasm compile and retain model/layout for analysis`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement the `getLinks` wasm read/analyze path

**Verifies:** wasm-ltm.AC1.3 (getLinks returns scored links), wasm-ltm.AC2.3

**Files:**
- Modify: `src/engine/src/internal/analysis.ts` (add a binding for `simlin_analyze_links_from_wasm_results`)
- Modify: `src/engine/src/direct-backend.ts:668-676` (`simGetLinks`)

**Implementation:**
1. `internal/analysis.ts`: declare a typed binding `simlin_analyze_links_from_wasm_results(model: Ptr, slabPtr: Ptr, slabLen: number, layoutPtr: Ptr, layoutLen: number, outErr: Ptr) => number` from `getExports()` (mirror the `simlin_analyze_get_links` binding at `:82-104`). Expose a small imperative-shell helper `analyzeLinksFromWasmResults(modelPtr, slab: Uint8Array, layoutBytes: Uint8Array): Link[]` that: `copyToWasm(slab)` + `copyToWasm(layoutBytes)`, `allocOutPtr()` for the error, calls the FFI, checks the error pointer (throw `SimlinError` if set, as `simlin_model_compile_to_wasm` does at `wasmgen.ts:112-118`), then `convertLinks(linksPtr)`; `finally` frees slabPtr/layoutPtr/outErr.
2. `direct-backend.ts` `simGetLinks`: replace the wasm throw branch with:
   - Read the blob's result slab: `const { resultsOffset, nSlots, nChunks } = entry.wasmLayout!;` then copy `new Uint8Array(entry.wasmExports!.memory.buffer, resultsOffset, nSlots * nChunks * 8).slice()` (the `.slice()` detaches a copy from the live, possibly-growable buffer).
   - Call `analyzeLinksFromWasmResults(entry.wasmModelPtr!, slabBytes, entry.wasmLayoutBytes!)` and return it.
   - Leave the VM branch (`simlin_analyze_get_links(entry.ptr)` + `convertLinks`) unchanged.

**Testing:** Covered by Task 4 (VM-vs-wasm getLinks parity). The slab byte-compatibility (f64 LE in both wasm modules) is implicitly verified by score equality.

**Verification:**
Run: `cd src/engine && npx jest tests/wasm-ltm.test.ts -t 'getLinks'` (after Task 4 adds the test file)
Expected: wasm `getLinks` returns links with `score` arrays.

**Commit:** `engine: read wasm LTM slab and analyze links via from-series FFI`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Populate `Run.links` for wasm LTM runs

**Verifies:** wasm-ltm.AC2.3 (Run.links populated for wasm)

**Files:**
- Modify: `src/engine/src/sim.ts:234`

**Implementation:**
1. Drop the `&& this._engine !== 'wasm'` clause: `const wantLinks = this.ltmEnabled;`. With Tasks 1-2 done, `this.getLinks()` now works for wasm, so `Run.links` is populated identically to VM. (`_engine` may remain for diagnostics; do not remove unless it is unused everywhere.)

**Testing:** Covered by Task 4 (`Run.links` populated for a wasm LTM run).

**Verification:**
Run: `cd src/engine && npx jest tests/wasm-ltm.test.ts -t 'Run.links'`
Expected: a wasm LTM `run()` yields a non-empty `run.links`.

**Commit:** `engine: populate Run.links for wasm LTM runs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: jest parity tests (node `DirectBackend`)

**Verifies:** wasm-ltm.AC1.3, wasm-ltm.AC2.3

**Files:**
- Add: `src/engine/tests/wasm-ltm.test.ts`
- Add (if no shared LTM fixture loader exists): a small helper to load `test/logistic_growth_ltm/logistic_growth.stmx` via `fs.readFileSync` + the existing XMILE-open path (e.g. `api.test.ts:43`'s `Project.open`/`projectOpenXmile`). Resolve the path as `path.join(__dirname, '../../../test/logistic_growth_ltm/logistic_growth.stmx')`.

**Implementation / Testing (each is a `test(...)`):**
1. `'simulate({engine:wasm, enableLtm}) resolves to a Sim'` (AC1.3): `await model.simulate({}, { engine: 'wasm', enableLtm: true })` resolves (no throw); the result is a `Sim`.
2. `'wasm getLinks returns scored links'` (AC1.3): after `runToEnd`, `sim.getLinks()` returns a non-empty array; at least one `Link` has a defined `score` array of length = step count.
3. `'wasm getLinks scores match VM'` (AC2.3): run the same model on `engine:'vm'` and `engine:'wasm'` (both `enableLtm:true`), match links by `(from,to)`, and assert each `score[]` equal within a tolerance (`1e-6`; reuse the `expectSeriesClose`-style helper from `wasm-backend.test.ts`). Assert identical link sets and polarities.
4. `'Run.links populated for wasm LTM run'` (AC2.3): `const run = await model.run({}, { analyzeLtm: true, engine: 'wasm' });` → `run.links.length > 0` and matches the VM run's links.

**Verification:**
Run: `cd src/engine && npx jest tests/wasm-ltm.test.ts`
Then the full gate: `pnpm --filter @simlin/engine test`
Expected: all four tests pass; suite green.

**Commit:** `engine: add node DirectBackend LTM-on-wasm parity tests`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

---

## Phase 3 Done When

- The up-front `enableLtm` rejection in `simNewWasm` and the `getLinks` wasm throw are gone; `Model.simulate({ engine: 'wasm', enableLtm: true })` resolves and `getLinks()` returns scored links (**wasm-ltm.AC1.3**).
- wasm `getLinks` scores match VM `getLinks` within `1e-6`, and `Run.links` is populated for a wasm LTM run (**wasm-ltm.AC2.3**).
- No analysis is reimplemented in TypeScript — `getLinks` marshals the slab+layout and reuses `convertLinks` over the Phase 2 FFI result (carries **wasm-ltm.AC5.1**).
- `pnpm --filter @simlin/engine test`, `pnpm lint`, and `pnpm tsc` (type-check) are green.
