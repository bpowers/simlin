# Systems Format Support - Phase 3: Translator (SystemsModel -> datamodel::Project)

**Goal:** Translate the parsed `SystemsModel` into simlin's `datamodel::Project` using stdlib module instantiation for flow types, with correct stock wiring, multi-outflow chaining, waste flow generation, and formula parenthesization.

**Architecture:** A `translate.rs` module converts each systems flow into a `Variable::Module` instance (referencing the appropriate stdlib module), generates `Variable::Flow` for actual transfers and waste, wires stocks with inflow/outflow lists, and chains multi-outflow modules via `remaining` outputs. An `open_systems()` entry point in `compat.rs` provides the top-level API. Formulas are parenthesized during expression-to-string conversion to preserve left-to-right evaluation semantics.

**Tech Stack:** Rust

**Scope:** 7 phases from original design (phase 3 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- `compat.rs` exposes `open_vensim(contents: &str) -> Result<Project>` as the MDL pattern. `open_systems` follows the same pattern.
- `Project` construction: `Model::sim_specs` is `None`; specs live on `Project::sim_specs`.
- `Stock::inflows` and `Stock::outflows` are `Vec<String>` of flow idents. Cloud/waste flows: the flow exists as `Variable::Flow` but doesn't appear in any stock's `inflows` (it's an outflow from a stock but drains to nowhere).
- `ModuleReference`: `src` = canonical name in parent scope (e.g., `"source_stock"`), `dst` = canonical name inside module (e.g., `"available"`). Use plain names for single-instance modules.
- `Equation::Scalar` strings use period (`.`) for module output references: `"a_outflows.actual"`. The compiler canonicalizes `.` to `·` (U+00B7) internally.
- INF in equations: use `"inf()"` in `Equation::Scalar` strings (0-arity builtin).
- `SimSpecs { start: 0.0, stop: N, dt: Dt::Dt(1.0), save_step: None, sim_method: SimMethod::Euler, time_units: None }`

---

## Acceptance Criteria Coverage

This phase implements and tests:

### systems-format.AC2: Translation produces correct datamodel structure
- **systems-format.AC2.1 Success:** Each systems flow produces a stdlib module instance with correct model_name (systems_rate, systems_conversion, systems_leak)
- **systems-format.AC2.2 Success:** Module input bindings correctly reference source stock, rate expression, and destination capacity
- **systems-format.AC2.3 Success:** Conversion flows produce a waste flow that is an outflow from the source stock with no destination
- **systems-format.AC2.4 Success:** Multi-outflow stocks produce chained modules where each module's `available` input references the previous module's `remaining` output
- **systems-format.AC2.5 Success:** Chain order matches reversed declaration order (last-declared flow gets highest priority)
- **systems-format.AC2.6 Success:** Infinite stocks translate to stocks with equation "inf()"
- **systems-format.AC2.7 Success:** SimSpecs set to start=0, dt=1, save_step=1, method=Euler

### systems-format.AC7: Left-to-right formula evaluation
- **systems-format.AC7.1 Success:** Systems formulas are parenthesized during translation to preserve left-to-right evaluation (e.g., `a + b * c` becomes `(a + b) * c`)

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Implement expression-to-string conversion with left-to-right parenthesization

**Files:**
- Modify: `src/simlin-engine/src/systems/ast.rs` -- add `Expr::to_equation_string()` method

**Verifies:** systems-format.AC7.1

**Implementation:**

Add a method to `Expr` that converts the AST to an equation string suitable for `Equation::Scalar`. The key requirement is preserving left-to-right evaluation by adding explicit parentheses around binary operations.

The systems format evaluates `a + b * c` as `(a + b) * c`, but simlin's equation parser uses standard operator precedence. To preserve left-to-right semantics, the translator must parenthesize:

- For a chain of binary operations like `BinOp(BinOp(a, +, b), *, c)`, emit `"(a + b) * c"`
- Each `BinOp` where the left operand is also a `BinOp` should wrap the left side in parens if the operations have different precedence groups
- Simpler approach: always parenthesize the left operand of any `BinOp` when it is itself a `BinOp`. This is safe and correct.

```rust
impl Expr {
    /// Convert to an equation string with explicit parenthesization
    /// to preserve left-to-right evaluation semantics.
    pub fn to_equation_string(&self) -> String { ... }
}
```

Conversion rules:
- `Int(n)` -> `format!("{n}")`
- `Float(f)` -> `format!("{f}")` (ensure decimal point is present)
- `Ref(name)` -> canonical name (lowercase, spaces to underscores)
- `Inf` -> `"inf()"`
- `Paren(inner)` -> `format!("({inner})")` (preserve original parens)
- `BinOp(left, op, right)` -> parenthesize left if it's a `BinOp` with different precedence

**Testing:**

Tests must verify:
- systems-format.AC7.1: `Expr` tree for `a + b * c` (parsed as `(a + b) * c` by the left-to-right parser) produces `"(a + b) * c"` string
- Simple expressions: `Expr::Int(5)` -> `"5"`, `Expr::Float(0.5)` -> `"0.5"`
- References: `Expr::Ref("Recruiters")` -> `"recruiters"` (canonicalized)
- Inf: `Expr::Inf` -> `"inf()"`
- Nested: `a * b + c` (left-to-right: `(a * b) + c`) -> `"a * b + c"` (no extra parens needed since precedence already matches)

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems::
```

**Commit:** `engine: add systems expression to equation string conversion`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement translator and compat.rs integration

**Verifies:** systems-format.AC2.1, AC2.2, AC2.3, AC2.4, AC2.5, AC2.6, AC2.7

**Files:**
- Create: `src/simlin-engine/src/systems/translate.rs`
- Modify: `src/simlin-engine/src/systems/mod.rs` -- add `mod translate;` and `pub use translate::translate;`
- Modify: `src/simlin-engine/src/compat.rs` -- add `open_systems()` entry point

**Implementation:**

`translate.rs` provides:
```rust
pub fn translate(model: &SystemsModel, num_rounds: u64) -> Result<Project>
```

The function takes a `SystemsModel` (from the parser) and the number of simulation rounds, and produces a `datamodel::Project`.

**Translation algorithm:**

1. **Create SimSpecs**: `start=0.0, stop=num_rounds as f64, dt=Dt::Dt(1.0), save_step=None, sim_method=Euler`

2. **Create stocks**: For each `SystemsStock`, create a `Variable::Stock`:
   - `ident`: canonicalized stock name
   - `equation`: `stock.initial.to_equation_string()` (for infinite stocks, this is `"inf()"`)
   - `inflows` and `outflows`: populated in step 4

3. **Group flows by source stock**: Build a map from source stock name to list of flows (preserving declaration order).

4. **For each source stock, process its outflows in REVERSED declaration order** (last-declared = highest priority):

   For each flow at position `i` in the reversed list:

   a. **Choose module model_name**: `"stdlib⁚systems_rate"`, `"stdlib⁚systems_leak"`, or `"stdlib⁚systems_conversion"` based on `flow.flow_type`.

   b. **Determine `available` input**:
      - If this is the FIRST flow in the reversed list (highest priority): `available = source_stock_name`
      - Otherwise: `available = "{prev_module_ident}.remaining"` (chain to previous module's remaining output)

   c. **Determine `dest_capacity` input**:
      - If dest stock has `max != Expr::Inf`: create an aux variable `{dest}_capacity` with equation `"{dest_max_expr} - {dest_ident}"` and use it
      - Otherwise: `"inf()"`

   d. **Create `Variable::Module`**:
      - `ident`: `"{source}_outflows_{dest}"` (or `"{source}_outflows"` if single outflow)
      - `model_name`: from step (a)
      - `references`: bind `available`, `rate`/`requested`, and `dest_capacity` inputs

      For Rate modules, the input name is `requested` (the rate expression).
      For Leak/Conversion modules, the input name is `rate`.

   e. **Create `Variable::Flow`** for the actual transfer:
      - `ident`: `"{source}_to_{dest}"`
      - `equation`: For Rate/Leak, `"{module_ident}.actual"`. For Conversion, `"{module_ident}.outflow"`.
      - Add to source stock's `outflows` and dest stock's `inflows`

   f. **For Conversion, create waste flow**:
      - `ident`: `"{source}_to_{dest}_waste"`
      - `equation`: `"{module_ident}.waste"`
      - Add to source stock's `outflows` only (no destination stock -- this is a cloud flow)

5. **Assemble Project**: Create a `Model` named `"main"` with all variables (stocks + modules + flows + aux helpers), empty views, and wrap in a `Project`.

**Module reference format examples:**

For `A(10) > B @ 7` (Rate):
```rust
Variable::Module(Module {
    ident: "a_outflows".to_string(),
    model_name: "stdlib⁚systems_rate".to_string(),
    references: vec![
        ModuleReference { src: "a".to_string(), dst: "available".to_string() },
        ModuleReference { src: "a_outflows_requested".to_string(), dst: "requested".to_string() },
        ModuleReference { src: "a_outflows_dest_capacity".to_string(), dst: "dest_capacity".to_string() },
    ],
    ..
})
```
Plus aux `a_outflows_requested` with equation `"7"` and aux `a_outflows_dest_capacity` with equation `"inf()"`.

For multi-outflow chaining (`A > B @ 7` then `A > C @ 7`):
- Module `a_outflows_c`: `available` bound to `"a"` (direct stock reference)
- Module `a_outflows_b`: `available` bound to `"a_outflows_c.remaining"` (chain)

**compat.rs addition:**
```rust
pub fn open_systems(contents: &str) -> Result<Project> {
    let model = systems::parse(contents)?;
    systems::translate(&model, 10)  // default 10 rounds
}
```

The default round count (10) can be a constant. The systems format doesn't specify simulation duration; the Python package defaults to 5 rounds. Use a reasonable default; Phase 4 tests will set explicit values.

**Testing:**

Tests must verify each AC:
- systems-format.AC2.1: Translate a Rate flow -> module with `model_name = "stdlib⁚systems_rate"`, Conversion -> `"stdlib⁚systems_conversion"`, Leak -> `"stdlib⁚systems_leak"`
- systems-format.AC2.2: Check module references have correct `src`/`dst` bindings for each flow type
- systems-format.AC2.3: Conversion flow -> waste flow exists in source stock's `outflows`, not in any stock's `inflows`
- systems-format.AC2.4: Multi-outflow stock -> second module's `available` references first module's `.remaining`
- systems-format.AC2.5: Given flows declared in order [B, C], chain is: C module (available=stock), B module (available=C.remaining) -- reversed order
- systems-format.AC2.6: Infinite stock -> `Equation::Scalar("inf()")`, is_infinite flag corresponds to `Expr::Inf` initial
- systems-format.AC2.7: Translated project has correct SimSpecs
- **Dynamic max parameter:** Test that `EngRecruiter(1, Recruiter)` (from `extended_syntax.txt`) produces a dest_capacity aux with equation `"recruiter - engrecruiter"` (referencing the stock name as a formula, not a constant). This validates that `Expr::Ref` in max parameters produces correct capacity expressions.
- **1.0 Conversion detection:** Test that `Departures > [Departed] @ 1.0` produces a `systems_conversion` module (not `systems_rate`), since `1.0` with a decimal point is TOKEN_DECIMAL and thus Conversion.

Write tests in `translate.rs` using `#[cfg(test)] mod tests`. Build `SystemsModel` directly (not by parsing text) for unit tests. Use `datamodel` assertions to verify structure.

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems::translate
```

**Commit:** `engine: implement systems format translator with module instantiation and chaining`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Integration test -- parse and translate all example files

**Files:**
- Modify: `src/simlin-engine/src/systems/translate.rs` -- add integration tests

**Implementation:**

Add integration tests that parse each valid example file from `third_party/systems/examples/` and verify the translated project compiles without errors:

```rust
#[test]
fn test_translate_hiring() {
    let contents = include_str!("../../../../third_party/systems/examples/hiring.txt");
    let model = parse(contents).unwrap();
    let project = translate(&model, 5).unwrap();
    // Verify: compiles via incremental path
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main");
    assert!(compiled.is_ok(), "hiring.txt should compile: {:?}", compiled.err());
}
```

Test each valid example: `hiring.txt`, `links.txt`, `maximums.txt`, `projects.txt`, `extended_syntax.txt`.

For `hiring.txt`, additionally verify structural details:
- `[Candidates]` translates to a stock with equation `"inf()"`
- Flow from Candidates to PhoneScreens uses `systems_rate` module
- Flow from PhoneScreens to Onsites uses `systems_conversion` module
- Flow from Employees to Departures uses `systems_leak` module
- Waste flows exist for conversion flows

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems::translate
```

Expected: All tests pass.

**Commit:** `engine: add systems format translation integration tests`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
