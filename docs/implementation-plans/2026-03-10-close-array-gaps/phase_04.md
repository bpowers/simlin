# Close Array Gaps Implementation Plan -- Phase 4

**Goal:** Extend protobuf and XMILE serialization to support EXCEPT equations, element-level dimension mappings, and DataSource metadata, enabling full round-tripping of arrayed models.

**Architecture:** Add new proto fields and messages for the three constructs that are currently rejected. Remove the three `validate_for_protobuf` guards and the `validate_for_xmile` guard. Add `simlin:` vendor extensions in XMILE for constructs beyond the XMILE spec. JSON serialization (already complete) serves as the reference implementation.

**Tech Stack:** Rust (simlin-engine crate), protobuf

**Scope:** 6 phases from original design (this is phase 4 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` for test index. JSON round-trip tests in `json.rs` serve as the reference implementation.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC6: Proto + XMILE serialization (#341, #342)
- **close-array-gaps.AC6.1 Success:** Proto round-trips EXCEPT equations with default_equation
- **close-array-gaps.AC6.2 Success:** Proto round-trips element-level dimension mappings
- **close-array-gaps.AC6.3 Success:** Proto round-trips DataSource metadata
- **close-array-gaps.AC6.4 Success:** XMILE round-trips EXCEPT via top-level `<eqn>` + `<element>` overrides
- **close-array-gaps.AC6.5 Success:** XMILE round-trips element-level mappings via `<simlin:mapping>` vendor extension
- **close-array-gaps.AC6.6 Success:** XMILE round-trips DataSource via `<simlin:data_source>` vendor extension
- **close-array-gaps.AC6.7 Backward compat:** Old protos without new fields deserialize correctly

---

## Codebase Verification Findings

- Confirmed: `ArrayedEquation` proto has fields 1 (`dimension_names`), 2 (`elements`). Phase 2 added field 3 (`has_except_default`). Next available: **4** for `default_equation`. (Design document assumed different field numbers; verified against actual proto file.)
- Confirmed: `Compat` proto has fields 1-4. Next available: **5** for `data_source`. (Verified against actual proto file.)
- Confirmed: `Dimension` proto has fields 1-5 (including `maps_to = 5`). Next available: **6** for `mappings`. (Verified against actual proto file.)
- Confirmed: 3 validate guards in `serde.rs:2238-2278`: `validate_equation_for_protobuf` (EXCEPT), `validate_compat_for_protobuf` (DataSource), `validate_dimension_for_protobuf` (element-level mappings). Top-level `validate_for_protobuf` at line 2280 calls all three.
- Confirmed: `validate_for_xmile` in `xmile/mod.rs:801-866` mirrors the same 3 rejections.
- Confirmed: `DataSource`/`DataSourceKind` already exist in `datamodel.rs:209-235`. `Compat` already has `data_source: Option<DataSource>`.
- Confirmed: `DimensionMapping` already exists in `datamodel.rs:771-778` with `target: String` and `element_map: Vec<(String, String)>`. `Dimension` uses `mappings: Vec<DimensionMapping>`.
- Confirmed: XMILE dimensions.rs handles only simple `maps_to`. No `simlin:mapping` extension exists.
- Confirmed: XMILE variables.rs hardcodes `None` for default_equation. No `simlin:data_source` extension.
- Confirmed: JSON handles all three correctly -- serves as reference. Tests: `test_arrayed_equation_with_default_equation_roundtrip` (line 3562), `test_data_source_roundtrip_through_compat` (line 3673), plus DimensionMapping round-trip tests.
- Confirmed: 4 rejection tests in serde.rs (lines 2354, 2383, 2409, 2460) will become round-trip success tests.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Extend proto schema for EXCEPT, DataSource, and element-level mappings

**Verifies:** close-array-gaps.AC6.1, close-array-gaps.AC6.2, close-array-gaps.AC6.3

**Files:**
- Modify: `src/simlin-engine/src/project_io.proto:44-54` (ArrayedEquation)
- Modify: `src/simlin-engine/src/project_io.proto:64-69` (Compat)
- Modify: `src/simlin-engine/src/project_io.proto:316-335` (Dimension)

**Implementation:**

1. Add to `ArrayedEquation` (field 4, following `has_except_default = 3` added in Phase 2):
```protobuf
optional string default_equation = 4;
```

2. Add new `DataSource` message and field to `Compat`:
```protobuf
message DataSource {
  enum Kind {
    DATA = 0;
    CONSTANTS = 1;
    LOOKUPS = 2;
    SUBSCRIPT = 3;
  };
  Kind kind = 1;
  string file = 2;
  string tab_or_delimiter = 3;
  string row_or_col = 4;
  string cell = 5;
};
```
Add to `Compat`: `optional DataSource data_source = 5;`

3. Add `DimensionMapping` message and field to `Dimension`:
```protobuf
message DimensionMapping {
  message ElementMapEntry {
    string from_element = 1;
    string to_element = 2;
  };
  string target = 1;
  repeated ElementMapEntry entries = 2;
};
```
Add to `Dimension`: `repeated DimensionMapping mappings = 6;`

4. Run `pnpm build:gen-protobufs` to regenerate bindings.

**Testing:**

Proto compilation and binding generation succeed.

**Verification:**

```bash
pnpm build:gen-protobufs
cargo build -p simlin-engine
```

Expected: Compiles without errors.

**Commit:** `engine: extend proto schema for EXCEPT, DataSource, and element-level mappings`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update serde.rs for new proto fields and remove guards

**Verifies:** close-array-gaps.AC6.1, close-array-gaps.AC6.2, close-array-gaps.AC6.3, close-array-gaps.AC6.7

**Files:**
- Modify: `src/simlin-engine/src/serde.rs:295` (Equation::Arrayed serialization)
- Modify: `src/simlin-engine/src/serde.rs:337-352` (Equation::Arrayed deserialization)
- Modify: `src/simlin-engine/src/serde.rs` (Compat serialize/deserialize paths)
- Modify: `src/simlin-engine/src/serde.rs` (Dimension serialize/deserialize paths)
- Modify: `src/simlin-engine/src/serde.rs:2238-2306` (remove validate guards)

**Implementation:**

1. **Remove guards:** Delete `validate_equation_for_protobuf` (lines 2238-2247), `validate_compat_for_protobuf` (lines 2249-2258), `validate_dimension_for_protobuf` (lines 2260-2278), and `validate_for_protobuf` (lines 2280-2306). Remove the `validate_for_protobuf()` call from the `serialize` function entry point.

2. **Equation::Arrayed serialization** (around line 295): Include `default_equation` in the proto output. Currently the third field is dropped with `_default_eq`. Change to encode it as the new `default_equation` proto field.

3. **Equation::Arrayed deserialization** (around lines 337-352): Read the `default_equation` field from proto. When absent (old protos), default to `None` for backward compatibility (AC6.7).

4. **Compat DataSource:** Update the Compat serialize path to encode `data_source` into the new proto `DataSource` message. Update deserialize to read it back, defaulting to `None` when absent.

5. **Dimension mappings:** Update the Dimension serialize path to encode `mappings` into the new proto `DimensionMapping` repeated field. The existing `maps_to` field (field 5) handles simple positional mappings. For backward compat on deserialize: if `mappings` field is present, use it; else fall back to `maps_to` field as a single positional mapping. This mirrors the JSON deserialization logic at `json.rs:1196-1211`.

**Testing:**

- close-array-gaps.AC6.7: Old protos (without new fields) deserialize correctly -- default_equation=None, data_source=None, mappings from maps_to field
- Existing rejection tests in serde.rs become success tests (see Task 4)

**Verification:**

```bash
cargo build -p simlin-engine
cargo test -p simlin-engine
```

Expected: Compiles. Some existing rejection tests will now fail (they expect Err but will get Ok) -- those are fixed in Task 4.

**Commit:** `engine: update proto serialization for EXCEPT, DataSource, and element-level mappings`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update XMILE serialization and remove guards

**Verifies:** close-array-gaps.AC6.4, close-array-gaps.AC6.5, close-array-gaps.AC6.6

**Files:**
- Modify: `src/simlin-engine/src/xmile/mod.rs:801-866` (remove validate_for_xmile)
- Modify: `src/simlin-engine/src/xmile/variables.rs:127-146` (convert_equation for EXCEPT)
- Modify: `src/simlin-engine/src/xmile/variables.rs` (From impls for Stock/Flow/Aux -- write default_equation and data_source)
- Modify: `src/simlin-engine/src/xmile/dimensions.rs` (element-level mapping vendor extension)

**Implementation:**

1. **Remove validate_for_xmile** in `xmile/mod.rs:801-866` and its call from `project_to_xmile`.

2. **EXCEPT in XMILE variables** (AC6.4): For `Equation::Arrayed` with `Some(default_equation)`:
   - On write: emit a top-level `<eqn>` with the default equation, plus `<element>` entries for each override. This matches XMILE's native format where an unsubscripted `<eqn>` serves as the default.
   - On read: in `convert_equation!`, if both a top-level `<eqn>` and `<element>` entries exist, construct `Equation::Arrayed` with the top-level eqn as `default_equation`.
   - Set `has_except_default: true` when `default_equation` is `Some` and element overrides exist.

3. **DataSource vendor extension** (AC6.6): Add `<simlin:data_source>` element to variable serialization.
   - On write: if `compat.data_source` is `Some`, emit `<simlin:data_source kind="constants" file="..." tab="..." row_or_col="..." cell="..."/>`.
   - On read: if `<simlin:data_source>` is present, populate `compat.data_source`.

4. **Element-level mapping vendor extension** (AC6.5): In `xmile/dimensions.rs`:
   - On write: for dimensions with element-level mappings (non-empty `element_map`), emit `<simlin:mapping target="DimA"><simlin:elem from="D1" to="A2"/></simlin:mapping>` inside the `<dim>` element.
   - On read: parse `<simlin:mapping>` elements and populate `DimensionMapping.element_map`.
   - Keep simple `maps_to` for backward compatibility with simple positional mappings.

**Testing:**

Tests in Task 4.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles without errors.

**Commit:** `engine: add XMILE vendor extensions for EXCEPT, DataSource, and element-level mappings`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_4 -->
### Task 4: Convert rejection tests to round-trip success tests

**Verifies:** close-array-gaps.AC6.1, close-array-gaps.AC6.2, close-array-gaps.AC6.3, close-array-gaps.AC6.4, close-array-gaps.AC6.5, close-array-gaps.AC6.6, close-array-gaps.AC6.7

**Files:**
- Modify: `src/simlin-engine/src/serde.rs:2354-2488` (4 rejection tests)
- Modify: `src/simlin-engine/src/xmile/` (add XMILE round-trip tests if not already present)

**Implementation:**

1. Convert the 4 existing rejection tests in serde.rs to round-trip success tests:

   - `test_protobuf_rejects_except_equation` (line 2354): Change to `test_protobuf_roundtrips_except_equation`. Serialize an `Equation::Arrayed` with `Some(default_equation)` to proto, deserialize, verify `default_equation` is preserved.

   - `test_protobuf_rejects_element_level_dimension_mapping` (line 2383): Change to `test_protobuf_roundtrips_element_level_dimension_mapping`. Serialize a dimension with element-level mappings, deserialize, verify mappings preserved.

   - `test_protobuf_rejects_data_source` (line 2409): Change to `test_protobuf_roundtrips_data_source`. Serialize compat with DataSource, deserialize, verify preserved.

   - `test_protobuf_rejects_multi_target_positional_mappings` (line 2460): Change to round-trip test if the proto supports multiple mappings, or keep as-is if only single mapping is supported in proto.

2. Add backward compatibility test (AC6.7): Deserialize a proto that was serialized WITHOUT the new fields, verify all new fields default correctly (default_equation=None, data_source=None, mappings from maps_to).

3. Add XMILE round-trip tests for each of the three constructs:
   - EXCEPT: write model with default_equation, re-read, verify preserved
   - Element mappings: write model with element-level mapping, re-read, verify preserved
   - DataSource: write model with data_source, re-read, verify preserved

**Testing:**

- close-array-gaps.AC6.1: Proto EXCEPT round-trip test
- close-array-gaps.AC6.2: Proto element mapping round-trip test
- close-array-gaps.AC6.3: Proto DataSource round-trip test
- close-array-gaps.AC6.4: XMILE EXCEPT round-trip test
- close-array-gaps.AC6.5: XMILE element mapping round-trip test
- close-array-gaps.AC6.6: XMILE DataSource round-trip test
- close-array-gaps.AC6.7: Backward compat test (old protos without new fields)

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All tests pass, including converted rejection tests and new round-trip tests.

**Commit:** `engine: convert serialization rejection tests to round-trip success tests`
<!-- END_TASK_4 -->
