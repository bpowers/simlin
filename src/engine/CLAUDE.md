# @simlin/engine

TypeScript API for interacting with the WASM-compiled simulation engine. Promise-based; in the browser, WASM runs in a Web Worker to avoid jank.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).

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

## Tests

- `tests/api.test.ts` -- Public API tests
- `tests/integration.test.ts` -- Integration tests
- `tests/worker-backend.test.ts`, `tests/worker-server.test.ts`, `tests/direct-backend.test.ts` -- Backend tests
- `tests/race.test.ts` -- Concurrency tests
- `tests/cleanup.test.ts` -- Resource cleanup tests
