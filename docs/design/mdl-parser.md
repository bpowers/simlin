# Vensim MDL Parser Design History

This document preserves the design history and detailed implementation notes for the native Rust Vensim MDL parser (`src/simlin-engine/src/mdl/`).

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

### Phase 6: Core Conversion (`convert/`)

- Variable type detection: stocks via top-level INTEG(), flows from rate expressions, auxiliaries for everything else
- Flow linking via is_all_plus_minus algorithm with synthetic net flow generation
- Sim specs extraction from control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER)
- Dimension building with range expansion and equivalence handling
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

## Error Handling & Compatibility Goals

- **No panics across FFI/WASM boundaries**: Use `Result` for all invariant failures in production paths.
- **Invalid MDL input**: Collect and report multiple errors rather than failing fast.
- **Preserve xmutil permissive fallbacks**: Keep xmutil-compatible behavior (atoi semantics, empty-equation shims, implicit defaults) and document each fallback. Long-term goal is full-fidelity translation.

## Panic/Unwrap Reduction (Jan 2026)

All primary production-path panic/unwrap risks have been fixed:
1. Tabbed array parsing (`normalizer.rs`): returns `Result`
2. Number parsing helper (`parser_helpers.rs`): returns `Result`
3. View parsing (`view/mod.rs`): uses `ok_or(ViewError::UnexpectedEndOfInput)`
4. Normalizer invariant (`normalizer.rs`): returns `Ok(None)` instead of panicking
5. Invariant unwraps in conversion/view processing: all use `match`/`continue`/`Option`

## C-LEARN Equivalence Analysis

The C-LEARN model (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) exercises subscripts, subranges, bang notation, and element-specific equations extensively.

**Test command:**
```bash
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture
```

As of January 2026, **26 differences** remain (reduced from initial 233), grouped into 8 root causes:

| # | Root Cause | Diffs | Status |
|---|-----------|-------|--------|
| 1 | Element ordering normalization | 0 | FIXED |
| 2 | Per-element equation string substitution | 0 | FIXED |
| 3 | Bang subscript formatting broken | 0 | FIXED |
| 4 | Docs/units taken from wrong equation | 0 | FIXED |
| 5 | Empty equation placeholder "" vs "0+0" | 0 | FIXED |
| 6 | Missing initial-value comment in ApplyToAll | ~4 | Open |
| 7 | Trailing tab in dimension element names | ~8 | Open |
| 8 | Miscellaneous (net flow, middle-dot, GF y-scale) | ~14 | Open |

### Remaining Fix Order
1. Root Cause 7 (~8 diffs) -- strip trailing tabs in lexer
2. Root Cause 6 (~4 diffs) -- extract initial-value comment from MDL
3. Root Cause 8 (~14 diffs) -- net flow synthesis, middle-dot, GF y-scale

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
