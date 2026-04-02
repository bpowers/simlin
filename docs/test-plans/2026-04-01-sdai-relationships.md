# SD-AI Relationships Array Generation -- Human Test Plan

## Prerequisites

- Environment initialized: `./scripts/dev-init.sh`
- All automated tests passing:
  - `cargo test -p simlin-engine -- test_generate_relationships` (unit tests in `json_sdai.rs`)
  - `cargo test -p simlin-mcp -- sdai_relationships` (integration tests in `edit_model.rs`)

## Phase 1: Unit-level relationship generation

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine -- test_generate_relationships` | All tests pass (AC-mapped + supplementary) |
| 2 | Inspect test output for any warnings or unexpected messages | Clean output with no warnings |

## Phase 2: Integration-level MCP tool verification

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-mcp -- sdai_relationships` | All 3 tests pass (`generated_after_edit`, `preserved_on_dry_run`, `reflect_current_equations_after_remove`) |
| 2 | Inspect test output for any warnings or unexpected messages | Clean output with no warnings |

## End-to-End: Full MCP EditModel round-trip

Verify that a real MCP call flow produces correct relationships in the output JSON file, exercising the full pipeline from SD-AI JSON parsing through polarity computation to serialization.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Create a temporary SD-AI JSON file with: `{"variables": [{"type": "stock", "name": "population", "equation": "1000"}, {"type": "flow", "name": "births", "equation": "population * 0.03"}], "specs": {"startTime": 0.0, "stopTime": 100.0}}` | File created successfully |
| 2 | Build and run the MCP server: `cargo build -p simlin-mcp` | Binary compiles without errors |
| 3 | Send an MCP `tools/call` request for `editModel` with `projectPath` pointing to the temp file, and operations `[{"upsertAuxiliary": {"name": "birth_rate", "equation": "0.03"}}]` | Tool responds with success result containing `model` and `relationships` |
| 4 | Read the saved file and parse as JSON | File contains valid JSON |
| 5 | Inspect the `relationships` array | Contains exactly one entry: `{"from": "population", "to": "births", "polarity": "+"}` |
| 6 | Verify no `reasoning` or `polarityReasoning` keys exist on any relationship object | Fields are absent (not present with null values) |

## End-to-End: Variable removal updates relationships

Verify that removing a variable causes relationships to be regenerated from current equations, not preserved from the stale input file.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Create SD-AI JSON with three variables: `A` (eq: "10"), `B` (eq: "A * 2"), `C` (eq: "A + 1"), and relationships `[A->B, B->C, A->C]` | File created |
| 2 | Call `editModel` with `removeVariable` for `B` | Tool responds with success |
| 3 | Read saved file | `relationships` array contains exactly one entry: `{"from": "a", "to": "c", "polarity": "+"}` (identifiers canonicalized to lowercase) |
| 4 | Verify the stale `B->C` and `A->B` relationships no longer appear | Only equation-derived relationships from current model state are present |

## End-to-End: Stock-flow structural edges excluded in real model

Verify that when a model has stocks with inflows/outflows declared in the SD-AI JSON, the structural edges do not appear in the generated relationships.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Create SD-AI JSON with stock `population` (eq: "1000", inflows: ["births"], outflows: ["deaths"]), flow `births` (eq: "population * birth_rate"), flow `deaths` (eq: "population * death_rate"), aux `birth_rate` (eq: "0.03"), aux `death_rate` (eq: "0.01") | File created |
| 2 | Call `editModel` with a trivial upsert (e.g., update `birth_rate` equation to "0.04") | Tool responds with success |
| 3 | Read saved file and inspect `relationships` | Contains equation-derived edges (`population->births`, `birth_rate->births`, `population->deaths`, `death_rate->deaths`) but NOT structural edges (`births->population`, `deaths->population`) |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| sdai-relationships.AC1.1 | `test_generate_relationships_basic_equation_deps` | Phase 1 |
| sdai-relationships.AC1.2 | `test_generate_relationships_flow_referencing_stock` | Phase 1 |
| sdai-relationships.AC1.3 | `test_generate_relationships_no_equation_no_relationships` | Phase 1 |
| sdai-relationships.AC2.1 | `test_generate_relationships_positive_polarity` | Phase 1 |
| sdai-relationships.AC2.2 | `test_generate_relationships_mixed_polarity` | Phase 1 |
| sdai-relationships.AC2.3 | `test_generate_relationships_unknown_polarity` | Phase 1 |
| sdai-relationships.AC3.1 | `test_generate_relationships_filters_stock_flow_structural_edges` | E2E: Stock-flow |
| sdai-relationships.AC3.2 | `test_generate_relationships_filters_stock_flow_structural_edges` | E2E: Stock-flow |
| sdai-relationships.AC3.3 | `test_generate_relationships_filters_stock_flow_structural_edges` | E2E: Stock-flow |
| sdai-relationships.AC4.1 | `sdai_relationships_generated_after_edit` | E2E: Round-trip |
| sdai-relationships.AC4.2 | `sdai_relationships_generated_after_edit` | E2E: Round-trip |
