# @simlin/engine

TypeScript API for interacting with the WASM-compiled simulation engine. Promise-based; in the browser, WASM runs in a Web Worker to avoid jank.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `src/project.ts` -- `Project` class: primary public API for loading and querying projects
- `src/model.ts` -- `Model` class: individual SD model interface
- `src/sim.ts` -- `Sim` class: simulation runner
- `src/backend.ts` -- Backend abstraction interface
- `src/worker-backend.ts` -- Web Worker backend (browser)
- `src/direct-backend.ts` -- Direct backend (Node.js / tests)
- `src/worker-server.ts` -- Worker coordination and lifecycle
- `src/types.ts` -- TypeScript types and interfaces
- `src/json-types.ts` -- JSON serialization types
- `src/errors.ts` -- Engine-specific error types
- `src/patch.ts` -- Model patching logic
- `src/worker-protocol.ts` -- Worker message protocol
- `src/backend-factory.ts` / `.browser.ts` / `.node.ts` -- Platform-specific backend factories
- `src/internal/` -- Internal modules (project, model, memory, error, import-export)
- `src/internal/wasmgen.ts` -- `simlin_model_compile_to_wasm` FFI wrapper + the pure `parseWasmLayout` / `readStridedSeries` decoders for the per-model wasm blob (re-exported via `@simlin/engine/internal`)
- `src/internal/canonicalize.ts` -- pure `canonicalizeIdent`, a faithful port of the Rust canonicalizer (used to resolve caller names to wasm-layout slots); not re-exported from the `internal` barrel

## Contracts

- `JsonProjectOperation` is a union type: `SetSimSpecsOp | AddModelOp`. The `AddModelOp` (`type: 'addModel'`) creates a new empty model in the project. Type guards `isSetSimSpecs` and `isAddModel` are provided.
- The engine processes `projectOps` before model-level `ops` in a patch, so `AddModel` can be combined with `upsertModule` in a single patch to atomically create a model and reference it.

### Simulation engine selection (vm vs wasm)

- `SimEngine = 'vm' | 'wasm'` (exported from `backend.ts`). `Model.simulate(overrides, { engine })` and `Model.run(overrides, { engine })` accept it; `'vm'` (the bytecode VM, via libsimlin) is the default. `'wasm'` runs the model as a self-contained per-model WebAssembly blob, intended for fast repeated re-runs (interactive scrubbing).
- The wasm path is currently exercised under Node (`DirectBackend`) and through the Web Worker (`WorkerBackend`). The VM remains the correctness oracle; the wasm twin is held to VM parity by tests.
- `EngineBackend.simNew(modelHandle, enableLtm, engine?)` takes the optional engine. `DirectBackend` demuxes every sim op on the entry's engine: a `'wasm'` handle has no native sim pointer (`ptr === 0`); it owns a `WebAssembly.Instance` plus decoded `WasmLayout`, drives the blob's exports directly (`run_to`/`reset`/`set_value`/`memory`), reads series strided from linear memory, and resolves caller names via `canonicalizeIdent`. `'vm'` (the default/absent case) calls libsimlin.
- Worker path: an optional `engine` field on the `simNew` worker message (`worker-protocol.ts` / `-server.ts` / `-backend.ts`) threads selection through; it is purely additive and defaults to vm when absent.
- Wasm restrictions (enforced authoritatively in the backend, covering the worker path): LTM + wasm is rejected at `simNew`; `simGetLinks` throws on a wasm sim; an unsupported model surfaces the compile error with **no VM fallback**. `Sim.getRun` only fetches link scores when LTM is enabled AND the engine is not wasm, so a wasm `Run` carries empty links.

## Tests

- `tests/api.test.ts` -- Public API tests
- `tests/integration.test.ts` -- Integration tests
- `tests/worker-backend.test.ts`, `tests/worker-server.test.ts`, `tests/direct-backend.test.ts` -- Backend tests
- `tests/race.test.ts` -- Concurrency tests
- `tests/cleanup.test.ts` -- Resource cleanup tests
- `tests/wasmgen.test.ts`, `tests/canonicalize.test.ts` -- Unit tests for the pure layout decoders and `canonicalizeIdent`
- `tests/wasm-backend.test.ts`, `tests/wasm-model.test.ts`, `tests/worker-wasm.test.ts` -- wasm-vs-VM parity through `DirectBackend`, the `Model`/`Sim` facade, and the Web Worker

## Benchmarks

`tests/backend-bench.ts` (runner) + `tests/bench-stats.ts` (pure median/warmup harness, always unit-tested) measure node VM-vs-wasm eval time via `Model.simulate({ engine })`. The runner is gated behind `RUN_BENCH` so it stays out of the default `pnpm test`. See [docs/dev/benchmarks.md](/docs/dev/benchmarks.md#node-vm-vs-wasm-eval-benchmark).
