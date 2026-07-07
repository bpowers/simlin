# Vensim MDL Parser

Pure Rust implementation of a Vensim MDL file parser, replacing the C++ `src/xmutil` dependency.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).
For design history and detailed implementation notes, see [docs/design/mdl-parser.md](/docs/design/mdl-parser.md).

## Current Status

- **Phases 1-8, 10**: Complete (lexer, parser, AST, builtins, conversion, views, macros, settings)
- **Phase 9 (Post-processing)**: Partial -- group parsing complete, name normalization not implemented
- **C-LEARN equivalence**: 26 diffs remaining (down from 233). See [docs/design/mdl-parser.md](/docs/design/mdl-parser.md) for analysis.

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
- `mod.rs` -- Main conversion orchestration, group building, `DataProvider` threading
- `variables.rs` -- Variable type detection (stock/flow/aux) and building; EXCEPT default_equation handling, GET DIRECT resolution
- `stocks.rs` -- Stock/flow linking via is_all_plus_minus algorithm
- `dimensions.rs` -- Dimension/subscript building with range expansion and `DimensionMapping` construction
- `external_data.rs` -- GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT resolution via `DataProvider` trait
- `macros.rs` -- Converts each `:MACRO:` block into a macro-marked `datamodel::Model` (via `datamodel::Model::new_macro`); the conversion pipeline is reused per macro body
- `multi_output.rs` -- Materializes `:`-list multi-output macro invocations at import (using the Pass-6 macro-marked models' `MacroSpec`s)
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

### Writing (`writer.rs`)

`project_to_mdl` / `project_to_mdl_with_warnings` (re-exported from the crate root as `to_mdl` / `to_mdl_with_warnings`) serialize a `datamodel::Project` back to Vensim MDL.

- **Context-aware expression printer.** `MdlPrintVisitor` walks each `Expr0` threading a `WriterContext` (built per model by `WriterContext::from_model`) that carries the model's variable-ident set, each arrayed variable's declared dimensions, the standalone-extrapolating-lookup idents, and each named dimension's element list. The context resolves ambiguities a context-free printer cannot:
  - **wildcard subscript recovery** (#847): `SUM(a[*])` where `a` is declared `a[DimA]` becomes `SUM(a[DimA!])` (Vensim's bang form); a star-range `*:Dim` collapses to the same `Dim!`. A position with no declared dimension falls back to a bare `*`.
  - **`pi` literal** (#850): Vensim has no `PI` builtin and rejects `PI()`, so a `pi` reference is emitted as the 17-significant-digit literal `3.141592653589793`.
  - **keyword-shadow resolution** (#853/#850): a zero-arg `App` whose name (`pi`/`time`/`time_step`/...) is a declared variable is emitted as the identifier, not the builtin, so a user aux named "Time Step" or "PI" does not silently rebind.
  - **INITIAL arity** (#852): `init` dispatches to `INITIAL` (1-arg) vs `ACTIVE INITIAL` (2-arg) at the call site; a 2-arg `normal` synthesizes the required 5-arg `RANDOM NORMAL` form.
  - **quoted-ident preservation** (#846): an already-quoted ident is passed through verbatim (interior not re-spaced, escaping not re-grown across passes).
  - Plus the pre-existing Vensim-pattern recognizers (PULSE, PULSE TRAIN, QUANTUM, LOG 2-arg, SAMPLE IF TRUE, ALLOCATE BY PRIORITY, TIME BASE, RANDOM POISSON, RANDOM 0 1) and native lookup-call syntax `table ( input )`.

- **Free-text sanitization choke point** (`sanitize_free_text`, #849). Every modeler-authored free-text sink (variable units/documentation, group name/doc, `22:` unit-equivalence tokens) routes through one function, so the anti-corruption policy lives in one place. Without it, a raw structural character lets free text terminate a construct early and the remainder re-parses as phantom variables (the confirmed `|`-in-doc repro). It maps `|` -> `/` (a NON-whitespace substitute so a field-final `|` cannot become a trim-sensitive trailing space that breaks comment-field idempotence), the four section-terminator runs -> space, and per-field separators (`~` in units, `,` in a `22:` token) -> space. Line endings normalize losslessly to LF (so a field carrying `\r` is a fixpoint, not an accumulating carriage return); the single LF->CRLF pass runs once at the end of `write_project`. Caveat: the **units** field is a typed unit *expression*, not prose -- sanitization keeps it structurally safe (never spawns phantom vars) but cannot make arbitrary text a valid unit expression, so units holding a bare number or a `|`-turned-`/` dangling operator hard-fail re-import (a loud error, not silent corruption).

- **Export warnings channel** (#856). `project_to_mdl_with_warnings` returns `(String, Vec<ExportWarning>)`; the plain `project_to_mdl` discards them. Warnings are a side channel that never changes the emitted text, so they do not affect the corpus round-trip ratchets. `Err` is reserved for structural impossibilities (>1 non-macro model, an ordinary non-macro `Module` variable, an unreconstructable macro-invocation cluster). Warnings flag degraded-but-representable constructs: a dropped `compat.non_negative`, a Discrete GF (emitted continuous), an Extrapolate GF on an inline `WITH LOOKUP` or on an unreferenced standalone lookup (no `TABXL` call site to mark it), a one-to-many dimension element mapping, a multi-word group name / group doc the reader drops, and an EXCEPT default that could not be reconstructed. The CLI (`simlin-cli`) prints them to STDERR so they never corrupt MDL written to STDOUT.

- **GF-kind (TABXL) preservation** (#854). MDL has no definition-level extrapolate flag: a lookup table is marked extrapolating only by a `TABXL` call site on re-import. So a plain `LOOKUP(table, x)` call to a *standalone* extrapolating lookup is emitted as `TABXL(table, x)` (the context tracks which idents are extrapolating lookups), which re-imports as Extrapolate rather than being clamped to Continuous. A *referenced* standalone table round-trips silently. A standalone table with NO call site (an unreferenced/dead table) has no `TABXL` to mark it, so the context scans call sites (`referenced_extrapolate_lookups`) and warns for that case rather than silently clamping; an *inline* `WITH LOOKUP` Extrapolate GF (no nameable table) warns too.

- **EXCEPT default reconstruction** (#858). An `Equation::Arrayed` with `has_except_default` drops its `default_equation` on the MDL surface, which silently turned uncovered elements into 0. The writer reconstructs it: from the dimension's declared element set (via the context) it materializes the default value explicitly for every element the explicit list omits. It warns and keeps explicit-only when membership is unavailable (dimensions not registered) or the default references its own dimensions (per-element substitution is not performed).

### Writer tests

`writer_tests.rs` (unit) and `writer_lossiness_tests.rs` (semantic-loss + warnings) are the curated cases; `writer_proptest.rs` adds property-based coverage (free-text never corrupts the variable set, equation write is a re-parse fixpoint, whole-model write is idempotent). All three are mounted `#[cfg(test)] #[path]` from `writer.rs` to stay under the per-file line cap. Integration round-trips live in [tests/integration/mdl_roundtrip.rs](/src/simlin-engine/tests/integration/mdl_roundtrip.rs); the corpus round-trip ratchet gates guard against regressions.

## Known Gaps

- Name post-processing (`SpaceToUnderBar`, `MakeViewNamesUnique`)
- Variable filtering (Time, ARRAY types in views)
- 26 C-LEARN equivalence diffs (see design doc for root cause analysis)
- Element-level dependency resolution (models like `ref`, `interleaved` have per-element equations that create false circular dependencies at the whole-variable level)

### Writer round-trip idempotence

Wildcard subscripts are no longer a gap (the context recovers `Dim!`). The equations section of a view-free model reaches a `write(parse(...))` fixpoint (pinned by `writer_proptest.rs`). The remaining non-fixpoint classes are tracked and deliberately excluded from the property tests (which compare only the equations section of view-free models):

- **Sketch instability**: a re-emitted sketch section is not yet byte-stable across a round trip.
- **Arrayed element order**: an arrayed variable's element list may reorder on the first re-import.
- **LHS / view-name escape doubling**: quoted names on the LHS / in the sketch can accrete escaping across passes.
- **Control-variable value substitution**: the `.Control` group's sim-specs / control-variable values are re-substituted on re-import (why the proptest fixpoint compares only the pre-`.Control` equations section).
- **Importer non-determinism** (tracked as #859): a separately-tracked source of first-import variance.

## Commands

```bash
cargo test -p simlin-engine mdl::                    # MDL-specific tests
cargo test -p simlin-engine --features xmutil test_mdl_equivalence -- --nocapture  # Equivalence tests
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture  # C-LEARN test
```
