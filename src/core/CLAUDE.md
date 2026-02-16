# @simlin/core

Shared data models and common utilities used by both frontend and backend TypeScript packages.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).

## Key Files

- `datamodel.ts` -- Protobuf-based core data structures: `Project`, `Model`, `Variable`, `Equation`, `Dimension`, `UnitMap`
- `canonicalize.ts` -- Variable name canonicalization (spaces, underscores, case normalization)
- `common.ts` -- Common types and utilities
- `collections.ts` -- Collection utility functions
- `errors.ts` -- Error type definitions
- `index.ts` -- Public exports

## Tests

- `tests/datamodel.test.ts` -- Data model tests
