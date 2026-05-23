# @simlin/engine WebAssembly Simulation Backend — Phase 3: browser / worker

**Goal:** Make `engine: 'wasm'` work through the Web Worker path with no structural protocol change — exactly one optional, additive `engine` field on the existing `simNew` message, threaded through the three worker sites, plus a parity test proving a `WorkerBackend`-driven wasm sim matches a node `DirectBackend`.

**Architecture:** In the browser, `WorkerBackend` (main thread) sends a `simNew` request over a `postMessage` discriminated-union protocol to `WorkerServer` (in a Web Worker), which delegates to an internal `DirectBackend`. Because `WorkerServer` wraps a `DirectBackend`, the entire Phase-2 wasm demux (and the blob's `WebAssembly.Instance`) already lives inside the Worker for free. The only delta is: (1) add an optional `engine?: 'vm' | 'wasm'` field to the `simNew` request variant, (2) forward `request.engine` in the server's `simNew` case, (3) add the `engine?` arg to `WorkerBackend.simNew` and include it in the message. No new message types, no new response shapes; `getSeries` already transfers its `Float64Array` zero-copy and is engine-agnostic.

**Tech Stack:** TypeScript; the existing postMessage discriminated-union worker protocol; jest with the in-memory loopback harness (`createTestPair`) that wires a real `WorkerBackend` to a real `WorkerServer` via fake transport closures (no real `Worker`/jsdom; `testEnvironment: node`).

**Scope:** Phase 3 of 4. Depends on Phase 2 (the `engine` param on `EngineBackend.simNew`/`DirectBackend.simNew`, the wasm demux, and `internal/wasmgen.ts`). The `worker-backend.ts` edit only typechecks once Phase 2 has widened `EngineBackend.simNew`.

**Codebase verified:** 2026-05-22

---

## Acceptance Criteria Coverage

### engine-wasm-sim.AC8: Browser/worker parity + minimal protocol
- **engine-wasm-sim.AC8.1 Success:** through `WorkerBackend`, `engine:'wasm'` produces series matching node `DirectBackend` (and the VM).
- **engine-wasm-sim.AC8.2 Success:** the protocol delta is exactly one optional `engine` field on the existing `simNew` message — no new message types/response shapes; `getSeries` still transfers zero-copy.

---

## Background: what exists today (verified)

All paths absolute from `/home/bpowers/src/simlin`. Cited line numbers match current committed code.

**The `simNew` request variant** (`src/engine/src/worker-protocol.ts:83`), inside the `WorkerRequest` union (opens `:34`, closes `:94`):
```ts
| { type: 'simNew'; requestId: number; modelHandle: WorkerModelHandle; enableLtm: boolean }
```
Optional-field precedent in the same union: `:36` `{ type: 'init'; requestId: number; wasmSource?: ArrayBuffer; wasmUrl?: string }` and `:52` `{ ...; includeStdlib?: boolean }`. `WorkerModelHandle`/`WorkerSimHandle` are `number` aliases (`:17-19`). `VALID_REQUEST_TYPES` (`:189`) and `isValidRequest` (`:203-209`) only check the `type` discriminant + `requestId` presence — **no field-level validation**, so adding an optional field touches neither.

**The response union** (`worker-protocol.ts:98-100`) is fixed and generic:
```ts
export type WorkerResponse =
  | { type: 'success'; requestId: number; result: unknown; transfer?: ArrayBuffer[] }
  | { type: 'error'; requestId: number; error: SerializedError };
```
The handle comes back inside `result: unknown` — **no per-request response shape**, so adding a request field needs no response change (AC8.2). `SerializedError` (`:25-30`) carries `name`/`message`/`code?`/`details?`; `serializeError`/`deserializeError` (`:114-152`) round-trip them, so a thrown `SimlinError`/`Error` from the worker's `DirectBackend` (unsupported model, `enableLtm` rejection) propagates to the main thread intact.

**The `WorkerServer` simNew case** (`src/engine/src/worker-server.ts:367-377`):
```ts
case 'simNew': {
  const modelHandle = this.getModelHandle(request.modelHandle);
  const backendSimHandle = this.backend.simNew(modelHandle, request.enableLtm);   // :369
  const parentProject = this.modelToProject.get(request.modelHandle);
  if (parentProject === undefined) {
    throw new Error(`Model handle ${request.modelHandle} not associated with a project`);
  }
  const workerSimHandle = this.registerSimHandle(backendSimHandle, parentProject);
  this.sendSuccess(requestId, workerSimHandle);
  return;
}
```
`this.backend` is a `DirectBackend` (`:31` field, `:44-47` constructor `this.backend = new DirectBackend()`). TypeScript narrows `request` to the `simNew` variant inside the case, so `request.engine` is available once the protocol field is added.

**Zero-copy `getSeries`** (`worker-server.ts:422-427`, `sendFloat64WithTransfer` `:517-523`, `detachable` `:530-535`): the case calls `this.backend.simGetSeries(handle, name)` (engine-agnostic) and ships the `Float64Array` with transfer. `detachable` `.slice()`s only if the view is partial (`byteOffset !== 0` or `buffer.byteLength !== byteLength`); the Phase-2 wasm `getSeries` returns a freshly-allocated `Float64Array(nChunks)` (byteOffset 0, owns its buffer), so it transfers as-is. Existing buffer-independence coverage: `worker-server.test.ts:771-817`.

**The `WorkerBackend.simNew` builder** (`src/engine/src/worker-backend.ts:512-519`):
```ts
simNew(modelHandle: ModelHandle, enableLtm: boolean): Promise<SimHandle> {
  return this.sendRequest<SimHandle>((requestId) => ({
    type: 'simNew',
    requestId,
    modelHandle,
    enableLtm,
  }));
}
```
`sendRequest<T>` (`:79-114`) returns `Promise<T>`, enqueues on a FIFO `_queue`, assigns a monotonic `requestId`, and resolves via `handleResponse` (`:61-73`) on `success` (or rejects with `deserializeError` on `error`). The message object literal must structurally satisfy the (Phase-3-widened) `simNew` request variant — adding `engine` to the protocol union makes the literal typecheck.

**The worker test harness** (`src/engine/tests/worker-backend.test.ts`): `createTestPair()` (`:35-61`) builds a real `WorkerBackend` and a real `WorkerServer` and wires them with fake transport closures (`setTimeout(..., 0)` to mimic async delivery), capturing every `transfer` list in a `transfers: (Transferable[] | undefined)[]` array (`TestPair`, `:24-29`). WASM is loaded from disk: `loadWasmSource()` → `core/libsimlin.wasm` (`:13,:21`); model fixture `loadTestXmile()` → `pysimlin/tests/fixtures/teacup.stmx` (`:15-18`). Existing "sim operations" tests (`:359-398`) show the pattern: `await backend.init(loadWasmSource()); projHandle = await backend.projectOpenXmile(data); modelHandle = await backend.projectGetModel(projHandle, null); const simHandle = await backend.simNew(modelHandle, false); await backend.simRunToEnd(simHandle); const series = await backend.simGetSeries(simHandle, 'teacup_temperature');`. `DirectBackend` is directly constructible in a node test (it backs `WorkerServer` and is used directly in `worker-server.test.ts`). Production wiring (`backend-factory.browser.ts:22-53`, `engine-worker.ts`) ships the whole structured-cloned message via `worker.postMessage`, so an added optional field flows through with no factory/worker-entry change.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
Subcomponent A: the additive protocol field + a worker parity test.

<!-- START_TASK_1 -->
### Task 1: Thread an optional `engine` field through the three worker sites

**Verifies:** engine-wasm-sim.AC8.2 (the protocol delta is exactly one optional `engine` field; no new message types or response shapes)

**Files:**
- Modify: `src/engine/src/worker-protocol.ts:83` (the `simNew` request variant)
- Modify: `src/engine/src/worker-server.ts:369` (forward `request.engine`)
- Modify: `src/engine/src/worker-backend.ts:512-519` (add the `engine?` arg + include it in the message)

**Implementation:**
1. **Protocol** (`worker-protocol.ts:83`): add the optional field, mirroring the `?:` precedent at `:36`/`:52`:
   ```ts
   | { type: 'simNew'; requestId: number; modelHandle: WorkerModelHandle; enableLtm: boolean; engine?: 'vm' | 'wasm' }
   ```
   Use the same `'vm' | 'wasm'` union as Phase 2's `SimEngine` (import the exported `SimEngine` type from `./backend` if it reads cleaner; an inline literal is acceptable and matches the surrounding style). Do **not** touch `VALID_REQUEST_TYPES` or `isValidRequest` (they are field-agnostic).
2. **Server** (`worker-server.ts:369`): forward the field — change the one line to:
   ```ts
   const backendSimHandle = this.backend.simNew(modelHandle, request.enableLtm, request.engine);
   ```
   Nothing else in the case changes (handle translation, project association, `registerSimHandle`, `sendSuccess` are engine-agnostic).
3. **Backend builder** (`worker-backend.ts:512-519`): widen the signature to match the Phase-2 `EngineBackend.simNew` and include `engine` in the message:
   ```ts
   simNew(modelHandle: ModelHandle, enableLtm: boolean, engine?: 'vm' | 'wasm'): Promise<SimHandle> {
     return this.sendRequest<SimHandle>((requestId) => ({
       type: 'simNew',
       requestId,
       modelHandle,
       enableLtm,
       engine,
     }));
   }
   ```

This is purely additive: `engine === undefined` (every existing caller, and the VM path) produces the same message shape as before (an absent optional field), so existing behavior is unchanged.

**Testing:**
This task is a typed, additive protocol change with no new behavior of its own (the wasm behavior is exercised in Task 2). Verify operationally:
- Existing worker tests still pass unchanged (the VM path is untouched).
- The project typechecks (the `worker-backend.ts` message literal satisfies the widened `simNew` variant; this also confirms Phase 2's interface widening is present, as required).

**Verification:**
Run: `pnpm -C src/engine exec tsc --noEmit 2>&1 | tail -10`
Expected: typechecks (no excess-property error on the `simNew` message literal; `WorkerBackend.simNew` matches `EngineBackend.simNew`).
Run: `pnpm -C src/engine exec jest tests/worker-backend.test.ts tests/worker-server.test.ts 2>&1 | tail -15`
Expected: all existing worker tests pass (additive change, VM path unchanged).

**Commit:** `engine: thread optional engine field through the worker simNew message`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Worker wasm-engine parity test (matches node DirectBackend; zero-copy preserved)

**Verifies:** engine-wasm-sim.AC8.1, engine-wasm-sim.AC8.2

**Files:**
- Test: `src/engine/tests/worker-wasm.test.ts` (new; reuse the `createTestPair` loopback pattern from `worker-backend.test.ts:35-61` and the WASM/fixture loaders at `:13-18`)

**Implementation:**
No production code. This test drives `engine: 'wasm'` end-to-end through the real postMessage protocol (real request/response serialization shapes, the FIFO queue, `handleResponse`/`deserializeError`) via the in-memory loopback, and compares against a node `DirectBackend`.

**Testing:**
- AC8.1: In one node test, build a `WorkerBackend` via `createTestPair()` and a separate `DirectBackend`; `await both.init(loadWasmSource())`; open the same model (teacup XMILE) in each; on the `WorkerBackend` create a wasm sim (`await backend.simNew(modelHandle, false, 'wasm')`), `simRunToEnd`, and `simGetSeries(name)`; on the `DirectBackend` run the same model via `engine: 'wasm'` and via `engine: 'vm'`. Assert the `WorkerBackend` wasm series equals the `DirectBackend` wasm series exactly, and equals the VM series within the engine's tolerance (wasm is not bit-identical to the VM's libm by design).
- AC8.2 (transfer + protocol shape): assert a wasm-sim `simGetSeries` round-trips a `Float64Array` and that the call adds exactly one transfer entry of length 1 to the cumulative `transfers` array — assert on the array-length delta around that single `simGetSeries` call (or that the last appended entry is a one-element `[ArrayBuffer]`), since `TestPair.transfers` (`worker-backend.test.ts:24-29`) accumulates across all calls (mirroring `worker-server.test.ts:771-817`), confirming the zero-copy path is unchanged for the wasm engine. Optionally assert the served request object for `simNew` carries `engine: 'wasm'` and no new message `type` was introduced.
- Error propagation through the worker (reinforces AC6/AC7 across the worker boundary): `await expect(backend.simNew(modelHandle, true, 'wasm')).rejects.toThrow(/LTM .* not supported|wasm/i)` (the `enableLtm` rejection serializes from the worker), and a wasm-unsupported model rejects rather than silently falling back.

**Verification:**
Run: `pnpm -C src/engine exec jest tests/worker-wasm.test.ts 2>&1 | tail -25`
Expected: the worker wasm series matches the `DirectBackend` (and the VM within tolerance); the transfer assertion passes; the error cases reject across the worker boundary.
Run: `pnpm -C src/engine test 2>&1 | tail -15`
Expected: the full engine suite is green.

**Commit:** `engine: parity-test the wasm engine through the Web Worker path`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 3 Done When

- A `WorkerBackend`-driven `engine: 'wasm'` sim produces series matching a node `DirectBackend` (and the VM within tolerance) through the real postMessage protocol (AC8.1).
- The protocol delta is exactly one optional, additive `engine` field on the existing `simNew` message — no new message types, no new response shapes, `VALID_REQUEST_TYPES`/`isValidRequest` untouched; `getSeries` still transfers its `Float64Array` zero-copy for the wasm engine (AC8.2).
- Worker-boundary error propagation works for the `enableLtm`-on-wasm rejection and unsupported models (no silent VM fallback).
- `tsc --noEmit` and `pnpm -C src/engine test` pass; existing worker tests are unchanged.
