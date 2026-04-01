# SD-AI Relationships Array Generation Design

## Summary

This design replaces the current "re-attach from original file" approach to the `relationships` array in SD-AI JSON output with on-the-fly generation from the model's equation dependency graph and symbolic polarity analysis. Today, when Simlin writes an SD-AI JSON model, it re-parses the original file to recover the `relationships` array (which is not captured in `datamodel::Project`) and filters out stale entries. The new approach generates relationships by walking the equation ASTs to extract variable dependencies, computing the sign (positive, negative, or unknown) of each causal link via the existing `compute_link_polarities()` infrastructure, and emitting one `Relationship` struct per equation-derived edge.

The implementation is a pure function (`generate_relationships`) in `json_sdai.rs` that takes the pre-computed polarity map and the `datamodel::Model`, builds a set of stock-flow structural edges (stock inflow/outflow connections) to exclude them (since the downstream SD-AI conformance evaluator generates those itself), and maps the remaining equation-derived edges to `Relationship` values with computed polarity. The function is called from the `SdaiJson` serialization path in `edit_model.rs`, replacing the re-parse-and-filter logic. This removes `filter_stale_relationships()` as dead code and ensures that the relationships array always reflects the current equation state.

## Definition of Done

When Simlin serializes a model to SD-AI JSON format (the `SourceFormat::SdaiJson` path in `edit_model.rs`), the output includes a `relationships` array generated from the model's equation dependencies and computed polarities. Specifically:

1. **Each equation-derived dependency becomes a relationship**: if variable A's equation references variable B, emit `{from: "B", to: "A", polarity: "+"|"-"|"?"}`.
2. **Polarity is computed via existing symbolic analysis** (`compute_link_polarities()` / `ltm.rs`).
3. **Stock-flow structural relationships are excluded**: no `{from: flow, to: stock}` entries (the conformance eval generates these itself via `makeRelationshipsFromStocks()`).
4. **No `reasoning` or `polarityReasoning` fields** are emitted.
5. **The current "re-attach from original file" logic is replaced** by generation -- relationships always reflect current equation state.

**Out of scope:**
- Changes to internal model representation (no `relationships` field in `datamodel::Project`)
- Changes to CreateModel/EditModel/ReadModel tool API signatures
- Native Simlin JSON format changes
- Round-tripping relationships from input files

## Acceptance Criteria

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

### sdai-relationships.AC4: Integration with SD-AI JSON serialization
- **sdai-relationships.AC4.1 Success:** Editing an SD-AI JSON model via `handle_edit_model` produces output containing generated `relationships` array
- **sdai-relationships.AC4.2 Success:** No `reasoning` or `polarityReasoning` fields appear in output

## Glossary

- **SD-AI JSON**: A JSON serialization format for AI-generated system dynamics models (`SdaiModel` in `json_sdai.rs`). Uses a flatter structure than native Simlin JSON, with a discriminated union for variables. The `relationships` array describes causal links between variables.
- **Polarity (causal link)**: The sign of the causal effect between two variables. `"+"` means an increase in the source causes an increase in the target; `"-"` means an increase causes a decrease; `"?"` means the direction cannot be determined statically.
- **Stock-flow structural edge**: A relationship between a flow and its associated stock that exists due to the stock-flow structure itself (e.g., inflow feeding a stock), as opposed to an equation-derived dependency. Excluded from generated `relationships` because the SD-AI conformance evaluator produces them independently via `makeRelationshipsFromStocks()`.
- **Equation-derived dependency**: A causal link that exists because one variable's equation references another. If `deaths = population * death_rate`, then `population` and `death_rate` are equation-derived dependencies of `deaths`.
- **`compute_link_polarities()`**: A salsa-tracked query in `db_analysis.rs` that computes polarity for every causal link by recursively walking equation ASTs. Handles arithmetic operators, monotonic functions, and lookup table monotonicity.
- **salsa**: An incremental computation framework used by simlin-engine. Functions decorated with `#[salsa::tracked]` cache results and recompute only when inputs change.
- **LTM (Loops That Matter)**: Loop dominance analysis framework in `ltm.rs`. The polarity analysis infrastructure used by this design was originally built for LTM's feedback loop detection and scoring.
- **Conformance evaluator**: The SD-AI evaluation system that scores AI-generated models. Independently generates stock-flow structural relationships via `makeRelationshipsFromStocks()`, which is why the generated `relationships` array excludes those edges.
- **AST (Abstract Syntax Tree)**: Parsed representation of a variable's equation. simlin-engine uses progressive lowering (`Expr0` through `Expr3`). Polarity analysis walks the `Expr2` stage.

## Architecture

Pure function in `json_sdai.rs` that takes pre-computed polarity data and a datamodel, and produces the relationships array. The caller in `edit_model.rs` orchestrates: it computes polarities via the existing salsa query, then passes the result to the generation function.

**Core function:**

```rust
pub fn generate_relationships(
    polarities: &HashMap<(String, String), LinkPolarity>,
    model: &datamodel::Model,
) -> Vec<Relationship>
```

**Data flow:**

1. `edit_model.rs` calls `compute_link_polarities(&db, source_model, sync.project)` -- the existing salsa-tracked query that walks equation ASTs and determines polarity for every causal link.
2. The polarity map (containing all edges including stock-flow structural ones) is passed to `generate_relationships()` along with the `datamodel::Model`.
3. `generate_relationships()` builds a set of stock-flow structural edges from the model's Stock `inflows`/`outflows`, filters those out of the polarity map, maps the remainder to `Relationship` structs, and returns a deterministically sorted `Vec<Relationship>`.
4. The caller sets `sdai_model.relationships = Some(result)` before serialization.

**Type mapping:** `LinkPolarity::Positive` -> `Polarity::Positive` (`"+"`), `LinkPolarity::Negative` -> `Polarity::Negative` (`"-"`), `LinkPolarity::Unknown` -> `Polarity::Unknown` (`"?"`). `reasoning` and `polarity_reasoning` are always `None`.

**Replaces:** The current logic that re-parses the original file to recover `relationships` and calls `filter_stale_relationships()`.

## Existing Patterns

The design leverages three existing patterns:

1. **Polarity analysis** (`src/simlin-engine/src/ltm.rs`): `compute_link_polarities()` already computes the sign of every causal link via recursive AST walking. It handles addition, subtraction, multiplication, division, monotonic functions, and graphical function (lookup table) monotonicity. This is the same infrastructure used for loop dominance analysis.

2. **Causal edge extraction** (`src/simlin-engine/src/db_analysis.rs`): `model_causal_edges()` builds the complete dependency graph including stock-flow structural edges and equation-derived edges. `compute_link_polarities()` uses this internally via `causal_graph_with_modules()`.

3. **SD-AI JSON serialization** (`src/simlin-engine/src/json_sdai.rs`): `SdaiModel::from(&Project)` converts the internal datamodel to SD-AI JSON format. The `Relationship` struct and `Polarity` enum with serde rename attributes (`"+"`, `"-"`, `"?"`) are already defined and tested.

No new patterns are introduced. The generation function is a composition of existing infrastructure with a filtering step.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Core Generation Function and Tests

**Goal:** Implement and test the pure `generate_relationships()` function.

**Components:**
- `generate_relationships()` function in `src/simlin-engine/src/json_sdai.rs` -- takes polarity map and datamodel, filters stock-flow edges, maps to `Vec<Relationship>`
- Unit tests in the same file's `#[cfg(test)]` module

**Covers:** sdai-relationships.AC1.1, sdai-relationships.AC1.2, sdai-relationships.AC1.3, sdai-relationships.AC2.1, sdai-relationships.AC2.2, sdai-relationships.AC3.1, sdai-relationships.AC3.2, sdai-relationships.AC3.3

**Dependencies:** None

**Done when:** Unit tests pass covering: basic equation deps, stock-flow filtering, mixed polarities, unknown polarity, deterministic ordering, empty model
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Integration and Cleanup

**Goal:** Wire generation into the SD-AI JSON serialization path and remove dead code.

**Components:**
- `SdaiJson` match arm in `src/simlin-mcp/src/tools/edit_model.rs` -- replace re-attach logic with generation calls
- Remove `filter_stale_relationships()` from `src/simlin-engine/src/json_sdai.rs`
- Integration test in `src/simlin-mcp/src/tools/edit_model.rs` verifying generated relationships in round-trip

**Covers:** sdai-relationships.AC4.1, sdai-relationships.AC4.2

**Dependencies:** Phase 1

**Done when:** Editing an SD-AI JSON model produces output with generated relationships array; `filter_stale_relationships()` is removed; existing tests pass
<!-- END_PHASE_2 -->

## Additional Considerations

**Edge case -- equation-less variables:** If a variable has no equation, no relationships target it (since generation is equation-derived). This is correct behavior: the sd-ai `behavioralPattern` eval uses `SDJsonToXMILE()` which generates `NAN(causes)` for equation-less variables, but an agent should produce complete models. Variables without equations referenced by other variables' equations still appear as `from` in relationships.

**Edge case -- unknown polarity:** When the symbolic analyzer can't determine polarity (complex expressions, conditional logic), `"?"` is emitted. The sd-ai `quantitativeCausalReasoning` eval checks `rel.polarity === reqRel.polarity`, so `"?"` won't match a required `"+"` or `"-"`. This is correct -- better to be honest about uncertainty than guess wrong.
