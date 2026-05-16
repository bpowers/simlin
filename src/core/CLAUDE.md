# @simlin/core

Last verified: 2026-05-15

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
- `Model.macroSpec?: MacroSpec` (`parameters`/`primaryOutput`/`additionalOutputs`) is set exactly when the model is a callable macro template (imported `:MACRO:` / XMILE `<macro>`). `macroSpecFromJson`/`macroSpecToJson` round-trip it; `additionalOutputs` is omitted from JSON when empty. Consumers gate macro-marked models out of module-reference UI (`@simlin/diagram`'s `isMacroModel`).

## Tests

- `tests/datamodel.test.ts` -- Data model tests (includes round-trip serialization for `canBeModuleInput`/`isPublic`)
