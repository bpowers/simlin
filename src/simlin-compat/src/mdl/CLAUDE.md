# Vensim MDL Parser Implementation

This directory contains the pure Rust implementation of a Vensim MDL file parser, replacing the C++ src/xmutil dependency.

## Current Status

**Phases 1-5 and 8 (Parsing): COMPLETE**
- Lexer, normalizer, parser, and AST types are fully implemented
- All equation types, expressions, subscripts, and macros parse correctly
- Comprehensive test coverage (527 tests)

**Phase 6 (Core Conversion): COMPLETE**
- `convert/`: Multi-pass AST to datamodel conversion fully implemented
- `xmile_compat.rs`: XMILE-compatible expression formatter with function renames,
  argument reordering, and name formatting
- Variable type detection (stocks via INTEG, flows, auxiliaries)
- Flow linking via is_all_plus_minus algorithm with synthetic net flow generation
- Sim specs extraction from control variables
- PurgeAFOEq logic for A FUNCTION OF placeholder handling
- Dimension building with range expansion and equivalence handling
- Number list / tabbed array conversion for arrayed equations
- Element-specific equation handling (single elements, apply-to-all with overrides, mixed subscripts)

**Phase 7 (Views/Diagrams): COMPLETE**
- `view/`: View/sketch parsing and datamodel conversion fully implemented
- Element parsing for all types (variables, valves, comments/clouds, connectors)
- Ghost/alias detection for duplicate variable appearances across views
- Built-in variable filtering (Time variable excluded from views)
- Flow point computation with stock inflow/outflow detection
- Angle calculation for arc connectors
- Coordinate transformation and multi-view composition
- Cloud detection and conversion
- 53+ tests for view parsing and conversion
- All tested models match xmutil output in element counts and types

**Phase 10 (Settings): COMPLETE**
- `settings.rs`: Post-equation parser for settings section
- Integration type parsing (type 15): Euler/RK2/RK4 method detection
- Unit equivalence parsing (type 22): name, equation, aliases

**Phase 9 (Groups): COMPLETE**
- Group marker parsing: `{**name**}` and `***\nname\n***|` formats
- Group hierarchy via parent tracking
- Variable assignment to groups (first equation only, not inside macros)
- Group name conflict resolution with symbol/dimension names

**Phase 9 (Post-processing): PARTIAL**
- Model post-processing (name deduplication) not implemented
- View composition for multi-view files is implemented

## Motivation

**Problems being solved:**
1. **Build complexity**: The C++ xmutil requires Bison/Flex, a C++ toolchain, and complex cross-compilation setup
2. **WASM compatibility**: Cannot easily include xmutil in WASM builds today.  would require a large wasi build dependency

## Architecture

### Target: Vensim MDL → datamodel (directly)

```
Vensim MDL file
    ↓
Rust lexer (mdl/lexer.rs)
    ↓
Hand-written recursive descent parser (mdl/parser.rs)
    ↓
minimal internal representations private to this package (for e.g. vensim equation AST, vensim diagram)
    ↓
simlin_core::datamodel::Project  ← target output
    ↓
(optional) simlin_compat::xmile::project_to_xmile()  ← "free" XMILE export
```

We deliberately skip the XMILE intermediate representation. By targeting `simlin_core::datamodel` directly:
- We can leverage the existing XMILE conversion functions in simlin-compat for free
- We avoid double-parsing (MDL → XMILE XML string → parse XMILE → datamodel)
- We can extend the datamodel if needed for Vensim-specific features

### Intermediate Structures

Some Vensim concepts require intermediate representations before conversion to datamodel:

1. **View/Diagram data**: Vensim's sketch format uses element indices, relative positioning, and multiple views that must be resolved before converting to absolute positions in a single view in the datamodel. These intermediate structures will live in `mdl/view.rs` (not yet implemented).

2. **Symbol table**: During parsing, we need to track variable definitions, subscript ranges, and macros before final resolution.

### Module Structure

```
src/simlin-compat/src/mdl/
├── CLAUDE.md          # This file - project context and goals
├── mod.rs             # Public exports: parse_mdl() function, re-exports
├── lexer.rs           # Hand-written RawLexer for MDL tokens (context-free)
├── normalizer.rs      # TokenNormalizer for context-sensitive transformations
├── parser.rs          # Hand-written recursive descent parser (replaced LALRPOP grammar)
├── ast.rs             # AST types produced by parser
├── reader.rs          # EquationReader: drives parser, captures comments, handles macros
├── builtins.rs        # Vensim built-in function recognition via to_lower_space()
├── convert/           # AST → datamodel conversion (IMPLEMENTED)
│   ├── mod.rs         # Main conversion logic, group building
│   ├── dimensions.rs  # Dimension/subscript building
│   ├── helpers.rs     # Utility functions (units, expressions)
│   ├── stocks.rs      # Stock/flow linking
│   ├── types.rs       # Internal types (SymbolInfo, etc.)
│   └── variables.rs   # Variable type detection and building
├── view/              # View/sketch parsing and conversion (IMPLEMENTED)
│   ├── mod.rs         # Main parsing logic: parse_views() function
│   ├── types.rs       # View types: VensimView, VensimElement, ViewError
│   ├── elements.rs    # Element line parsing (types 1, 10, 11, 12)
│   ├── convert.rs     # VensimView → datamodel::View conversion
│   └── processing.rs  # Coordinate transforms, angle calculation, flow points
├── xmile_compat.rs    # XMILE-compatible expression formatter (IMPLEMENTED)
└── settings.rs        # Settings section parser (integration type, unit equivalences)
```

## Reference Implementation

The C++ implementation we're replacing lives in:
- `src/xmutil/third_party/xmutil/` (~15,000 lines total)

Key files to reference:
- `Vensim/VYacc.y` - Bison grammar (266 lines) - **primary reference for parser**
- `Vensim/VensimLex.cpp` - Lexer
- `Vensim/VensimParse.cpp` - Parser driver and semantic actions
- `Vensim/VensimView.cpp` - View/sketch parsing (~450 lines)
- `Symbol/Variable.cpp`, `Equation.cpp`, `Expression.cpp` - Data structures
- `Xmile/XMILEGenerator.cpp` - XMILE output (useful for understanding transformations)

## Vensim MDL Format Features (100% compatibility required)

All features implemented in xmutil must be supported. This checklist is organized by implementation phase and component.

---

## Phase 1: Lexer (`lexer.rs`)

### Token Types
- [x] Numbers: integers, floats, scientific notation (e.g., `1e-6`, `1.5E+3`)
- [x] Strings/Symbols: variable names (can contain spaces, underscores)
- [x] Quoted strings with escape sequences (`\"` inside quotes)
- [x] Operators: `+ - * / ^ < > = ( ) [ ] , ; : |`
- [x] Compound operators: `:=` (data equals), `<=`, `>=`, `<>`
- [x] Keywords: `:AND:`, `:OR:`, `:NOT:`, `:NA:`
- [x] Special keywords: `:MACRO:`, `:END OF MACRO:`
- [x] Interpolation modes: `:INTERPOLATE:`, `:RAW:`, `:HOLD BACKWARD:`, `:LOOK FORWARD:`
- [x] Exception keyword: `:EXCEPT:`
- [x] Equivalence: `<->`
- [x] Map arrow: `->`
- [x] Comment terminators: `~` and `|`
- [x] Bang subscript modifier: `!`
- [x] End token: `\\\\\\---///` (end of equations section)
- [x] Group markers: `{**name**}` and `***name***|` formats via `GroupStar` token

### Lexer State Management
- [x] Track position for error messages
- [x] Handle multi-line tokens (line continuation with `\` at EOL)
- [x] Skip whitespace appropriately
- [x] Handle nested comments (`{ { nested } }`)
- [x] Comment extraction handled by EquationReader (text between second `~` and `|`)

### Known Limitations (Low Priority)
- EOF without explicit EqEnd marker: When the input ends without `\\\---///`, the reader simply returns `None` rather than synthesizing an EqEnd. This is acceptable for well-formed files but could mask truncated input issues. Consider synthesizing EqEnd on EOF for better error reporting in the future.

---

## Phase 2: AST Types (`ast.rs`)

### Expression AST
- [x] `ExpressionNumber` - `Expr::Const(f64, Loc)`
- [x] `ExpressionVariable` - `Expr::Var(name, subscripts, Loc)`
- [x] `ExpressionOperator` - `Expr::Op2(BinaryOp, ...)` for binary, `Expr::Op1(UnaryOp, ...)` for unary
- [x] `ExpressionLogical` - `BinaryOp::And`, `BinaryOp::Or`, `UnaryOp::Not`
- [x] `ExpressionFunction` - `Expr::App(name, subscripts, args, CallKind, Loc)`
- [x] `ExpressionFunctionMemory` - same as above (memory functions are function calls)
- [x] `ExpressionLookup` - `Expr::App` with `CallKind::Symbol` for lookup invocations
- [x] `ExpressionTable` - `LookupTable` struct with x_vals, y_vals, ranges, format
- [x] `ExpressionLiteral` - `Expr::Literal(Cow<str>, Loc)`
- [x] `ExpressionParen` - `Expr::Paren(Box<Expr>, Loc)`
- [x] `ExpressionSymbolList` - handled via `SubscriptDef` for dimension definitions
- [x] `ExpressionNumberTable` - `Equation::TabbedArray` and `Equation::NumberList` (number lists support constants, unary minus, and `:NA:` - NOT unary plus or expressions, matching xmutil)

### Left-Hand Side
- [x] Variable with optional subscript list - `Lhs { name, subscripts, ... }`
- [x] Exception list (`:EXCEPT:` clauses) - `ExceptList { subscripts, loc }`
- [x] Interpolation mode specification - `InterpMode` enum

### Equation Types
- [x] Regular equation: `Equation::Regular(Lhs, Expr)`
- [x] Empty RHS: `Equation::EmptyRhs(Lhs, Loc)`
- [x] Lookup definition: `Equation::Lookup(Lhs, LookupTable)`
- [x] WITH LOOKUP: `Equation::WithLookup(Lhs, Box<Expr>, LookupTable)`
- [x] Data equation: `Equation::Data(Lhs, Option<Expr>)`
- [x] Subscript/dimension definition: `Equation::SubscriptDef(name, SubscriptDef)`
- [x] Equivalence: `Equation::Equivalence(name1, name2, Loc)`
- [x] Tabbed array: `Equation::TabbedArray(Lhs, Vec<f64>)`
- [x] Number list: `Equation::NumberList(Lhs, Vec<f64>)`
- [x] Implicit: `Equation::Implicit(Lhs)` for exogenous data

### Symbol Lists
- [x] Simple list: `SymList` in parser produces `Vec<Subscript>`
- [x] Bang-marked elements: `Subscript::BangElement(name, Loc)`
- [x] Ranges: `SubscriptElement::Range(start, end, Loc)`
- [x] Mapping lists: `SubscriptMapping { entries: Vec<MappingEntry>, ... }`

---

## Phase 3: Parser (`parser.lalrpop`)

### Grammar Rules (from VYacc.y)
- [x] `fulleq`: `FullEqWithUnits` rule handles equation + units + section end detection
- [x] `eqn`: `Eqn` rule with all equation variants
- [x] `lhs`: `Lhs` rule with except/interp support
- [x] `var`: `Var` rule returning `(name, subscripts)`
- [x] `sublist`: `SubList` rule producing `Vec<Subscript>`
- [x] `symlist`: `SymList` rule for comma-separated symbols with optional bang
- [x] `subdef`: `SubDef` rule with range support `(start - end)`
- [x] `exceptlist`: `ExceptList` rule for `:EXCEPT:` clauses
- [x] `mapsymlist`: `MapSymList` rule for dimension mappings
- [x] `maplist`: `MapList` rule for optional `-> mapping` clause
- [x] `exprlist`: `ExprList` rule with comma/semicolon support and trailing semicolon
- [x] `exp`: Full expression grammar with `AddSub`, `LogicOr`, `Cmp`, `LogicAnd`, `MulDiv`, `Unary`, `Power`, `Atom`
- [x] `tablevals`: `TableVals` rule for pairs format with optional range
- [x] `xytablevals`: `XYTableVals` rule for legacy format
- [x] `tablepairs`: `TablePairs` rule for `(x,y)` pairs
- [x] `units`: `UnitExpr` and `UnitTerm` rules for unit expressions
- [x] `unitsrange`: `UnitsRange` rule with 2 or 3 element ranges and `?` support
- [x] `macrostart`: Handled in `FullEqWithUnits` returning `SectionEnd::MacroStart`
- [x] `macroend`: Handled in `FullEqWithUnits` returning `SectionEnd::MacroEnd`

### Operator Precedence (low to high) - All Implemented
1. `- +` (addition/subtraction) - `AddSub` rule
2. `:OR:` - `LogicOr` rule
3. `= < > <= >= <>` - `Cmp` rule
4. `:AND:` - `LogicAnd` rule
5. `* /` - `MulDiv` rule
6. `:NOT:`, unary `+`, `-` - `Unary` rule
7. `^` (right-associative) - `Power` rule

### Table Formats
- [x] Pairs format: `TableVals` with optional `[(xmin,ymin)-(xmax,ymax)]` prefix
- [x] Pairs with embedded range: `TableVals` handles inner pairs (ignored per xmutil)
- [x] XY vector format (legacy): `XYTableVals` with `transform_legacy()` conversion

---

## Phase 4: Built-in Functions (`builtins.rs`)

Function recognition is implemented via `is_builtin()` which uses `to_lower_space()` canonicalization.
All functions below are recognized and emit `Token::Function` during normalization.

### Mathematical Functions
- [x] `ABS`, `EXP`, `SQRT`, `LN`, `LOG` (LOG is log10)
- [x] `SIN`, `COS`, `TAN`, `ARCSIN`, `ARCCOS`, `ARCTAN`
- [x] `MIN`, `MAX`
- [x] `INTEGER` (truncate)
- [x] `MODULO`
- [x] `QUANTUM` (round to increment)

### Conditional/Logical
- [x] `IF THEN ELSE(condition, true_val, false_val)`
- [x] `ZIDZ(num, denom)` - zero if divide by zero
- [x] `XIDZ(num, denom, x)` - x if divide by zero

### Time Functions
- [x] `PULSE(start, width)`
- [x] `PULSE TRAIN(start, width, interval, end)`
- [x] `STEP(height, time)`
- [x] `RAMP(slope, start, end)`

### Delay/Smooth Functions (Stateful)
- [x] `SMOOTH(input, delay_time)`
- [x] `SMOOTHI(input, delay_time, initial)`
- [x] `SMOOTH3(input, delay_time)`
- [x] `SMOOTH3I(input, delay_time, initial)`
- [x] `SMOOTH N(input, delay_time, order)` (note: SMOOTH N not SMOOTHN)
- [x] `DELAY1(input, delay_time)`
- [x] `DELAY1I(input, delay_time, initial)`
- [x] `DELAY3(input, delay_time)`
- [x] `DELAY3I(input, delay_time, initial)`
- [x] `DELAY FIXED(input, delay_time, initial)`
- [x] `DELAY N(input, delay_time, initial, order)`
- [x] `DELAY CONVEYOR(input, delay_time, initial)`
- [x] `TREND(input, avg_time, initial)`
- [x] `FORECAST(input, avg_time, horizon)`

### Lookup Functions
- [x] `WITH LOOKUP(input, table)` - special `Token::WithLookup` via `is_with_lookup()`
- [x] `LOOKUP INVERT(lookup_var, value)`
- [x] `LOOKUP AREA(lookup_var, x1, x2)`
- [x] `LOOKUP EXTRAPOLATE(lookup_var, x)` - extrapolates at call time (does NOT mark table)
- [x] `LOOKUP FORWARD(lookup_var, x)`
- [x] `LOOKUP BACKWARD(lookup_var, x)`
- [x] `TABXL(lookup_var, x)` - marks table as extrapolating
- [x] `GET DATA AT TIME(data_var, time)`
- [x] `GET DATA LAST TIME(data_var)`

### Array Functions
- [x] `SUM(array)` - sum across subscripts
- [x] `PROD(array)` - product across subscripts
- [x] `VMAX(array)` - vector max
- [x] `VMIN(array)` - vector min
- [x] `ELMCOUNT(dimension)` - element count
- [x] `VECTOR SELECT(selection, sel_values, index_dim, missing_val, action)`
- [x] `VECTOR ELM MAP(vector, index)`
- [x] `VECTOR SORT ORDER(vector, direction)`
- [x] `VECTOR REORDER(vector, order)`
- [x] `VECTOR LOOKUP(vector, index, missing)`

### Integration/State
- [x] `INTEG(rate, initial)` - stock integration
- [x] `ACTIVE INITIAL(equation, initial)` - with separate init
- [x] `INITIAL(value)` - value at initialization time
- [x] `REINITIAL(initial, condition)` - reinitialize on condition
- [x] `SAMPLE IF TRUE(condition, input, initial)` - conditional sampling

### Random Functions
- [x] `RANDOM 0 1()` - uniform [0,1]
- [x] `RANDOM UNIFORM(min, max, seed)`
- [x] `RANDOM NORMAL(min, max, mean, stddev, seed)`
- [x] `RANDOM POISSON(mean, seed)` (listed as RANDOM POISSON)
- [x] `RANDOM PINK NOISE(mean, stddev, seed)`

### Special
- [x] `NA` - `:NA:` token handled by lexer as `Token::Na`, parsed as `Expr::Na`
- [x] `A FUNCTION OF` - recognized as builtin (used for placeholder equations)
- [x] `GAME(input)` - gaming/interactive input
- [x] `TIME BASE` - time as number
- [x] `GET DIRECT DATA(...)` - external data (also handled as GET XLS placeholder)
- [x] `GET DATA MEAN(...)` - data statistics
- [x] `NPV(...)` - net present value
- [x] `ALLOCATE BY PRIORITY(...)`
- [x] `TABBED ARRAY` - special handling via `is_tabbed_array()` and `Token::TabbedArray`

### GET XLS/VDF Functions (Special Handling)
- [x] `GET XLS(...)` - converted to `{GET XLS...}` placeholder symbol
- [x] `GET VDF(...)` - converted to `{GET VDF...}` placeholder symbol
- [x] `GET 123(...)` - converted to placeholder
- [x] `GET DATA(...)` - converted to placeholder
- [x] `GET DIRECT(...)` - converted to placeholder

---

## Phase 5: Subscript/Array Handling

### Dimension Definitions (Parsing Complete)
- [x] Simple dimension: `DimA: elem1, elem2, elem3` - `Equation::SubscriptDef`
- [x] Numeric range dimension: `DimA: (A1-A10)` - `SubscriptElement::Range`
- [x] Subrange definition: `SubA: elem1, elem2 -> ParentDim` - with `SubscriptMapping`
- [x] Dimension mapping: `DimA: A1, A2, A3 -> DimB` - `MappingEntry::Name`
- [x] Explicit mapping: `DimA: A1, A2, A3 -> (DimB: B1, B2, B3)` - `MappingEntry::DimensionMapping`
- [x] Equivalence: `DimA <-> DimB` - `Equation::Equivalence`

### Subscripted Equations (Parsing Complete)
- [x] Apply-to-all: `Var[DimA] = expr` - parsed with `Lhs.subscripts`
- [x] Element-specific: `Var[elem1] = expr1` - same representation
- [x] Exception-based: `Var[DimA] :EXCEPT: [elem1, elem2] = expr` - `Lhs.except`
- [x] Multi-dimensional: `Var[DimA, DimB] = expr` - multiple subscripts
- [x] Mixed indexing: `Var[elem1, DimB]` - parsed correctly

### Conversion-Phase Requirements (Implemented in convert.rs)
- [x] **Subscript range expansion**: `(A1-A10)` ranges expanded to individual elements via `expand_range()` function which validates prefix matching and numeric suffix extraction.
- [x] **Implicit TIME lookup**: Bare LHS equations (`exogenous data ~ ~ |`) parsed as `Equation::Implicit` are converted to lookup tables over TIME with default `(0,1),(1,1)` table via `make_default_lookup()`.
- [x] **Range-only units normalization**: When units have only a range like `[0, 100]` without an explicit unit expression, the `Units.expr` is `None`. During conversion, this is treated as dimensionless ("1") rather than truly having no units.

### Bang Notation
- [x] `[dim!]` parsed as `Subscript::BangElement`
- [x] Used in `:EXCEPT:` clauses via `ExceptList`

---

## Phase 6: Core Conversion (`convert/`)

### Variable Type Detection
- [x] Stock detection: has top-level INTEG() in equation via `is_top_level_integ()`
- [x] Flow detection: identified from INTEG() rate expressions via `collect_flows()`
- [x] Auxiliary: everything else (non-stock, non-flow)
- [x] Mark inflows/outflows on stocks in `link_stocks_and_flows()`

### Equation Processing
- [x] `mark_variable_types()` - auto-detect variable types from equations
- [x] `link_stocks_and_flows()` - identify and link flows to stocks using is_all_plus_minus algorithm
  - Collects flow lists from ALL valid stock equations (not just first)
  - Synthesizes net flow only when flow lists differ or decomposition fails
- [x] `select_equation()` - PurgeAFOEq logic: remove "A FUNCTION OF" placeholders and empty RHS equations

### Special Variable Handling
- [x] `INITIAL TIME` → sim_specs.start
- [x] `FINAL TIME` → sim_specs.stop
- [x] `TIME STEP` → sim_specs.dt
- [x] `SAVEPER` → sim_specs.save_step
- [x] Mark these as "unwanted" (don't emit as regular variables)
- [x] Extract time units from TIME STEP or FINAL TIME

### Synthetic Flow Generation
- [x] Generate net flow variables for stocks with non-decomposable rates (constants, expressions)
- [x] Collision avoidance via suffix numbering

### XMILE-Compatible Expression Formatting (`xmile_compat.rs`)
- [x] Function renames: IF THEN ELSE, LOG→LOG10/LN, ELMCOUNT→SIZE, ZIDZ/XIDZ→SAFEDIV
- [x] Argument reordering: DELAY N, SMOOTH N, RANDOM NORMAL
- [x] Name formatting: spaces to underscores, TIME→STARTTIME mappings
- [x] Special transformations: PULSE, PULSE TRAIN, QUANTUM, SAMPLE IF TRUE, ALLOCATE BY PRIORITY
- [x] Lookup invocation: Symbol calls become LOOKUP(table, input)

### Dimension Building
- [x] Build dimensions from subscript definitions
- [x] Handle range elements via `expand_range()`
- [x] Element ownership tracking (larger dimension owns elements)
- [x] Dimension equivalences (`<->`) create alias dimensions with maps_to
- [x] Cartesian product for multi-dimensional number lists

### TABXL Detection
- [x] Scan expressions for TABXL usage (LOOKUP EXTRAPOLATE does NOT mark tables)
- [x] Set GraphicalFunctionKind::Extrapolate for lookups referenced by TABXL

### Integration Type (Phase 10 - COMPLETE)
- [x] Parse from settings section (type code 15)
- [x] Map: 0,2 → Euler, 1,5 → RK4, 3,4 → RK2 (RK2 maps to Euler since SimMethod doesn't have RK2)

### Unit Equivalences (Phase 10 - COMPLETE)
- [x] Parse from settings section (type code 22)
- [x] Format: `Dollar,$,Dollars,$s`

### Groups (COMPLETE)
- [x] Parse group markers `{**name**}` and `***\nname\n***|` during equation parsing
- [x] Maintain group hierarchy (nested groups via parent index)
- [x] `AdjustGroupNames()` - ensure unique group names that don't conflict with symbols
- [x] Assign variables to groups (on first equation only, not inside macros)

---

## Phase 7: Views/Diagrams (`view/`)

### Sketch Section Parsing

The sketch section follows the equation section, starting with `\\\\\\---///`.

#### View Header
- [x] Parse view marker: `\\\\\\---///` - `skip_to_sketch_start()` in `mod.rs`
- [x] Parse version line: `V300 ` or `V364 ` prefix - `parse_version()` in `mod.rs`
- [x] Parse view title: line starting with `*` (e.g., `*View 1`) - `parse_all()` in `mod.rs`
- [x] Parse font line: pipe-separated values (skipped after recognition)
- [x] Extract x/y scaling ratios from font line (default 1.0 used)

#### Element Types (identified by first integer)
- [x] Type 10: Variable element - `parse_variable()` in `elements.rs`
- [x] Type 11: Valve element - `parse_valve()` in `elements.rs`
- [x] Type 12: Comment/cloud element - `parse_comment()` in `elements.rs`
- [x] Type 1: Connector element - `parse_connector()` in `elements.rs`
- [x] Type 30: Unknown/ignored - handled in `parse_element_line()`

#### Variable Element (Type 10) Parsing
- [x] Parse: `10,uid,name,x,y,width,height,shape,bits,...`
- [x] Name: either variable name or numeric reference
- [x] Position: x, y coordinates (center of element)
- [x] Dimensions: width, height
- [x] Shape flags: bit 5 (0x20) = attached to valve (flow indicator)
- [x] Bits flags: bit 0 = 0 means ghost, bit 0 = 1 means primary definition
- [x] Track ghost vs primary definition status via `is_ghost` field

#### Valve Element (Type 11) Parsing
- [x] Parse: `11,uid,name,x,y,width,height,shape,...`
- [x] Shape bit 5: attached flag
- [x] Valve position defines flow location in diagram

#### Comment Element (Type 12) Parsing
- [x] Parse: `12,uid,text,x,y,width,height,shape,bits,...`
- [x] Includes clouds (boundary markers) and text annotations
- [x] Bits bit 2: if set, actual text is on the next line (scratch_name)
- [x] Scratch name handling in `parse_elements()` consumes next line for text

#### Connector Element (Type 1) Parsing
- [x] Parse: `1,uid,from,to,ignore,ignore,polarity,...,npoints|(x,y),...`
- [x] From/To: UIDs of connected elements
- [x] Polarity: ASCII code for '+', '-', 'S' (same), 'O'/'0' (opposite)
- [x] Letter polarity: 'S'/'s' → '+', 'O'/'0' → '-' via `parse_polarity()`
- [x] Control points: `npoints|(x,y)` format via `parse_points()`

### View Processing Logic

#### Ghost Variable Handling
- [x] First appearance of variable in any view becomes primary definition
- [x] Subsequent appearances become ghosts
- [x] `associate_variables()` in `processing.rs` tracks primary definitions

#### Variable-View Association
- [x] `PrimaryMap` tracks which view contains primary definition
- [x] Only non-ghost elements set the variable's view
- [x] Flow attachment: detected via `attached` field on VensimVariable

#### Connector Validation
- [x] Handle valve indirection: connector to valve → use flow var via `convert_connector()`
- [x] Skip connectors to stocks (flow connections handled separately)
- [x] Skip connectors involving clouds (handled as flow endpoints)

#### Flow Definition Placement
- [x] Flow position computed from valve if attached
- [x] Default fallback if no endpoints found: extend 150 units

#### Straggler Attachment
- [x] Default flow endpoints (150 units) for unconnected flows

### Coordinate Transformation

#### Scaling
- [x] `transform_view_coordinates()`: transform all element coordinates
- [x] Find minimum x, y across all elements via `min_x()`, `min_y()`
- [x] Apply offset to shift origin
- [x] Apply scale ratios: `new_coord = old_coord * ratio + offset`
- [x] Scale width/height by same ratios

#### View Composition
- [x] `compose_views()`: stack multiple views vertically with 80px gap
- [x] Track UID offsets per view for element references
- [x] `max_x()` / `max_y()`: compute view bounds

### Datamodel View Generation

#### View Element Output
- [x] Auxiliary (`ViewElement::Aux`): x, y position
- [x] Stock (`ViewElement::Stock`): x, y position
- [x] Flow (`ViewElement::Flow`): x, y from valve, plus FlowPoints for endpoints
- [x] Ghost/Alias (`ViewElement::Alias`): x, y, with alias_of_uid

#### Flow Pipe Points
- [x] `compute_flow_points()` in `processing.rs`
- [x] Search connectors from valve to find connected stocks/clouds
- [x] Determine inflow vs outflow by checking stock's inflow/outflow lists in SymbolInfo
- [x] Set pipe endpoints based on stock positions
- [x] Default if not connected: extend 150 units in default direction

#### Cloud Output
- [x] `is_cloud_endpoint()` detects comments used as flow endpoints
- [x] `ViewElement::Cloud` with flow_uid reference

#### Connector/Link Output
- [x] `ViewElement::Link` with from_uid, to_uid, shape
- [x] Calculate angle using `angle_from_points()` in `processing.rs`
- [x] Shape: `Straight` or `Arc(angle)` based on control point

#### Angle Calculation
- [x] Three-point arc calculation for curved connectors
- [x] Find circle center from perpendicular bisectors
- [x] Calculate tangent angle at start point
- [x] Fall back to straight-line angle if geometry fails
- [x] Handle degenerate cases (vertical/horizontal lines)
- [x] `xmile_angle_to_canvas()` / `canvas_angle_to_xmile()` conversions

#### Sector/Group Output
- [x] Multiple views → wrap in `ViewElement::Group` elements
- [x] Group attributes: name, x, y, width, height via `create_sector_group()`
- [x] `merge_views()` combines all elements into single StockFlow view

---

## Phase 8: Macros

### Macro Parsing (Complete)
- [x] Detect `:MACRO: name(args)` start - `Token::Macro` + `SectionEnd::MacroStart`
- [x] Track macro state in `EquationReader` - `MacroState::InMacro`
- [x] Parse equations within macro body - accumulated in `equations` vector
- [x] Detect `:END OF MACRO:` end - `Token::EndOfMacro` + `SectionEnd::MacroEnd`
- [x] Store macro definition - `MacroDef { name, args, equations, loc }`
- [x] Return as `MdlItem::Macro(Box<MacroDef>)`

### Macro Output (Conversion Phase - Not Yet Implemented)
- [ ] Generate `<macro name="...">` element
- [ ] `<eqn>` with macro name
- [ ] `<parm>` elements for each argument
- [ ] Nested model content from macro equations

---

## Phase 9: Model Post-Processing

### Groups/Sectors (COMPLETE)
- [x] Parse group markers `{**name**}` and `***\nname\n***|` during equation parsing
- [x] Maintain group hierarchy (nested groups via parent index)
- [x] xmutil's numeric-leading owner logic (starts with digit + differs from previous)
- [x] `AdjustGroupNames()` - ensure unique group names that don't conflict with symbols/dimensions
- [x] Assign variables to groups (on first equation only, not inside macros)
- [x] Exclude control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER)
- [x] Exclude subscript and equivalence definitions
- [x] Groups persist through XMILE roundtrips

### Name Processing (NOT STARTED)
- [ ] `SpaceToUnderBar()`: replace spaces with underscores
- [ ] `QuotedSpaceToUnderBar()`: add quotes if contains periods
- [ ] `MakeViewNamesUnique()`: deduplicate view titles
- [ ] Remove special chars from view names: `. - + , / * ^`
- [ ] Long name mode: use compressed comment as variable name

### Variable Filtering (NOT STARTED)
- [ ] Skip "Time" variable in views (handled by XMILE runtime)
- [ ] Skip ARRAY and ARRAY_ELM types in views
- [ ] Skip UNKNOWN type variables
- [ ] Filter "unwanted" variables (sim spec vars)

---

## Phase 10: Settings Section Parsing - COMPLETE

Located after views, starting with `///---\\\` marker.

### Settings Marker
- [x] Parse `///---\\\` section start (handled by finding `:L<%^E!@` block marker)
- [x] Parse `:L\177<%^E!@` settings block marker (supports optional \x7F character)

### Setting Types (first integer in colon-delimited line)
- [x] Type 15: Integration type (4th comma-separated value: 0,2=Euler, 1,5=RK4, 3,4=RK2)
- [x] Type 22: Unit equivalence strings (comma-separated: name, aliases, $ for equation)

---

## Future: Module-Style View Splitting

The current `merge_views` approach combines all views into a single StockFlow view
with group wrappers. This works for flat Vensim models but will need enhancement
for module/level-structured models. Key considerations:

- **Ghost(adds) parameter**: xmutil's `vele->Ghost(adds)` determines cross-level
  references. The `adds` parameter tracks which variables are "added" to a view
  from another module/level.
- **Cross-level connector handling**: xmutil's XMILEGenerator.cpp:910-960 has
  Ghost logic with `adds` set, which handles connectors that cross module boundaries.
- This is relevant when Vensim models use module/level structure that maps to
  XMILE submodels. Currently, all views are merged into a single flat view.

---

## Testing Strategy Updates

### Unit Test Coverage
- [x] Lexer: Each token type, edge cases, error recovery (29+ tests in `lexer.rs`)
- [x] Normalizer: Section state, function classification, TABBED ARRAY, GET XLS (30+ tests in `normalizer.rs`)
- [x] Builtins: Canonicalization, function recognition (15+ tests in `builtins.rs`)
- [x] Parser helpers: Number parsing, equation creation (15+ tests in `parser_helpers.rs`)
- [x] Reader: Full equation parsing, comments, macros, all equation types (60+ tests in `reader.rs`)
- [x] Convert: Stock/flow detection, PurgeAFOEq, synthetic flows, arrayed equations, groups (55+ tests in `convert/`)
- [x] XMILE compat: Function renames, argument reordering, name formatting (30+ tests in `xmile_compat.rs`)
- [x] Settings: Integration method, unit equivalences, line endings (44+ tests in `settings.rs`)
- [x] Views: Element parsing, coordinate transforms, angle calculation, flow points (51+ tests in `view/`)

### Integration Test Coverage (`test_equivalence.rs`)
- [x] SIR.mdl: Verifies stock/flow linking, sim specs extraction
- [x] Simple inline models: Basic variable types, control var filtering

### Integration Test Approach
```rust
fn test_model_equivalence(mdl_path: &str) {
    let old_project = old_open_vensim(mdl_path);  // via xmutil
    let new_project = mdl::parse_mdl(mdl_path);   // new impl

    // Compare core structures
    assert_eq!(old_project.models, new_project.models);
    assert_eq!(old_project.dimensions, new_project.dimensions);
    // View comparison may need fuzzy matching for coordinates
}
```

### Test Models
- Simple models: basic stocks, flows, aux
- Array models: subscripts, mappings, exceptions
- Lookup models: various table formats
- Large models: real-world complexity
- View-heavy models: multiple views, ghosts, connectors

### Test Corpus
- Use existing test models in `test/` directory
- Consider collecting real-world Vensim models for edge cases

---

## Implementation Order

Implementation proceeds through the phases detailed above. Here's a summary with dependencies:

### Core Foundation (Phases 1-3) - COMPLETE
1. **Lexer** (`lexer.rs`) - DONE: RawLexer with context-free tokenization
2. **Normalizer** (`normalizer.rs`) - DONE: TokenNormalizer for context-sensitive transforms
3. **AST types** (`ast.rs`) - DONE: All node types defined
4. **Parser** (`parser.lalrpop`) - DONE: Full grammar with all equation types
5. **Reader** (`reader.rs`) - DONE: EquationReader for comment capture and macro assembly

### Semantic Processing (Phases 4-6) - COMPLETE
4. **Built-ins** (`builtins.rs`) - DONE: Function recognition via to_lower_space()
5. **Subscripts** - DONE: Parsing integrated into parser, range expansion in convert.rs
6. **Conversion** (`convert/`) - DONE: Multi-pass AST → datamodel transformation
7. **XMILE Formatting** (`xmile_compat.rs`) - DONE: Expression formatting with function transformations

### Visual Layer (Phase 7)
7. **Views** (`view/`) - DONE: View/diagram parsing and datamodel conversion

### Advanced Features (Phases 8-10)
8. **Macros** - DONE (parsing): MacroDef captured, output not implemented
9. **Groups** - DONE: Group markers, hierarchy, variable assignment, conflict resolution
10. **Settings parsing** - DONE: Integration type (Euler/RK2/RK4), unit equivalences
11. **Model post-processing** - PARTIAL: View composition done, name normalization not implemented

### Next Steps
1. Fix remaining 26 C-LEARN equivalence differences (see "C-LEARN Equivalence Analysis" below)
2. Implement macro output in datamodel format
3. Test against full model corpus for equivalence with xmutil
4. Consider removing xmutil C++ dependency once equivalence is verified

## Extending the Datamodel

If Vensim has features that don't map cleanly to the current datamodel, we can extend `simlin_core::datamodel`.
Our goal here is for the overall Simlin project to be as simple, maintainable, and full featured as possible, improving or generalizing the datamodel or other parts of Simlin are great ways to approach this problem.

Any extensions should be discussed and designed carefully to maintain clean abstractions.

## Lessons from Go's C-to-Go Migrations

1. **Test-driven correctness**: Maintain passing tests throughout migration
2. **Don't change semantics during translation**: Behavior changes come in separate commits

## Session Continuity

This is a multi-session project. When resuming work:
1. Check the "Current Status" section at the top for high-level progress
2. Run existing tests to verify nothing regressed: `cargo test -p simlin-compat mdl::`
3. Run equivalence tests to verify view element counts match: `cargo test -p simlin-compat --features xmutil test_mdl_equivalence -- --nocapture`
4. Run the C-LEARN equivalence test: `cargo test -p simlin-compat --features xmutil test_clearn_equivalence -- --ignored --nocapture`
5. Next priority: Fix C-LEARN equivalence differences (see "C-LEARN Equivalence Analysis" below)
6. Update the checklist and equivalence analysis as fixes are completed

## Commands

```bash
# Run all tests
cargo test -p simlin-compat

# Run MDL-specific tests
cargo test -p simlin-compat mdl::

# Run a specific test
RUST_BACKTRACE=1 cargo test -p simlin-compat mdl::lexer::tests::test_name

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy -p simlin-compat
```

## Known Deficiencies

- **Empty equation placeholder `0+0`**: When a Vensim equation has an empty RHS
  (e.g., `x = ~ ~ |`), we emit `0+0` as the equation string. This is a
  compatibility shim matching xmutil's behavior. We should eventually investigate
  why these equations are empty in the source model and handle them more
  meaningfully (e.g., treating them as data variables or flagging them as errors).

---

## Error Handling & Compatibility Goals

- **No panics across FFI/WASM boundaries**: Treat every invariant failure as a
  `Result` error, never a `panic!`/`unwrap` in production paths. Add regression
  tests for those failure cases so behavior is explicit and stable.
- **Invalid MDL input**: Strive to collect and report *multiple* errors rather
  than failing fast on the first one (within reason for parser architecture).
- **Error detail**: We do **not** need rich line/column diagnostics beyond what
  we already track; MDL files are generated and rarely hand-edited.
- **Preserve xmutil permissive fallbacks**: Keep xmutil-compatible behavior
  (atoi semantics, empty-equation shims, implicit defaults, etc.), and **document
  each fallback** where it occurs. These are often xmutil shortcomings; the
  long-term goal is full-fidelity translation that can eventually remove shims
  like `0+0` for empty equations.

## Panic/Unwrap Reduction: Findings (Jan 2026)

These were the primary production-path panic/unwrap risks discovered in the MDL
parser/converter. All have been fixed. (Most unwraps in this package are in unit
tests.)

1. **Tabbed array parsing** (`normalizer.rs`): FIXED -- `parse_number_token()`
   returns `Result` instead of panicking.
2. **Number parsing helper** (`parser_helpers.rs`): FIXED -- returns `Result`.
3. **View parsing** (`view/mod.rs`): FIXED -- uses
   `ok_or(ViewError::UnexpectedEndOfInput)` instead of `unwrap()`.
4. **Normalizer invariant** (`normalizer.rs`): FIXED -- unreachable newline
   case now returns `Ok(None)` instead of panicking.
5. **Invariant unwraps in conversion/view processing**: FIXED --
   - `lexer.rs`: KEYWORDS table stores first char explicitly (no `unwrap()`);
     `bump_n` uses `debug_assert!` instead of `assert!`.
   - `convert/stocks.rs`: uses `match`/`continue` instead of `unwrap()`.
   - `convert/variables.rs`: returns `Option` directly (no `unwrap()`).
   - `view/processing.rs`: uses `match` to extract `to_index` with early return.

Production code is now free of input-reachable panics. The full error type
hierarchy (`LexError -> NormalizerError -> ReaderError -> ConvertError`, plus
`ViewError`) implements `Display` and `std::error::Error` with proper `source()`
chaining.

## C-LEARN Equivalence Analysis

The C-LEARN model (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) is a
large, real-world model that exercises subscripts, subranges, bang notation, and
element-specific equations extensively. The equivalence test compares the native
Rust parser output against the C++ xmutil path.

**Test command:**
```bash
cargo test -p simlin-compat --features xmutil test_clearn_equivalence -- --ignored --nocapture
```

As of January 2026, there are **26 differences** between the two paths (reduced
from an initial 233). These have been analyzed and grouped into 8 root causes,
of which 5 have been fixed.

### Difference Summary

| # | Root Cause | Diffs | Status |
|---|-----------|-------|--------|
| 1 | Element ordering normalization | 0 | FIXED (test normalization) |
| 2 | Per-element equation string substitution | 0 | FIXED |
| 3 | Bang subscript formatting broken | 0 | FIXED |
| 4 | Docs/units taken from wrong equation | 0 | FIXED |
| 5 | Empty equation placeholder `""` vs `"0+0"` | 0 | FIXED |
| 6 | Missing initial-value comment in ApplyToAll | ~4 | Open |
| 7 | Trailing tab in dimension element names | ~8 | Open |
| 8 | Miscellaneous (net flow, middle-dot, GF y-scale, dimension maps_to) | ~14 | Open |

### Root Cause 1: Element Ordering (FIXED)

**Status:** FIXED via test normalization. The native parser and xmutil may produce
array elements in different orders; this is not a semantic difference. The
equivalence test now sorts elements by canonicalized subscript key before
comparison (`mdl_equivalence.rs:normalize_equation`).

### Root Cause 2: Per-Element Equation String Substitution (FIXED)

**Status:** FIXED. When there are multiple equations for the same variable
(element-specific overrides or multi-subrange definitions), per-element
substitution now replaces dimension references with the specific element being
computed, matching xmutil's `GetLHSSpecific` behavior.

**Implementation:** `ElementContext` struct in `xmile_compat.rs` carries
per-element substitution mappings. `SubrangeMapping` handles positional
resolution for sibling subranges. The formatter threads context through all
expression formatting methods. Substitution is only applied when
`expanded_eqs.len() > 1` (multiple equations) to avoid unnecessary expansion
of true apply-to-all equations.

### Root Cause 3: Bang Subscript Formatting (FIXED)

**Status:** FIXED. Two sub-bugs resolved:

1. **Canonicalization mismatch:** `subrange_dims` now uses canonical form via
   `canonical_name()` (= `to_lower_space()`), matching the formatter's lookup
   format. Previously used `space_to_underbar()` which never matched.

2. **Implicit subrange detection:** After explicit subrange collection (from
   `maps_to`), the code now scans `dimension_elements` for implicit subranges:
   if all elements of a dimension are owned by a single different parent
   dimension, it's detected as a subrange. This matches the C++ `SetOwner`
   mechanism.

### Root Cause 4: Documentation/Units Taken From Wrong Equation (FIXED)

**Status:** FIXED. The `extract_metadata` helper now iterates all equations and
uses the first one with non-empty documentation/units, rather than always using
the first equation. This correctly handles Vensim's convention where only the
last element-specific equation carries units and docs.

### Root Cause 5: Empty Equation Placeholder (FIXED)

**Status:** FIXED. Empty RHS equations and lookup-only variables now emit `"0+0"`
to match xmutil.

### Root Cause 6: Missing Initial-Value Comment in ApplyToAll (5 diffs)

**Problem:** Some `ApplyToAll` equations include a third field that is an
initial-value annotation comment. For example, `buffer_factor` has
`Some("Ref_Buffer_Factor")` in xmutil but `None` in native. These comments appear
to be the text of the initial value expression from the Vensim ACTIVE INITIAL or
similar constructs.

**Affected variables:** `buffer_factor`, `co2_ff_emissions`,
`forestry_emissions_by_target`, `im_1_emissions`, `last_set_target_year`

### Root Cause 7: Trailing Tab in Dimension Element Names (1 diff)

**Location:** `lexer.rs:283` (`is_symbol_char` includes `'\t'`) and
`lexer.rs:321-324` (trailing trim only strips spaces and underscores)

**Problem:** The MDL file has tab characters between element names and commas
(e.g., `HFC134a\t,`). The lexer treats `\t` as a valid symbol character but
doesn't strip trailing tabs, producing element names like `"hfc134a\t"`.

**Fix:** Add `'\t'` to the trailing-character trim logic at lines 321-324, or
exclude `'\t'` from `is_symbol_char`.

**Affected:** `hfc_type` dimension (8 of 9 elements have trailing tabs).

### Root Cause 8: Miscellaneous (7 diffs)

**Net flow synthesis (4 diffs):** The `c_in_atmosphere` stock uses a complex rate
expression with many terms. xmutil creates a single synthetic
`c_in_atmosphere_net_flow` variable combining all in/outflows; native correctly
decomposes into individual flows. Related: `flux_c_from_permafrost_release` is
typed as Aux in xmutil but Flow in native. This may be a case where native is
arguably more correct than xmutil.

**Middle-dot canonicalization (2 diffs):** `goal_1.5_for_temperature` vs
`goal_1\u{00B7}5_for_temperature`. xmutil converts middle-dot `\u{00B7}` to
period `.`; native preserves it.

**Graphical function y-scale (2 diffs):** xmutil auto-computes y-scale from data
points; native preserves Vensim's explicitly specified y-scale range.

### Remaining Fix Order

1. **Root Cause 7** (~8 diffs) -- Trivial: strip trailing tabs in lexer
2. **Root Cause 6** (~4 diffs) -- Medium: extract initial-value comment from MDL
3. **Root Cause 8** (~14 diffs) -- Various: net flow synthesis, middle-dot
   canonicalization, GF y-scale, dimension maps_to, subrange normalization

### Key C++ Reference Code for Subscript Handling

When implementing fixes for Root Causes 1-3, these C++ files are essential
references:

- **`ContextInfo.cpp:7-60`** (`GetLHSSpecific`): Per-element dimension reference
  substitution. Tracks current LHS element context and maps dimension references
  to specific elements during expression output.

- **`SymbolList.cpp:29-50`** (`SetOwner`): Ownership assignment. The largest
  dimension containing all of a symbol's elements becomes its owner. Used to
  determine subrange status for bang subscript formatting.

- **`SymbolList.cpp:52-113`** (`OutputComputable`): Bang subscript output logic.
  Checks `s->Owner() != s` to decide between `SubrangeName.*` and bare `*`.

- **`XMILEGenerator.cpp:420-543`** (`generateEquation`): Multi-equation expansion.
  Iterates all equations, calls `SubscriptExpand()` on each, generates
  per-element entries. Determines parent dimension from the expanded element sets.

- **`Variable.cpp:326-349`** (`OutputComputable`): Non-bang subscript resolution
  for array-typed variables. Uses `GetLHSSpecific` to substitute dimension
  references with specific elements.
