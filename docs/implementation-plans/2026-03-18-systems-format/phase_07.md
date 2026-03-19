# Systems Format Support - Phase 7: Module Diagram Generation

**Goal:** Extend the layout engine to include `Variable::Module` instances in auto-generated diagrams, generating `ViewElement::Module` elements, positioning them via SFDP, and creating connectors to/from module dependencies.

**Architecture:** Remove 9 module exclusion sites in `layout/mod.rs` that currently skip `Variable::Module` during dependency graph construction, positioning, label placement, and validation. Add `create_missing_module_elements()` following the `create_missing_auxiliary_elements` pattern. Add module dimensions to `LayoutConfig`. No changes needed to `graph.rs` (SFDP graph is built from `dep_graph` in `mod.rs`).

**Tech Stack:** Rust

**Scope:** 7 phases from original design (phase 7 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- 9 production module exclusion sites in `layout/mod.rs` at lines: 120, 1160, 1277, 1388, 1562, 1766, 2148, 2167, 2172
- `ViewElement::Module` already exists: `{ name, uid, x, y, label_side }` (same shape as `ViewElement::Aux`)
- `LayoutConfig` in `config.rs` has `stock_width/height`, `flow_width/height`, `aux_width/height`, `cloud_width/height` -- no module dims yet
- `create_missing_auxiliary_elements` (lines 1227-1267) is the pattern to follow
- `build_full_graph` (lines 734-831) builds SFDP graph from `dep_graph` -- modules auto-appear if added to dep_graph
- `verify_layout` in `tests/layout.rs` checks Stock/Flow/Aux only -- needs Module extension
- Existing test `test_generate_layout_ignores_module_dependencies_for_connectors` needs updating
- The design mentions `MODULE_WIDTH = 55` and `MODULE_HEIGHT = 45` matching existing rendering constants

---

## Acceptance Criteria Coverage

This phase implements and tests:

### systems-format.AC6: Layout engine generates module diagram elements
- **systems-format.AC6.1 Success:** Layout engine creates ViewElement::Module for each Variable::Module
- **systems-format.AC6.2 Success:** Modules appear as nodes in the SFDP force-directed graph
- **systems-format.AC6.3 Success:** Connectors are generated between modules and connected stocks/flows
- **systems-format.AC6.4 Success:** Module label placement is optimized alongside other elements
- **systems-format.AC6.5 Success:** Systems format models produce complete, renderable diagrams

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add module dimensions to LayoutConfig

**Files:**
- Modify: `src/simlin-engine/src/layout/config.rs` -- add `module_width` and `module_height` fields

**Implementation:**

Add to the `LayoutConfig` struct:
```rust
pub module_width: f64,
pub module_height: f64,
```

In `impl Default for LayoutConfig`:
```rust
module_width: 55.0,
module_height: 45.0,
```

In `validate()` (if present), add `max(1.0)` clamps following the existing dimension validation pattern.

**Verification:**

Run:
```bash
cargo build -p simlin-engine
```

**Commit:** `engine: add module dimensions to LayoutConfig`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove module exclusions and add module support to layout pipeline

**Verifies:** systems-format.AC6.1, AC6.2, AC6.3, AC6.4

**Files:**
- Modify: `src/simlin-engine/src/layout/mod.rs` -- modify 9 exclusion sites, add `create_missing_module_elements`

**Implementation:**

The changes are localized to 9 specific sites in `mod.rs`. For each site, the change removes or modifies the `matches!(var, datamodel::Variable::Module(_))` check:

**Site 1 (line ~120) -- `LayoutEngine::new`, display_names loop:**
Remove the `if matches!(var, datamodel::Variable::Module(_)) { continue; }` block. Let modules be included in `display_names` and UID seeding.

**Site 2 (line ~1160) -- `apply_layout_positions`, uid_to_ident map:**
Remove the `if matches!(var, datamodel::Variable::Module(_)) { return None; }` block. Let modules be included so their SFDP positions are applied.

**Site 3 (line ~1277) -- `create_connectors`, model_var_idents set:**
Remove the `.filter(|v| !matches!(v, datamodel::Variable::Module(_)))`. Let modules be included in `model_var_idents` so missing UIDs for modules are treated as errors (not silently skipped).

**Site 4 (line ~1388) -- `apply_optimal_label_placement`, uid_to_ident map:**
Remove the `if matches!(var, datamodel::Variable::Module(_)) { return None; }` block. Let modules participate in label placement optimization.

**Site 5 (line ~1562) -- `apply_feedback_loop_curvature`, uid_to_ident map:**
Remove the `if matches!(var, datamodel::Variable::Module(_)) { return None; }` block. Let modules participate in feedback loop arc computation.

**Site 6 (line ~1766) -- `validate_view_completeness`:**
Change `datamodel::Variable::Module(_) => {}` to check for `ViewElement::Module` presence, following the pattern used for stocks/flows/auxes. The validation should verify that every `Variable::Module` has a corresponding `ViewElement::Module`.

**Site 7 (line ~2148) -- `compute_metadata`, variable loop:**
Add a `datamodel::Variable::Module(m)` arm that records the module ident in `uid_to_ident`, similar to how auxes are handled.

**Site 8 (line ~2167) -- `compute_metadata`, `all_idents` set:**
Remove the `.filter(|v| !matches!(v, datamodel::Variable::Module(_)))`. Let module idents be included in `all_idents`.

**Site 9 (line ~2172) -- `compute_metadata`, dep_graph construction:**
Remove the `if matches!(var, datamodel::Variable::Module(_)) { continue; }`. Add module dependencies to `dep_graph`:
- A module depends on whatever variables its `references.src` fields reference
- Variables whose equations reference `module.output` depend on the module

This is the most complex change. The module's dependencies come from its `ModuleReference` bindings. Look at how the existing `dep_graph` is built (using `identifier_set()` or similar) and ensure modules participate.

**Add `create_missing_module_elements`:**

Following the `create_missing_auxiliary_elements` pattern (lines 1227-1267):

```rust
fn create_missing_module_elements(
    &mut self,
    layout: &Layout<NodeId>,
    var_to_node: &HashMap<Ident<Canonical>, NodeId>,
) {
    let existing_uids: HashSet<i32> = self.elements.iter()
        .filter_map(|e| match e {
            ViewElement::Module(m) => Some(m.uid),
            _ => None,
        })
        .collect();

    for var in &self.model.variables {
        if let datamodel::Variable::Module(m) = var {
            let canonical = Ident::new(&m.ident);
            let uid = self.uid_manager.alloc(&canonical);
            if existing_uids.contains(&uid) {
                continue;
            }
            let (x, y) = if let Some(node_id) = var_to_node.get(&canonical) {
                if let Some(pos) = layout.get(node_id) {
                    (pos.x, pos.y)
                } else {
                    (0.0, 0.0)
                }
            } else {
                (0.0, 0.0)
            };
            let name = self.display_name(&canonical)
                .map(|n| format_label_with_line_breaks(&n))
                .unwrap_or_else(|| m.ident.clone());
            self.elements.push(ViewElement::Module(view_element::Module {
                name,
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
            }));
            self.positions.insert(uid, (x, y));
        }
    }
}
```

Call this after `create_missing_auxiliary_elements` in the pipeline.

**Testing:**

Tests must verify:
- AC6.1: A model with `Variable::Module` produces `ViewElement::Module` after layout
- AC6.2: Module appears as a node in the SFDP graph (verified by non-zero x,y position)
- AC6.3: Connectors exist between module and its dependency stocks
- AC6.4: Module has a non-default `label_side` after optimization (or at least participates)

**Verification:**

Run:
```bash
cargo test -p simlin-engine layout
cargo test --features "file_io,testing" --test layout
```

**Commit:** `engine: include modules in layout generation (remove 9 exclusion sites)`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update layout tests and add systems format layout test

**Verifies:** systems-format.AC6.5

**Files:**
- Modify: `src/simlin-engine/tests/layout.rs` -- update `verify_layout`, fix existing tests, add systems format test

**Implementation:**

**Update `verify_layout`:**

Extend the `view_names` set construction (line ~46-54) to include `ViewElement::Module`:
```rust
ViewElement::Module(m) => {
    assert!(view_names.insert(m.name.clone()), "duplicate module: {}", m.name);
}
```

Extend the variable-to-view-element check loop to also look for `Variable::Module` and expect corresponding `ViewElement::Module`.

**Update existing unit test:**

The unit test `test_generate_layout_ignores_module_dependencies_for_connectors` at `src/simlin-engine/src/layout/mod.rs:3649` (a unit test in the `layout` module, NOT in `tests/layout.rs`) currently asserts that modules are excluded. Update it to instead verify:
- Module elements are present in the view
- Connectors to/from modules exist
- Rename the test to reflect the new behavior (e.g., `test_generate_layout_includes_module_elements_and_connectors`)

**Add systems format layout test:**

Create a test that loads a systems format example, translates it, generates layout, and verifies completeness:

```rust
#[test]
fn test_systems_format_layout_with_modules() {
    let contents = std::fs::read_to_string("test/systems-format/hiring.txt").unwrap();
    let model = simlin_engine::systems::parse(&contents).unwrap();
    let project = simlin_engine::systems::translate(&model, 5).unwrap();

    let view = generate_layout(&project, "main", None).unwrap();
    let model = &project.models[0];

    // Count expected module elements
    let module_count = model.variables.iter()
        .filter(|v| matches!(v, datamodel::Variable::Module(_)))
        .count();
    let view_module_count = view.elements.iter()
        .filter(|e| matches!(e, ViewElement::Module(_)))
        .count();
    assert_eq!(module_count, view_module_count,
        "Every Variable::Module should have a ViewElement::Module");

    // Verify all modules have valid positions
    for elem in &view.elements {
        if let ViewElement::Module(m) = elem {
            assert!(m.x.is_finite() && m.y.is_finite(),
                "Module {} should have finite coordinates", m.name);
        }
    }

    // Use extended verify_layout
    verify_layout(&view, model, "systems_hiring");
}
```

**Testing notes:**
- This test depends on Phases 1-3 (stdlib modules and translator). The test fixture at `test/systems-format/hiring.txt` was created in Phase 4.
- If the `verify_layout` helper doesn't support Module elements yet, extend it first.

**Verification:**

Run:
```bash
cargo test --features "file_io,testing" --test layout
```

Expected: All tests pass, including the new systems format layout test and updated module test.

**Commit:** `test: add module layout tests and verify systems format diagram generation`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
