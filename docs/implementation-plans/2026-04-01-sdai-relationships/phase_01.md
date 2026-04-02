# SD-AI Relationships Array Generation - Implementation Plan

**Goal:** Generate the SD-AI JSON `relationships` array from equation dependency graphs and computed polarities, replacing the re-attach-from-original-file approach.

**Architecture:** Pure function `generate_relationships()` in `json_sdai.rs` takes pre-computed polarity map and datamodel, filters stock-flow structural edges, maps remaining equation-derived edges to `Relationship` structs with computed polarity. Called from `edit_model.rs` during SD-AI JSON serialization.

**Tech Stack:** Rust (simlin-engine crate), serde, std collections

**Scope:** 2 phases from original design (phases 1-2)

**Codebase verified:** 2026-04-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### sdai-relationships.AC1: Equation-derived dependencies become relationships
- **sdai-relationships.AC1.1 Success:** Auxiliary `C = A + B` produces `{from: "A", to: "C"}` and `{from: "B", to: "C"}` in relationships array
- **sdai-relationships.AC1.2 Success:** Flow equation referencing a stock (e.g. `deaths = population * death_rate`) produces `{from: "population", to: "deaths"}` and `{from: "death_rate", to: "deaths"}`
- **sdai-relationships.AC1.3 Edge:** Variable with no equation produces no relationships with that variable as `to`

### sdai-relationships.AC2: Polarity is computed correctly
- **sdai-relationships.AC2.1 Success:** `C = A + B` yields polarity `"+"` for both relationships
- **sdai-relationships.AC2.2 Success:** `C = A - B` yields polarity `"+"` for A, `"-"` for B
- **sdai-relationships.AC2.3 Edge:** Expression where polarity is indeterminate yields `"?"`

### sdai-relationships.AC3: Stock-flow structural edges are excluded
- **sdai-relationships.AC3.1 Success:** Stock with inflow `births` does NOT produce `{from: "births", to: "population"}`
- **sdai-relationships.AC3.2 Success:** Stock with outflow `deaths` does NOT produce `{from: "deaths", to: "population"}`
- **sdai-relationships.AC3.3 Success:** Flow's equation referencing a stock (equation-derived) IS included

---

## Reference Files

The implementor should read the following CLAUDE.md files for project conventions:

- `/home/bpowers/src/simlin/CLAUDE.md` -- root project guidelines (TDD mandate, commit style, comment standards)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- engine module map, test infrastructure, cargo features

Key source files for this phase:

- `/home/bpowers/src/simlin/src/simlin-engine/src/json_sdai.rs` -- target file; `Polarity` enum (line 126), `Relationship` struct (line 149), `SdaiModel` (line 187), `filter_stale_relationships()` (line 422), existing `#[cfg(test)] mod tests` (line 684)
- `/home/bpowers/src/simlin/src/simlin-engine/src/ltm.rs` -- `LinkPolarity` enum (line 18): `Positive`, `Negative`, `Unknown`
- `/home/bpowers/src/simlin/src/simlin-engine/src/db_analysis.rs` -- `compute_link_polarities()` (line 388) returns `HashMap<(String, String), crate::ltm::LinkPolarity>` where tuple is `(from_ident, to_ident)`
- `/home/bpowers/src/simlin/src/simlin-engine/src/datamodel.rs` -- `Model` (line 783), `Variable` enum (line 343: `Stock`, `Flow`, `Aux`, `Module`), `Stock` (line 284) has `ident: String`, `inflows: Vec<String>`, `outflows: Vec<String>`
- `/home/bpowers/src/simlin/src/simlin-engine/src/testutils.rs` -- `x_aux()` (line 15), `x_stock()` (line 48), `x_flow()` (line 143), `x_model()` (line 88)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Implement `generate_relationships()` and tests for equation deps and polarity

**Verifies:** sdai-relationships.AC1.1, sdai-relationships.AC1.2, sdai-relationships.AC1.3, sdai-relationships.AC2.1, sdai-relationships.AC2.2, sdai-relationships.AC2.3

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-engine/src/json_sdai.rs` (add function after line 437, add tests in existing `#[cfg(test)] mod tests` starting at line 684)

**Implementation:**

Add these imports near the top of `json_sdai.rs` (after the existing `use crate::datamodel;` on line 24):

```rust
use std::collections::{HashMap, HashSet};

use crate::ltm::LinkPolarity;
```

Add the `generate_relationships` function after `filter_stale_relationships()` (after line 437, before the `// Conversions FROM datamodel types TO SDAI types` section at line 440):

```rust
/// Generate relationships from pre-computed equation dependency polarities.
///
/// Takes the polarity map produced by `compute_link_polarities()` and the
/// datamodel, filters out stock-flow structural edges (which the SD-AI
/// conformance evaluator generates independently), and maps remaining
/// equation-derived edges to `Relationship` values with computed polarity.
pub fn generate_relationships(
    polarities: &HashMap<(String, String), LinkPolarity>,
    model: &datamodel::Model,
) -> Vec<Relationship> {
    // Stock-flow structural edges are excluded because the conformance
    // evaluator generates them itself via makeRelationshipsFromStocks().
    let mut stock_flow_edges: HashSet<(&str, &str)> = HashSet::new();
    for var in &model.variables {
        if let datamodel::Variable::Stock(stock) = var {
            for inflow in &stock.inflows {
                stock_flow_edges.insert((inflow.as_str(), stock.ident.as_str()));
            }
            for outflow in &stock.outflows {
                stock_flow_edges.insert((outflow.as_str(), stock.ident.as_str()));
            }
        }
    }

    let mut relationships: Vec<Relationship> = polarities
        .iter()
        .filter(|((from, to), _)| !stock_flow_edges.contains(&(from.as_str(), to.as_str())))
        .map(|((from, to), polarity)| Relationship {
            reasoning: None,
            from: from.clone(),
            to: to.clone(),
            polarity: match polarity {
                LinkPolarity::Positive => Polarity::Positive,
                LinkPolarity::Negative => Polarity::Negative,
                LinkPolarity::Unknown => Polarity::Unknown,
            },
            polarity_reasoning: None,
        })
        .collect();

    relationships.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    relationships
}
```

**Testing:**

Tests go in the existing `#[cfg(test)] mod tests` block in `json_sdai.rs` (line 684). The test module already has `use super::*;`, which brings in `HashMap`, `HashSet`, `LinkPolarity`, `generate_relationships`, `Relationship`, and `Polarity` from the parent module -- no additional imports needed in the test module.

Use `crate::testutils::{x_aux, x_stock, x_flow, x_model}` to construct model fixtures. Construct polarity `HashMap`s directly -- no salsa DB needed since `generate_relationships` is a pure function. Write one test function per AC sub-item:

- **`test_generate_relationships_basic_equation_deps`** (sdai-relationships.AC1.1): Build polarity map `{("A","C")->Positive, ("B","C")->Positive}` and model with an aux `C`. Assert output contains exactly two relationships: `{from:"A", to:"C"}` and `{from:"B", to:"C"}`.
- **`test_generate_relationships_flow_referencing_stock`** (sdai-relationships.AC1.2): Build polarity map `{("population","deaths")->Positive, ("death_rate","deaths")->Positive}` and model with flow `deaths`. Assert both relationships present.
- **`test_generate_relationships_no_equation_no_relationships`** (sdai-relationships.AC1.3): Build polarity map with entries only for variable `A`. Model contains `A` and `B` (no equation for `B`). Assert no relationships have `to: "B"`.
- **`test_generate_relationships_positive_polarity`** (sdai-relationships.AC2.1): Same setup as AC1.1. Assert both relationships have `polarity: Polarity::Positive`.
- **`test_generate_relationships_mixed_polarity`** (sdai-relationships.AC2.2): Build polarity map `{("A","C")->Positive, ("B","C")->Negative}`. Assert `from:"A"` has `Polarity::Positive`, `from:"B"` has `Polarity::Negative`.
- **`test_generate_relationships_unknown_polarity`** (sdai-relationships.AC2.3): Build polarity map with `("X","Y")->Unknown`. Assert relationship has `polarity: Polarity::Unknown`.

Also verify for all relationships: `reasoning` is `None` and `polarity_reasoning` is `None`.

**Verification:**

```bash
cargo test -p simlin-engine json_sdai
```

Expected: all new tests pass alongside existing tests.

**Commit:** `engine: add generate_relationships for SD-AI JSON polarity-based relationship generation`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for stock-flow filtering, deterministic ordering, and empty model

**Verifies:** sdai-relationships.AC3.1, sdai-relationships.AC3.2, sdai-relationships.AC3.3

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-engine/src/json_sdai.rs` (add tests in existing `#[cfg(test)] mod tests`)

**Testing:**

Add tests to the existing test module. Use a model representing the canonical population system: stock `population` with inflow `births` and outflow `deaths`, flow `deaths` with equation `population * death_rate`, aux `death_rate`.

Build a polarity map that contains BOTH structural edges and equation-derived edges:
- `("births", "population") -> Positive` -- structural (inflow to stock)
- `("deaths", "population") -> Negative` -- structural (outflow to stock)
- `("population", "deaths") -> Positive` -- equation-derived (stock referenced in flow equation)
- `("death_rate", "deaths") -> Positive` -- equation-derived

Tests must verify:
- **sdai-relationships.AC3.1:** Output does NOT contain `{from: "births", to: "population"}` (structural inflow edge filtered).
- **sdai-relationships.AC3.2:** Output does NOT contain `{from: "deaths", to: "population"}` (structural outflow edge filtered).
- **sdai-relationships.AC3.3:** Output DOES contain `{from: "population", to: "deaths"}` (equation-derived, stock -> flow direction, not filtered).

Additional edge case tests:
- **Deterministic ordering:** With multiple relationships, output is sorted by `(from, to)`. Build a polarity map with entries that would sort differently if unsorted. Assert output order matches lexicographic sort on `(from, to)`.
- **Empty model:** Empty polarity map + empty model (no variables) produces empty `Vec<Relationship>`.

**Verification:**

```bash
cargo test -p simlin-engine json_sdai
```

Expected: all tests pass.

**Commit:** `engine: add stock-flow filtering and edge case tests for generate_relationships`

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->
