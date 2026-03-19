# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase fixes lookup call syntax and variable name casing:

### mdl-roundtrip-fidelity.AC3: Lookup fidelity
- **mdl-roundtrip-fidelity.AC3.1 Success:** Lookup invocations emit as `table_name ( input )` not `LOOKUP(table_name, input)`
- **mdl-roundtrip-fidelity.AC3.3 Success:** Lookups without explicit bounds still compute bounds from data (existing behavior for XMILE-sourced models)

### mdl-roundtrip-fidelity.AC4: Equation formatting
- **mdl-roundtrip-fidelity.AC4.3 Success:** Variable name casing on equation LHS matches original (e.g. `Endogenous Federal Funds Rate=`)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Detect LOOKUP calls and emit native Vensim syntax

**Verifies:** mdl-roundtrip-fidelity.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

In `MdlPrintVisitor::walk()` (lines 527-625), function calls are handled in the `Expr0::App(UntypedBuiltinFn(func, args), _)` arm (around line 550). Currently, a `lookup` call is emitted as `LOOKUP(table_name, input)` via the generic function formatting path.

Note: Verify the internal function name stored in `UntypedBuiltinFn` by checking `builtins.rs`. The code below assumes the internal name is lowercase `"lookup"`. Match against whatever case is used internally (it may be `"lookup"` or `"LOOKUP"`).

Add special-case detection. Two approaches (pick the one that fits the existing code structure better):

**Option A: In the `Expr0::App` arm (simpler):**

```rust
Expr0::App(UntypedBuiltinFn(func, args), _) => {
    // Detect lookup calls and emit native Vensim syntax
    if func == "lookup" && args.len() == 2 {
        if let Expr0::Var(table_ident, _) = &*args[0] {
            let table_name = format_mdl_ident(table_ident.as_str());
            let input = self.walk(&args[1]);
            return format!("{table_name} ( {input} )");
        }
    }
    // ... existing generic path ...
}
```

**Option B: Add to `recognize_vensim_patterns` chain (lines 492-525):**

Add a `recognize_lookup_call` function following the existing pattern of `recognize_random_0_1`, `recognize_log_2arg`, etc.:

```rust
fn recognize_lookup_call(
    expr: &Expr0,
    walk: &mut dyn FnMut(&Expr0) -> String,
) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(func, args), _) = expr {
        if func == "lookup" && args.len() == 2 {
            if let Expr0::Var(table_ident, _) = &*args[0] {
                let table_name = format_mdl_ident(table_ident.as_str());
                let input = walk(&args[1]);
                return Some(format!("{table_name} ( {input} )"));
            }
        }
    }
    None
}
```

Then add to the chain in `recognize_vensim_patterns`.

**Testing:**
- mdl-roundtrip-fidelity.AC3.1: An equation AST containing `lookup(federal_funds_rate_lookup, time)` emits as `federal funds rate lookup ( Time )` not `LOOKUP(federal funds rate lookup, Time)`
- mdl-roundtrip-fidelity.AC3.3: Existing behavior for lookups without explicit bounds is unchanged (this is verified by the y_range tests from Phase 2 continuing to pass)

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: emit native Vensim lookup call syntax in MDL output`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for lookup syntax emission

**Verifies:** mdl-roundtrip-fidelity.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- Construct an equation AST with a lookup call (Expr0::App with func="lookup", args=[Var("table_name"), Var("time")])
- Walk it through MdlPrintVisitor
- Assert output is `table name ( Time )` (spaces around parens, space-separated ident)
- Also test the negative case: a regular function call (not "lookup") should still emit normally

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Use view element names for equation LHS casing

**Verifies:** mdl-roundtrip-fidelity.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

Currently in `write_single_entry()` (line 978), the LHS variable name is produced by `format_mdl_ident(ident)` where `ident` is the canonical lowercase form. This produces `federal funds rate` from `federal_funds_rate`, but cannot recover the original casing `Endogenous Federal Funds Rate`.

The view elements store the original-casing name in their `name` field (e.g., `view_element::Aux { name: "Endogenous Federal Funds Rate", ... }`). Build a mapping from canonical ident to display name from the view elements.

In `write_equations_section()` (lines 1595-1648) or the parent `write_project()`, build a `HashMap<String, String>` mapping canonical idents to their view element names:

```rust
fn build_display_name_map(views: &[View]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for view in views {
        let View::StockFlow(sf) = view;
        for element in &sf.elements {
            if let Some((ident, name)) = element_ident_and_name(element) {
                map.entry(ident).or_insert(name);
            }
        }
    }
    map
}
```

Where `element_ident_and_name()` extracts the canonical ident (via existing `canonicalize`/`normalize_ident` functions) and the original-casing name from Aux, Stock, Flow elements.

Then in `write_single_entry()`, look up the display name:

```rust
let display_name = display_names
    .get(ident)
    .map(|n| n.as_str())
    .unwrap_or(&format_mdl_ident(ident));
```

Use `display_name` for the equation LHS instead of `format_mdl_ident(ident)`.

Pass the display name map through to `write_variable_entry` and `write_single_entry` (and `write_stock_entry` for stocks). This requires updating function signatures to accept the map reference.

**Testing:**
- mdl-roundtrip-fidelity.AC4.3: A variable with canonical ident `endogenous_federal_funds_rate` and view element name `Endogenous Federal Funds Rate` emits as `Endogenous Federal Funds Rate=` on the equation LHS
- Fallback: a variable with no matching view element uses `format_mdl_ident` (lowercase, space-separated)

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: use original variable name casing for MDL equation LHS`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Tests for equation LHS casing

**Verifies:** mdl-roundtrip-fidelity.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- Build a model with a variable `endogenous_federal_funds_rate` and a view element named `Endogenous Federal Funds Rate`
- Write to MDL
- Assert the equation line starts with `Endogenous Federal Funds Rate=` (preserving the view element's casing)
- Test a variable without a matching view element: should use the canonical format_mdl_ident output

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->
