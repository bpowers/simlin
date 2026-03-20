# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase implements the multi-view split and element metadata emission in the MDL writer:

### mdl-roundtrip-fidelity.AC1: Multi-view MDL output
- **mdl-roundtrip-fidelity.AC1.1 Success:** mark2.mdl roundtrip produces exactly 2 views with names `*1 housing` and `*2 investments`
- **mdl-roundtrip-fidelity.AC1.2 Success:** Each view contains the correct elements -- every element line from the original view appears in the corresponding output view (unordered set comparison)
- **mdl-roundtrip-fidelity.AC1.3 Success:** Single-view models (no ViewElement::Group markers) produce a single view as before
- **mdl-roundtrip-fidelity.AC1.4 Success:** Each view has its own font specification line matching the original

### mdl-roundtrip-fidelity.AC2: Element metadata preservation
- **mdl-roundtrip-fidelity.AC2.1 Success:** Stock elements preserve original width/height/bits (e.g. `53,32,3,131` not hardcoded `40,20,3,3`)
- **mdl-roundtrip-fidelity.AC2.2 Success:** Aux, flow, cloud, and alias elements preserve original dimensions and bits
- **mdl-roundtrip-fidelity.AC2.3 Success:** Elements without compat data (e.g. from XMILE imports) use hardcoded defaults without error

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Split merged view into multiple named views based on Group boundaries

**Verifies:** mdl-roundtrip-fidelity.AC1.1, mdl-roundtrip-fidelity.AC1.2, mdl-roundtrip-fidelity.AC1.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

The current `write_stock_flow_view()` (lines 1667-1720) emits a single view and skips `ViewElement::Group` elements (line 1715). Replace this with logic that splits on Group boundaries.

Modify `write_sketch_section()` (lines 1654-1720). The key change: before calling `write_stock_flow_view`, partition the StockFlow's elements into separate view segments using Group elements as delimiters.

Algorithm:

```rust
fn write_sketch_section(&mut self, views: &[View]) {
    self.buf.push_str("V300  Do not put anything below this section - it will be ignored\r\n");
    for view in views {
        let View::StockFlow(sf) = view;
        let segments = split_view_on_groups(sf);
        for (view_name, elements, font) in &segments {
            self.write_view_segment(view_name, elements, font);
        }
    }
    self.buf.push_str("///---\\\\\\\r\n");
}

/// Splits a StockFlow's elements into view segments at Group boundaries.
/// Returns Vec of (view_name, elements, font_override).
/// If no Group elements exist, returns a single segment (AC1.3 compatibility).
fn split_view_on_groups(sf: &StockFlow) -> Vec<(String, Vec<&ViewElement>, &Option<String>)> {
    let mut segments = Vec::new();
    let mut current_name = sf.name.clone().unwrap_or_else(|| "View 1".to_string());
    let mut current_elements: Vec<&ViewElement> = Vec::new();

    for element in &sf.elements {
        if let ViewElement::Group(group) = element {
            if !current_elements.is_empty() {
                segments.push((current_name, current_elements, &sf.font));
                current_elements = Vec::new();
            }
            current_name = group.name.clone();
        } else if !matches!(element, ViewElement::Module(_)) {
            current_elements.push(element);
        }
    }
    if !current_elements.is_empty() {
        segments.push((current_name, current_elements, &sf.font));
    }
    segments
}
```

Note: All sub-views share `&sf.font` because the parser merges multiple MDL views into a single StockFlow. If the original MDL had different fonts per view, only one is preserved. This is a design-level constraint; for mark2.mdl both views use the same font (`Verdana|10`) so this is not an issue for the roundtrip test.

Verify that `write_sketch_section` and the new `write_view_segment` use CRLF (`\r\n`) consistently with the rest of the writer. Recent commit `646ac509` standardized MDL output to CRLF.

Then `write_view_segment()` replaces the old `write_stock_flow_view()`, emitting:
1. `*{view_name}\r\n`
2. Font line (see Task 4)
3. All elements via existing per-type write functions
4. View terminator

**Testing:**
Tests must verify:
- mdl-roundtrip-fidelity.AC1.1: A model with two Group elements produces 2 separate named views
- mdl-roundtrip-fidelity.AC1.2: Elements are partitioned correctly between views
- mdl-roundtrip-fidelity.AC1.3: A model with no Group elements produces a single view (existing behavior preserved)

Write unit tests in the writer's `#[cfg(test)]` module that construct StockFlow with/without Group elements and verify the split logic.

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: split merged MDL views on Group boundaries in writer`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Tests for multi-view split

**Verifies:** mdl-roundtrip-fidelity.AC1.1, mdl-roundtrip-fidelity.AC1.2, mdl-roundtrip-fidelity.AC1.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
- AC1.1: Construct a StockFlow with elements [Group("1 housing"), Aux, Stock, Group("2 investments"), Aux, Flow]. Write to MDL. Assert output contains `*1 housing` and `*2 investments` view headers.
- AC1.2: Assert the elements between `*1 housing` and `*2 investments` are the first batch, and elements after `*2 investments` are the second batch.
- AC1.3: Construct a StockFlow with NO Group elements. Write to MDL. Assert a single view is produced with a default name.

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-5) -->

<!-- START_TASK_3 -->
### Task 3: Use compat metadata for element dimensions in write functions

**Verifies:** mdl-roundtrip-fidelity.AC2.1, mdl-roundtrip-fidelity.AC2.2, mdl-roundtrip-fidelity.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

Update each element writing function to use compat dimensions when present, falling back to hardcoded defaults.

**`write_stock_element`** (lines 1172-1181): Currently hardcodes `40,20,3,3`. Change to:

```rust
fn write_stock_element(&mut self, stock: &view_element::Stock, ...) {
    let (w, h, bits) = match &stock.compat {
        Some(c) => (c.width, c.height, c.bits),
        None => (40.0, 20.0, 3),  // AC2.3: default for non-MDL sources
    };
    write!(self.buf, "10,{uid},{name},{x},{y},{w},{h},3,{bits},0,0,0,0,0,0\r\n", ...).unwrap();
}
```

Apply the same pattern to:
- **`write_aux_element`** (lines 1159-1168): default `(40.0, 20.0, 3)`, shape=8
- **`write_flow_element`** (lines 1262-1293): valve uses `flow.compat` default `(6.0, 8.0, 3)` shape=34; attached label uses `flow.label_compat` default `(49.0, 8.0, 3)` shape=40
- **`write_cloud_element`** (lines 1359-1367): default `(10.0, 8.0, 3)`, shape=0
- **`write_alias_element`** (lines 1370-1387): default `(40.0, 20.0, 2)`, shape=8

Each function needs access to the element's compat field, so update signatures to pass the full view element struct (or add the compat as a parameter) if not already available.

**Testing:**
Tests must verify:
- mdl-roundtrip-fidelity.AC2.1: Stock with `compat = Some(ViewElementCompat { width: 53.0, height: 32.0, bits: 131 })` emits `53,32,3,131` in the element line
- mdl-roundtrip-fidelity.AC2.2: Each element type with compat emits preserved dimensions
- mdl-roundtrip-fidelity.AC2.3: Each element type with `compat = None` emits hardcoded defaults

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: use ViewElementCompat dimensions in MDL element output`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Use StockFlow.font for font line emission

**Verifies:** mdl-roundtrip-fidelity.AC1.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs`

**Implementation:**

In the view segment writing function (formerly `write_stock_flow_view`, now `write_view_segment` or equivalent), the font line is currently hardcoded (line 1671):

```rust
self.buf.push_str("$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\r\n");
```

Change to use the font from StockFlow when present:

```rust
if let Some(font) = font {
    write!(self.buf, "${font}\r\n").unwrap();
} else {
    self.buf.push_str("$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\r\n");
}
```

**Testing:**
- mdl-roundtrip-fidelity.AC1.4: A StockFlow with `font = Some("192-192-192,0,Verdana|10||...")` emits that font string in the view section. A StockFlow with `font = None` emits the hardcoded default.

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: emit preserved font spec in MDL view output`

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Tests for compat dimensions and font in writer output

**Verifies:** mdl-roundtrip-fidelity.AC2.1, mdl-roundtrip-fidelity.AC2.2, mdl-roundtrip-fidelity.AC2.3, mdl-roundtrip-fidelity.AC1.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (test module)

**Testing:**
Write unit tests in the writer's test module:
- Stock element with compat `{width: 53.0, height: 32.0, bits: 131}` produces element line containing `53,32,3,131`
- Stock element with `compat: None` produces element line containing `40,20,3,3` (default)
- Aux element with compat produces correct dimensions
- Flow element: valve dimensions from `compat`, label dimensions from `label_compat`
- Cloud and alias elements with compat produce correct dimensions
- StockFlow with font produces correct `$` font line
- StockFlow without font produces default font line

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->
