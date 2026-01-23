# Vensim MDL Parser Implementation

This directory contains the pure Rust implementation of a Vensim MDL file parser, replacing the C++ src/xmutil dependency.

## Current Status

**Phases 1-5 and 8 (Parsing): COMPLETE**
- Lexer, normalizer, parser, and AST types are fully implemented
- All equation types, expressions, subscripts, and macros parse correctly
- Comprehensive test coverage (221 tests)

**Phases 6-7 and 9-10 (Conversion): NOT STARTED**
- `parse_mdl()` is a stub - AST to datamodel conversion not implemented
- View/diagram parsing not implemented
- Settings section parsing not implemented

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
Rust parser using LALRPOP (mdl/parser.lalrpop)
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

1. **View/Diagram data**: Vensim's sketch format uses element indices, relative positioning, and multiple views that must be resolved before converting to absolute positions in a single view in the datamodel. These intermediate structures live in `mdl/view.rs`.

2. **Symbol table**: During parsing, we need to track variable definitions, subscript ranges, and macros before final resolution.

### Module Structure

```
src/simlin-compat/src/mdl/
├── CLAUDE.md          # This file - project context and goals
├── mod.rs             # Public exports: parse_mdl() function, re-exports
├── lexer.rs           # Hand-written RawLexer for MDL tokens (context-free)
├── normalizer.rs      # TokenNormalizer for context-sensitive transformations
├── parser.lalrpop     # LALRPOP grammar (based on VYacc.y)
├── ast.rs             # AST types produced by parser
├── parser_helpers.rs  # Helper functions for parser (number parsing, equation creation)
├── reader.rs          # EquationReader: drives parser, captures comments, handles macros
├── builtins.rs        # Vensim built-in function recognition via to_lower_space()
├── convert.rs         # AST → datamodel conversion (NOT YET IMPLEMENTED)
└── view.rs            # View/diagram parsing and conversion (NOT YET IMPLEMENTED)
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
- [x] `LOOKUP EXTRAPOLATE(lookup_var, x)`
- [x] `LOOKUP FORWARD(lookup_var, x)`
- [x] `LOOKUP BACKWARD(lookup_var, x)`
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

### Conversion-Phase Requirements (Not Yet Implemented)
- [ ] **Subscript range expansion**: `(A1-A10)` ranges must be expanded to individual elements during conversion. The parser stores these as `SubscriptElement::Range(start, end, loc)` which needs to be validated (numeric suffix extraction) and expanded.
- [ ] **Implicit TIME lookup**: Bare LHS equations (`exogenous data ~ ~ |`) are parsed as `Equation::Implicit`. During conversion, these should become lookup tables over TIME if they represent data variables.
- [ ] **Range-only units normalization**: When units have only a range like `[0, 100]` without an explicit unit expression, the `Units.expr` is `None`. During conversion, this should be treated as dimensionless ("1") rather than truly having no units.

### Bang Notation
- [x] `[dim!]` parsed as `Subscript::BangElement`
- [x] Used in `:EXCEPT:` clauses via `ExceptList`

---

## Phase 6: Core Conversion (`convert.rs`)

### Variable Type Detection
- [ ] Stock detection: has INTEG() in equation
- [ ] Flow detection: appears in INTEG() rate expression of a stock
- [ ] Auxiliary: everything else (non-stock, non-flow)
- [ ] Mark inflows/outflows on stocks

### Equation Processing
- [ ] `MarkTypes()` - auto-detect variable types from equations
- [ ] `MarkStockFlows()` - identify and link flows to stocks
- [ ] `PurgeAFOEq()` - remove "A FUNCTION OF" placeholder equations

### Special Variable Handling
- [ ] `INITIAL TIME` → sim_specs.start
- [ ] `FINAL TIME` → sim_specs.stop
- [ ] `TIME STEP` → sim_specs.dt
- [ ] `SAVEPER` → sim_specs.save_interval
- [ ] Mark these as "unwanted" (don't emit as regular variables)
- [ ] Extract time units from these variables

### Integration Type
- [ ] Parse from settings section (type code 15)
- [ ] Map: 0,2 → Euler, 1,5 → RK4, 3,4 → RK2

### Unit Equivalences
- [ ] Parse from settings section (type code 22)
- [ ] Format: `Dollar,$,Dollars,$s`

### Groups
- [ ] Parse group markers `*NN name` during equation parsing
- [ ] Maintain group hierarchy (nested groups)
- [ ] `AdjustGroupNames()` - ensure unique group names
- [ ] Assign variables to groups

---

## Phase 7: Views/Diagrams (`view.rs`)

### Sketch Section Parsing

The sketch section follows the equation section, starting with `\\\\\\---///`.

#### View Header
- [ ] Parse view marker: `\\\\\\---///`
- [ ] Parse version line: `V300 ` or `V364 ` prefix
- [ ] Parse view title: line starting with `*` (e.g., `*View 1`)
- [ ] Parse font line: pipe-separated values (8 fields, optional PPI at end)
- [ ] Extract x/y scaling ratios from font line (default 1.0)

#### Element Types (identified by first integer)
- [ ] Type 10: Variable element (VensimVariableElement)
- [ ] Type 11: Valve element (VensimValveElement)
- [ ] Type 12: Comment/cloud element (VensimCommentElement)
- [ ] Type 1: Connector element (VensimConnectorElement)
- [ ] Type 30: Unknown/ignored

#### Variable Element (Type 10) Parsing
- [ ] Parse: `10,uid,name,x,y,width,height,shape,bits,...`
- [ ] Name: either variable name or numeric reference
- [ ] Position: x, y coordinates (center of element)
- [ ] Dimensions: width, height (half-values, actual size is 2x)
- [ ] Shape flags: bit 5 (0x20) = attached to valve (flow indicator)
- [ ] Bits flags: bit 0 = 0 means ghost, bit 0 = 1 means primary definition
- [ ] Bit 2: scratch name indicator (name on next line)
- [ ] Link variable name to Variable in symbol table
- [ ] Track ghost vs primary definition status

#### Valve Element (Type 11) Parsing
- [ ] Parse: `11,uid,name,x,y,width,height,shape,...`
- [ ] Always follows its associated flow variable in element list
- [ ] Shape bit 5: attached flag
- [ ] Valve position defines flow location in diagram

#### Comment Element (Type 12) Parsing
- [ ] Parse: `12,uid,text,x,y,width,height,shape,bits,...`
- [ ] Includes clouds (boundary markers) and text annotations
- [ ] Bits bit 2: if set, actual text is on the next line

#### Connector Element (Type 1) Parsing
- [ ] Parse: `1,uid,from,to,ignore,ignore,polarity,...,npoints|(x,y),...`
- [ ] From/To: UIDs of connected elements
- [ ] Polarity: ASCII code for '+', '-', 'S' (same), 'O'/'0' (opposite)
- [ ] Letter polarity: 'S'/'s' → '+', 'O'/'0' → '-' (set LetterPolarity flag)
- [ ] Control points: `npoints|(x,y)` format for curved connectors

### View Processing Logic

#### Ghost Variable Handling
- [ ] First appearance of variable in any view becomes primary definition
- [ ] Subsequent appearances become ghosts
- [ ] `UpgradeGhost()`: promote ghost to primary if no primary exists
- [ ] `CheckGhostOwners()`: ensure all variables have exactly one primary

#### Variable-View Association
- [ ] Each variable tracks which view it's primarily defined in
- [ ] `SetView()` / `GetView()` on Variable
- [ ] Only non-ghost elements set the variable's view
- [ ] Flow attachment: if attached to valve, mark as flow

#### Connector Validation
- [ ] `FindInArrow()`: check if connector exists from source to target
- [ ] `RemoveExtraArrowsIn()`: invalidate connectors not matching inputs
- [ ] Handle valve indirection: connector to valve→ actually to flow var

#### Flow Definition Placement
- [ ] `AddFlowDefinition()`: add missing flow to diagram
- [ ] Position between upstream and downstream stocks
- [ ] If only one stock found, offset by 60 units

#### Straggler Attachment
- [ ] Variables without views: find ghost and upgrade to primary
- [ ] Flows without views: place between connected stocks
- [ ] Remaining unplaced variables: dump at (200, 200) on first view

#### Link Checking
- [ ] `CheckLinksIn()`: verify all input dependencies have connectors
- [ ] Add missing connectors for non-array, non-stock inputs
- [ ] `FindVariable()`: locate or create element for a variable

### Coordinate Transformation

#### Scaling
- [ ] `SetViewStart()`: transform all element coordinates
- [ ] Find minimum x, y across all elements
- [ ] Apply offset to shift origin
- [ ] Apply scale ratios: `new_coord = old_coord * ratio + offset`
- [ ] Scale width/height by same ratios
- [ ] Connector points also scaled via `ScalePoints()`

#### View Composition
- [ ] For multiple views merged into one: offset y by previous view height + 80
- [ ] Track UID offsets per view for element references
- [ ] `GetViewMaxX()` / `GetViewMaxY()`: compute view bounds

### XMILE View Generation

#### View Element Output
- [ ] Auxiliary (`<aux>`): x, y position
- [ ] Stock (`<stock>`): x, y, width, height (if non-default size)
- [ ] Flow (`<flow>`): x, y from valve, plus `<pts>` for pipe endpoints
- [ ] Ghost/Alias (`<alias>`): x, y, uid, with `<of>` child for target name

#### Stock Sizing
- [ ] Default Vensim size: 80x40 (stored as 40x20 half-values)
- [ ] XMILE expects: x, y as top-left corner
- [ ] Transform center to corner: x -= width/2, y -= height/2
- [ ] Minimum size: 60x40

#### Flow Pipe Points
- [ ] Search connectors from valve (uid-1) to find connected stocks/clouds
- [ ] Determine inflow vs outflow by checking stock's inflow/outflow lists
- [ ] Set pipe endpoints based on stock positions
- [ ] Handle horizontal vs vertical flows (adjust x or y to anchor)
- [ ] Default if not connected: extend 150 units in default direction

#### Connector Output
- [ ] Calculate angle using `AngleFromPoints(from, control, to)`
- [ ] Output `<connector uid="N" angle="A" polarity="P">`
- [ ] `<from>`: variable name or `<alias uid="N"/>`
- [ ] `<to>`: variable name

#### Angle Calculation
- [ ] Three-point arc calculation for curved connectors
- [ ] Find circle center from perpendicular bisectors
- [ ] Calculate tangent angle at start point
- [ ] Fall back to straight-line angle if geometry fails
- [ ] Handle degenerate cases (vertical/horizontal lines)

#### Sector/Group Output
- [ ] Multiple views → wrap in `<group>` elements
- [ ] Group attributes: name, x, y, width, height
- [ ] Variables in group listed with `<var>` children

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

### Name Processing
- [ ] `SpaceToUnderBar()`: replace spaces with underscores
- [ ] `QuotedSpaceToUnderBar()`: add quotes if contains periods
- [ ] `MakeViewNamesUnique()`: deduplicate view titles
- [ ] Remove special chars from view names: `. - + , / * ^`
- [ ] Long name mode: use compressed comment as variable name

### Variable Filtering
- [ ] Skip "Time" variable in views (handled by XMILE runtime)
- [ ] Skip ARRAY and ARRAY_ELM types in views
- [ ] Skip UNKNOWN type variables
- [ ] Filter "unwanted" variables (sim spec vars)

---

## Phase 10: Settings Section Parsing

Located after views, starting with `///---\\\` marker.

### Settings Marker
- [ ] Parse `///---\\\` section start
- [ ] Parse `:L\177<%^E!@` settings block marker

### Setting Types (first integer in colon-delimited line)
- [ ] Type 15: Integration type (4th integer: 0/2=Euler, 1/5=RK4, 3/4=RK2)
- [ ] Type 22: Unit equivalence strings

---

## Testing Strategy Updates

### Unit Test Coverage
- [x] Lexer: Each token type, edge cases, error recovery (29+ tests in `lexer.rs`)
- [x] Normalizer: Section state, function classification, TABBED ARRAY, GET XLS (30+ tests in `normalizer.rs`)
- [x] Builtins: Canonicalization, function recognition (15+ tests in `builtins.rs`)
- [x] Parser helpers: Number parsing, equation creation (15+ tests in `parser_helpers.rs`)
- [x] Reader: Full equation parsing, comments, macros, all equation types (60+ tests in `reader.rs`)
- [ ] Views: Each element type, coordinate transforms (Phase 7 not started)

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

### Semantic Processing (Phases 4-6)
4. **Built-ins** (`builtins.rs`) - DONE: Function recognition via to_lower_space()
5. **Subscripts** - DONE (parsing): Integrated into parser
6. **Conversion** (`convert.rs`) - NOT STARTED: AST → datamodel transformation

### Visual Layer (Phase 7)
7. **Views** (`view.rs`) - NOT STARTED: View/diagram parsing and conversion

### Advanced Features (Phases 8-10)
8. **Macros** - DONE (parsing): MacroDef captured, conversion not started
9. **Model post-processing** - NOT STARTED: Name normalization, view composition
10. **Settings parsing** - NOT STARTED: Integration type, unit equivalences

### Next Steps
1. Implement `convert.rs` to transform AST → datamodel for simple equations
2. Test against simple models to verify roundtrip
3. Add subscript expansion during conversion
4. Add view parsing (`view.rs`)
5. Add settings section parsing
6. Test against full model corpus

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
3. The next major milestone is implementing `convert.rs` (Phase 6)
4. Update the checklist as features are completed

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
