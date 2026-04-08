# @simlin/engine

Last verified: 2026-04-08

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

## Contracts

- `JsonProjectOperation` is a union type: `SetSimSpecsOp | AddModelOp`. The `AddModelOp` (`type: 'addModel'`) creates a new empty model in the project. Type guards `isSetSimSpecs` and `isAddModel` are provided.
- The engine processes `projectOps` before model-level `ops` in a patch, so `AddModel` can be combined with `upsertModule` in a single patch to atomically create a model and reference it.

## Tests

- `tests/api.test.ts` -- Public API tests
- `tests/integration.test.ts` -- Integration tests
- `tests/worker-backend.test.ts`, `tests/worker-server.test.ts`, `tests/direct-backend.test.ts` -- Backend tests
- `tests/race.test.ts` -- Concurrency tests
- `tests/cleanup.test.ts` -- Resource cleanup tests
