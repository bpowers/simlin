# Module Editing -- Human Test Plan

Generated: 2026-04-08

## Prerequisites

- Development environment running (`./scripts/dev-init.sh` completed)
- All automated tests passing: `cd src/diagram && npx jest --testPathPatterns='module-' --no-coverage` (158 tests across 8 suites)
- Application running locally
- At least one project open in the editor

## Phase 1: Module Creation

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Open a project in the editor. Click the SpeedDial (floating action button) in the bottom-right corner. | SpeedDial opens and shows tool options including a "Module" icon alongside Stock, Flow, Aux, and Link. |
| 1.2 | Select the Module tool from the SpeedDial. | The cursor changes to indicate module placement mode. |
| 1.3 | Click on the canvas at an empty location. | A new module element appears at the click location. An inline text editor is immediately active on the module, ready for name input. |
| 1.4 | Type "hares" and press Enter. | The module is named "hares". The inline editor closes. The module appears as a box labeled "hares" on the canvas. |
| 1.5 | Select the module tool again, click on the canvas, then immediately press Escape without typing. | The new module is removed from the canvas entirely. No orphaned or unnamed module remains. |
| 1.6 | Observe the newly created "hares" module on the canvas. | There is NO orange/red warning dot on the module. Since no module in this model has a model reference yet, warnings are suppressed. |
| 1.7 | Select the "hares" module. In the details panel on the right, find the "Model Reference" dropdown. Select "Create new model..." from the dropdown. | A new model is created (e.g., "hares" or similar name). The model reference dropdown now shows this model as selected. |
| 1.8 | Create a second module named "foxes" on the canvas. Do not assign a model reference to it. | The "foxes" module now shows a warning indicator (orange/red dot) because there is at least one module in the model that HAS a reference (the "hares" module), so warnings are no longer globally suppressed. |
| 1.9 | Select the "foxes" module. In the Model Reference dropdown, observe the available options. | The dropdown shows: project models (excluding the current model and any that would create cycles), stdlib models (delay1, delay3, npv, smth1, smth3, etc.), "Create new model...", and "Duplicate..." (only if a reference is already set). |
| 1.10 | Assign a stdlib model (e.g., "delay1") as the reference for the foxes module. | The warning indicator disappears from the foxes module. The model reference dropdown shows "delay1" selected. |

## Phase 2: Module Details Panel

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Select a module that has a model reference set (e.g., the "hares" module from Phase 1). | The right panel shows ModuleDetails: module name as header, Model Reference dropdown, Input Wiring section, Output Ports section, Units editor, Documentation editor, Delete button. There is NO equation editor. |
| 2.2 | Look at the Model Reference dropdown. | It displays the currently referenced model name (e.g., "hares"). |
| 2.3 | In the details panel, find the Units editor (placeholder text like "Units"). Type "individuals" and click away (blur). | The units value persists. Re-selecting the module shows "individuals" in the units field. |
| 2.4 | In the details panel, find the Documentation editor (placeholder like "Notes" or "Documentation"). Type "Number of hares in ecosystem" and blur. | The documentation value persists. Re-selecting the module shows the typed text. |
| 2.5 | Look at the "Input Wiring" section. Click "Add Input". | A new empty row appears in the input wiring table with two dropdowns: one for selecting a source variable (from the parent model) and one for selecting a destination input port (from the referenced child model). |
| 2.6 | In the new wiring row, select a source variable from the parent model dropdown. Then select a destination input port from the child model dropdown. | The wiring row shows the selected src and dst values. The dropdowns show the correct available variables: src shows stocks/flows/auxes from the parent model (no modules), dst shows only variables marked as input ports in the child model. |
| 2.7 | Click the "X" or remove button on the wiring row. | The wiring row is removed. The "No inputs configured" message reappears if this was the only row. |
| 2.8 | Look at the "Output Ports" section. | It lists all public variables from the referenced model. If the referenced model has no public variables, it shows "No public outputs". |
| 2.9 | Click the "Open Model" button. | The editor navigates into the referenced model (drill-in). The breadcrumb bar at the top updates to show the navigation path. |

## Phase 3: Hierarchical Navigation

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | With the editor at the root model (breadcrumb shows just the model name, hamburger menu icon on the left), double-click on a configured module. | The editor navigates into the child model. The canvas shows the child model's elements. The breadcrumb bar now shows "main > child_model_name". A back arrow replaces the hamburger menu. |
| 3.2 | Click the back arrow in the breadcrumb bar. | The editor returns to the parent model. The canvas restores the previous view: same scroll position, same zoom level, same selection. |
| 3.3 | Navigate into a module. Inside that child model, create another module, configure it with a model reference, and double-click to navigate into it. | The editor navigates to depth 2. The breadcrumb shows "main > level1 > level2". All three segments are visible. |
| 3.4 | Click the "main" breadcrumb segment (the first one). | The editor jumps directly back to the root model, skipping the intermediate level. The breadcrumb collapses. Selection and viewport are restored to the state when you left root. |
| 3.5 | Navigate 3 levels deep (main > A > B > C). Verify the breadcrumb shows all 4 segments. Click the breadcrumb for level "A". | The editor navigates to model A. The breadcrumb shows "main > A". Levels B and C are removed from the stack. |
| 3.6 | Navigate into a stdlib model (e.g., double-click on a module referencing "delay1"). | The editor shows the stdlib model's contents. A "read-only" badge appears next to the model name in the breadcrumb. The canvas is not editable. |
| 3.7 | Navigate into a child model. Open the search/variable finder. | Only variables from the current (child) model appear in search results. Variables from the parent or other models do not appear. |

## Phase 4: Shared Model Awareness

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Create two modules that both reference the same model (e.g., both reference "population"). | Both modules are created successfully and display the same model reference. |
| 4.2 | Navigate into one of the shared modules by double-clicking. | The editor shows the shared model. A banner or indicator shows that this model is used by N instances (e.g., "Used by 2 modules"). |
| 4.3 | If only one module references a model, navigate into it. | No shared-use banner appears (or the banner shows "Used by 1 module" and is styled differently). |
| 4.4 | Navigate into a stdlib model. | A "read-only" indicator is prominently displayed. Editing tools are disabled or hidden. |

## End-to-End: Full Module Lifecycle

**Purpose**: Verify the complete workflow from module creation through configuration, navigation, wiring, and deletion.

1. Start at the root model. Create a new model named "ecosystem" via the module tool: select Module tool, click canvas, type "ecosystem", press Enter.
2. Select the "ecosystem" module. In the details panel, select "Create new model..." from the Model Reference dropdown. A new model called "ecosystem" is created and assigned.
3. Click "Open Model" to navigate into the ecosystem model.
4. Inside the ecosystem model, create a stock named "population" and an aux named "growth_rate". Mark "population" as public (if the UI supports this toggle).
5. Navigate back to root using the breadcrumb (click "main").
6. Verify that the "ecosystem" module shows "population" in its Output Ports section.
7. Create an aux "food_supply" in the root model.
8. In the ecosystem module's Input Wiring, click "Add Input". Set src to "food_supply" and dst to the appropriate input port in the ecosystem model.
9. Verify the wiring persists after deselecting and reselecting the module.
10. Create a second module referencing the same "ecosystem" model. Navigate into it and confirm it shows the same variables.
11. Navigate back to root. Delete one of the ecosystem modules via the "Delete Module" button in the details panel.
12. Confirm only one ecosystem module remains. The remaining module still functions correctly.

## End-to-End: Deep Nesting Roundtrip

**Purpose**: Verify that module features work identically at depth 3+ with no special-casing.

1. Create models at 3 levels: main has module "level1" referencing model "model_a", model_a has module "level2" referencing model "model_b", model_b has module "level3" referencing model "model_c".
2. Navigate all the way to model_c (breadcrumb: "main > model_a > model_b > model_c").
3. At depth 3, create a new stock. Verify it appears in the canvas and variable list.
4. Navigate back one level at a time using the back arrow, verifying viewport/selection restoration at each step.
5. Navigate directly from depth 3 to root by clicking "main" in the breadcrumb. Verify root state is fully restored.

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC1.3 (Inline name editing) | Uses shared `editNameOnPointerUp` flow; no module-specific code to unit test. Relies on DOM focus/blur timing that jsdom does not replicate faithfully. | 1. Select Module tool from SpeedDial. 2. Click on the canvas. 3. Verify the inline text editor is immediately active with cursor blinking. 4. Type a name and press Enter. 5. Verify the name is applied. |
| AC1.13 (Cancel removes module) | Uses shared `handleEditingNameCancel` with `inCreationUid`. DOM event propagation (Escape key) and the async race between Canvas and Editor make this fragile in jsdom. | 1. Select Module tool. 2. Click on canvas to place module. 3. Immediately press Escape without typing. 4. Verify the module element is completely removed from the canvas (no ghost element, no unnamed module). |
| AC2.7 (Units/docs editable) | Slate rich-text editors are notoriously difficult to test in jsdom. Focus, blur, and content persistence require real browser events. Automated tests verify editor rendering, but actual editing round-trips require manual verification. | 1. Select a configured module. 2. Click the Units field. Type "widgets/year". Blur the field. 3. Deselect the module, then reselect it. Verify "widgets/year" persists. 4. Repeat for the Documentation field with a multi-line description. |
| AC3.6 (Scoped search) | Search scoping is an emergent property of the architecture (model-level `getView()`), not a testable function in isolation. | 1. Create variables in both root and child models with distinct names. 2. Navigate into the child model. 3. Open the search/variable finder (Ctrl+K or search icon). 4. Verify only child model variables appear. 5. Type part of a root model variable name. Verify it does NOT appear in results. |
