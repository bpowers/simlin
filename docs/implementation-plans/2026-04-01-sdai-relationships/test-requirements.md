# SD-AI Relationships - Test Requirements

## Automated Tests

### sdai-relationships.AC1: Equation-derived dependencies become relationships

#### sdai-relationships.AC1.1 -- Auxiliary equation deps produce relationships

- **Criterion:** Auxiliary `C = A + B` produces `{from: "A", to: "C"}` and `{from: "B", to: "C"}` in relationships array
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_basic_equation_deps`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Construct a `HashMap<(String, String), LinkPolarity>` with entries `("A","C") -> Positive` and `("B","C") -> Positive`. Build a datamodel with an aux `C` (via `x_aux`). Call `generate_relationships()` and assert the output contains exactly two relationships with the expected `from`/`to` pairs.

#### sdai-relationships.AC1.2 -- Flow referencing stock produces relationships

- **Criterion:** Flow equation referencing a stock (e.g. `deaths = population * death_rate`) produces `{from: "population", to: "deaths"}` and `{from: "death_rate", to: "deaths"}`
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_flow_referencing_stock`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Construct polarity map with `("population","deaths") -> Positive` and `("death_rate","deaths") -> Positive`. Build a model with flow `deaths` (via `x_flow`). Call `generate_relationships()` and assert both relationships are present.

#### sdai-relationships.AC1.3 -- Variable with no equation produces no inbound relationships

- **Criterion:** Variable with no equation produces no relationships with that variable as `to`
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_no_equation_no_relationships`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Construct a polarity map containing only edges targeting variable `A` (not `B`). Build a model containing both `A` and `B`. Call `generate_relationships()` and assert no relationship has `to: "B"`.

### sdai-relationships.AC2: Polarity is computed correctly

#### sdai-relationships.AC2.1 -- Addition yields positive polarity

- **Criterion:** `C = A + B` yields polarity `"+"` for both relationships
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_positive_polarity`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Same fixture as AC1.1 (both polarities are `LinkPolarity::Positive`). Assert both output relationships have `polarity: Polarity::Positive`. Also assert `reasoning` is `None` and `polarity_reasoning` is `None`.

#### sdai-relationships.AC2.2 -- Subtraction yields mixed polarity

- **Criterion:** `C = A - B` yields polarity `"+"` for A, `"-"` for B
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_mixed_polarity`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Construct polarity map with `("A","C") -> Positive` and `("B","C") -> Negative`. Call `generate_relationships()` and assert `from:"A"` has `Polarity::Positive` and `from:"B"` has `Polarity::Negative`.

#### sdai-relationships.AC2.3 -- Indeterminate expression yields unknown polarity

- **Criterion:** Expression where polarity is indeterminate yields `"?"`
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_unknown_polarity`
- **Phase/Task:** Phase 1, Task 1
- **Approach:** Construct polarity map with `("X","Y") -> Unknown`. Call `generate_relationships()` and assert the relationship has `polarity: Polarity::Unknown`.

### sdai-relationships.AC3: Stock-flow structural edges are excluded

#### sdai-relationships.AC3.1 -- Stock inflow structural edge excluded

- **Criterion:** Stock with inflow `births` does NOT produce `{from: "births", to: "population"}`
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_stock_flow_filtering` (shared test for AC3.1, AC3.2, AC3.3)
- **Phase/Task:** Phase 1, Task 2
- **Approach:** Build a population model: stock `population` with inflows=`["births"]` and outflows=`["deaths"]`, flow `deaths`, aux `death_rate`. Construct a polarity map containing both structural edges (`("births","population") -> Positive`, `("deaths","population") -> Negative`) and equation-derived edges (`("population","deaths") -> Positive`, `("death_rate","deaths") -> Positive`). Assert the output does NOT contain `{from: "births", to: "population"}`.

#### sdai-relationships.AC3.2 -- Stock outflow structural edge excluded

- **Criterion:** Stock with outflow `deaths` does NOT produce `{from: "deaths", to: "population"}`
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_stock_flow_filtering` (same test as AC3.1)
- **Phase/Task:** Phase 1, Task 2
- **Approach:** Same fixture as AC3.1. Assert the output does NOT contain `{from: "deaths", to: "population"}`.

#### sdai-relationships.AC3.3 -- Equation-derived stock reference IS included

- **Criterion:** Flow's equation referencing a stock (equation-derived) IS included
- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/json_sdai.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `test_generate_relationships_stock_flow_filtering` (same test as AC3.1)
- **Phase/Task:** Phase 1, Task 2
- **Approach:** Same fixture as AC3.1. Assert the output DOES contain `{from: "population", to: "deaths"}` (equation-derived dependency, reversed direction from the structural edge).

### sdai-relationships.AC4: Integration with SD-AI JSON serialization

#### sdai-relationships.AC4.1 -- EditModel produces generated relationships

- **Criterion:** Editing an SD-AI JSON model via `handle_edit_model` produces output containing generated `relationships` array
- **Test type:** Integration
- **Test file:** `src/simlin-mcp/src/tools/edit_model.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `sdai_relationships_generated_after_edit` (replaces current `sdai_relationships_preserved_after_edit`)
- **Phase/Task:** Phase 2, Task 1
- **Approach:** Write an SD-AI JSON fixture to a temp file (stock `population` with eq="1000", flow `births` with eq="population * 0.03"). Apply an `upsertAuxiliary` operation for `birth_rate`. Read back the saved file. The fixture's stock has no `inflows`/`outflows` fields in the SD-AI JSON, so after conversion to datamodel there are no stock-flow structural edges to filter. The only equation-derived dependency is `births` referencing `population`, producing exactly one generated relationship: `{from: "population", to: "births", polarity: "+"}`. Assert the relationships array exists and contains this relationship.

#### sdai-relationships.AC4.2 -- No reasoning or polarityReasoning fields in output

- **Criterion:** No `reasoning` or `polarityReasoning` fields appear in output
- **Test type:** Integration
- **Test file:** `src/simlin-mcp/src/tools/edit_model.rs` (`#[cfg(test)] mod tests`)
- **Test function:** `sdai_relationships_generated_after_edit` (same test as AC4.1)
- **Phase/Task:** Phase 2, Task 1
- **Approach:** In the same test as AC4.1, after parsing the saved JSON, access the relationships array as raw `serde_json::Value` objects. Assert that `rels[0].get("reasoning")` is `None` and `rels[0].get("polarityReasoning")` is `None`. These fields use `#[serde(skip_serializing_if = "is_none")]`, so they must be entirely absent from the JSON output, not present with null values.

## Supplementary Tests (not directly AC-mapped)

These tests verify properties not tied to a single acceptance criterion but essential for correctness:

| Test function | File | Type | What it verifies |
|---|---|---|---|
| `test_generate_relationships_deterministic_ordering` | `json_sdai.rs` | Unit | Output is sorted lexicographically by `(from, to)`, ensuring deterministic serialization regardless of HashMap iteration order |
| `test_generate_relationships_empty_model` | `json_sdai.rs` | Unit | Empty polarity map + empty model produces empty `Vec<Relationship>` |
| `sdai_relationships_preserved_on_dry_run` | `edit_model.rs` | Integration | Dry-run mode does not modify the file on disk (existing test, no changes needed) |
| `sdai_relationships_reflect_current_equations_after_remove` | `edit_model.rs` | Integration | After removing a variable, generated relationships reflect only current equations (replaces `remove_variable_filters_stale_sdai_relationships`) |

## Human Verification

No acceptance criteria require human verification. All criteria are fully testable through automated unit and integration tests:

- AC1.x and AC2.x test a pure function (`generate_relationships`) with constructed inputs, requiring no human judgment.
- AC3.x test filtering logic against known structural edges, fully deterministic.
- AC4.x test end-to-end serialization through the MCP tool, verifiable by parsing JSON output.

The polarity computation itself (the correctness of `compute_link_polarities()`) is outside the scope of this design -- it is pre-existing infrastructure with its own test suite in `ltm.rs`, `db_analysis.rs`, and `db_ltm_tests.rs`. The `generate_relationships` function treats the polarity map as an opaque input and is tested independently of the polarity analysis engine.

## Coverage Matrix

| Acceptance Criterion | Test Type | Test File | Test Function(s) | Phase.Task |
|---|---|---|---|---|
| sdai-relationships.AC1.1 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_basic_equation_deps` | P1.T1 |
| sdai-relationships.AC1.2 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_flow_referencing_stock` | P1.T1 |
| sdai-relationships.AC1.3 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_no_equation_no_relationships` | P1.T1 |
| sdai-relationships.AC2.1 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_positive_polarity` | P1.T1 |
| sdai-relationships.AC2.2 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_mixed_polarity` | P1.T1 |
| sdai-relationships.AC2.3 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_unknown_polarity` | P1.T1 |
| sdai-relationships.AC3.1 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_stock_flow_filtering` | P1.T2 |
| sdai-relationships.AC3.2 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_stock_flow_filtering` | P1.T2 |
| sdai-relationships.AC3.3 | Unit | `src/simlin-engine/src/json_sdai.rs` | `test_generate_relationships_stock_flow_filtering` | P1.T2 |
| sdai-relationships.AC4.1 | Integration | `src/simlin-mcp/src/tools/edit_model.rs` | `sdai_relationships_generated_after_edit` | P2.T1 |
| sdai-relationships.AC4.2 | Integration | `src/simlin-mcp/src/tools/edit_model.rs` | `sdai_relationships_generated_after_edit` | P2.T1 |

**Total: 11 acceptance criteria, all covered by automated tests (8 unit, 3 integration assertions across 2 integration test functions).**
