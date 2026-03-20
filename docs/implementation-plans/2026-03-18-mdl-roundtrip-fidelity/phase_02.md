# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase captures Vensim-specific metadata during MDL parsing:

### mdl-roundtrip-fidelity.AC2: Element metadata preservation
- **mdl-roundtrip-fidelity.AC2.1 Success:** Stock elements preserve original width/height/bits (e.g. `53,32,3,131` not hardcoded `40,20,3,3`)
- **mdl-roundtrip-fidelity.AC2.2 Success:** Aux, flow, cloud, and alias elements preserve original dimensions and bits

### mdl-roundtrip-fidelity.AC3: Lookup fidelity
- **mdl-roundtrip-fidelity.AC3.2 Success:** Explicit lookup range bounds are preserved (e.g. `[(0,0)-(300,10)]` not computed `[(0,0.98)-(300,8.29)]`)

Note: AC2.1 and AC2.2 are captured here (parse-side) and verified in writer output in Phase 3.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Add bits field to parsed view types and store during element parsing

**Files:**
- Modify: `src/simlin-engine/src/mdl/view/types.rs`
- Modify: `src/simlin-engine/src/mdl/view/elements.rs`

**Implementation:**

In `types.rs`, add a `bits` field to the parsed intermediate types:

`VensimVariable` (line 30): add `pub bits: i32,`
`VensimValve` (around line 45): add `pub bits: i32,`
`VensimComment` (around line 65): add `pub bits: i32,`

In `elements.rs`, where element lines are parsed:
- For VensimVariable: the `bits` value is already parsed as a local variable (line 125, `let bits = ...`) but only used to derive `is_ghost`. Store the raw value: add `bits` to the VensimVariable construction.
- For VensimValve: the `parse_valve` function (elements.rs, around line 147) currently parses up to `shape` but does NOT parse `bits`. Extend `parse_valve` to also read the `bits` field, which follows `shape` in the type 11 line format (same field position as type 10 lines). Store it on VensimValve.
- For VensimComment: find where cloud element lines are parsed (type 12 lines). Extend parsing to include `bits` and store it.

Note: Updating these structs will also require updating all test construction sites in `types.rs` tests (3+ sites at lines ~337, ~372, ~405) to include the new `bits` field. Use `cargo check` to find all sites.

**Verification:**
Run `cargo check -p simlin-engine` -- types module should compile.

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Pass width/height/bits through to view element compat in convert.rs

**Verifies:** mdl-roundtrip-fidelity.AC2.1, mdl-roundtrip-fidelity.AC2.2 (parse-side)

**Files:**
- Modify: `src/simlin-engine/src/mdl/view/convert.rs`

**Implementation:**

In `convert_variable()` (lines 405-498), where view elements are constructed, create `ViewElementCompat` from the parsed dimensions:

For Stock (line 470):
```rust
view_element::Stock {
    name: ...,
    uid: ...,
    x: ...,
    y: ...,
    label_side: LabelSide::Top,
    compat: Some(view_element::ViewElementCompat {
        width: var.width as f64,
        height: var.height as f64,
        bits: var.bits as u32,
    }),
}
```

Apply the same pattern for Aux (line 491) and Alias (line 457).

For Flow (line 482): the flow element combines a VensimValve (for the valve) and a VensimVariable (for the attached label). Find where the valve data is available and populate:
- `compat` with valve width/height/bits
- `label_compat` with the attached variable's width/height/bits

For Cloud in `convert_comment_as_cloud()` (line 559): populate compat from VensimComment's width/height/bits.

**Testing:**
Tests must verify AC2.1 and AC2.2 (parse-side):
- mdl-roundtrip-fidelity.AC2.1: Parse a stock element line with known dimensions (e.g. `10,1,Test Stock,100,50,53,32,3,131,...`), verify resulting view_element::Stock has `compat == Some(ViewElementCompat { width: 53.0, height: 32.0, bits: 131 })`
- mdl-roundtrip-fidelity.AC2.2: Parse an aux, flow, cloud, and alias element, verify each has correct compat values

Unit tests should go in `src/simlin-engine/src/mdl/view/convert.rs` (existing `#[cfg(test)]` module) or a new test module in the view directory.

**Verification:**
Run `cargo test -p simlin-engine` -- new tests pass.

**Commit:** `engine: capture MDL element dimensions in ViewElementCompat during parsing`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Tests for compat field population

**Verifies:** mdl-roundtrip-fidelity.AC2.1, mdl-roundtrip-fidelity.AC2.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/view/convert.rs` (or appropriate test location)

**Testing:**
Write unit tests that parse MDL element lines and verify compat fields:
- Stock: parse element line with width=53, height=32, bits=131 -> verify compat matches
- Aux: parse element line with non-default dimensions -> verify compat
- Flow: parse valve + attached label lines -> verify both compat and label_compat
- Cloud: parse cloud element line -> verify compat
- Alias: parse alias element line -> verify compat

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-5) -->

<!-- START_TASK_4 -->
### Task 4: Parse font specification line in MDL view parser

**Verifies:** mdl-roundtrip-fidelity.AC1.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/view/mod.rs`
- Modify: `src/simlin-engine/src/mdl/view/convert.rs` (if font needs to flow through conversion)

**Implementation:**

In `mod.rs` (lines 141-146), the font line is currently consumed and discarded:
```rust
// Skip font line if present (we ignore PPI values per xmutil)
if let Some(line) = self.peek_line()
    && line.starts_with('$')
{
    self.read_line();
}
```

Change this to capture the font line content (excluding the `$` prefix) and store it. The font string should flow through to `StockFlow.font`. Trace the path from the view parser through the view conversion to where `StockFlow` is constructed:

- In `convert_view()` (convert.rs line 364) or `merge_views()` (convert.rs line 262), the font string from the first parsed view should be stored in `StockFlow.font`.
- Pass the parsed font string through the `ViewHeader` struct (types.rs, currently holds only `version` and `title`) or as a separate parameter.

**Testing:**
Tests must verify AC1.4:
- Parse an MDL view section with font line `$192-192-192,0,Verdana|10||0-0-0|...`, verify `StockFlow.font == Some("192-192-192,0,Verdana|10||0-0-0|...")`
- Parse an MDL view without a font line (if possible), verify `StockFlow.font == None`

**Verification:**
Run `cargo test -p simlin-engine` -- tests pass.

**Commit:** `engine: capture MDL font specification during view parsing`

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Test for font parsing

**Verifies:** mdl-roundtrip-fidelity.AC1.4

**Files:**
- Test in: `src/simlin-engine/src/mdl/view/mod.rs` or `convert.rs` test module

**Testing:**
Write a test that parses a minimal MDL with a font spec line and verifies the font string is preserved in StockFlow.font. The font line format is:
`$192-192-192,0,Verdana|10||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0`

**Verification:**
Run `cargo test -p simlin-engine` -- all tests pass.

<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 6-8) -->

<!-- START_TASK_6 -->
### Task 6: Preserve explicit y_range in build_graphical_function

**Verifies:** mdl-roundtrip-fidelity.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs`

**Implementation:**

In `build_graphical_function()` (lines 1318-1389), the y_scale is currently always computed from data points (lines 1352-1366). The `table.y_range` is available but ignored. Change to preserve explicit bounds when present, mirroring the existing x_range handling (lines 1337-1350):

```rust
let y_scale = if let Some(y_range) = table.y_range {
    GraphicalFunctionScale {
        min: y_range.0,
        max: y_range.1,
    }
} else {
    let y_min = y_vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let y_max = y_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_max = if (y_min - y_max).abs() < f64::EPSILON {
        y_min + 1.0
    } else {
        y_max
    };
    GraphicalFunctionScale {
        min: y_min,
        max: y_max,
    }
};
```

Update the comment to explain the change: when explicit y_range bounds exist in the MDL source, preserve them for roundtrip fidelity; only compute from data when no explicit bounds are present.

**Verification:**
Run `cargo check -p simlin-engine` -- compiles.

<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Update y_range test

**Verifies:** mdl-roundtrip-fidelity.AC3.2, mdl-roundtrip-fidelity.AC3.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (test at lines 2763-2794)

**Implementation:**

The test `test_graphical_function_y_scale_computed_from_data` currently asserts that y_scale is computed from data points (y_min=0.5, y_max=1.36) even though the MDL input specifies `[(0,0)-(2,5)]` (y_range 0-5).

Update the test to reflect the new behavior: when explicit y_range is present, it should be used:
- Change assertion on `gf.y_scale.min` from `0.5` to `0.0` (the file-specified value)
- Change assertion on `gf.y_scale.max` from `1.36` to `5.0` (the file-specified value)
- Rename the test to reflect the new semantics (e.g., `test_graphical_function_y_scale_from_explicit_range`)

Add a second test for AC3.3: a lookup WITHOUT explicit y_range bounds should still compute bounds from data (the existing behavior for XMILE-sourced models).

**Testing:**
- mdl-roundtrip-fidelity.AC3.2: Lookup with explicit `[(0,0)-(2,5)]` range has `y_scale = {min: 0.0, max: 5.0}` (from file, not data)
- mdl-roundtrip-fidelity.AC3.3: Lookup without explicit range has y_scale computed from actual data points

**Verification:**
Run `cargo test -p simlin-engine` -- both tests pass.

**Commit:** `engine: preserve explicit lookup y-range bounds from MDL source`

<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Verify all Phase 2 changes

**Verification:**

```bash
cargo test -p simlin-engine
```

All tests pass, including the new compat, font, and y_range tests.

<!-- END_TASK_8 -->

<!-- END_SUBCOMPONENT_C -->
