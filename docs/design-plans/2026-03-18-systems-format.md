# Systems Format Support Design

## Summary

This design adds native support for the `systems` text format -- a line-oriented DSL created by Will Larson for discrete stock-and-flow modeling -- to the simlin engine. The format has three flow types (Rate, Conversion, Leak) with semantics that differ from standard system dynamics: flows use integer truncation, stocks are debited sequentially in priority order, and formulas evaluate left-to-right without operator precedence. Rather than implementing these semantics as special-case engine code, the approach defines three small stdlib modules (`.stmx` files for each flow type) and translates each systems flow into a module instantiation. Multi-outflow priority ordering is handled by chaining modules through a `remaining` output, which lets simlin's existing topological sort enforce the correct evaluation order without any custom scheduling logic.

The implementation spans seven phases: defining the stdlib modules, building a two-stage parser (lexer and AST), translating the AST into simlin's datamodel via module instantiation, validating simulation output against the Python reference implementation, writing a round-trip serializer that reconstructs the original `.txt` format by inspecting module structure, promoting ALLOCATE BY PRIORITY from a format-specific MDL workaround to a first-class engine builtin, and extending the layout engine to generate diagram elements for modules. The parser and ALLOCATE BY PRIORITY work are independent of each other, while the translator, simulation tests, writer, and layout work build sequentially on the parser and stdlib modules.

## Definition of Done

Add read and write support for the `systems` text format (from `third_party/systems`), translate it into simlin's native datamodel, and produce simulation output matching the Python `systems` package. Implement ALLOCATE BY PRIORITY as a general Vensim-compatible engine builtin and review/extend the existing ALLOCATE AVAILABLE implementation. The writer must be able to reconstruct the original `.txt` format by identifying and stripping synthesized helper variables (cascading auxiliaries, waste flows/stocks) during serialization.

**Deliverables:**
1. Systems format reader (Rust parser + translator to simlin datamodel using stdlib modules)
2. Systems format writer (datamodel -> `.txt`, stripping synthesized variables by reading module structure)
3. ALLOCATE BY PRIORITY engine builtin + ALLOCATE AVAILABLE review
4. Test fixtures with pre-generated CSV expected outputs from the Python package
5. Module support in diagram generation (extend layout engine to include modules)

**Success criteria:**
- All valid example files from `third_party/systems/examples/` parse, translate, simulate, and produce output matching the Python-generated CSVs
- Round-trip fidelity: parse -> datamodel -> write produces semantically equivalent `.txt` (synthesized helper variables stripped, flow types correctly reconstructed from module metadata)
- Stdlib modules (systems_rate, systems_conversion, systems_leak) correctly handle sequential debiting via chaining, Conversion waste flows, and maximum capacity semantics
- ALLOCATE BY PRIORITY works as a general engine builtin (not just for systems format)
- Layout engine generates diagram view elements for modules, including connectors to/from module variables

**Out of scope:**
- Visualization/diagram features from the systems package (`systems-viz`)
- Error recovery / partial parsing of malformed files
- Continuous-time extensions to the systems format

## Acceptance Criteria

### systems-format.AC1: Parser handles all valid syntax constructs
- **systems-format.AC1.1 Success:** Plain stock declaration (`Name`) creates stock with initial=0, max=inf
- **systems-format.AC1.2 Success:** Parameterized stock (`Name(10)`, `Name(10, 20)`) sets initial and max values
- **systems-format.AC1.3 Success:** Infinite stock (`[Name]`) creates stock with initial=inf, show=false equivalent
- **systems-format.AC1.4 Success:** Rate flow with integer (`A > B @ 5`) produces Rate type
- **systems-format.AC1.5 Success:** Conversion flow with decimal (`A > B @ 0.5`) produces Conversion type
- **systems-format.AC1.6 Success:** Explicit flow types (`Rate(5)`, `Conversion(0.5)`, `Leak(0.2)`) parse correctly
- **systems-format.AC1.7 Success:** Formula expressions with references, arithmetic, and parentheses parse correctly
- **systems-format.AC1.8 Success:** Comment lines (`# ...`) are ignored
- **systems-format.AC1.9 Success:** Stock-only lines without `@` create stocks but no flow
- **systems-format.AC1.10 Edge:** Stock initialized at later reference (`a > b @ 5` then `b(2) > c @ 3`) resolves correctly
- **systems-format.AC1.11 Failure:** Duplicate stock initialization with conflicting values raises error

### systems-format.AC2: Translation produces correct datamodel structure
- **systems-format.AC2.1 Success:** Each systems flow produces a stdlib module instance with correct model_name (systems_rate, systems_conversion, systems_leak)
- **systems-format.AC2.2 Success:** Module input bindings correctly reference source stock, rate expression, and destination capacity
- **systems-format.AC2.3 Success:** Conversion flows produce a waste flow that is an outflow from the source stock with no destination
- **systems-format.AC2.4 Success:** Multi-outflow stocks produce chained modules where each module's `available` input references the previous module's `remaining` output
- **systems-format.AC2.5 Success:** Chain order matches reversed declaration order (last-declared flow gets highest priority)
- **systems-format.AC2.6 Success:** Infinite stocks translate to stocks with equation "inf"
- **systems-format.AC2.7 Success:** SimSpecs set to start=0, dt=1, save_step=1, method=Euler

### systems-format.AC3: Simulation output matches Python systems package
- **systems-format.AC3.1 Success:** `hiring.txt` simulation matches Python output for 5 rounds
- **systems-format.AC3.2 Success:** `links.txt` simulation matches (tests formula references in flow rates)
- **systems-format.AC3.3 Success:** `maximums.txt` simulation matches (tests destination capacity limiting)
- **systems-format.AC3.4 Success:** `projects.txt` simulation matches (tests complex formulas with division and parentheses)
- **systems-format.AC3.5 Success:** `extended_syntax.txt` simulation matches (tests Rate, Leak, Conversion, formula references, stock maximums)
- **systems-format.AC3.6 Success:** Both VM and interpreter paths produce identical results

### systems-format.AC4: Round-trip writer reconstructs original format
- **systems-format.AC4.1 Success:** Writer strips all module variables from output
- **systems-format.AC4.2 Success:** Writer strips waste flows from output
- **systems-format.AC4.3 Success:** Writer reconstructs flow type (Rate/Conversion/Leak) from module model_name
- **systems-format.AC4.4 Success:** Writer recovers original declaration order from module remaining chain
- **systems-format.AC4.5 Success:** Writer identifies infinite stocks and produces `[Name]` syntax
- **systems-format.AC4.6 Success:** Round-trip (parse -> translate -> write -> parse -> translate -> simulate) produces matching output for all fixtures

### systems-format.AC5: ALLOCATE BY PRIORITY works as native builtin
- **systems-format.AC5.1 Success:** ALLOCATE BY PRIORITY in XMILE equations compiles and executes correctly
- **systems-format.AC5.2 Success:** Results match ALLOCATE AVAILABLE with equivalent rectangular priority profiles
- **systems-format.AC5.3 Success:** Existing MDL allocation tests continue to pass
- **systems-format.AC5.4 Success:** MDL writer emits ALLOCATE BY PRIORITY syntax for the Vensim form

### systems-format.AC6: Layout engine generates module diagram elements
- **systems-format.AC6.1 Success:** Layout engine creates ViewElement::Module for each Variable::Module
- **systems-format.AC6.2 Success:** Modules appear as nodes in the SFDP force-directed graph
- **systems-format.AC6.3 Success:** Connectors are generated between modules and connected stocks/flows
- **systems-format.AC6.4 Success:** Module label placement is optimized alongside other elements
- **systems-format.AC6.5 Success:** Systems format models produce complete, renderable diagrams

### systems-format.AC7: Left-to-right formula evaluation
- **systems-format.AC7.1 Success:** Systems formulas are parenthesized during translation to preserve left-to-right evaluation (e.g., `a + b * c` becomes `(a + b) * c`)
- **systems-format.AC7.2 Success:** Parenthesized formulas in the systems format (e.g., `(a + b) / 2`) translate correctly

## Glossary

- **System dynamics (SD)**: A modeling methodology where systems are described as stocks (accumulators) connected by flows (rates of change), simulated over time. Simlin is an SD modeling tool.
- **Stock-and-flow model**: The core SD representation where stocks hold quantities and flows move quantities between stocks at each time step.
- **XMILE**: An XML-based open standard for representing system dynamics models. Simlin's native internal format and the format used for stdlib module definitions (`.stmx` files).
- **Vensim**: A widely-used commercial system dynamics modeling tool. Its `.mdl` file format is one of the formats simlin can read, and several engine builtins (like ALLOCATE AVAILABLE) originate from Vensim.
- **dt**: The simulation time step. In the systems format this is fixed at 1 (one discrete "round" per step), unlike standard SD where dt is a configurable fractional value.
- **Euler method**: A numerical integration method for advancing stock values each time step: `stock(t+dt) = stock(t) + net_flow * dt`.
- **Stdlib module**: A reusable sub-model defined as a `.stmx` file in `src/simlin-engine/stdlib/`, compiled into the engine binary via `stdlib.gen.rs`. Existing examples include SMOOTH, DELAY, and TREND.
- **Module instantiation (`Variable::Module`)**: The mechanism by which a stdlib module is used in a model: a `Variable::Module` references a module definition and binds its inputs to expressions from the parent model. Outputs are referenced via `module_name.output_name` syntax.
- **Sequential source debiting**: The systems format's priority mechanism for multiple outflows from a single stock. Flows are processed in reversed declaration order; each flow immediately reduces the source stock's available balance before the next flow is evaluated.
- **Remaining chain**: The translation pattern where each module's `remaining` output feeds into the next module's `available` input, encoding sequential debiting as a data dependency that simlin's topological sort resolves automatically.
- **Waste flow**: A flow generated during Conversion translation that removes the unconverted portion from the source stock. It is an outflow with no destination stock (the material "vanishes"), analogous to a cloud flow.
- **Cloud flow**: In XMILE/SD terminology, a flow that originates from or drains into "nowhere" -- representing material entering or leaving the system boundary.
- **ALLOCATE AVAILABLE**: An existing simlin engine builtin (from Vensim) that distributes a limited supply across multiple demands using priority profiles and a bisection algorithm.
- **ALLOCATE BY PRIORITY**: A higher-level allocation function that creates rectangular priority profiles from simple numeric priorities, then delegates to ALLOCATE AVAILABLE.
- **Priority profile**: In the ALLOCATE AVAILABLE algorithm, a curve (ptype, ppriority, pwidth, pextra) describing how a demand's priority varies as supply changes.
- **AST rewrite / lowering (Expr0 -> Expr1)**: A compiler pass that transforms the initial parsed expression tree into a normalized form. ALLOCATE BY PRIORITY is desugared into ALLOCATE AVAILABLE during this pass.
- **SFDP**: Scalable Force-Directed Placement, a layout algorithm used by simlin's layout engine to position model elements in auto-generated diagrams.
- **ViewElement**: A datamodel type representing a visual element in a model diagram (stock, flow, auxiliary, connector, or module box).
- **`compat.rs`**: The engine's format compatibility layer, providing `open_<format>()` entry points that parse external formats into `datamodel::Project`.
- **Round-trip fidelity**: The property that parsing a `.txt` file, translating it to the internal datamodel, and writing it back produces a semantically equivalent `.txt` file.

## Architecture

### The Systems Format

The `systems` text format (`third_party/systems`) is a line-oriented DSL for stock-and-flow models with three flow types (Rate, Conversion, Leak), formula references between stocks, maximum capacity constraints, and infinite stocks. It uses dt=1 integer "rounds" with no configurable time range.

Critical semantic differences from standard SD:
- **Sequential source debiting**: flows iterate in reversed declaration order; source stocks are debited immediately during iteration, creating priority-based allocation when a stock has multiple outflows.
- **Conversion**: drains the entire source stock but adds only `floor(src * rate)` to the destination. The remainder vanishes.
- **Leak**: moves `floor(src * rate)` from source to dest non-destructively.
- **Rate**: moves `min(rate, source)` as a capped fixed transfer.
- **Integer truncation**: Conversion and Leak use `floor()`.
- **Implicit type detection**: bare decimal (`@ 0.5`) becomes Conversion; bare integer (`@ 5`) becomes Rate.

### Parser Pipeline

Two-stage parser in `src/simlin-engine/src/systems/`:

1. **Lexer** (`lexer.rs`): line-oriented tokenizer producing comment lines, stock declarations (with optional initial/max and infinite `[Name]` syntax), and flow lines (source, dest, `@` rate with optional type prefix).

2. **Parser** (`parser.rs`): builds a `SystemsModel` intermediate representation containing `Vec<SystemsStock>` (name, initial expression, max expression, is_infinite flag) and `Vec<SystemsFlow>` (source name, dest name, flow type enum, rate expression). Preserves declaration order for correct reversed-iteration reconstruction.

Formula parsing supports `+`, `-`, `*`, `/`, parenthesized sub-expressions, stock name references, `inf` literal, integers, and decimals.

### Translation via Stdlib Modules

Three stdlib modules encapsulate the flow type semantics, defined as `.stmx` files in `src/simlin-engine/stdlib/`:

**`systems_rate.stmx`:**
- Inputs: `available` (remaining source value), `requested` (rate expression), `dest_capacity` (max_dest - current_dest, or inf)
- Outputs: `actual` = MIN(requested, MIN(available, dest_capacity)), `remaining` = available - actual

**`systems_leak.stmx`:**
- Inputs: `available`, `rate`, `dest_capacity`
- Outputs: `actual` = MIN(INT(available * rate), dest_capacity), `remaining` = available - actual

**`systems_conversion.stmx`:**
- Inputs: `available`, `rate`, `dest_capacity`
- Outputs: `outflow` = MIN(INT(available * rate), dest_capacity), `waste` = available - outflow, `remaining` = 0

#### Single-outflow translation

Rate (`A(10) > B @ 7`):
- Stock A (initial=10), Stock B (initial=0)
- Module `a_outflows`: systems_rate (available=A, requested=7, dest_capacity=inf)
- Flow `a_to_b` with equation `a_outflows.actual` (outflow from A, inflow to B)

Conversion (`A(10) > B @ 0.5`):
- Module `a_outflows`: systems_conversion (available=A, rate=0.5, dest_capacity=inf)
- Flow `a_to_b` with equation `a_outflows.outflow` (outflow from A, inflow to B)
- Flow `a_to_b_waste` with equation `a_outflows.waste` (outflow from A only, no destination stock)

Leak (`A(10) > B @ Leak(0.2)`):
- Module `a_outflows`: systems_leak (available=A, rate=0.2, dest_capacity=inf)
- Flow `a_to_b` with equation `a_outflows.actual` (outflow from A, inflow to B)

#### Multi-outflow chaining (sequential debiting)

When stock A has multiple outflows, modules chain via `remaining`. Flows are processed in reversed declaration order (last-declared gets highest priority):

```
# Given: A(10) > B @ 7  (declared first, lower priority)
#         A > C @ 7      (declared second, higher priority)

Module a_outflows_c: systems_rate (available=A, requested=7, ...)
Flow   a_to_c = a_outflows_c.actual

Module a_outflows_b: systems_rate (available=a_outflows_c.remaining, requested=7, ...)
Flow   a_to_b = a_outflows_b.actual
```

The dependency chain (a_outflows_b depends on a_outflows_c.remaining) ensures simlin's topological sort evaluates them in the correct order. No custom evaluation mode needed.

### Writer

The writer reconstructs the systems format from the datamodel by reading module structure:

1. Find all `Variable::Module` with `model_name` in {systems_rate, systems_conversion, systems_leak}
2. Read module input bindings to extract source stock, rate expression, destination stock
3. Reconstruct flow type from `model_name`
4. Walk the `remaining` chain to recover original declaration order (the module bound directly to a stock was last-declared/highest-priority; reverse the chain to get original order)
5. Strip all module variables and waste flows from output
6. Identify infinite stocks by equation `"inf"`

Output format: explicit flow type syntax (always `Conversion(...)`, `Leak(...)`, `Rate(...)`) to avoid ambiguity from implicit type detection.

### ALLOCATE BY PRIORITY Builtin

ALLOCATE BY PRIORITY is syntactic sugar for ALLOCATE AVAILABLE with rectangular priority profiles. Implemented as an AST rewrite during Expr0->Expr1 lowering:

```
ALLOCATE BY PRIORITY(request, priority, size, width, supply)
  -> ALLOCATE AVAILABLE(request, pp, supply)
  where pp[i] = (1, priority[i], width, 0)  // ptype=1 rectangular, pextra=0
```

This reuses the entire existing ALLOCATE AVAILABLE pipeline (parser, compiler, VM). The only new code is the `BuiltinFn::AllocateByPriority` variant and its lowering transform in `ast/expr1.rs`.

### Module Diagram Generation

The layout engine (`src/simlin-engine/src/layout/`) currently excludes modules at 9 sites. Extend it to:

1. Add module dimensions to `LayoutConfig` (width=55, height=45 matching `MODULE_WIDTH`/`MODULE_HEIGHT` constants)
2. Include module variables in the SFDP force-directed graph as nodes
3. Create `ViewElement::Module` elements for each `Variable::Module` (analogous to `create_missing_auxiliary_elements`)
4. Generate connectors to/from module elements (revise the module-exclusion filter in `create_connectors`)
5. Include modules in `validate_view_completeness`, `apply_layout_positions`, `apply_optimal_label_placement`, and `recalculate_bounds`

The existing drawing infrastructure (Module.tsx, `render_module()` in elements.rs, Canvas.tsx z-order 4) already handles rendering.

## Existing Patterns

### Format Reader Pattern
The MDL reader (`src/simlin-engine/src/mdl/`) provides the reference pattern: lexer -> parser -> conversion context -> `datamodel::Project`. The conversion context runs multiple passes (collect symbols, build dimensions, classify variables, link stocks and flows, build project). Integration via `compat.rs` with a public `open_<format>(contents: &str) -> Result<Project>` function.

The systems reader follows the same integration pattern but with a simpler pipeline (no dimensions, no view parsing, no macros).

### Stdlib Module Pattern
SMOOTH, DELAY, and TREND are defined as `.stmx` files in `src/simlin-engine/stdlib/` and embedded via `stdlib.gen.rs`. The systems flow type modules follow this same pattern. Module instantiation uses `Variable::Module` with `ModuleReference` bindings for inputs, and parent variables reference module outputs via `module_name.output_name` equations.

### ALLOCATE Pattern
ALLOCATE AVAILABLE is fully implemented: `BuiltinFn::AllocateAvailable` in builtins.rs -> `Opcode::AllocateAvailable` in bytecode.rs -> bisection-based allocation in alloc.rs -> VM dispatch in vm.rs. The MDL reader already translates `ALLOCATE BY PRIORITY` to ALLOCATE AVAILABLE during MDL->XMILE conversion (`mdl/xmile_compat.rs`). This design promotes that translation from format-specific to engine-native.

### No Precedent For
- A format reader that produces modules as its primary output structure (existing readers produce stocks/flows/auxes directly)
- "Waste" flows without destination stocks (flows that are outflows from a stock but not inflows to any stock -- needs verification that the compiler handles this)
- Module view elements in auto-generated layouts (currently excluded at 9 sites)

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Stdlib Modules for Flow Types
**Goal:** Define systems_rate, systems_leak, and systems_conversion as stdlib modules

**Components:**
- `src/simlin-engine/stdlib/systems_rate.stmx` -- rate flow module (available, requested, dest_capacity -> actual, remaining)
- `src/simlin-engine/stdlib/systems_leak.stmx` -- leak flow module (available, rate, dest_capacity -> actual, remaining)
- `src/simlin-engine/stdlib/systems_conversion.stmx` -- conversion flow module (available, rate, dest_capacity -> outflow, waste, remaining)
- Regenerate `src/simlin-engine/src/stdlib.gen.rs` to embed new modules

**Dependencies:** None (first phase)

**Done when:** Modules parse, compile, and simulate correctly when instantiated manually in test models. Unit tests verify each flow type's behavior matches the Python `systems` package for representative inputs.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Systems Format Parser
**Goal:** Parse `.txt` files into a `SystemsModel` intermediate representation

**Components:**
- `src/simlin-engine/src/systems/mod.rs` -- module entry point
- `src/simlin-engine/src/systems/lexer.rs` -- line-oriented tokenizer
- `src/simlin-engine/src/systems/parser.rs` -- builds SystemsModel from tokens
- `src/simlin-engine/src/systems/ast.rs` -- SystemsModel, SystemsStock, SystemsFlow, FlowType types

**Dependencies:** None (independent of Phase 1)

**Done when:** All valid examples from `third_party/systems/examples/` parse into correct SystemsModel ASTs. Unit tests verify stock declarations (plain, parameterized, infinite), flow types (Rate, Conversion, Leak, implicit detection), formula expressions (references, arithmetic, parentheses, inf literal), and comment handling.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Translator (SystemsModel -> datamodel::Project)
**Goal:** Translate parsed SystemsModel into simlin datamodel using stdlib modules

**Components:**
- `src/simlin-engine/src/systems/translate.rs` -- translation logic: stock mapping, module instantiation, flow creation, chaining for multi-outflow, waste flow generation for Conversion, SimSpecs configuration
- `src/simlin-engine/src/compat.rs` -- add `open_systems(contents: &str) -> Result<Project>` entry point

**Dependencies:** Phase 1 (stdlib modules), Phase 2 (parser)

**Done when:** Each example file translates to a valid `datamodel::Project` that compiles without errors. Verify correct module instantiation (right model_name, correct input bindings), correct stock inflow/outflow wiring, correct chaining for multi-outflow stocks. Waste flows exist for Conversion and are outflows-only (no destination stock).
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Simulation Integration Tests
**Goal:** Verify simulation output matches the Python `systems` package

**Components:**
- `test/systems-format/examples/` -- copy valid example files from `third_party/systems/examples/` and add pre-generated CSV expected outputs
- `src/simlin-engine/tests/systems_format.rs` -- integration tests: parse -> translate -> compile -> simulate -> compare against CSV
- Python script to generate expected CSV outputs from the `systems` package

**Dependencies:** Phase 3 (translator)

**Done when:** All example fixtures produce simulation output matching Python-generated CSVs within floating-point tolerance. Both VM and interpreter paths cross-validated.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Systems Format Writer
**Goal:** Reconstruct `.txt` format from datamodel by reading module structure

**Components:**
- `src/simlin-engine/src/systems/writer.rs` -- module identification, chain walking, flow type reconstruction, waste flow stripping, infinite stock detection, `.txt` output generation
- `src/simlin-engine/src/compat.rs` -- add `to_systems(project: &Project) -> Result<String>` entry point

**Dependencies:** Phase 4 (working translator with test fixtures)

**Done when:** Round-trip tests pass: parse -> translate -> write -> parse -> translate -> simulate produces matching output for all example fixtures.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: ALLOCATE BY PRIORITY Builtin
**Goal:** Implement ALLOCATE BY PRIORITY as a first-class engine feature via AST rewrite

**Components:**
- `src/simlin-engine/src/builtins.rs` -- register `allocate_by_priority` as 5-arg builtin, add `BuiltinFn::AllocateByPriority` variant
- `src/simlin-engine/src/ast/expr1.rs` -- AST rewrite: AllocateByPriority -> AllocateAvailable with synthesized rectangular priority profiles
- `src/simlin-engine/src/mdl/xmile_compat.rs` -- simplify existing MDL translation to use new native support
- `src/simlin-engine/src/mdl/writer.rs` -- update `recognize_allocate()` to handle the native form

**Dependencies:** None (independent of Phases 1-5)

**Done when:** ALLOCATE BY PRIORITY works in XMILE equations (not just MDL import). Tests verify equivalence with ALLOCATE AVAILABLE using rectangular profiles for representative allocation scenarios. Existing MDL allocation tests continue to pass.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Module Diagram Generation
**Goal:** Extend the layout engine to include modules in auto-generated diagrams

**Components:**
- `src/simlin-engine/src/layout/mod.rs` -- remove module exclusions at 9 identified sites, add `create_missing_module_elements()`, include modules in SFDP graph, connectors, label placement, bounds calculation, and completeness validation
- `src/simlin-engine/src/layout/config.rs` -- add module dimensions to LayoutConfig
- `src/simlin-engine/src/layout/graph.rs` -- include module nodes in force-directed graph

**Dependencies:** Phase 3 (translator produces modules in datamodel)

**Done when:** Layout engine generates ViewElement::Module elements for all Variable::Module instances. Connectors are drawn between modules and the stocks/flows that reference them. Layout tests verify module placement for systems format models.
<!-- END_PHASE_7 -->

## Additional Considerations

**Waste flows without destination stocks:** The Conversion translation creates flows that are outflows from a stock but not inflows to any stock (the "waste" disappears). This is analogous to XMILE cloud flows. Verify early in Phase 1/3 that the compiler and VM handle this correctly -- if not, use a hidden waste stock as the destination.

**Formula operator precedence:** The Python `systems` package evaluates formulas strictly left-to-right with no operator precedence (`3 + 4 * 2` evaluates as `(3 + 4) * 2 = 14`, not `3 + 8 = 11`). Simlin's equation parser follows standard precedence. The translator must parenthesize systems formulas to preserve left-to-right semantics.

**Negative flow values:** The Python Leak implementation has no guard against negative rates. If `rate < 0`, `floor(src * rate)` produces a negative transfer (source gains, dest loses). While unlikely in practice, the stdlib module should reproduce this behavior for output fidelity.

**Default flow without rate:** The syntax `A > B` (no `@`) creates stocks but no flow in the Python implementation. The parser should handle this as a stock-only declaration.
