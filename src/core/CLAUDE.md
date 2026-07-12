# @simlin/core

Shared data models and common utilities used by both frontend and backend TypeScript packages.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `datamodel.ts` -- Protobuf-based core data structures: `Project`, `Model`, `Variable`, `Equation`, `Dimension`, `UnitMap`
- `canonicalize.ts` -- Variable name canonicalization (spaces, underscores, case normalization)
- `common.ts` -- Common types and utilities
- `collections.ts` -- Collection utility functions
- `errors.ts` -- Error type definitions
- `index.ts` -- Public exports

## Contracts

- Variables carry BOTH `ident` (canonical; the `Model.variables` Map key, and it must match engine-canonical idents in sim results, error details, and view-element lookups) and an optional `rawName` (the display spelling from the wire `name`, e.g. "Total Students"). `*FromJson` populates `rawName`; `*ToJson` emits `rawName ?? ident` as `name`. This is what keeps the editor's full-upsert paths from downgrading an imported model's display names one edit at a time (issue #906): the engine stores the payload's `name` verbatim and does all matching canonically (issue #890), so the payload's spelling is authoritative for presentation.
- `Stock`, `Flow`, and `Aux` interfaces all carry `canBeModuleInput` and `isPublic` boolean fields. These are read from `compat` in JSON deserialization and written back to `compat` when true. The fields control which variables appear as module input/output ports in the diagram editor.
- The full engine `compat` field set round-trips through `datamodel.ts`: `activeInitial`, `nonNegative`, `canBeModuleInput`, `isPublic`, `dataSource`, plus the conveyor/queue markers -- `conveyor`/`queue` on `Stock`, `leakage`/`spreadflow`/`overflow` on `Flow`. This is load-bearing: the editor re-serializes a variable as a FULL upsert on any edit (`Editor.tsx` via `stockToJson`/`flowToJson`/`auxToJson`), so any compat field the conversion drops is silently stripped from the model the moment an unrelated field is edited. `Aux` and `Module` deliberately omit the conveyor markers (the engine's uniform Compat accepts them there, but no importer or editor produces them on those kinds).
- `Model.macroSpec?: MacroSpec` (`parameters`/`primaryOutput`/`additionalOutputs`) is set exactly when the model is a callable macro template (imported `:MACRO:` / XMILE `<macro>`). `macroSpecFromJson`/`macroSpecToJson` round-trip it; `additionalOutputs` is omitted from JSON when empty. Consumers gate macro-marked models out of module-reference UI (`@simlin/diagram`'s `isMacroModel`).

## Tests

- `tests/datamodel.test.ts` -- Data model tests (includes round-trip serialization for `canBeModuleInput`/`isPublic` and the conveyor/queue compat markers)
- `tests/datamodel-roundtrip-e2e.test.ts` -- Drives the REAL WASM engine serializer to pin the editor's full-upsert fidelity contract (skips when the engine build is absent)
