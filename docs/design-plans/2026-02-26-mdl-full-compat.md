# MDL Full Compatibility Design

## Summary

This design completes Simlin's Vensim MDL compatibility by closing the gap between what the MDL parser can read and what the simulation engine can actually compile and run. Today, many Vensim models -- including large real-world models like C-LEARN v77 -- parse successfully but fail during conversion or simulation due to missing language features: external data file references (GET DIRECT DATA and related functions), array exception syntax (`:EXCEPT:`), rich dimension mappings between subscript families, and roughly a dozen unimplemented builtin functions. The work spans the full engine pipeline, from MDL conversion through the datamodel, compiler, and VM.

The approach has four pillars. First, a pluggable `DataProvider` trait resolves external data references at compile time, turning data-backed variables into ordinary lookup tables so the VM needs no changes. Second, the `Equation::Arrayed` variant gains a default-equation field to represent `:EXCEPT:` semantics natively, which the compiler expands to dense per-element equations during lowering. Third, missing builtins are categorized by complexity: pure math functions become VM opcodes, stateful functions (DELAY FIXED, NPV) become stock-flow model compositions following the existing stdlib pattern, and array operations get compiler-level support. Fourth, dimension mappings are extended from a simple single-target pointer to a list of element-level correspondences. The work is sequenced across seven phases -- tactical bug fixes first, then datamodel extensions, then feature implementations, culminating in full C-LEARN simulation validated against reference Vensim output.

## Definition of Done

Complete the simlin-engine MDL parser/converter/compiler pipeline so that **all** Vensim MDL models in the test suite parse, convert, and simulate correctly -- including the full C-LEARN v77 model.

Specifically:
1. All 47 sdeverywhere test models parse, convert, and simulate correctly via the MDL path (no `#[ignore]` for feature gaps).
2. C-LEARN v77 (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) fully parses, converts, and simulates with results validated against reference VDF data within cross-simulator tolerance.
3. A pluggable sync `DataProvider` trait for external data loading, with a filesystem implementation for native builds and an adapter pattern for WASM/browser callers.
4. First-class `:EXCEPT:` support in the datamodel (preserves intent for MDL round-tripping).
5. Stdlib model implementations for missing builtins (VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, SAMPLE IF TRUE / SAMPLE UNTIL, RAMP FROM TO, SSHAPE, ALLOCATE AVAILABLE, NPV, QUANTUM, DELAY FIXED, GET DATA BETWEEN TIMES, etc.).
6. No panics on any valid MDL input.

**Serialization scope:**
- **MDL format**: All changes roundtrip correctly through MDL read/write.
- **sd.json format**: Full fidelity representing the datamodel (DataProvider metadata, EXCEPT, new equation types).
- **Protobuf**: Returns error for unsupported new constructs (tracked as GitHub issue).
- **XMILE**: Returns error for unsupported new constructs (tracked as GitHub issue).

## Acceptance Criteria

### mdl-full-compat.AC1: SDEverywhere models simulate via MDL path
- **mdl-full-compat.AC1.1 Success:** All 47 sdeverywhere test models parse and convert to datamodel without errors
- **mdl-full-compat.AC1.2 Success:** All 47 sdeverywhere test models simulate with results matching expected output within tolerance (2e-3 absolute or 5e-6 relative)

### mdl-full-compat.AC2: C-LEARN model works end-to-end
- **mdl-full-compat.AC2.1 Success:** C-LEARN simulation completes without `not_simulatable` error
- **mdl-full-compat.AC2.2 Success:** C-LEARN simulation results match VDF reference data within cross-simulator tolerance (1% relative error)

### mdl-full-compat.AC3: External data via DataProvider
- **mdl-full-compat.AC3.1 Success:** `DataProvider` trait compiles for both native and WASM targets
- **mdl-full-compat.AC3.2 Success:** `FilesystemDataProvider` loads CSV data files and produces correct lookup tables
- **mdl-full-compat.AC3.3 Success:** `FilesystemDataProvider` loads Excel (XLS) data files via `calamine` crate (feature-gated)
- **mdl-full-compat.AC3.4 Success:** GET DIRECT DATA variables simulate as time-indexed lookups with correct interpolation
- **mdl-full-compat.AC3.5 Failure:** `NullDataProvider` returns clear error when data file is referenced but no provider configured

### mdl-full-compat.AC4: EXCEPT support
- **mdl-full-compat.AC4.1 Success:** `Equation::Arrayed` with `default_equation` roundtrips through JSON serialization
- **mdl-full-compat.AC4.2 Success:** MDL writer emits `:EXCEPT:` syntax for Arrayed equations with default_equation
- **mdl-full-compat.AC4.3 Success:** EXCEPT equations compile and simulate correctly (except/except2 test models pass)

### mdl-full-compat.AC5: Missing builtins
- **mdl-full-compat.AC5.1 Success:** QUANTUM, SSHAPE, RAMP_FROM_TO produce correct values for standard test inputs
- **mdl-full-compat.AC5.2 Success:** DELAY FIXED, SAMPLE IF TRUE, NPV simulate correctly as stdlib model expansions
- **mdl-full-compat.AC5.3 Success:** GET DATA BETWEEN TIMES retrieves correct values from data-backed lookups
- **mdl-full-compat.AC5.4 Success:** VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, ALLOCATE AVAILABLE produce correct array outputs

### mdl-full-compat.AC6: No panics
- **mdl-full-compat.AC6.1 Success:** Compiler handles missing/sparse array element keys without panic (returns error)
- **mdl-full-compat.AC6.2 Success:** MDL conversion of any valid MDL file completes without panic (returns Result with errors)

### mdl-full-compat.AC7: Serialization
- **mdl-full-compat.AC7.1 Success:** sd.json format includes all new datamodel fields (default_equation, DimensionMapping, DataSource)
- **mdl-full-compat.AC7.2 Success:** sd.json roundtrip preserves all new fields
- **mdl-full-compat.AC7.3 Failure:** Protobuf and XMILE serialization return explicit errors for unsupported constructs (not silent data loss)

## Glossary

- **MDL**: Vensim's native model file format. A text format containing variable definitions, equations, subscript declarations, and sketch layout information. Simlin has a native Rust parser in `src/simlin-engine/src/mdl/`.
- **XMILE**: An XML-based interchange standard for system dynamics models (IEEE/OASIS). Simlin's internal representation originated from XMILE concepts; the MDL pipeline converts Vensim constructs into XMILE-compatible structures.
- **C-LEARN v77**: A large climate policy simulation model built in Vensim, used here as the benchmark for full MDL compatibility. Exercises nearly every Vensim language feature including external data, dimension mappings, EXCEPT syntax, and advanced builtins.
- **SDEverywhere**: An open-source project that translates Vensim models to other runtimes. Simlin's test suite includes 47 SDEverywhere test models as compatibility benchmarks.
- **DataProvider**: A trait introduced by this design for resolving external data references (CSV, Excel files) during model compilation. Implementations include `FilesystemDataProvider` for native builds and `NullDataProvider` as a WASM fallback.
- **`:EXCEPT:` syntax**: A Vensim language feature for array equations. A default equation applies to all subscript elements except those explicitly overridden.
- **Dimension mapping**: A declaration that establishes correspondence between elements of two different subscript families (dimensions). Simple mappings assume positional correspondence; element-level mappings explicitly name which source element maps to which target.
- **Lookup table / Graphical function**: A piecewise-linear function defined by (x, y) data points, used for nonlinear relationships in equations. The VM interpolates between points at runtime.
- **Stdlib model**: A builtin function implemented as a stock-and-flow model composition rather than a VM opcode. Functions like DELAY1 and SMOOTH3 are defined as `.stmx` files in `stdlib/` and compiled into `stdlib.gen.rs`.
- **calamine**: A Rust crate for reading Excel files (XLS and XLSX). Used behind a feature flag by `FilesystemDataProvider` for Vensim's GET XLS DATA function.
- **VDF**: Vensim Data File, a proprietary binary format containing simulation output. Used here as reference data to validate C-LEARN simulation results.
- **Lowering**: Transforming higher-level representations into lower-level ones. Here, the compiler lowers EXCEPT equations (default + overrides) into dense per-element equations before code generation.
- **DimensionsContext**: An internal compiler structure for subscript resolution during array equation compilation, tracking which dimensions are in scope and how elements map to memory offsets.
- **FFI**: Foreign Function Interface. `libsimlin` exposes a flat C API so the engine can be called from TypeScript/WASM, Go, Python, and C/C++.

## Architecture

### Overview

This design closes the gap between what the MDL parser can parse and what the engine can simulate. The work spans four layers: MDL conversion fixes, datamodel extensions, compiler/VM enhancements, and new builtin implementations.

The key architectural decisions:

1. **External data via compile-time resolution.** A sync `DataProvider` trait loads data files during compilation. Data variables become regular lookup-backed variables in the compiled model. The VM stays data-agnostic.

2. **EXCEPT as a default-equation pattern.** The existing `Equation::Arrayed` variant gains a `default_equation` field. Elements not explicitly listed use the default. The compiler expands to dense arrays during lowering.

3. **Builtins as stdlib models where possible.** Stateful functions (DELAY FIXED, SAMPLE IF TRUE, NPV) become stock-flow model compositions. Pure math functions (QUANTUM, SSHAPE) become VM builtins. Array operations (VECTOR SELECT, ALLOCATE AVAILABLE) get compiler-level support.

4. **Extended dimension mappings.** The datamodel `Dimension` struct gains richer mapping support beyond single `maps_to: Option<String>`, enabling element-level correspondence needed by C-LEARN.

### Data Flow

```
MDL file
  |  (mdl/lexer -> normalizer -> parser -> reader)
  v
MDL AST (mdl/ast.rs)
  |  (mdl/convert/)
  v
datamodel::Project  <-- DataProvider resolves GET_* here
  |  (compiler/)
  v
Bytecode + Lookups  <-- data variables are now regular lookups
  |  (vm.rs)
  v
Simulation results
```

### DataProvider Contract

```rust
/// Trait for resolving external data references during compilation.
/// Native builds use FilesystemDataProvider; WASM callers provide
/// pre-loaded data via an adapter implementing this trait.
pub trait DataProvider {
    /// Load a time-indexed data series from an external file.
    /// Returns a lookup table (time -> value pairs) suitable for
    /// interpolation during simulation.
    fn load_data(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_or_col_label: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load a constant value (scalar or array element) from an external file.
    fn load_constant(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64>;

    /// Load a lookup table (graphical function) from an external file.
    fn load_lookup(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load dimension element names from an external file.
    fn load_subscript(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>>;
}
```

### EXCEPT Datamodel Extension

```rust
pub enum Equation {
    Scalar(String),
    ApplyToAll(Vec<DimensionName>, String),
    Arrayed(
        Vec<DimensionName>,
        Vec<(ElementName, String, Option<String>, Option<GraphicalFunction>)>,
        // NEW: default equation for elements not explicitly listed
        Option<String>,
    ),
}
```

When `default_equation` is `Some(eq)`, elements not present in the element list use `eq`. This directly represents `:EXCEPT:` semantics: the default applies everywhere except the listed overrides. Existing code that constructs `Arrayed` with `None` is unaffected.

### Dimension Mapping Extension

```rust
pub struct Dimension {
    pub name: String,
    pub elements: DimensionElements,
    /// Dimension mappings. Replaces the previous `maps_to: Option<String>`.
    /// Supports both simple (single target) and multi-entry (element-level
    /// correspondence) mappings needed by complex Vensim models.
    pub mappings: Vec<DimensionMapping>,
}

pub struct DimensionMapping {
    /// Target dimension name
    pub target: String,
    /// Element-level correspondence. When empty, positional mapping is assumed.
    /// When present, maps source elements to target elements by name.
    pub element_map: Vec<(String, String)>,
}
```

### Builtin Categorization

| Category | Functions | Implementation |
|----------|-----------|----------------|
| VM builtins (pure math) | QUANTUM, SSHAPE, RAMP_FROM_TO | New opcodes in `bytecode.rs`, handlers in `vm.rs` |
| Stdlib models (stateful) | DELAY FIXED, SAMPLE IF TRUE, NPV | `.stmx` files in `stdlib/`, compiled to `stdlib.gen.rs` |
| Compiler-level (arrays) | VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, ALLOCATE AVAILABLE | Array iteration patterns in `compiler/expr.rs` |
| Conversion-level (data) | GET DIRECT DATA/CONSTANTS/SUBSCRIPT/LOOKUPS, GET XLS DATA, GET DATA BETWEEN TIMES | Resolved during `mdl/convert/` via DataProvider |

## Existing Patterns

**Stdlib model composition.** DELAY1, DELAY3, SMTH1, SMTH3, TREND, and PREVIOUS are implemented as stock-flow model compositions in `stdlib/*.stmx`, compiled into `stdlib.gen.rs`. The `BuiltinVisitor` in `builtins_visitor.rs` walks the AST and expands function calls into module instantiations. New stateful builtins (DELAY FIXED, SAMPLE IF TRUE, NPV) follow this exact pattern.

**Lookup-backed variables.** Variables with graphical functions are compiled to lookup tables with interpolation in the VM. External data loaded via DataProvider produces the same lookup structure, so data variables slot into existing compilation infrastructure with no VM changes.

**Array compilation.** The compiler uses `DimensionsContext` for subscript resolution, `array_view.rs` for stride/offset computation, and `SparseRange` for non-contiguous iteration. EXCEPT expansion and VECTOR operations build on this infrastructure.

**MDL conversion pipeline.** `mdl/convert/` transforms the MDL AST into `datamodel::Project` in multiple passes: dimensions first, then variables, then views. The DataProvider integration adds a data-resolution pass between dimension building and variable conversion.

**JSON serialization.** `src/json.rs` provides full-fidelity serialization matching the Go `sd` package schema. New datamodel fields (default_equation, DimensionMapping, data source metadata) extend the existing JSON schema.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Tactical Fixes
**Goal:** Fix high-leverage bugs that cause cascading failures in existing tests and C-LEARN

**Components:**
- `:NA:` emission in `src/simlin-engine/src/mdl/xmile_compat.rs` -- emit `NaN` instead of `:NA:` for core parser compatibility
- Dimension alias casing in `src/simlin-engine/src/mdl/convert/dimensions.rs` -- preserve original case from MDL source
- Equation type handling in `src/simlin-engine/src/mdl/convert/variables.rs` -- add `Data`, `TabbedArray`, `NumberList`, `Implicit` arms to `build_equation_rhs_with_context`
- Subscript context in `src/simlin-engine/src/builtins_visitor.rs` -- provide `ElementContext` when expanding stdlib modules within `Ast::Arrayed` expressions
- Compiler panic guard in `src/simlin-engine/src/compiler/mod.rs` -- use `.get()` instead of direct indexing for sparse element key lookups
- C-LEARN equivalence diffs in MDL conversion -- trailing tab stripping, initial-value comment extraction, net flow synthesis, middle-dot Unicode normalization

**Dependencies:** None (first phase)

**Done when:** C-LEARN equivalence test has zero diffs. Existing sdeverywhere tests still pass. No panics on C-LEARN parse/convert. Covers `mdl-full-compat.AC6.1`, `mdl-full-compat.AC6.2`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Datamodel Extensions
**Goal:** Extend the datamodel to represent EXCEPT, dimension mappings, and data source metadata

**Components:**
- `Equation::Arrayed` in `src/simlin-engine/src/datamodel.rs` -- add `Option<String>` default_equation field
- `Dimension` in `src/simlin-engine/src/datamodel.rs` -- replace `maps_to: Option<String>` with `mappings: Vec<DimensionMapping>`
- New `DimensionMapping` struct in `src/simlin-engine/src/datamodel.rs`
- New `DataSource` metadata struct in `src/simlin-engine/src/datamodel.rs` -- stores parsed GET_* function arguments (file, sheet, row/col labels) for variables backed by external data
- JSON serialization in `src/simlin-engine/src/json.rs` -- full-fidelity for all new fields
- MDL writer in `src/simlin-engine/src/mdl/writer.rs` -- roundtrip for EXCEPT, dimension mappings
- Update all `Equation::Arrayed` match arms throughout the codebase for the new field
- File GitHub issues for protobuf and XMILE serialization gaps

**Dependencies:** Phase 1

**Done when:** Datamodel compiles with new types. JSON roundtrip tests pass for EXCEPT equations, multi-entry dimension mappings, and data source metadata. MDL writer emits `:EXCEPT:` syntax. GitHub issues filed. Covers `mdl-full-compat.AC4.1`, `mdl-full-compat.AC4.2`, `mdl-full-compat.AC7.1`, `mdl-full-compat.AC7.2`, `mdl-full-compat.AC7.3`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: EXCEPT Semantics
**Goal:** Full EXCEPT support from MDL parsing through simulation

**Components:**
- MDL converter in `src/simlin-engine/src/mdl/convert/variables.rs` -- consume `lhs.except` to produce `Arrayed` equations with `default_equation`
- Compiler expansion in `src/simlin-engine/src/compiler/mod.rs` -- expand default+overrides to dense per-element equations before codegen
- Expression substitution -- apply element-specific dimension references in default equations during expansion

**Dependencies:** Phase 2

**Done when:** `test/sdeverywhere/models/except/` and `test/sdeverywhere/models/except2/` parse, convert, and simulate correctly. Covers `mdl-full-compat.AC4.3`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: DataProvider Infrastructure and External Data
**Goal:** External data loading from CSV/Excel files via pluggable DataProvider trait

**Components:**
- `DataProvider` trait in `src/simlin-engine/src/datamodel.rs` or new `src/simlin-engine/src/data_provider.rs`
- `FilesystemDataProvider` implementation -- CSV parsing, Excel reading (via feature-gated dependency)
- `NullDataProvider` -- returns errors, used when no data files available (WASM default)
- MDL converter integration in `src/simlin-engine/src/mdl/convert/` -- resolve `GET DIRECT DATA`, `GET DIRECT CONSTANTS`, `GET DIRECT LOOKUPS`, `GET DIRECT SUBSCRIPT`, `GET XLS DATA` during conversion
- Data variables become lookup-backed Aux variables with `GraphicalFunction` tables from loaded data
- `GET DIRECT SUBSCRIPT` resolution -- data-driven dimension definitions
- `open_vensim()` in `src/simlin-engine/src/compat.rs` -- accept optional `DataProvider` parameter
- `libsimlin` FFI in `src/libsimlin/src/lib.rs` -- expose DataProvider configuration

**Dependencies:** Phase 2

**Done when:** `test/sdeverywhere/models/directdata/`, `directconst/`, `directlookups/`, `directsubs/`, `extdata/` all simulate correctly. Covers `mdl-full-compat.AC3.1`, `mdl-full-compat.AC3.2`, `mdl-full-compat.AC3.3`, `mdl-full-compat.AC3.4`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Missing Builtins -- VM and Stdlib
**Goal:** Implement all missing Vensim builtin functions

**Components:**
- **VM builtins** in `src/simlin-engine/src/builtins.rs`, `bytecode.rs`, `vm.rs`, `interpreter.rs`:
  - QUANTUM(x, quantum) -- quantize to nearest multiple
  - SSHAPE(x, bottom, top) -- S-shaped growth function
  - RAMP_FROM_TO(slope, start_time, end_time) -- bounded RAMP
- **Stdlib models** in `src/simlin-engine/stdlib/`:
  - `delayfixed.stmx` -- DELAY FIXED (FIFO queue delay using internal array)
  - `sampleiftrue.stmx` -- SAMPLE IF TRUE (conditional sampling with hold)
  - `npv.stmx` -- NPV (net present value accumulation)
- **Builtin registration** in `src/simlin-engine/src/mdl/builtins.rs` and `src/simlin-engine/src/builtins.rs` -- ensure MDL function names map to the correct engine builtins
- GET DATA BETWEEN TIMES -- interpolation function on data-backed lookups (may be a VM builtin or compiler transformation depending on complexity)

**Dependencies:** Phase 4 (GET DATA BETWEEN TIMES needs data infrastructure)

**Done when:** `test/sdeverywhere/models/quantum/`, `npv/`, `sample/`, `delayfixed/`, `delayfixed2/`, `getdata/` all simulate correctly. Covers `mdl-full-compat.AC5.1`, `mdl-full-compat.AC5.2`, `mdl-full-compat.AC5.3`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Compiler-Level Array Operations
**Goal:** Implement VECTOR operations and ALLOCATE AVAILABLE at the compiler level

**Components:**
- VECTOR SELECT in `src/simlin-engine/src/compiler/expr.rs` -- compile to conditional element selection with array iteration
- VECTOR ELM MAP in `src/simlin-engine/src/compiler/expr.rs` -- compile to index-remapped array access
- VECTOR SORT ORDER in `src/simlin-engine/src/compiler/expr.rs` -- compile to sort-index computation (may need VM support for comparison-based sorting)
- ALLOCATE AVAILABLE in `src/simlin-engine/src/compiler/expr.rs` or as stdlib model -- priority-based allocation across array elements
- Extended dimension mapping in compiler -- use new `DimensionMapping` struct for element-level subscript resolution
- MDL function name normalization in `src/simlin-engine/src/mdl/xmile_compat.rs` -- emit parser-compatible identifiers for vector functions

**Dependencies:** Phase 2 (dimension mappings), Phase 3 (EXCEPT for some test models)

**Done when:** `test/sdeverywhere/models/mapping/`, `multimap/`, `subscript/`, `allocate/` simulate correctly. Vector operations work in C-LEARN equations. Covers `mdl-full-compat.AC5.4`.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: C-LEARN Simulation and Full Test Enablement
**Goal:** C-LEARN simulates correctly; all sdeverywhere models pass

**Components:**
- C-LEARN-specific equation fixes -- any remaining conversion issues surfaced by simulation attempts
- C-LEARN simulation test in `src/simlin-engine/tests/simulate.rs` -- add test comparing against VDF reference data
- Enable all 47 sdeverywhere models in simulation test suite -- remove `#[ignore]` annotations and exclusion lists
- Enable remaining sdeverywhere models in mdl_equivalence tests
- Fix any remaining C-LEARN equivalence diffs
- Remaining sdeverywhere models not covered by earlier phases (`flatten/input2`, `prune`, `preprocess/` models)

**Dependencies:** Phases 1-6

**Done when:** `cargo test --features file_io -p simlin-engine` passes with all sdeverywhere models enabled. C-LEARN simulation matches reference VDF within cross-simulator tolerance (1% relative error). Covers `mdl-full-compat.AC1.1`, `mdl-full-compat.AC1.2`, `mdl-full-compat.AC2.1`, `mdl-full-compat.AC2.2`, `mdl-full-compat.AC2.3`.
<!-- END_PHASE_7 -->

## Additional Considerations

**DELAY FIXED complexity.** Unlike DELAY1/DELAY3 which are continuous delays modeled as stock-flow chains, DELAY FIXED requires a discrete FIFO queue. The stdlib model approach works but the internal array must be sized to `ceil(delay_time / time_step)` elements, which depends on simulation specs known at compile time. The compiler passes sim specs to stdlib model instantiation already (for DELAY N), so this pattern exists.

**ALLOCATE AVAILABLE complexity.** This is the most complex single builtin. It performs priority-based allocation of a scarce resource across array elements, considering both priority order and maximum request constraints. If stdlib model composition proves unwieldy, it may need to be a compiler-level transformation that generates specialized iteration code. The implementation plan should investigate both approaches.

**Excel file support.** GET XLS DATA requires reading Excel files. This uses the [`calamine`](https://github.com/tafia/calamine) crate behind a feature flag. The DataProvider trait abstracts this -- `FilesystemDataProvider` handles both CSV and Excel; WASM callers provide pre-parsed data through a custom DataProvider.

**Implementation scoping.** This design has 7 phases. Phases 1-4 are sequential with clear dependencies. Phases 5 and 6 can proceed in parallel after Phase 4. Phase 7 depends on all prior phases. A single implementation plan covers all 7 phases.
