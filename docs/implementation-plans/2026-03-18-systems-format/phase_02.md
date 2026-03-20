# Systems Format Support - Phase 2: Systems Format Parser

**Goal:** Parse `.txt` files into a `SystemsModel` intermediate representation with stock declarations, flow definitions (Rate, Conversion, Leak), formula expressions, and comment handling.

**Architecture:** Two-stage parser in `src/simlin-engine/src/systems/`: a line-oriented lexer that classifies each line as a comment, stock-only declaration, or flow line and tokenizes the rate formula, then a parser that builds a `SystemsModel` IR from the token stream. The IR preserves declaration order (critical for sequential debiting priority). Follows the MDL reader's module organization pattern (`mod.rs` + submodule files).

**Tech Stack:** Rust

**Scope:** 7 phases from original design (phase 2 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- `src/simlin-engine/src/systems/` does NOT exist yet -- new code
- MDL module at `src/simlin-engine/src/mdl/` provides the reference pattern: `mod.rs` declares submodules, uses `pub` visibility on `mod mdl;` in `lib.rs`
- MDL lexer uses `Cow<'input, str>` for zero-copy string slices -- systems parser can use owned `String` since the format is simpler
- No `systems` module exists in `lib.rs` -- needs `pub mod systems;` added
- Python `systems` package source at `third_party/systems/systems/` reveals exact parsing rules:
  - Stock name regex: `[a-zA-Z][a-zA-Z0-9_]*`
  - Implicit type detection: single `TOKEN_DECIMAL` after `@` -> Conversion; otherwise Rate
  - Explicit types: `Rate(...)`, `Conversion(...)`, `Leak(...)` (case-insensitive)
  - Formula tokenization: whitespace-delimited, supports `+ - * /`, parenthesized sub-expressions, stock name references, integers, decimals
  - Formulas evaluate left-to-right with NO operator precedence
  - Stock deduplication: later declarations update initial/max if current value is still default; conflicting non-default values raise error
  - `[Name]` syntax: infinite stock (initial=inf, show=false)
  - Stock params: `Name(initial)` or `Name(initial, max)` where each can be a formula

---

## Acceptance Criteria Coverage

This phase implements and tests:

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

### systems-format.AC7: Left-to-right formula evaluation
- **systems-format.AC7.2 Success:** Parenthesized formulas in the systems format (e.g., `(a + b) / 2`) translate correctly

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Create AST types and module structure

**Files:**
- Create: `src/simlin-engine/src/systems/mod.rs`
- Create: `src/simlin-engine/src/systems/ast.rs`
- Modify: `src/simlin-engine/src/lib.rs` -- add `pub mod systems;`

**Implementation:**

Add `pub mod systems;` to `lib.rs` near the existing `pub mod mdl;` declaration (around line 46).

`mod.rs` declares submodules and re-exports the public API:
```rust
pub mod ast;
mod lexer;
mod parser;

pub use parser::parse;
```

`ast.rs` defines the intermediate representation:

```rust
/// A formula expression in the systems format.
/// Formulas are evaluated strictly left-to-right with no operator precedence.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Integer literal (e.g., `5`)
    Int(i64),
    /// Decimal literal (e.g., `0.5`)
    Float(f64),
    /// Reference to a stock name (e.g., `Recruiters`)
    Ref(String),
    /// The `inf` literal
    Inf,
    /// Binary operation (left, op, right)
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    /// Parenthesized expression
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowType {
    Rate,
    Conversion,
    Leak,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemsStock {
    pub name: String,
    pub initial: Expr,
    pub max: Expr,
    pub is_infinite: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemsFlow {
    pub source: String,
    pub dest: String,
    pub flow_type: FlowType,
    pub rate: Expr,
}

/// The intermediate representation produced by the parser.
/// Declaration order is preserved (critical for sequential debiting priority).
#[derive(Debug, Clone, PartialEq)]
pub struct SystemsModel {
    pub stocks: Vec<SystemsStock>,
    pub flows: Vec<SystemsFlow>,
}
```

The `Expr` type distinguishes `Int` from `Float` to support implicit type detection (a bare `Int` after `@` is Rate, a bare `Float` is Conversion). The `Paren` variant preserves parenthesization from the source.

**Verification:**

Run:
```bash
cargo build -p simlin-engine
```

Expected: Builds without errors (empty lexer.rs and parser.rs with stub `parse` function).

**Commit:** `engine: add systems format AST types and module structure`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement lexer and parser

**Verifies:** systems-format.AC1.1, AC1.2, AC1.3, AC1.4, AC1.5, AC1.6, AC1.7, AC1.8, AC1.9, AC1.10, AC1.11, AC7.2

**Files:**
- Create: `src/simlin-engine/src/systems/lexer.rs`
- Create: `src/simlin-engine/src/systems/parser.rs`

**Implementation:**

The parser is a single public function:
```rust
pub fn parse(input: &str) -> Result<SystemsModel>
```

**Lexer** (`lexer.rs`): A line-oriented tokenizer. Process input line by line:

1. Trim whitespace. Skip empty lines.
2. If line starts with `#`, skip (comment).
3. If line contains ` > `, it's a flow line: split on ` > ` to get source part (possibly with `[...]` brackets) and dest+rate part. Split dest+rate on ` @ ` to get destination and rate expression.
4. If line contains no ` > ` and no ` @ `, it's a stock-only declaration line.

For stock names: parse `[Name]` as infinite stock, `Name`, `Name(expr)`, or `Name(expr, expr)` for parameterized stocks. Stock name must match `[a-zA-Z][a-zA-Z0-9_]*`.

For rate expressions after `@`: detect explicit flow type prefix (`Rate(...)`, `Conversion(...)`, `Leak(...)`, case-insensitive) or parse as implicit type (bare formula).

For formula parsing: recursive descent supporting:
- Atoms: integer literal, decimal literal, `inf`, stock name reference, parenthesized sub-expression
- Binary operators: `+`, `-`, `*`, `/` (no precedence -- parse flat left-to-right)

**Parser** (`parser.rs`): Builds `SystemsModel` from the lexer output.

Key behaviors:
- **Stock deduplication**: maintain a map of stock names to their index in the `stocks` vec. When a stock name appears again (as source, dest, or stock-only line), update `initial` and `max` only if the new value is non-default AND the existing value is still default. If both are non-default and differ, return an error (AC1.11).
- **Default values**: initial = `Expr::Int(0)`, max = `Expr::Inf`.
- **Implicit type detection**: if the rate expression is a single `Expr::Float`, the flow type is `Conversion`. If it's a single `Expr::Int`, the flow type is `Rate`. If it has operators, references, or parentheses, the flow type is `Rate`. **Critical nuance:** The presence of a decimal point makes it Conversion regardless of numeric value. `@ 1.0` is Conversion (TOKEN_DECIMAL), while `@ 1` is Rate (TOKEN_WHOLE). This matches the Python lexer which classifies `1.0` as TOKEN_DECIMAL. Example: `Departures > [Departed] @ 1.0` in `hiring.txt` is a Conversion flow, not Rate.
- **Flow without rate**: `A > B` (no `@`) creates stocks for A and B but adds no flow (AC1.9). Note: the syntax `A > B` without `@` should be treated as a stock-only declaration of both A and B with a flow relationship implied. Per the Python implementation, this creates stocks but no flow.
- **Declaration order**: flows are appended to `SystemsModel.flows` in the order they appear in the input. This order is reversed during translation (Phase 3) for priority ordering.

**Testing:**

Tests must verify each AC listed above:
- systems-format.AC1.1: Parse `"Name"` stock-only line -> stock with initial=Int(0), max=Inf
- systems-format.AC1.2: Parse `"Name(10)"` and `"Name(10, 20)"` -> correct initial/max Exprs
- systems-format.AC1.3: Parse `"[Name]"` -> is_infinite=true, initial=Inf
- systems-format.AC1.4: Parse `"A > B @ 5"` -> FlowType::Rate
- systems-format.AC1.5: Parse `"A > B @ 0.5"` -> FlowType::Conversion
- systems-format.AC1.6: Parse `"A > B @ Rate(5)"`, `"A > B @ Conversion(0.5)"`, `"A > B @ Leak(0.2)"` -> correct FlowTypes
- systems-format.AC1.7: Parse `"A > B @ Recruiters * 3"`, `"A > B @ Developers / (Projects+1)"` -> correct Expr trees
- systems-format.AC1.8: Parse input with `"# comment"` lines -> comments ignored, correct model
- systems-format.AC1.9: Parse `"Name"` and `"Name(5)"` without flow syntax -> stocks created, no flows
- systems-format.AC1.10: Parse `"a > b @ 5\nb(2) > c @ 3"` -> b has initial=Int(2)
- systems-format.AC1.11: Parse input with conflicting stock params -> error result
- systems-format.AC7.2: Parse `"A > B @ (a + b) / 2"` -> Expr tree with Paren node

Additionally, test parsing all valid example files from `third_party/systems/examples/`:
- `hiring.txt` -- covers infinite stocks, implicit Conversion/Rate, explicit Leak, stock params
- `links.txt` -- covers formula references (`Recruiters * 3`), stock with `(initial, max)`
- `maximums.txt` -- covers `(initial, max)` on both source and dest
- `extended_syntax.txt` -- covers stock-only lines, explicit Rate, formula max in stock params
- `projects.txt` -- covers complex formulas with division and parenthesized sub-expressions

Follow project testing patterns: co-located tests in `parser.rs` or `lexer.rs` using `#[cfg(test)] mod tests { ... }`. Read test infrastructure docs in `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` and `/home/bpowers/src/simlin/docs/dev/rust.md`.

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems::
```

Expected: All tests pass.

**Commit:** `engine: implement systems format parser with lexer and formula support`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
