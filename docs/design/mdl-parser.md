# Vensim MDL Parser Design

The design and the Vensim format notes of the native Rust Vensim MDL reader and writer (`src/simlin-engine/src/mdl/`).

For current status and agent guidance, see `src/simlin-engine/src/mdl/CLAUDE.md`.

## Motivation

**Problems being solved:**
1. **Build complexity**: The C++ xmutil requires Bison/Flex, a C++ toolchain, and complex cross-compilation setup
2. **WASM compatibility**: Cannot easily include xmutil in WASM builds today; would require a large WASI build dependency

## Architecture

### Target: Vensim MDL -> datamodel (directly)

```
Vensim MDL file
    |
Rust lexer (mdl/lexer.rs)
    |
Hand-written recursive descent parser (mdl/parser.rs)
    |
minimal internal representations private to this package
    |
simlin_engine::datamodel::Project  <-- target output
    |
(optional) xmile::project_to_xmile()  <-- "free" XMILE export
```

We deliberately skip the XMILE intermediate representation. By targeting `datamodel` directly:
- We leverage existing XMILE conversion functions for free
- We avoid double-parsing (MDL -> XMILE XML string -> parse XMILE -> datamodel)
- We can extend the datamodel if needed for Vensim-specific features

### Intermediate Structures

Some Vensim concepts require intermediate representations before conversion to datamodel:

1. **View/Diagram data**: Vensim's sketch format uses element indices, relative positioning, and multiple views that must be resolved before converting to absolute positions in a single view in the datamodel.

2. **Symbol table**: During parsing, we need to track variable definitions, subscript ranges, and macros before final resolution.

## Vensim MDL Format Features

All features implemented in xmutil must be supported. This section documents the full feature set organized by implementation phase.

### Phase 1: Lexer (`lexer.rs`)

#### Token Types
- Numbers: integers, floats, scientific notation (e.g., `1e-6`, `1.5E+3`)
- Strings/Symbols: variable names (can contain spaces, underscores)
- Quoted strings with escape sequences (`\"` inside quotes)
- Operators: `+ - * / ^ < > = ( ) [ ] , ; : |`
- Compound operators: `:=` (data equals), `<=`, `>=`, `<>`
- Keywords: `:AND:`, `:OR:`, `:NOT:`, `:NA:`
- Special keywords: `:MACRO:`, `:END OF MACRO:`
- Interpolation modes: `:INTERPOLATE:`, `:RAW:`, `:HOLD BACKWARD:`, `:LOOK FORWARD:`
- Exception keyword: `:EXCEPT:`
- Equivalence: `<->`
- Map arrow: `->`
- Comment terminators: `~` and `|`
- Bang subscript modifier: `!`
- End token: `\\\\\\---///` (end of equations section)
- Group markers: `{**name**}` and `***name***|` formats via `GroupStar` token

#### Lexer State Management
- Track position for error messages
- Handle multi-line tokens (line continuation with `\` at EOL)
- Skip whitespace appropriately
- Handle nested comments (`{ { nested } }`)
- Comment extraction handled by EquationReader (text between second `~` and `|`)

### Phase 2: AST Types (`ast.rs`)

#### Expression AST
- `Expr::Const(f64, Loc)` - numeric literals
- `Expr::Var(name, subscripts, Loc)` - variable references
- `Expr::Op2(BinaryOp, ...)` for binary, `Expr::Op1(UnaryOp, ...)` for unary operators
- `BinaryOp::And`, `BinaryOp::Or`, `UnaryOp::Not` - logical operations
- `Expr::App(name, subscripts, args, CallKind, Loc)` - function calls
- `Expr::Literal(Cow<str>, Loc)` - string literals
- `Expr::Paren(Box<Expr>, Loc)` - parenthesized expressions
- `Equation::TabbedArray` and `Equation::NumberList` - number tables

#### Equation Types
- `Equation::Regular(Lhs, Expr)` - standard equation
- `Equation::Lookup(Lhs, LookupTable)` - lookup definition
- `Equation::WithLookup(Lhs, Box<Expr>, LookupTable)` - WITH LOOKUP
- `Equation::Data(Lhs, Option<Expr>)` - data equation
- `Equation::SubscriptDef(name, SubscriptDef)` - dimension definition
- `Equation::Equivalence(name1, name2, Loc)` - dimension equivalence

### Phase 3: Parser

#### Operator Precedence (low to high)

The table `mdl::parser` implements. Its binary levels disagree with Vensim and XMILE, which put `+`/`-` above the comparisons and `:AND:` below them (GH #914); the importer emits operands flat so the XMILE grammar re-establishes the intended grouping, and the writer groups by the XMILE/Vensim table (`ast::BinaryOp::precedence`).

1. `- +` (addition/subtraction)
2. `:OR:`
3. `= < > <= >= <>`
4. `:AND:`
5. `* /`
6. `:NOT:`, unary `+`, `-`
7. `^` (right-associative)

### Phase 4: Built-in Functions (`builtins.rs`)

Function recognition via `is_builtin()` using `to_lower_space()` canonicalization. Categories:
- Mathematical: ABS, EXP, SQRT, LN, LOG, SIN, COS, TAN, MIN, MAX, INTEGER, MODULO, QUANTUM
- Conditional: IF THEN ELSE, ZIDZ, XIDZ
- Time: PULSE, PULSE TRAIN, STEP, RAMP
- Delay/Smooth: SMOOTH, SMOOTH3, DELAY1, DELAY3, DELAY FIXED, DELAY N, TREND, FORECAST
- Lookup: WITH LOOKUP, LOOKUP INVERT/AREA/EXTRAPOLATE/FORWARD/BACKWARD, TABXL, GET DATA AT TIME
- Array: SUM, PROD, VMAX, VMIN, ELMCOUNT, VECTOR SELECT/ELM MAP/SORT ORDER/REORDER/LOOKUP
- Integration: INTEG, ACTIVE INITIAL, INITIAL, SAMPLE IF TRUE
- Random: RANDOM 0 1, RANDOM UNIFORM/NORMAL/POISSON/PINK NOISE
- Special: NA, A FUNCTION OF, GAME, GET DIRECT DATA, GET XLS/VDF functions

What some of these mean, as the importer translates them:

- **PULSE(start, width)** is 1 while `Time + TIME STEP/2` is strictly between `start` and `start + width`, and a width of 0 is one TIME STEP (vensim.com/documentation/fn_pulse.html).
- **PULSE TRAIN(start, width, interval, end)** is PULSE repeated every `interval` up to `end`, inclusive, each pulse at least one TIME STEP long (fn_pulse_train.html, which gives no equation; the inclusive end follows Vensim's own run in `test/sdeverywhere/models/pulsetrain/pulsetrain.dat`).
- **INTEGER** truncates toward zero and **MODULO** is the remainder of that truncated division, `A - QUANTUM(A, B)` (fn_integer.html, fn_modulo.html; `test/test-models/tests/rounding/output.tab`), so they import as the engine's `TRUNC` and `REM`, not XMILE's flooring `INT` and `MOD`. Vensim's function reference has no INT or MOD; a two-argument `MOD` call of a name the model does not define is read as the floored modulus, the writer's spelling of it.
- **QUANTUM(A, B)** returns A for B at or below zero (fn_quantum.html).
- **A FUNCTION OF** "is not intended for use in writing equations, and precludes simulation" (fn_a_function_of.html); it imports as `NAN`, an equation that reads but does not simulate.

### Phase 6: Core Conversion (`convert/`)

- Variable type detection: stocks via top-level INTEG(), flows from rate expressions, auxiliaries for everything else
- Flow linking via is_all_plus_minus algorithm with synthetic net flow generation
- Sim specs extraction from control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER)
- Dimension building with range expansion and equivalence handling
- A variable's dimensions are those of the elements its equations define: per axis, a dimension a left-hand side names that holds exactly the defined elements, else the smallest declared dimension holding them all, the first declared of several that size (GH #1059). Vensim runs a variable whose equations define only some elements of a dimension over those elements (`test/test-models/tests/except_subranges`); which of several dimensions holding them is the variable's is Simlin's rule, since Vensim keeps no dimension per variable.
- Element keys are canonical (`CanonicalElementName::from_subscript`), as every reader of a datamodel stores them; an element's spelling is its dimension's.
- An import is a function of the file: no decision reads a `HashMap`'s iteration order (`import_determinism_tests`).
- XMILE-compatible expression formatting (`xmile_compat.rs`)
- Group building with hierarchy and conflict resolution

### Phase 7: Views/Diagrams (`view/`)

- Sketch section parsing after `\\\\\\---///` marker
- Element types: variable (10), valve (11), comment/cloud (12), connector (1)
- Ghost/alias detection for duplicate variable appearances
- Coordinate transformation and multi-view composition
- Flow point computation with stock inflow/outflow detection
- Arc angle calculation for curved connectors

### Phase 10: Settings

- Integration type parsing (type 15): Euler/RK2/RK4 method detection
- Unit equivalence parsing (type 22)

## Writer

`writer.rs` writes a `datamodel::Project` as MDL. Its contract is that a save reads back as the datamodel it saved, up to what MDL cannot hold (views, which a save lays out anew, and an equation's spelling): `save_roundtrip_tests` holds every corpus `.mdl` to it, and `writer_proptest` holds generated models to it and to the save being a fixed point. What MDL cannot spell is written as close as it can be, with an `ExportWarning`. The format constrains the writer in ways worth stating:

- An element equation names no dimension, so an arrayed variable is written under left-hand sides that name its dimensions wherever its equations allow (`writer_arrayed.rs`): a number list, a group of elements agreeing across a dimension, or an `:EXCEPT:` equation over its dimensions or a subrange its default names. Otherwise the reader infers the dimensions from the elements, and of several dimensions holding the same elements it takes the first declared; the writer warns.
- Every equation written holds Vensim's rule for a subscript range on a right-hand side: "When you use a Subscript Range in an equation it must appear on the left hand side" (vensim.com/documentation/ref_subscripts.html), unless it is mapped onto one there (ref_subscript_mapping.html). An `:EXCEPT:` default no left-hand side can hold that way, or whose equation would except every element it names, is written as the equations of its elements, with a warning that the default is not kept.
- A function Vensim does not have (ROUND, INT, MOD) is written by name: Simlin reads it back, Vensim does not. Lowering one to Vensim primitives would repeat its argument, wrong for a stochastic one.
- A display newline in a name is the `\n` escape inside a quoted name, as Vensim writes one, and a reference is spelled as its definition is, since the reader matches names as written.

## Error Handling & Compatibility Goals

- **No panics across FFI/WASM boundaries**: Use `Result` for all invariant failures in production paths.
- **Invalid MDL input**: Collect and report multiple errors rather than failing fast.
- **Preserve xmutil permissive fallbacks**: Keep xmutil-compatible behavior (atoi semantics, empty-equation shims, implicit defaults) and document each fallback. Long-term goal is full-fidelity translation.

## Panic/Unwrap Reduction

The production paths return errors rather than panicking:
1. Tabbed array parsing (`normalizer.rs`): returns `Result`
2. Number parsing helper (`parser_helpers.rs`): returns `Result`
3. View parsing (`view/mod.rs`): uses `ok_or(ViewError::UnexpectedEndOfInput)`
4. Normalizer invariant (`normalizer.rs`): returns `Ok(None)` instead of panicking
5. Invariant unwraps in conversion/view processing: all use `match`/`continue`/`Option`

## C-LEARN Equivalence Analysis

The C-LEARN model (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) exercises subscripts, subranges, bang notation, and element-specific equations extensively, and the equivalence harness compares the native import of it with xmutil's:

```bash
cargo test --release -p simlin-engine --features xmutil --test integration test_clearn_equivalence -- --ignored --nocapture
```

The differences fall into these classes:

- `:NA:` is the finite `float::NA` (`-2^109`), where xmutil writes `nan`.
- A lookup-only table's equation is empty, where xmutil writes the `0+0` sentinel; the engine reads both as a table-only variable.
- A variable's own name in its equation, where xmutil writes `self`; `time` in a macro, where xmutil writes `time$`.
- A dimension's element-level mapping onto `cop`, which xmutil leaves empty.
- A graphical function's y scale (two tables), unexamined.

### Key C++ Reference Code for Subscript Handling

- `ContextInfo.cpp:7-60` (`GetLHSSpecific`): Per-element dimension reference substitution
- `SymbolList.cpp:29-50` (`SetOwner`): Ownership assignment for subrange detection
- `SymbolList.cpp:52-113` (`OutputComputable`): Bang subscript output logic
- `XMILEGenerator.cpp:420-543` (`generateEquation`): Multi-equation expansion
- `Variable.cpp:326-349` (`OutputComputable`): Non-bang subscript resolution

## Lessons from Go's C-to-Go Migrations

1. **Test-driven correctness**: Maintain passing tests throughout migration
2. **Don't change semantics during translation**: Behavior changes come in separate commits

## Future: Module-Style View Splitting

The current `merge_views` approach combines all views into a single StockFlow view with group wrappers. Enhancement needed for module/level-structured models:
- `vele->Ghost(adds)` parameter determines cross-level references
- Cross-level connector handling in `XMILEGenerator.cpp:910-960`
