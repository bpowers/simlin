# simlin-engine

Core simulation engine for system dynamics models. Compiles, type-checks, unit-checks, and simulates SD models. See the root `CLAUDE.md` for full development guidelines; this file maps where functionality lives.

**Maintenance note**: Keep this file up to date when adding, removing, or reorganizing modules.

## Compilation pipeline

Equation text flows through these stages in order:

1. **`src/lexer/`** - Tokenizer for equation syntax
2. **`src/parser/`** - Recursive descent parser producing `Expr0` AST
3. **`src/ast/`** - AST type system with progressive lowering: `Expr0` (parsed) -> `Expr1` (modules expanded) -> `Expr2` (dimensions resolved) -> `Expr3` (subscripts expanded). `array_view.rs` tracks array dimensions and sparsity. `Expr2Context` trait includes `has_mapping_to()` for cross-dimension mapping lookups during `find_matching_dimension`.
4. **`src/builtins.rs`** - Builtin function definitions (e.g. `MIN`, `PULSE`, `LOOKUP`, `QUANTUM`, `SSHAPE`, `VECTOR SELECT`, `VECTOR ELM MAP`, `VECTOR SORT ORDER`, `VECTOR RANK`, `ALLOCATE AVAILABLE`, `ALLOCATE BY PRIORITY`, `NPV`, `MODULO`, `PREVIOUS`, `INIT`). `is_stdlib_module_function()` is the authoritative predicate for deciding whether a function name expands to a stdlib module; shared by `equation_is_stdlib_call()` (pre-scan) and `contains_stdlib_call()` (walk-time). `builtins_visitor.rs` handles implicit module instantiation and PREVIOUS/INIT helper rewriting: unary `PREVIOUS(x)` desugars to `PREVIOUS(x, 0)`, direct scalar args compile to `LoadPrev`, and module-backed or expression args are first rewritten through synthesized scalar helper auxes. `INIT(x)` compiles to `LoadInitial`, using the same helper rewrite when needed. Tracks `module_idents` so `PREVIOUS(module_var)` never reads a multi-slot module directly.
5. **`src/compiler/`** - Multi-pass compilation to bytecode:
   - `mod.rs` - Orchestration; includes A2A hoisting logic that detects array-producing builtins (VectorElmMap, VectorSortOrder, Rank, AllocateAvailable, AllocateByPriority) during array expansion, hoists them into `AssignTemp` pre-computations, and emits per-element `TempArrayElement` reads
   - `context.rs` - Symbol tables and variable metadata; `lower_preserving_dimensions()` skips Pass 1 dimension resolution to keep full array views for array-producing builtins. Handles `@N` position syntax resolution: in scalar context (no active A2A dimension, not inside an array-reducing builtin), `DimPosition(@N)` resolves to a concrete element offset; inside array-reducing builtins (`preserve_wildcards_for_iteration`), dimension views are preserved for iteration. Two wildcard-preservation contexts: `with_preserved_wildcards()` for reducers (SUM, MEAN, etc.) where `ActiveDimRef` resolves to a concrete offset, and `with_vector_builtin_wildcards()` for array-producing builtins (VectorSortOrder, VectorElmMap, etc.) where `ActiveDimRef` is promoted to `Wildcard` to preserve the full array view
   - `expr.rs` - Expression compilation
   - `codegen.rs` - Bytecode emission; routes array-producing builtins through dedicated opcodes instead of element-wise iteration. `emit_array_reduce()` is the shared helper for single-argument array builtins (SUM, SIZE, STDDEV, MIN, MAX, MEAN): pushes view, emits reduction opcode, pops view
   - `dimensions.rs` - Dimension checking/inference
   - `subscript.rs` - Array subscript expansion and iteration
   - `pretty.rs` - Debug pretty-printing
6. **`src/bytecode.rs`** - Instruction set definition, opcodes, type aliases (`LiteralId`, `ModuleId`, `DimId`, `TempId`, etc.). Includes `LoadPrev`/`LoadInitial` opcodes for `PREVIOUS()`/`INIT()` intrinsics and vector operation opcodes (`VectorSelect`, `VectorElmMap`, `VectorSortOrder`, `Rank`, `AllocateAvailable`, `AllocateByPriority`) that operate on view-stack arrays and write results to temp storage.
7. **`src/vm.rs`** - Stack-based bytecode VM. Hot loop uses proven-safe unchecked array access validated at compile time by `ByteCodeBuilder`. Maintains `prev_values` and `initial_values` snapshot buffers for `LoadPrev`/`LoadInitial` opcodes. Implements vector operation dispatch (VectorSelect, VectorElmMap, VectorSortOrder, Rank, AllocateAvailable, AllocateByPriority). Array reducers (ArrayMax, ArrayMin, ArrayMean, ArrayStddev) return NaN for empty views; ArraySum returns 0.0 (additive identity).
8. **`src/alloc.rs`** - Allocation helpers for VM priority allocation: `allocate_available()` (bisection-based priority allocation), `alloc_curve()` (per-requester allocation curves for 6 profile types), `normal_cdf()`/`erfc_approx()`.

## Data model and project structure

- **`src/common.rs`** - Error types (`ErrorCode` with 100+ variants), `Result`, identifier types (`RawIdent`, `Ident<Canonical>`, dimension/element name types), canonicalization
- **`src/errors.rs`** - Human-readable error formatting: `FormattedError`/`FormattedErrors`, `FormattedErrorKind`, `UnitErrorKind`. `format_diagnostic()` converts a salsa `Diagnostic` to `FormattedError`; `format_diagnostic_with_datamodel()` adds source snippets from the datamodel. `collect_formatted_errors()` is the bulk entry point that aggregates all diagnostics into a `FormattedErrors` value. Canonical implementation shared by both `simlin-mcp` and `libsimlin` (which re-exports from here).
- **`src/datamodel.rs`** - Core structures: `Project`, `Model`, `Variable`, `Equation` (including `Arrayed` variant with `default_equation` for EXCEPT semantics and `has_except_default` bool flag), `Dimension` (with `mappings: Vec<DimensionMapping>` replacing the old `maps_to` field, and `parent: Option<String>` for indexed subdimension relationships), `DimensionMapping`, `DataSource`/`DataSourceKind`, `UnitMap`. View element types (`Aux`, `Stock`, `Flow`, `Alias`, `Cloud`) carry an optional `ViewElementCompat` with original Vensim sketch dimensions/bits for MDL roundtrip fidelity. `StockFlow` has an optional `font` string for the Vensim default font spec.
- **`src/variable.rs`** - Variable variants (`Stock`, `Flow`, `Aux`, `Module`), `ModuleInput`, `Table` (graphical functions). `classify_dependencies()` is the primary API for extracting dependency categories from an AST in a single walk, returning a `DepClassification` with five sets: `all` (every referenced ident), `init_referenced`, `previous_referenced`, `previous_only` (idents only inside PREVIOUS), and `init_only` (idents only inside INIT/PREVIOUS). `parse_var_with_module_context` accepts a `module_idents` set so `PREVIOUS(module_var)` rewrites through a scalar helper aux instead of `LoadPrev`.
- **`src/dimensions.rs`** - `DimensionsContext` for dimension matching, subdimension detection, and element-level mappings. Supports indexed subdimensions via `parent` field (child maps to first N elements of parent). `has_mapping_to()` checks for element-level dimension mappings between two dimensions. `SubdimensionRelation` caches parent-child offset mappings for both named (element containment) and indexed (declared parent) dimensions
- **`src/model.rs`** - Model compilation stages (`ModelStage0` -> `ModelStage1` -> `ModuleStage2`), dependency resolution, topological sort. `collect_module_idents` pre-scans datamodel variables to identify which names will expand to modules (preventing incorrect `LoadPrev` compilation). `init_referenced_vars` extends the Initials runlist to include variables referenced by `INIT()` calls, ensuring their values are captured in the `initial_values` snapshot. Unit checking uses salsa tracked functions in `db.rs`.
- **`src/project.rs`** - `Project` struct aggregating models. `from_salsa(datamodel, db, source_project, cb)` builds a Project from a pre-synced salsa database (all variable parsing comes from salsa-cached results). `from_datamodel(datamodel)` is a convenience wrapper that creates a local DB and syncs. Production code uses `db::compile_project_incremental` with `ltm_enabled`/`ltm_discovery_mode` on `SourceProject`.
- **`src/results.rs`** - `Results` (variable offsets + timeseries data), `Specs` (time/integration config)
- **`src/patch.rs`** - `ModelPatch`/`ProjectPatch` (both `Clone`) for representing and applying model changes. `ModelOperation` variants: `UpsertVariable`, `DeleteVariable`, `RenameVariable`, `UpsertView`, `DeleteView`, `UpdateStockFlows`, `SetLoopName` (names a feedback loop by variable list, writing `LoopMetadata` entries). `ModelPatch` is consumed by `incremental_layout()` to determine what variables were added/removed/renamed

## Incremental compilation (salsa)

The primary compilation path uses salsa tracked functions for fine-grained incrementality. Key modules:

- **`src/db.rs`** - `SimlinDb`, `SourceProject`/`SourceModel`/`SourceVariable` salsa inputs, `compile_project_incremental()` entry point, dependency graph computation, diagnostic accumulation via `CompilationDiagnostic` accumulator. `SourceProject` carries `ltm_enabled` and `ltm_discovery_mode` flags for LTM compilation. `Diagnostic` includes a `severity` field (`Error`/`Warning`) and `DiagnosticError` variants: `Equation`, `Model`, `Unit`, `Assembly`. `SourceEquation::Arrayed` carries `has_except_default` to drive EXCEPT default application. `VariableDeps` includes `init_referenced_vars` to track variables referenced by `INIT()` calls. Dependency extraction uses two calls to `classify_dependencies()` (one for the dt AST, one for the init AST) instead of separate walker functions. `parse_source_variable_with_module_context` is the sole parse entry point (the non-module-context variant was removed). `variable_relevant_dimensions` provides dimension-granularity invalidation: scalar variables produce an empty dimension set so dimension changes never invalidate their parse results.
- **`src/db_analysis.rs`** - Salsa-tracked causal graph analysis: `model_causal_edges`, `model_loop_circuits`, `model_cycle_partitions`, `model_detected_loops`. Element-level tracked functions: `model_element_causal_edges` (expands variable-level edges to per-element edges based on `ElementDependencyKind` classification: SameElement, CrossElement, or Scalar), `model_element_loop_circuits` (Johnson's algorithm on element graph), `model_element_cycle_partitions` (stock-to-stock SCCs at element granularity). Produces `DetectedLoop` structs with polarity.
- **`src/db_ltm.rs`** - LTM (Loops That Matter) equation parsing and compilation as salsa tracked functions. `model_ltm_variables` is the unified entry point: generates link scores, loop scores, relative loop scores, pathway scores, and composite scores for any model (root, stdlib, user-defined). Auto-detects sub-model behavior by checking for input ports with causal pathways. Also handles implicit helper/module vars synthesized while parsing LTM equations. `LtmSyntheticVar` carries a `dimensions` field (list of dimension names) so A2A link/loop scores expand to per-element slots during simulation and discovery. `build_element_level_loops` groups element-level circuits into A2A loops (shared ID, with dimensions) or mixed scalar loops.
- **`src/db_ltm_tests.rs`** - Unit tests for LTM equation text generation via salsa tracked functions.
- **`src/db_ltm_unified_tests.rs`** - Tests for `model_ltm_variables`: simple models, stdlib modules (SMOOTH), passthrough modules, discovery mode.
- **`src/db_ltm_module_tests.rs`** - Module-specific LTM tests: SMOOTH compilation with LTM, composite score generation for stdlib modules, user-defined modules with feedback.
- **`src/db_element_graph_tests.rs`** - Tests for element-level causal graph expansion, element-level loop circuits, and element-level cycle partitions via the salsa pipeline. Covers A2A, cross-element, scalar-to-arrayed, and arrayed-to-scalar edge patterns.
- **`src/db_diagnostic_tests.rs`** - Verification tests for diagnostic accumulation paths.
- **`src/db_differential_tests.rs`** - Differential tests verifying `classify_dependencies()` produces identical results to the old per-category walker functions, plus fragment-phase agreement tests ensuring dt/init ASTs yield consistent dependency classifications.
- **`src/db_dimension_invalidation_tests.rs`** - Tests for dimension-granularity salsa invalidation: verifying that dimension changes only re-parse variables that reference those dimensions.
- **`src/db_tests.rs`** - Core salsa pipeline tests.

## Format import/export

- **`src/compat.rs`** - Top-level format entry points: `open_vensim()`, `open_vensim_with_data()`, `open_xmile()`, `open_systems()`, `to_xmile()`, `to_systems()`, `.dat`/CSV loading
- **`src/data_provider/`** - `DataProvider` trait for resolving external data references (GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT). `NullDataProvider` (default), `FilesystemDataProvider` (CSV/Excel via calamine; feature-gated on `file_io`)
- **`src/xmile/`** - XMILE (XML interchange format) parsing and generation. Submodules: `model.rs`, `variables.rs`, `dimensions.rs`, `views.rs`. Uses `simlin:` vendor-extension elements for features beyond the XMILE spec: `simlin:mapping`/`simlin:elem` for element-level dimension mappings, `simlin:data-source` for external data references, `simlin:except` for EXCEPT equation metadata
- **`src/mdl/`** - Native Rust Vensim MDL parser and writer (replaces C++ xmutil):
  - `lexer.rs` -> `normalizer.rs` -> `parser.rs` -> `reader.rs` (pipeline)
  - `ast.rs`, `builtins.rs` (Vensim function recognition)
  - convert/ subdir - Multi-pass AST to datamodel conversion (includes `external_data.rs` for GET DIRECT resolution via `DataProvider`)
  - view/ subdir - Sketch/diagram parsing (`elements.rs`, `processing.rs`, `types.rs`, `mod.rs`) and datamodel conversion (`convert.rs`). Captures font specs and element dimensions/bits during parsing for roundtrip fidelity.
  - `writer.rs` - MDL output: variable equations (with original casing, native LOOKUP syntax, backslash continuations), sketch sections (view splitting on Group boundaries, element ordering)
  - `xmile_compat.rs` - Expression formatting for XMILE output
  - `settings.rs` - Integration settings parser
- **`src/vdf.rs`** - Vensim VDF (binary data file) parser. Parses all structural elements (sections, records, name/slot/offset tables, data blocks). Model-guided name-to-OT mapping via `build_section6_guided_ot_map()` uses section-6 OT class codes to identify contiguous stock/non-stock blocks, classifies variables using the parsed model, and assigns OT indices by alphabetical sort within each block. See `docs/design/vdf.md` for the format specification and reverse-engineering history.
- **`src/systems/`** - Systems format (`.txt`) parser, translator, and writer. A line-oriented notation for stock-and-flow models with implicit flow typing (Rate, Conversion, Leak). Pipeline: `lexer.rs` -> `parser.rs` -> `ast.rs` (IR) -> `translate.rs` (to `datamodel::Project`). Each systems flow becomes a stdlib module instance (`systems_rate`, `systems_leak`, or `systems_conversion`), with chained `available`/`remaining` wiring for multi-outflow stocks. `writer.rs` reconstructs `.txt` format from a translated `datamodel::Project` by inspecting module structure. Public API: `systems::parse()`, `systems::translate::translate()`, `systems::project_to_systems()`
- **`src/json.rs`** - JSON serialization matching Go `sd` package schema
- **`src/json_sdai.rs`** - JSON schema for AI metadata augmentation. `generate_relationships()` derives `Relationship` entries from pre-computed equation dependency polarities (via `compute_link_polarities`), filtering out stock-flow structural edges and sorting deterministically
- **`src/serde.rs`** - Generic serde utilities

## Unit analysis

- **`src/units.rs`** - Unit parsing and `UnitMap` representation
- **`src/units_check.rs`** - Dimensional consistency checking across equations
- **`src/units_infer.rs`** - Unit inference for variables without explicit declarations

## Special features

- **`src/analysis.rs`** - High-level model analysis API: `analyze_model(project, db, source_project, model_name)` bundles compilation, LTM loop discovery, and dominant-period calculation into a single `ModelAnalysis` result. The caller provides a `SimlinDb` and `SourceProject` (already synced); all compilation and structural analysis use the incremental salsa path. Uses element-level causal graph (`model_element_causal_edges` + `causal_graph_from_element_edges`) so arrayed models get per-element loop detection. Passes `LtmSyntheticVar` metadata and datamodel dimensions to `discover_loops_with_graph` for A2A link score expansion. Returns gracefully on simulation failure (empty loop fields, model snapshot intact).
- **`src/ltm.rs`** - Loops That Matter: core data types and causal graph construction. `CausalGraph` builds adjacency lists and detects feedback loops via Johnson's circuit algorithm. `Loop` struct carries `id`, `links`, `stocks`, `polarity`, and `dimensions` (non-empty for A2A loops where per-element evaluation is needed; empty for scalar or mixed loops). `CyclePartitions` groups loops into stock-to-stock SCCs. `Link` records polarity between two variables. `LoopPolarity` is determined by counting negative links in a cycle.
- **`src/ltm_finding.rs`** - Strongest-path loop discovery algorithm (Eberlein & Schoenberg 2020). Post-processes simulation results containing link score synthetic variables to find the most important loops at each timestep. `discover_loops_with_graph(results, causal_graph, stocks, ltm_vars, dims)` is the primary entry point: `ltm_vars` and `dims` enable A2A link score expansion (per-element edges from `parse_link_offsets`); when empty, all link scores are treated as scalar. `SearchGraph` provides DFS-based strongest-path traversal from stock nodes. Returns `Vec<FoundLoop>` with per-timestep link/loop/pathway scores.
- **`src/ltm_augment.rs`** - Equation generators for LTM synthetic variables: `generate_link_score_equation_for_link` (ceteris-paribus link scores), `generate_loop_score_variables` (loop/relative loop scores), `build_partial_equation` (AST-based PREVIOUS wrapping), `quote_ident` (identifier quoting for equations). Array support: `classify_reducer` (walks target Expr2 AST to identify reducing builtins -- Linear for SUM/MEAN, Nonlinear for MIN/MAX/STDDEV/RANK, Constant for SIZE), `generate_element_to_scalar_equation` (per-element link score equations for arrayed-to-scalar edges). Module link score formulas (black-box delta-ratio and composite-ref) are inlined directly into `link_score_equation_text` in `db.rs`.
- **`src/diagram/`** - Diagram/sketch rendering: `elements.rs`, `connector.rs`, `flow.rs`, `render.rs`, `common.rs`, `constants.rs`, `label.rs`, `arrowhead.rs`
- **`src/layout/`** - Automatic diagram layout generation (available on all targets including WASM; uses serial fallback when rayon is unavailable). Two entry points: `generate_best_layout()` (public) generates a complete diagram from scratch; `incremental_layout()` (public) preserves existing element positions and adds/removes only what changed. Submodules: `sfdp.rs` (force-directed placement), `annealing.rs` (crossing reduction), `chain.rs` (stock-flow chain positioning), `config.rs` (layout parameters including `module_width`/`module_height`), `connector.rs` (link routing), `graph.rs` (graph data structures), `metadata.rs` (feedback loops, dominant periods), `placement.rs` (label optimization, normalization), `text.rs` (label sizing), `uid.rs` (UID management), `layout_tests.rs` (unit tests for composable layout blocks and incremental operations). `LayoutState` is the public mutable state struct used by both paths: `LayoutState::new()` for fresh layout, `LayoutState::from_existing_view()` for incremental. Incremental helpers: `identify_new_elements()`, `compute_new_element_positions()`, `settle_new_elements()`, `diff_connectors()`, `diff_clouds()`, `apply_deletion()`, `apply_rename()`. The convenience wrappers `generate_best_layout()` and `generate_layout_with_config()` remain as the primary public API for callers. Generates view elements for modules (not just stocks/flows/auxes).

## Utilities

- **`src/io.rs`** - `atomic_write(path, contents)`: writes bytes to a sibling `.new` temp file, fsyncs it, then renames over the target. Cleans up the temp file on error. Best-effort parent-directory fsync after rename for durability on power loss. Used by MCP and CLI tools that need crash-safe file output.

## Cargo features

- **`file_io`** - Filesystem-based data providers (CSV/Excel). Required by `simulate` and `simulate_ltm` tests.
- **`schema`** - JSON Schema derivation via `schemars`.
- **`ai_info`** - AI metadata signing.

## Generated files (do not edit by hand)

- **`src/project_io.gen.rs`** - Protobuf bindings from `project_io.proto`
- **`src/stdlib.gen.rs`** - Embedded standard library models from `stdlib/*.stmx` (includes `systems_rate`, `systems_leak`, `systems_conversion` for systems format flow types)

## Tests

- **`src/test_common.rs`**, **`src/testutils.rs`** - Helpers and fixtures (e.g. `x_model`, `x_stock`, `x_flow`). `TestProject` is the primary builder for test models: chainable methods like `.aux()`, `.stock()`, `.flow()`, `.array_aux()`, `.array_stock()`, `.array_flow()`, `.named_dimension()`, and `.build_datamodel()` / `.build_sim_with_ltm()`. `assert_compiles_incremental()` verifies the model compiles without errors via the salsa path.
- **`src/array_tests.rs`** - Array-specific tests (feature-gated)
- **`src/json_proptest.rs`**, **`src/json_sdai_proptest.rs`** - Property-based tests
- **`src/unit_checking_test.rs`** - Unit checking regression tests
- **`src/test_sir_xmile.rs`** - SIR epidemiology model integration tests
- **`src/test_open_vensim.rs`** - Vensim compatibility tests (requires `xmutil` feature)
- **`src/systems_stdlib_tests.rs`** - Systems format stdlib module tests (rate, leak, conversion wiring)
- **`tests/test_helpers.rs`** - Shared test helper module (`ensure_results` for CSV result comparison)
- **`tests/simulate.rs`** - End-to-end simulation integration tests
- **`tests/simulate_systems.rs`** - Systems format simulation integration tests (fixtures in `test/systems-format/`)
- **`tests/simulate_ltm.rs`** - LTM feature tests
- **`tests/systems_roundtrip.rs`** - Systems format parse-translate-write round-trip tests
- **`tests/layout.rs`** - Layout generation integration tests (chains, connectors, modules, LTM metadata, dominant periods, incremental layout operations)
- **`tests/json_roundtrip.rs`** - JSON serialization roundtrip
- **`tests/roundtrip.rs`** - XMILE/MDL roundtrip tests
- **`tests/vm_alloc.rs`** - VM memory allocation tests
- **`tests/mdl_equivalence.rs`** - MDL parser equivalence vs C++ xmutil
- **`tests/mdl_roundtrip.rs`** - MDL writer roundtrip integration tests (MDL->MDL, XMILE->MDL, view sketch)
- **`benches/compiler.rs`** - Compiler pipeline benchmarks on real models (WRLD3, C-LEARN)
- **`benches/simulation.rs`** - VM execution and compilation benchmarks (synthetic models)
- **`benches/array_ops.rs`** - Array operation benchmarks (sum, broadcast, element-wise)
