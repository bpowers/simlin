# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase is infrastructure. **Verifies: None** -- enables Phases 2-6.

---

<!-- START_SUBCOMPONENT_A (tasks 1-5) -->

<!-- START_TASK_1 -->
### Task 1: Add ViewElementCompat struct, compat fields, and font to datamodel types

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs`

**Implementation:**

Add the `ViewElementCompat` struct inside the `view_element` module (before the `Aux` struct around line 468):

```rust
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ViewElementCompat {
    pub width: f64,
    pub height: f64,
    pub bits: u32,
}
```

Note: Use `#[cfg_attr(feature = "debug-derive", derive(Debug))]` (not unconditional `derive(Debug)`) to match the existing pattern on all other view element structs. This avoids bloating WASM binaries with Debug impls.

Add `pub compat: Option<ViewElementCompat>` as the last field on:
- `view_element::Aux` (line 469)
- `view_element::Stock` (line 479)
- `view_element::Cloud` (line 555)
- `view_element::Alias` (line 545)

For `view_element::Flow` (line 497), add two fields:
- `pub compat: Option<ViewElementCompat>` -- valve (type 11) dimensions
- `pub label_compat: Option<ViewElementCompat>` -- attached label (type 10) dimensions

Add `pub font: Option<String>` to `StockFlow` (line 632).

**Verification:**
Will not compile yet -- construction sites need updating in subsequent tasks.

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update protobuf schema and regenerate bindings

**Files:**
- Modify: `src/simlin-engine/src/project_io.proto`

**Implementation:**

Add a `ViewElementCompat` message inside the `ViewElement` message (before the `Aux` submessage):

```protobuf
message ViewElementCompat {
  optional double width = 1;
  optional double height = 2;
  optional uint32 bits = 3;
}
```

Add compat fields to each view element submessage using the next available tag number:
- `Aux` (next tag 6): `ViewElementCompat compat = 6;`
- `Stock` (next tag 6): `ViewElementCompat compat = 6;`
- `Flow` (next tag 7): `ViewElementCompat compat = 7;` and `ViewElementCompat label_compat = 8;`
- `Cloud` (next tag 5): `ViewElementCompat compat = 5;`
- `Alias` (next tag 6): `ViewElementCompat compat = 6;`

Add font to the `View` protobuf message (which maps to `StockFlow` in Rust datamodel) at next tag 9: `optional string font = 9;`

Regenerate bindings:

```bash
pnpm build:gen-protobufs
```

**Verification:**
Run `cargo check -p simlin-engine` -- proto-generated types should compile. Datamodel construction sites will still error.

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update serde.rs serialization and deserialization

**Files:**
- Modify: `src/simlin-engine/src/serde.rs`

**Implementation:**

Add helper functions for ViewElementCompat conversion near the existing `compat_to_proto` / `compat_from_proto` helpers (around line 309):

```rust
fn view_compat_to_proto(
    compat: &Option<view_element::ViewElementCompat>,
) -> Option<project_io::view_element::ViewElementCompat> {
    compat.as_ref().map(|c| project_io::view_element::ViewElementCompat {
        width: Some(c.width),
        height: Some(c.height),
        bits: Some(c.bits),
    })
}

fn view_compat_from_proto(
    compat: Option<project_io::view_element::ViewElementCompat>,
) -> Option<view_element::ViewElementCompat> {
    compat.map(|c| view_element::ViewElementCompat {
        width: c.width.unwrap_or(0.0),
        height: c.height.unwrap_or(0.0),
        bits: c.bits.unwrap_or(0),
    })
}
```

Update each view element's `From` impl pair. Pattern for Aux (apply same to Stock, Cloud, Alias):

In `From<project_io::view_element::Aux> for view_element::Aux` (line 1187):
- Add: `compat: view_compat_from_proto(a.compat),`

In `From<view_element::Aux> for project_io::view_element::Aux` (line 1201):
- Add: `compat: view_compat_to_proto(&a.compat),`

For Flow, handle both `compat` and `label_compat` using the same helpers.

Update StockFlow/View conversions:
- In `From<project_io::View> for View` (around line 1819): add `font: v.font,`
- In `From<View> for project_io::View` (around line 1780): add `font: sf.font.clone(),`

Update the serde roundtrip test fixtures in the same file to include `compat: None` on view elements and `font: None` on StockFlow constructions (tests at lines 1213, 1255, 1392, 1612, 1677, 1758, 1764, 1839, 1858).

**Verification:**
Run `cargo check -p simlin-engine` -- serde module should compile. Other construction sites may still error.

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Fix all remaining construction sites

**Files (compiler-driven -- run `cargo check -p simlin-engine` repeatedly to find each site):**
- `src/simlin-engine/src/json.rs` -- JSON deserialization (lines ~1040-1156)
- `src/simlin-engine/src/xmile/views.rs` -- XMILE parser and tests (~30 sites)
- `src/simlin-engine/src/mdl/view/convert.rs` -- MDL view converter (~6 sites)
- `src/simlin-engine/src/mdl/writer.rs` -- writer test helpers and tests (~20 sites)
- `src/simlin-engine/src/layout/mod.rs` -- layout production code and tests (~17 sites)
- `src/simlin-engine/src/layout/placement.rs` -- placement normalization (~3 sites)
- `src/simlin-engine/src/diagram/flow.rs` -- flow test helpers (~3 sites)
- `src/simlin-engine/src/diagram/elements.rs` -- element tests (~8 sites)
- `src/simlin-engine/src/diagram/connector.rs` -- connector tests (~2 sites)
- `src/simlin-engine/src/diagram/render.rs` -- render tests (~5 sites)
- `src/simlin-engine/src/diagram/render_png.rs` -- render_png tests (~5 sites)
- `src/simlin-engine/src/patch.rs` -- patch tests (~1 site)
- `src/simlin-engine/src/stdlib.gen.rs` -- generated stdlib models (~60 sites, see note)

**Pattern for all sites:**
- `view_element::Aux`, `Stock`, `Cloud`, `Alias` struct literals: add `compat: None,`
- `view_element::Flow` struct literals: add `compat: None, label_compat: None,`
- `StockFlow` struct literals: add `font: None,`

**stdlib.gen.rs note:** This file is generated. Check `package.json` scripts or `scripts/` directory for a stdlib generator command (e.g., `build:gen-stdlib`). If a generator exists, update its template to include the new fields, then regenerate. If no generator exists, manually add the fields.

**Verification:**
Run `cargo check -p simlin-engine` -- should compile with zero errors.

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Verify build and tests pass, commit

**Verification:**

```bash
cargo test -p simlin-engine
```

All existing tests must pass. No new tests needed for this infrastructure phase.

**Commit:** `engine: add ViewElementCompat and font fields for MDL roundtrip fidelity`

<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_A -->
