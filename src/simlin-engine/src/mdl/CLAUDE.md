# Vensim MDL Parser

Pure Rust implementation of a Vensim MDL file parser, replacing the C++ `src/xmutil` dependency.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).
For design history and detailed implementation notes, see [doc/design/mdl-parser.md](/doc/design/mdl-parser.md).

## Current Status

- **Phases 1-8, 10**: Complete (lexer, parser, AST, builtins, conversion, views, macros, settings)
- **Phase 9 (Post-processing)**: Partial -- group parsing complete, name normalization not implemented
- **C-LEARN equivalence**: 26 diffs remaining (down from 233). See [doc/design/mdl-parser.md](/doc/design/mdl-parser.md) for analysis.

## Module Map

### Parsing Pipeline
- `lexer.rs` -- Hand-written `RawLexer` for MDL tokens (context-free)
- `normalizer.rs` -- `TokenNormalizer` for context-sensitive transformations (function detection, section tracking)
- `parser.rs` -- Recursive descent parser producing AST
- `ast.rs` -- AST types: `Expr`, `Equation`, `Lhs`, `LookupTable`, `SubscriptDef`
- `reader.rs` -- `EquationReader`: drives parser, captures comments, handles macros
- `builtins.rs` -- Vensim built-in function recognition via `to_lower_space()` canonicalization
- `settings.rs` -- Post-equation settings section parser (integration type, unit equivalences)

### Conversion (`convert/`)
- `mod.rs` -- Main conversion orchestration, group building
- `variables.rs` -- Variable type detection (stock/flow/aux) and building
- `stocks.rs` -- Stock/flow linking via is_all_plus_minus algorithm
- `dimensions.rs` -- Dimension/subscript building with range expansion
- `types.rs` -- Internal types (`SymbolInfo`, etc.)
- `helpers.rs` -- Utility functions (units, expressions)

### Views (`view/`)
- `mod.rs` -- Main view parsing: `parse_views()` entry point
- `elements.rs` -- Element line parsing (types 1, 10, 11, 12)
- `types.rs` -- View types: `VensimView`, `VensimElement`, `ViewError`
- `convert.rs` -- `VensimView` -> `datamodel::View` conversion
- `processing.rs` -- Coordinate transforms, angle calculation, flow points

### Expression Formatting
- `xmile_compat.rs` -- XMILE-compatible expression formatter (function renames, argument reordering, name formatting, per-element subscript substitution)

## Known Gaps

- Macro output in datamodel format (parsing complete, conversion not implemented)
- Name post-processing (`SpaceToUnderBar`, `MakeViewNamesUnique`)
- Variable filtering (Time, ARRAY types in views)
- 26 C-LEARN equivalence diffs (see design doc for root cause analysis)

## Commands

```bash
cargo test -p simlin-engine mdl::                    # MDL-specific tests
cargo test -p simlin-engine --features xmutil test_mdl_equivalence -- --nocapture  # Equivalence tests
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture  # C-LEARN test
```
