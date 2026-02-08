# simlin-engine

Core simulation engine for system dynamics models. Compiles, type-checks, unit-checks, and simulates SD models. See the root `CLAUDE.md` for full development guidelines; this file maps where functionality lives.

**Maintenance note**: Keep this file up to date when adding, removing, or reorganizing modules.

## Compilation pipeline

Equation text flows through these stages in order:

1. **`src/lexer/`** - Tokenizer for equation syntax
2. **`src/parser/`** - Recursive descent parser producing `Expr0` AST
3. **`src/ast/`** - AST type system with progressive lowering: `Expr0` (parsed) -> `Expr1` (modules expanded) -> `Expr2` (dimensions resolved) -> `Expr3` (subscripts expanded). `array_view.rs` tracks array dimensions and sparsity.
4. **`src/builtins.rs`** - Builtin function definitions (e.g. `MIN`, `PULSE`, `LOOKUP`). `builtins_visitor.rs` handles implicit module instantiation from `MODULE()` calls.
5. **`src/compiler/`** - Multi-pass compilation to bytecode:
   - `mod.rs` - Orchestration
   - `context.rs` - Symbol tables and variable metadata
   - `expr.rs` - Expression compilation
   - `codegen.rs` - Bytecode emission
   - `dimensions.rs` - Dimension checking/inference
   - `subscript.rs` - Array subscript expansion and iteration
   - `pretty.rs` - Debug pretty-printing
6. **`src/bytecode.rs`** - Instruction set definition, opcodes, type aliases (`LiteralId`, `ModuleId`, `DimId`, `TempId`, etc.)
7. **`src/vm.rs`** - Stack-based bytecode VM. Hot loop uses proven-safe unchecked array access validated at compile time by `ByteCodeBuilder`.
8. **`src/interpreter.rs`** - AST-walking interpreter serving as a reference "spec" to verify VM correctness.

## Data model and project structure

- **`src/common.rs`** - Error types (`ErrorCode` with 100+ variants), `Result`, identifier types (`RawIdent`, `Ident<Canonical>`, dimension/element name types), canonicalization
- **`src/datamodel.rs`** - Core structures: `Project`, `Model`, `Variable`, `Equation`, `Dimension`, `UnitMap`
- **`src/variable.rs`** - Variable variants (`Stock`, `Flow`, `Aux`, `Module`), `ModuleInput`, `Table` (graphical functions)
- **`src/dimensions.rs`** - Dimension context and dimension matching for arrays
- **`src/model.rs`** - Model compilation stages (`ModelStage0` -> `ModelStage1` -> `ModuleStage2`), dependency resolution, topological sort
- **`src/project.rs`** - `Project` struct aggregating models. `with_ltm()` for loop analysis.
- **`src/results.rs`** - `Results` (variable offsets + timeseries data), `Specs` (time/integration config)
- **`src/patch.rs`** - `ModelPatch`/`ProjectPatch` for representing and applying model changes

## Format import/export

- **`src/compat.rs`** - Top-level format entry points: `open_vensim()`, `open_xmile()`, `to_xmile()`, `.dat`/CSV loading
- **`src/xmile/`** - XMILE (XML interchange format) parsing and generation. Submodules: `model.rs`, `variables.rs`, `dimensions.rs`, `views.rs`
- **`src/mdl/`** - Native Rust Vensim MDL parser (replaces C++ xmutil):
  - `lexer.rs` -> `normalizer.rs` -> `parser.rs` -> `reader.rs` (pipeline)
  - `ast.rs`, `builtins.rs` (Vensim function recognition)
  - `convert/` - Multi-pass AST to datamodel conversion (`variables.rs`, `stocks.rs`, `dimensions.rs`, `types.rs`, `helpers.rs`)
  - `view/` - Sketch/diagram parsing (`elements.rs`, `types.rs`, `convert.rs`, `processing.rs`)
  - `xmile_compat.rs` - Expression formatting for XMILE output
  - `settings.rs` - Integration settings parser
- **`src/json.rs`** - JSON serialization matching Go `sd` package schema
- **`src/json_sdai.rs`** - JSON schema for AI metadata augmentation
- **`src/serde.rs`** - Generic serde utilities

## Unit analysis

- **`src/units.rs`** - Unit parsing and `UnitMap` representation
- **`src/units_check.rs`** - Dimensional consistency checking across equations
- **`src/units_infer.rs`** - Unit inference for variables without explicit declarations

## Special features

- **`src/ltm.rs`** - Loops That Matter: feedback loop detection and dominance analysis
- **`src/ltm_augment.rs`** - Synthetic variable generation for loop instrumentation
- **`src/diagram/`** - Diagram/sketch rendering: `elements.rs`, `connector.rs`, `flow.rs`, `render.rs`, `common.rs`, `constants.rs`, `label.rs`, `arrowhead.rs`

## Generated files (do not edit by hand)

- **`src/project_io.gen.rs`** - Protobuf bindings from `project_io.proto`
- **`src/stdlib.gen.rs`** - Embedded standard library models from `stdlib/*.stmx`

## Tests

- **`src/test_common.rs`**, **`src/testutils.rs`** - Helpers and fixtures (e.g. `x_model`, `x_stock`, `x_flow`)
- **`src/array_tests.rs`** - Array-specific tests (feature-gated)
- **`src/json_proptest.rs`**, **`src/json_sdai_proptest.rs`** - Property-based tests
- **`src/unit_checking_test.rs`** - Unit checking regression tests
- **`src/test_sir_xmile.rs`** - SIR epidemiology model integration tests
- **`src/test_open_vensim.rs`** - Vensim compatibility tests (requires `xmutil` feature)
- **`tests/simulate.rs`** - End-to-end simulation integration tests
- **`tests/simulate_ltm.rs`** - LTM feature tests
- **`tests/json_roundtrip.rs`** - JSON serialization roundtrip
- **`tests/roundtrip.rs`** - XMILE/MDL roundtrip tests
- **`tests/vm_alloc.rs`** - VM memory allocation tests
- **`tests/mdl_equivalence.rs`** - MDL parser equivalence vs C++ xmutil
- **`benches/simulation.rs`**, **`benches/array_ops.rs`** - Performance benchmarks
