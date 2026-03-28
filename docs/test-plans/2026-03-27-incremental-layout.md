# Human Test Plan: Incremental Layout

## Prerequisites
- Environment initialized: `./scripts/dev-init.sh`
- All automated tests passing: `cargo test -p simlin-engine && cargo test -p simlin-mcp && cargo test -p libsimlin`

## Phase 1: Round-Trip Fidelity

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Open `test/test-models/samples/SIR/SIR.stmx` in the Simlin app. Observe the stock-flow diagram layout. | Diagram renders with 3 stocks (susceptible, infectious, recovered), 2 flows (succumbing, recovering), and 3 auxiliaries (total_population, duration, contact_infectivity). All connectors are visible. |
| 1.2 | Trigger diagram sync (e.g., via MCP `edit_model` with a no-op or by saving with no changes). Compare the resulting view positions to the pre-sync positions. | Every element retains its exact position. No visual movement is perceptible. |
| 1.3 | Open `test/test-models/samples/teacup/teacup.stmx`. Repeat the same no-op sync. | Same result: all positions preserved, no visual shift. |

## Phase 2: Incremental Add

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Start with the SIR model. Using MCP `edit_model` (or the app), add a new auxiliary `vaccination_rate` with equation `susceptible * 0.01`. | A new element named "vaccination rate" appears in the diagram. |
| 2.2 | Inspect the position of the new element relative to "susceptible". | "vaccination rate" is placed near "susceptible" (its only dependency), not overlapping another element. |
| 2.3 | Verify the 3 original stocks, 2 flows, and remaining auxiliaries have not moved from their pre-edit positions. | No existing element has visually shifted. |
| 2.4 | Check that a connector (link) from "susceptible" to "vaccination rate" is present. | A directed connector arrow exists from susceptible to vaccination_rate. |

## Phase 3: Incremental Delete

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Starting from the original SIR model, delete `contact_infectivity`. | The "contact infectivity" element disappears from the diagram. |
| 3.2 | Verify that the connector from "contact infectivity" to "succumbing" is also removed. | No dangling connectors exist. All remaining connectors connect two visible elements. |
| 3.3 | Verify remaining elements have not moved. | Stocks, flows, and other auxiliaries retain their positions. |

## Phase 4: Incremental Rename

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Starting from the original SIR model, rename `total_population` to `total_pop`. | The element previously labeled "total population" now reads "total pop". |
| 4.2 | Verify its position has not changed. | The renamed element sits in exactly the same spot. |
| 4.3 | Verify connectors involving the renamed variable are intact. | All connectors that previously referenced total_population now reference total_pop, with no broken or missing arrows. |

## Phase 5: Combined Operations

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | Starting from the original SIR model, apply a combined patch: (a) delete `contact_infectivity`, (b) rename `total_population` to `total_pop`, (c) add `immunity_rate = 1/duration` and change recovering to `infectious * immunity_rate`. | All three operations take effect in a single update. |
| 5.2 | Verify `contact_infectivity` is gone, `total_pop` is present at the old `total_population` position, and `immunity_rate` is a new element near `duration` and `recovering`. | Visual layout matches expectations: deleted element absent, renamed element in place, new element positioned logically. |
| 5.3 | Verify the connector chain: `duration -> immunity_rate -> recovering` (two links). The old direct `duration -> recovering` link should be absent. | Two new connectors replace the old single connector. |

## Phase 6: Fallback to Full Layout

| Step | Action | Expected |
|------|--------|----------|
| 6.1 | Create a new model from scratch (no existing views) using MCP `create_model` or the app. Add at least one stock, one flow, and one auxiliary. Trigger diagram sync. | A complete diagram is generated from scratch, covering all model variables with connectors. |
| 6.2 | Verify the generated layout is reasonable: no overlapping elements, connectors are visible, stocks are connected to their flows. | Layout is legible without manual rearrangement. |

## Phase 7: MCP and libsimlin Caller Integration

| Step | Action | Expected |
|------|--------|----------|
| 7.1 | Using the MCP `edit_model` tool with a JSON project file that has an existing view, add a new variable. | The response includes the updated view with the new element and preserved original positions. |
| 7.2 | Using `simlin_project_diagram_sync` through the WASM/FFI path, apply a patch to a model with existing views. | The sync produces an updated view with incremental changes, not a full regeneration. |

## End-to-End: Multi-Step Editing Session

Purpose: Validate that sequential incremental edits accumulate correctly without layout drift.

| Step | Action | Expected |
|------|--------|----------|
| E2E.1 | Start with the SIR model. Record all element positions. | Baseline positions captured. |
| E2E.2 | Add `vaccination_rate = susceptible * 0.01`. Record positions. | Original positions unchanged. vaccination_rate placed near susceptible. |
| E2E.3 | Add `immunity_loss = recovered * 0.001`. Record positions. | Previous positions (including vaccination_rate from step E2E.2) unchanged. immunity_loss placed near recovered. |
| E2E.4 | Delete `contact_infectivity`. Record positions. | contact_infectivity gone. All remaining positions (including both new variables) unchanged. |
| E2E.5 | Rename `duration` to `recovery_time`. Record positions. | Element at duration's old position now labeled "recovery time". All other positions unchanged. |
| E2E.6 | Compare final positions of the 3 original stocks against the step E2E.1 baseline. | Stocks have not moved from their original positions through 4 incremental edits. |

## Notes

The test-requirements document states: "No acceptance criteria require human verification. All criteria are automatable." The manual steps above are provided for end-to-end confidence and edge-case coverage around subjective layout quality, but every acceptance criterion is already covered by automated tests.
