# @simlin/core

Last verified: 2026-04-08

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

- `Stock`, `Flow`, and `Aux` interfaces all carry `canBeModuleInput` and `isPublic` boolean fields. These are read from `compat` in JSON deserialization and written back to `compat` when true. The fields control which variables appear as module input/output ports in the diagram editor.

## Tests

- `tests/datamodel.test.ts` -- Data model tests (includes round-trip serialization for `canBeModuleInput`/`isPublic`)
