# Incremental Diagram Layout Design

## Summary

This design adds an incremental diagram layout capability to simlin-engine. Today, every time the model changes, the layout system regenerates the entire diagram from scratch -- repositioning all elements even when only a single variable was added or removed. For AI-driven editing workflows through MCP and libsimlin, this means the diagram jumps unpredictably after each edit, destroying any manual arrangement the user has made.

The approach refactors the current monolithic `LayoutEngine` into a set of composable building blocks (chain layout, auxiliary placement, connector creation, etc.) that both the existing fresh-layout path and the new incremental path can share. The incremental orchestrator seeds its state from the existing diagram, applies patch operations (deletions, renames, additions) to that state, then runs force-directed placement and annealing only on newly added elements while pinning all existing elements in place. This means existing element positions, connector shapes, and cloud placements are preserved exactly, while new elements settle into good positions near their connections to existing structure. When no prior diagram exists, the system falls back to full layout generation.

## Definition of Done

1. **New engine-level function** that takes the old view, new model state, and applied patch, and produces an updated view with minimal changes to existing element positions.
2. **Deletions**: Elements for removed variables are deleted from the view (including associated links/clouds).
3. **Renames**: Elements for renamed variables have their names updated in-place, positions preserved.
4. **Additions**: New elements are placed near their connections to existing structure, with new chains getting proper internal stock-flow layout. Existing elements remain completely fixed.
5. **Settlement**: Force-directed + annealing runs on only the new elements to find good positions, with existing elements pinned.
6. **Fallback**: When no existing view exists, falls back to current full `generate_best_layout`.
7. **Both callers updated**: MCP `sync_diagram` and libsimlin `simlin_project_diagram_sync` use the new incremental path.
8. **Test suite**: Integration tests using default_projects models with add/edit/remove scenarios, verifying existing element positions are preserved and new elements are well-placed.

## Acceptance Criteria

### incremental-layout.AC1: Existing element positions preserved
- **AC1.1 Success:** Seeding LayoutState from an existing view and reconstructing produces a byte-identical StockFlow
- **AC1.2 Success:** After incremental layout with additions, all pre-existing Stock/Flow/Aux/Module elements have identical (x, y) coordinates
- **AC1.3 Edge:** Incremental layout on a model with no new/deleted/renamed elements produces an identical view (no-op patch)

### incremental-layout.AC2: Deletions remove elements and references
- **AC2.1 Success:** DeleteVariable removes the corresponding ViewElement from the view
- **AC2.2 Success:** Links referencing a deleted element's UID (from_uid or to_uid) are removed
- **AC2.3 Success:** Clouds referencing a deleted flow's UID (flow_uid) are removed
- **AC2.4 Edge:** Deleting all variables in a chain removes the entire chain's elements, flows, and clouds

### incremental-layout.AC3: Renames update identity, preserve position
- **AC3.1 Success:** RenameVariable updates the element's name and ident
- **AC3.2 Success:** Renamed element retains its original (x, y) position and UID

### incremental-layout.AC4: New elements placed near connections
- **AC4.1 Success:** A new auxiliary connected to existing variables is placed within a reasonable radius of those variables' centroid
- **AC4.2 Success:** A new stock-flow chain connected to existing structure is positioned near the connected elements
- **AC4.3 Success:** A new chain with no connections to existing structure is placed at the diagram periphery
- **AC4.4 Edge:** Multiple new auxiliaries clustering to the same connection point are spread apart (not stacked)

### incremental-layout.AC5: Connectors preserved and updated correctly
- **AC5.1 Success:** Links whose (from_uid, to_uid) still exist in the new dep_graph retain their original shape, arc angle, and polarity
- **AC5.2 Success:** Links whose endpoints no longer exist are removed
- **AC5.3 Success:** New dependency edges get links with default shapes (straight or structural arc)
- **AC5.4 Success:** After settlement, new elements have settled positions (not at their raw initial placement)
- **AC5.5 Edge:** A link between two existing variables that gains an intermediate new auxiliary: old direct link removed, two new links created

### incremental-layout.AC6: Fallback to full generation
- **AC6.1 Success:** When no existing view is present, incremental_layout delegates to generate_best_layout and produces a complete diagram

### incremental-layout.AC7: Both callers use incremental path
- **AC7.1 Success:** MCP sync_diagram uses incremental_layout when the model has existing views
- **AC7.2 Success:** libsimlin simlin_project_diagram_sync uses incremental_layout when the model has existing views

### incremental-layout.AC8: Test coverage
- **AC8.1 Success:** Each composable block has unit tests on synthetic inputs
- **AC8.2 Success:** Integration tests cover add/edit/remove scenarios on default_projects models
- **AC8.3 Success:** Combined operations (rename + add + delete in one patch) are tested

## Glossary

- **StockFlow**: The data model struct (`datamodel::StockFlow`) representing a complete stock-and-flow diagram view -- the collection of all visual elements (stocks, flows, auxiliaries, connectors, clouds) and their positions.
- **ViewElement**: An enum over the visual element types that can appear in a StockFlow view: `Aux`, `Stock`, `Flow`, `Link`, `Module`, `Alias`, `Cloud`, and `Group`. Each variant carries a UID and position.
- **UID**: A unique integer identifier assigned to each view element. UIDs are stable across incremental layout operations, enabling the three-way connector diff. Managed by `UidManager`.
- **ModelPatch**: A struct containing a list of `ModelOperation` values (upsert, delete, rename) targeting a specific model. The incremental layout reads the patch to determine what changed.
- **Chain**: A connected subgraph of stocks and their flows (e.g., Susceptible -> infection_rate -> Infected). Chains are the primary structural unit for layout.
- **dep_graph**: The dependency graph mapping each variable to the set of variables it depends on. Used to determine which connectors should exist and to compute initial positions for new elements.
- **SFDP (Scalable Force-Directed Placement)**: A graph layout algorithm that positions nodes by iterating repulsive and attractive forces until convergence. Implemented in `layout/sfdp.rs`.
- **Annealing**: A stochastic optimization pass (`layout/annealing.rs`) that reduces edge crossings by randomly perturbing node positions and accepting improvements. Runs interleaved with SFDP.
- **Pinned nodes**: Nodes in the `ConstrainedGraph` marked as immovable -- they receive zero force during SFDP and are excluded from annealing perturbation.
- **ConstrainedGraph**: Graph data structure (`layout/graph.rs`) supporting rigid groups (elements that move together) and pinned nodes (elements that do not move).
- **Link**: A `ViewElement` variant representing a connector arrow between two elements, identified by `(from_uid, to_uid)`. Links carry shape data (straight vs. arc), arc angles, and polarity.
- **Cloud**: A `ViewElement` variant representing a source or sink at a flow endpoint with no corresponding stock. Associated with a specific flow via `flow_uid`.
- **Three-way connector diff**: The algorithm for updating connectors incrementally: index old links by `(from_uid, to_uid)`, compute which pairs should exist from the new dep_graph, classify each as preserved, removed, or added.
- **Settlement**: Running SFDP and annealing on newly placed elements to refine their positions, with all existing elements pinned.
- **LayoutState**: The proposed shared mutable context struct replacing `LayoutEngine`'s fields. Each composable block operates on `&mut LayoutState`.
- **MCP (Model Context Protocol)**: Protocol for AI assistants to interact with external tools. Simlin's MCP server exposes model editing; `sync_diagram` regenerates the diagram after edits.

## Architecture

### Approach: Decompose Then Compose

Refactor the monolithic `LayoutEngine` into composable building blocks, then build both fresh and incremental layout paths from the same pieces. This eliminates code duplication while enabling incremental layout as a new composition of existing capabilities.

### LayoutState

A shared mutable context struct replaces `LayoutEngine`'s `&mut self` pattern:

```rust
pub struct LayoutState {
    pub elements: Vec<ViewElement>,
    pub positions: HashMap<i32, Position>,  // uid -> (x, y)
    pub uid_manager: UidManager,
    pub display_names: HashMap<String, String>,  // ident -> display name
    pub flow_templates: HashMap<String, FlowTemplate>,
    pub cloud_ident_to_uid: HashMap<String, i32>,
    pub cloud_ident_to_flow_ident: HashMap<String, String>,
    pub flow_ident_to_clouds: HashMap<String, Vec<String>>,
}
```

Each composable block takes `&mut LayoutState` plus its specific inputs (metadata, config) and returns `Result`. Two orchestrator functions compose the blocks differently.

### Composable Blocks

| Block | Source | Responsibility |
|-------|--------|---------------|
| `compute_metadata` | `layout/metadata.rs` (already standalone) | Chains, dep_graph, flow_to_stocks |
| `position_chains` | Extracted from `chain.rs` | SFDP + annealing on chain-level graph |
| `layout_chain` | Extracted from `layout_chain_at_position` | Stocks left-to-right, flows at midpoints within one chain |
| `place_auxiliaries` | Extracted from `layout_auxiliaries_and_connectors` | SFDP with rigid chain groups + interleaved annealing |
| `build_connectors` | Extracted from `create_connectors` | Links from dep_graph, skipping structural flow-stock edges |
| `build_clouds` | Extracted from `add_clouds_for_flow` | Clouds for flows missing a from/to stock |
| `optimize_labels` | Extracted from `apply_optimal_label_placement` | Minimizes label overlap |
| `apply_loop_curvature` | Extracted from `apply_feedback_loop_curvature` | Arc angles for feedback loop connectors |
| `normalize_coords` | Extracted from `normalize_coordinates` | Shift to DIAGRAM_ORIGIN_MARGIN |

SFDP (`sfdp.rs`) and annealing (`annealing.rs`) are already standalone library functions and require no changes.

### Fresh Layout Orchestrator

Composes all blocks in the same order as today's 7-phase pipeline. Behavior is identical to the current `LayoutEngine::generate_layout` -- this is a pure structural refactor. `generate_best_layout` still tries 4 seeds in parallel and picks fewest crossings.

### Incremental Layout Orchestrator

New public function:

```rust
pub fn incremental_layout(
    old_view: &StockFlow,
    project: &Project,
    model_name: &str,
    patch: &ModelPatch,
    db_state: Option<(&mut SimlinDb, SourceProject)>,
) -> Result<StockFlow>
```

Composition:

1. **Compute metadata** for the post-patch model (new chains, dep_graph, flow_to_stocks).
2. **Seed LayoutState from old view** -- populate elements, positions, uid_manager from existing view elements. Every existing element's UID and position is preserved.
3. **Process deletions** -- for each `DeleteVariable` in the patch, remove the ViewElement and associated links/clouds.
4. **Process renames** -- for each `RenameVariable`, update element name/ident in place. Position and UID unchanged.
5. **Identify new elements** -- walk new model's variables, find any without a corresponding element in state.
6. **Layout new chains** -- for new stock-flow chains, run `layout_chain` for internal arrangement, position near centroid of connected existing elements.
7. **Place new auxiliaries** -- run `place_auxiliaries` with all existing elements pinned and only new aux nodes free. Uses existing SFDP + interleaved annealing.
8. **Incremental connector update** -- three-way diff of connectors (see below). Rebuild clouds for changed flow-stock relationships.
9. **Polish** -- run `optimize_labels`, `apply_loop_curvature` (new links only). Skip `normalize_coords` to preserve absolute positions.

### Incremental Connector Update

Connectors are identified by their `(from_uid, to_uid)` pair. Since UIDs are stable for preserved elements, diffing is straightforward:

1. **Index old links** -- `HashMap<(from_uid, to_uid), ViewElement::Link>` capturing hand-crafted shapes, arc angles, and polarities.
2. **Compute new dependency edges** -- from post-patch dep_graph, determine `(from_uid, to_uid)` pairs that should exist.
3. **Three-way classification:**
   - **Preserved**: exists in both old and new -- keep old Link exactly as-is (shape, arc, polarity all preserved).
   - **Removed**: exists in old only -- delete.
   - **Added**: exists in new only -- create with default shape (straight, or arc for structural stock-flow edges).
4. **Clouds**: index by `flow_uid`. Add/remove as flow-stock relationships change. Existing clouds for unchanged relationships keep their positions.
5. **Feedback loop curvature**: apply only to newly-created links. Existing links retain their shapes.

### New Element Placement Strategy

Initial positions determine SFDP convergence quality:

- **New chains**: compute centroid of all existing elements connected to the new chain (via dep_graph). Place chain anchor at centroid + offset to avoid overlap. If no connections, place at bounding box periphery.
- **New auxiliaries**: place at centroid of their existing dependencies/dependents. Multiple new auxes clustering to the same spot spread in a small circle (8-point ring, consistent with existing strategy in `layout_auxiliaries_and_connectors`).
- **New clouds**: handled by `build_clouds` using flow position and orientation.
- **Pinning**: all existing elements are completely rigid (zero displacement) during SFDP + annealing settlement. Only new elements move.

## Existing Patterns

### Layout Pipeline Structure

The current `LayoutEngine` in `src/simlin-engine/src/layout/mod.rs` uses a 7-phase sequential pipeline with shared mutable state. This design preserves the pipeline concept while making the phases composable. The fresh layout orchestrator produces identical output.

### SFDP with Pinned Nodes

`ConstrainedGraph` already supports pinned nodes via `ConstrainedGraphBuilder::pin()`. Pinned nodes receive zero force in `apply_forces_with_rigid_constraints`. The incremental layout uses this existing mechanism to freeze existing elements.

### Annealing with Filter

`run_annealing_with_filter` already accepts a `can_perturb` predicate and per-node `displacement_limit`. The auxiliary layout phase already uses this to restrict perturbation to aux nodes while keeping chain elements rigid. The incremental layout reuses this same pattern to restrict perturbation to new elements.

### UidManager Seeding

`UidManager::add(uid, ident)` already supports seeding from existing UIDs (used in `LayoutEngine::new` to seed from model variable UIDs). The incremental layout extends this to seed from view element UIDs.

### No Pattern Divergence

This design introduces no new patterns. Every mechanism it relies on (pinned nodes, perturbation filtering, UID seeding, initial position seeding) already exists in the codebase and is used by the current layout system internally. The incremental layout composes these existing mechanisms differently.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Extract LayoutState and Composable Blocks

**Goal:** Refactor `LayoutEngine` into `LayoutState` + standalone functions without changing behavior.

**Components:**
- `LayoutState` struct in `src/simlin-engine/src/layout/mod.rs` -- shared mutable context extracted from `LayoutEngine` fields
- Standalone functions extracted from `LayoutEngine` methods: `build_connectors`, `build_clouds`, `optimize_labels`, `apply_loop_curvature`, `normalize_coords`, `place_auxiliaries`, `layout_chain`
- Fresh layout orchestrator function composing all blocks in the same order as today's pipeline
- `generate_best_layout` updated to use the new orchestrator

**Dependencies:** None (first phase)

**Done when:** All existing layout tests pass, `generate_best_layout` produces identical output (verified by snapshot comparison on default_projects models)
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Unit Tests for Extracted Blocks

**Goal:** Establish test coverage for each composable block in isolation.

**Components:**
- Unit tests for `build_connectors` -- synthetic LayoutState with known dep_graph, verify correct links produced
- Unit tests for `build_clouds` -- flows with missing endpoints, verify clouds created with correct flow_uid
- Unit tests for `optimize_labels` -- overlapping elements, verify label sides change to reduce overlap
- Unit tests for `layout_chain` -- single chain, verify stock/flow element creation and positioning
- Unit tests for `place_auxiliaries` -- pinned chain elements + free aux nodes, verify aux positions are near connections and chain positions unchanged

**Dependencies:** Phase 1 (extracted blocks exist)

**Covers:** incremental-layout.AC8.1 (block-level test coverage)

**Done when:** Each block has at least one test verifying core behavior on synthetic inputs
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Seed LayoutState from Existing View

**Goal:** Implement `LayoutState::from_existing_view` that populates all state from an old diagram.

**Components:**
- `LayoutState::from_existing_view(old_view: &StockFlow, model: &Model)` constructor in `src/simlin-engine/src/layout/mod.rs`
- Populates elements, positions, uid_manager, display_names, flow_templates, and cloud mappings from existing view elements
- Round-trip test: seed from view, reconstruct StockFlow, assert identical to input

**Dependencies:** Phase 1 (LayoutState struct exists)

**Covers:** incremental-layout.AC1.1 (state seeding preserves all view data)

**Done when:** Round-trip test passes -- seeding from a default_projects view and reconstructing produces an identical StockFlow
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Patch Processing (Delete, Rename, Connector Diff)

**Goal:** Implement the mutation logic that applies patch semantics to a seeded LayoutState.

**Components:**
- Deletion logic in `src/simlin-engine/src/layout/mod.rs` -- removes ViewElement for `DeleteVariable`, plus associated links (matching from_uid/to_uid) and clouds (matching flow_uid)
- Rename logic -- updates element name/ident for `RenameVariable`, preserves position and UID
- Three-way connector diff -- indexes old links by `(from_uid, to_uid)`, classifies as preserved/removed/added against new dep_graph
- Cloud diffing -- indexes old clouds by flow_uid, adds/removes based on changed flow-stock relationships

**Dependencies:** Phase 3 (seeded LayoutState), Phase 1 (build_connectors for creating new links)

**Covers:** incremental-layout.AC2.1, AC2.2, AC2.3 (deletion), incremental-layout.AC3.1, AC3.2 (rename), incremental-layout.AC5.1, AC5.2, AC5.3 (connector preservation)

**Done when:** Tests verify: deleted variables have no view elements or dangling references, renamed elements have new names but same positions/UIDs, preserved connectors retain hand-crafted shapes, removed connectors are gone, new connectors are created with default shapes
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: New Element Placement and Settlement

**Goal:** Implement initial positioning of new elements and pinned SFDP + annealing settlement.

**Components:**
- New element identification -- compare seeded (post-mutation) state against new model variables to find elements needing layout
- New chain layout -- `layout_chain` for internal arrangement, positioned near centroid of connected existing elements
- New auxiliary placement -- centroid-of-neighbors initial position, 8-point ring for clustering
- Pinned settlement -- `place_auxiliaries` with existing elements in `ConstrainedGraph.pinned`, only new nodes free to move
- Integration of placement into `incremental_layout` orchestrator function

**Dependencies:** Phase 4 (patch processing), Phase 1 (place_auxiliaries, layout_chain blocks)

**Covers:** incremental-layout.AC1.2 (existing positions unchanged), incremental-layout.AC4.1, AC4.2, AC4.3 (new element placement), incremental-layout.AC5.4 (settlement)

**Done when:** Integration tests with default_projects: existing element positions are byte-identical before and after, new elements are positioned near their connections (within bounding box of connected elements + margin), no overlapping elements
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Wire Up Callers and Fallback

**Goal:** Update MCP and libsimlin to use `incremental_layout` when an existing view is present.

**Components:**
- MCP `sync_diagram` in `src/simlin-mcp/src/tools/edit_model.rs` -- call `incremental_layout` when model has existing views, fall back to `generate_best_layout` when not
- libsimlin `simlin_project_diagram_sync` in `src/libsimlin/src/layout.rs` -- same conditional logic
- Both callers pass the relevant `ModelPatch` to `incremental_layout`
- Fallback path: when `old_view` has no elements or no views exist, delegates to `generate_best_layout`

**Dependencies:** Phase 5 (incremental_layout function complete)

**Covers:** incremental-layout.AC6.1 (fallback), incremental-layout.AC7.1, AC7.2 (both callers updated)

**Done when:** End-to-end tests through both MCP and libsimlin code paths verify incremental behavior. Models with no existing view get full layout generation.
<!-- END_PHASE_6 -->

## Additional Considerations

**Absolute position preservation:** `normalize_coords` is skipped in the incremental path to avoid shifting existing elements. If the diagram grows significantly due to new elements, the view_box expands but existing coordinates remain stable.

**UID stability across patches:** The incremental layout depends on UIDs being stable identifiers for existing elements. Since `UidManager::add` seeds from existing view UIDs and new allocations start past the maximum existing UID, there is no collision risk.

**Multiple patches in sequence:** The design supports applying multiple patches incrementally. Each call to `incremental_layout` takes the current view state, so chaining patches works naturally -- the output of one becomes the input to the next.
