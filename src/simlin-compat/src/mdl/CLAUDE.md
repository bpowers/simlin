# Vensim MDL Parser Implementation

This directory contains the pure Rust implementation of a Vensim MDL file parser, replacing the C++ src/xmutil dependency.

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
├── lib.rs             # Public exports: parse_mdl() function
├── lexer.rs           # Hand-written lexer for MDL tokens
├── parser.lalrpop     # LALRPOP grammar (based on VYacc.y)
├── ast.rs             # AST types produced by parser
├── convert.rs         # AST → datamodel conversion
├── view.rs            # View/diagram intermediate structures and conversion
├── builtins.rs        # Vensim built-in function mappings
└── tests/             # Unit tests
    └── integration_tests.rs
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
- [ ] Numbers: integers, floats, scientific notation (e.g., `1e-6`, `1.5E+3`)
- [ ] Strings/Symbols: variable names (can contain spaces, underscores)
- [ ] Quoted strings with escape sequences (`\"` inside quotes)
- [ ] Operators: `+ - * / ^ < > = ( ) [ ] , ; : |`
- [ ] Compound operators: `:=` (data equals), `<=`, `>=`, `<>`
- [ ] Keywords: `:AND:`, `:OR:`, `:NOT:`, `:NA:`
- [ ] Special keywords: `:MACRO:`, `:END OF MACRO:`
- [ ] Interpolation modes: `:INTERPOLATE:`, `:RAW:`, `:HOLD BACKWARD:`, `:LOOK FORWARD:`
- [ ] Exception keyword: `:EXCEPT:`
- [ ] Equivalence: `<->`
- [ ] Map arrow: `->`
- [ ] Comment terminators: `~` and `|`
- [ ] Bang subscript modifier: `!`
- [ ] End token: `\\\\\\---///` (end of equations section)
- [ ] Group markers: `*NN name` (star followed by group number and name)

### Lexer State Management
- [ ] Track line number and position for error messages
- [ ] Handle multi-line tokens (equations can span lines)
- [ ] Skip whitespace appropriately
- [ ] Handle comment extraction (text between `~` delimiters)

---

## Phase 2: AST Types (`ast.rs`)

### Expression AST
- [ ] `ExpressionNumber` - numeric literals
- [ ] `ExpressionVariable` - variable references with optional subscripts
- [ ] `ExpressionOperator` - binary/unary operators (+, -, *, /, ^, <, >, =, <=, >=, <>)
- [ ] `ExpressionLogical` - AND, OR, NOT
- [ ] `ExpressionFunction` - function calls with arguments
- [ ] `ExpressionFunctionMemory` - stateful functions (SMOOTH, DELAY, etc.)
- [ ] `ExpressionLookup` - lookup table invocation
- [ ] `ExpressionTable` - inline lookup table definition
- [ ] `ExpressionLiteral` - literal strings (for "A FUNCTION OF" etc.)
- [ ] `ExpressionParen` - parenthesized expressions
- [ ] `ExpressionSymbolList` - dimension/subscript definitions
- [ ] `ExpressionNumberTable` - tabbed array values

### Left-Hand Side
- [ ] Variable with optional subscript list
- [ ] Exception list (`:EXCEPT:` clauses)
- [ ] Interpolation mode specification

### Equation Types
- [ ] Regular equation: `lhs = expression`
- [ ] Empty RHS: `lhs =` (treated as "A FUNCTION OF")
- [ ] Lookup definition: `lhs(tablevals)`
- [ ] WITH LOOKUP: `lhs = WITH LOOKUP(input, table)`
- [ ] Data equation: `lhs :DATA: expression`
- [ ] Subscript/dimension definition: `DimName: elements -> mapping`
- [ ] Equivalence: `Dim1 <-> Dim2`
- [ ] Tabbed array: `lhs = :TABBED ARRAY: values`

### Symbol Lists
- [ ] Simple list: `a, b, c`
- [ ] Bang-marked elements: `a!` (indicates exception)
- [ ] Ranges: `(a1-a10)` expanding to a1, a2, ..., a10
- [ ] Mapping lists: `DimA -> DimB` or `DimA -> (DimB: b1, b2, b3)`

---

## Phase 3: Parser (`parser.lalrpop`)

### Grammar Rules (from VYacc.y)
- [ ] `fulleq`: Full equation with units and comments
- [ ] `eqn`: Core equation variants
- [ ] `lhs`: Left-hand side with optional except/interp
- [ ] `var`: Variable with optional subscripts
- [ ] `sublist`: Subscript list `[...]`
- [ ] `symlist`: Comma-separated symbols
- [ ] `subdef`: Subscript definition with ranges
- [ ] `exceptlist`: Exception specifications
- [ ] `mapsymlist`: Mapped symbol list for dimension definitions
- [ ] `maplist`: Optional mapping arrow clause
- [ ] `exprlist`: Comma or semicolon separated expressions
- [ ] `exp`: Expression with full operator precedence
- [ ] `tablevals`: Table pairs format `(x,y), (x,y), ...`
- [ ] `xytablevals`: XY table vector format
- [ ] `tablepairs`: Individual coordinate pairs
- [ ] `units`: Unit expressions with mult/div
- [ ] `unitsrange`: Units with optional range `[min, max]` or `[min, max, step]`
- [ ] `macrostart`: `:MACRO:` block beginning
- [ ] `macroend`: `:END OF MACRO:`

### Operator Precedence (low to high)
1. `- +` (addition/subtraction)
2. `:OR:`
3. `= < > <= >= <>`
4. `:AND:`
5. `* /`
6. `:NOT:`
7. `^` (exponentiation, right-associative)

### Table Formats
- [ ] Pairs format: `[(xmin,ymin)-(xmax,ymax)], (x1,y1), (x2,y2), ...`
- [ ] Pairs with embedded range: `[(xmin,ymin)-(xmax,ymax), pairs], pairs`
- [ ] XY vector format (legacy): `xmin, xmax, y1, y2, y3, ...`

---

## Phase 4: Built-in Functions (`builtins.rs`)

### Mathematical Functions
- [ ] `ABS`, `EXP`, `SQRT`, `LN`, `LOG` (LOG is log10)
- [ ] `SIN`, `COS`, `TAN`, `ARCSIN`, `ARCCOS`, `ARCTAN`
- [ ] `MIN`, `MAX`
- [ ] `INTEGER` (truncate)
- [ ] `MODULO`
- [ ] `QUANTUM` (round to increment)

### Conditional/Logical
- [ ] `IF THEN ELSE(condition, true_val, false_val)`
- [ ] `ZIDZ(num, denom)` - zero if divide by zero
- [ ] `XIDZ(num, denom, x)` - x if divide by zero

### Time Functions
- [ ] `PULSE(start, width)`
- [ ] `PULSE TRAIN(start, width, interval, end)`
- [ ] `STEP(height, time)`
- [ ] `RAMP(slope, start, end)`

### Delay/Smooth Functions (Stateful)
- [ ] `SMOOTH(input, delay_time)`
- [ ] `SMOOTHI(input, delay_time, initial)`
- [ ] `SMOOTH3(input, delay_time)`
- [ ] `SMOOTH3I(input, delay_time, initial)`
- [ ] `SMOOTHN(input, delay_time, order)`
- [ ] `DELAY1(input, delay_time)`
- [ ] `DELAY1I(input, delay_time, initial)`
- [ ] `DELAY3(input, delay_time)`
- [ ] `DELAY3I(input, delay_time, initial)`
- [ ] `DELAY(input, delay_time, initial)`
- [ ] `DELAYN(input, delay_time, initial, order)`
- [ ] `DELAY CONVEYOR(input, delay_time, initial)`
- [ ] `TREND(input, avg_time, initial)`
- [ ] `FORECAST(input, avg_time, horizon)`

### Lookup Functions
- [ ] `WITH LOOKUP(input, table)` - handled specially by parser
- [ ] `LOOKUP INVERT(lookup_var, value)`
- [ ] `LOOKUP AREA(lookup_var, x1, x2)`
- [ ] `LOOKUP EXTRAPOLATE(lookup_var, x)`
- [ ] `LOOKUP FORWARD(lookup_var, x)`
- [ ] `LOOKUP BACKWARD(lookup_var, x)`
- [ ] `GET DATA AT TIME(data_var, time)`
- [ ] `GET DATA LAST TIME(data_var)`

### Array Functions
- [ ] `SUM(array)` - sum across subscripts
- [ ] `PROD(array)` - product across subscripts
- [ ] `VMAX(array)` - vector max
- [ ] `VMIN(array)` - vector min
- [ ] `ELMCOUNT(dimension)` - element count
- [ ] `VECTOR SELECT(selection, sel_values, index_dim, missing_val, action)`
- [ ] `VECTOR ELM MAP(vector, index)`
- [ ] `VECTOR SORT ORDER(vector, direction)`
- [ ] `VECTOR REORDER(vector, order)`
- [ ] `VECTOR LOOKUP(vector, index, missing)`

### Integration/State
- [ ] `INTEG(rate, initial)` - stock integration
- [ ] `ACTIVE INITIAL(equation, initial)` - with separate init
- [ ] `INITIAL(value)` - value at initialization time
- [ ] `REINITIAL(initial, condition)` - reinitialize on condition
- [ ] `SAMPLE IF TRUE(condition, input, initial)` - conditional sampling

### Random Functions
- [ ] `RANDOM 0 1()` - uniform [0,1]
- [ ] `RANDOM UNIFORM(min, max, seed)`
- [ ] `RANDOM NORMAL(min, max, mean, stddev, seed)`
- [ ] `RANDOM POISSON(mean, seed)`
- [ ] `RANDOM PINK NOISE(mean, stddev, seed)`

### Special
- [ ] `NAN` - Not a Number
- [ ] `NA` - :NA: token, typically -1e38
- [ ] `GAME(input)` - gaming/interactive input
- [ ] `TIME BASE` - time as number
- [ ] `GET DIRECT DATA(...)` - external data
- [ ] `GET DATA MEAN(...)` - data statistics
- [ ] `NPV(...)` - net present value
- [ ] `ALLOCATE BY PRIORITY(...)`

---

## Phase 5: Subscript/Array Handling

### Dimension Definitions
- [ ] Simple dimension: `DimA: elem1, elem2, elem3`
- [ ] Numeric range dimension: `DimA: (1-10)` → DimA1, DimA2, ..., DimA10
- [ ] Subrange definition: `SubA: elem1, elem2 -> ParentDim`
- [ ] Dimension mapping: `DimA: A1, A2, A3 -> DimB`
- [ ] Explicit mapping: `DimA: A1, A2, A3 -> (DimB: B1, B2, B3)`
- [ ] Equivalence: `DimA <-> DimB`

### Subscripted Equations
- [ ] Apply-to-all: `Var[DimA] = expr` (same expr for all elements)
- [ ] Element-specific: `Var[elem1] = expr1` then `Var[elem2] = expr2`
- [ ] Exception-based: `Var[DimA] :EXCEPT: [elem1, elem2] = expr`
- [ ] Multi-dimensional: `Var[DimA, DimB] = expr`
- [ ] Mixed indexing: `Var[elem1, DimB]` (specific first dim, all second)
- [ ] Subscript expansion in expressions

### Bang Notation
- [ ] `[dim!]` marks elements for exception handling
- [ ] Used in :EXCEPT: clauses to specify what to exclude

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

### Macro Parsing
- [ ] Detect `:MACRO: name(args)` start
- [ ] Create separate namespace for macro body
- [ ] Parse equations within macro body
- [ ] Detect `:END OF MACRO:` end
- [ ] Store MacroFunction with name, args, local namespace

### Macro Output
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
- [ ] Lexer: Each token type, edge cases, error recovery
- [ ] Parser: Each grammar rule, operator precedence, table formats
- [ ] Builtins: Each function with typical and edge case inputs
- [ ] Subscripts: All dimension/subscript patterns
- [ ] Views: Each element type, coordinate transforms

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

### Core Foundation (Phases 1-3)
1. **Lexer** (`lexer.rs`) - Must be completed first
2. **AST types** (`ast.rs`) - Define all node types before parser
3. **Parser** (`parser.lalrpop`) - Depends on lexer and AST

### Semantic Processing (Phases 4-6)
4. **Built-ins** (`builtins.rs`) - Function recognition during conversion
5. **Subscripts** - Integrated into parser and conversion
6. **Conversion** (`convert.rs`) - AST → datamodel transformation

### Visual Layer (Phase 7)
7. **Views** (`view.rs`) - Can be done last, independent of core functionality

### Advanced Features (Phases 8-10)
8. **Macros** - Can be deferred until basic functionality works
9. **Model post-processing** - Name normalization, view composition
10. **Settings parsing** - Integration type, unit equivalences

### Recommended Approach
1. Get lexer + parser working on simple equations first
2. Add basic conversion without subscripts
3. Test against simple models
4. Add subscript support
5. Add view parsing
6. Add remaining advanced features
7. Test against full model corpus

## Extending the Datamodel

If Vensim has features that don't map cleanly to the current datamodel, we can extend `simlin_core::datamodel`.
Our goal here is for the overall Simlin project to be as simple, maintainable, and full featured as possible, improving or generalizing the datamodel or other parts of Simlin are great ways to approach this problem.

Any extensions should be discussed and designed carefully to maintain clean abstractions.

## Lessons from Go's C-to-Go Migrations

1. **Test-driven correctness**: Maintain passing tests throughout migration
2. **Don't change semantics during translation**: Behavior changes come in separate commits

## Session Continuity

This is a multi-session project. When resuming work:
1. Check the feature checklist above for current status
2. Run existing tests to verify nothing regressed
3. Continue with the next unchecked item in implementation order
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
