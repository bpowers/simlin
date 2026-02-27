# MDL Full Compatibility -- Phase 2: Datamodel Extensions

**Goal:** Extend the datamodel to represent EXCEPT syntax, richer dimension mappings, and data source metadata, with full JSON serialization and MDL writer support.

**Architecture:** Three datamodel extensions: (1) `Equation::Arrayed` gains a third field for default equation (EXCEPT), (2) `Dimension.maps_to` replaced by `mappings: Vec<DimensionMapping>` for element-level correspondence, (3) new `DataSource` metadata struct for external data references. JSON serialization provides full fidelity; MDL writer emits `:EXCEPT:` syntax. Protobuf and XMILE return errors for unsupported constructs (tracked as GitHub issues).

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 2 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC4: EXCEPT support
- **mdl-full-compat.AC4.1 Success:** `Equation::Arrayed` with `default_equation` roundtrips through JSON serialization
- **mdl-full-compat.AC4.2 Success:** MDL writer emits `:EXCEPT:` syntax for Arrayed equations with default_equation

### mdl-full-compat.AC7: Serialization
- **mdl-full-compat.AC7.1 Success:** sd.json format includes all new datamodel fields (default_equation, DimensionMapping, DataSource)
- **mdl-full-compat.AC7.2 Success:** sd.json roundtrip preserves all new fields
- **mdl-full-compat.AC7.3 Failure:** Protobuf and XMILE serialization return explicit errors for unsupported constructs (not silent data loss)

---

## Reference Files

- `/home/bpowers/src/simlin/src/simlin-engine/src/datamodel.rs` -- `Equation` enum (line 192), `Dimension` struct (line 737)
- `/home/bpowers/src/simlin/src/simlin-engine/src/json.rs` -- JSON serialization (`ArrayedEquation` line 65, `Dimension` line 480)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/writer.rs` -- MDL writer (`write_arrayed_entries` line 926, `write_dimension_def` line 973)
- `/home/bpowers/src/simlin/src/simlin-engine/src/serde.rs` -- protobuf serialization (line 294+)
- `/home/bpowers/src/simlin/src/simlin-engine/src/xmile/variables.rs` -- XMILE Equation handling
- `/home/bpowers/src/simlin/src/simlin-engine/src/xmile/dimensions.rs` -- XMILE Dimension handling
- `/home/bpowers/src/simlin/src/simlin-engine/tests/json_roundtrip.rs` -- existing roundtrip tests
- `/home/bpowers/src/simlin/src/simlin-engine/tests/mdl_roundtrip.rs` -- MDL writer roundtrip tests

All `Equation::Arrayed` match sites (~30+ locations):
- `datamodel.rs`, `json.rs`, `serde.rs`, `variable.rs`, `project.rs`, `interpreter.rs`
- `mdl/writer.rs`, `mdl/convert/variables.rs`
- `xmile/variables.rs`, `xmile/dimensions.rs`
- `ai_info.rs`, `json_sdai.rs`, `layout/mod.rs`, `diagram/render.rs`
- `db.rs`, `db_tests.rs`
- `tests/mdl_equivalence.rs`, `tests/mdl_roundtrip.rs`
- `src/simlin-cli/src/gen_stdlib.rs`

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
## Subcomponent A: Equation::Arrayed Default Equation Extension

<!-- START_TASK_1 -->
### Task 1: Add default_equation field to Equation::Arrayed

**Verifies:** mdl-full-compat.AC4.1 (datamodel representation)

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs:192-204`

**Implementation:**

Add a third field `Option<String>` to the `Equation::Arrayed` variant to represent the default equation for `:EXCEPT:` syntax. When `Some(eq)`, elements not present in the element list use `eq` as their equation:

```rust
pub enum Equation {
    Scalar(String),
    ApplyToAll(Vec<DimensionName>, String),
    Arrayed(
        Vec<DimensionName>,
        Vec<(
            ElementName,
            String,
            Option<String>,
            Option<GraphicalFunction>,
        )>,
        // Default equation for elements not explicitly listed (EXCEPT semantics).
        // When Some, this equation applies to all elements not in the Vec above.
        Option<String>,
    ),
}
```

Existing code that constructs `Arrayed` with the current 2-field shape will need a third `None` argument added. Existing match sites that destructure `(dims, elements)` will need a third binding.

**Verification:**
Run: `cargo check -p simlin-engine`
Expected: Does NOT compile yet (match arms need updating in Task 2)

**Commit:** No commit yet (combined with Task 2)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update all Equation::Arrayed match arms across codebase

**Verifies:** mdl-full-compat.AC4.1

**Files:**
- Modify: All files listed in "All Equation::Arrayed match sites" in Reference Files above

**Implementation:**

Fix every compilation error from the new third field. For each match site:

**Pattern-match sites** -- add a third binding or wildcard:
```rust
// Before:
Equation::Arrayed(dims, elements) => { ... }

// After (when default_equation not needed):
Equation::Arrayed(dims, elements, _default_eq) => { ... }

// After (when default_equation IS needed, e.g. writer):
Equation::Arrayed(dims, elements, default_equation) => { ... }
```

**Constructor sites** -- add `None` as third argument:
```rust
// Before:
Equation::Arrayed(dims, elements)

// After:
Equation::Arrayed(dims, elements, None)
```

Key files that need the third field handled meaningfully (not just `_`):
- `json.rs` -- serialize/deserialize default_equation (Task 4)
- `mdl/writer.rs` -- emit `:EXCEPT:` syntax (Task 5)
- `serde.rs` -- protobuf handling (Task 6)
- `xmile/variables.rs` -- XMILE handling (Task 6)
- `tests/mdl_equivalence.rs` -- normalization of default_equation

All other sites can use `_default_eq` wildcard for now.

**Verification:**
Run: `cargo check -p simlin-engine`
Expected: Compiles successfully (serialization tests may fail until Tasks 4-5)

Run: `cargo test -p simlin-engine`
Expected: All tests pass (no behavioral change since all new fields are `None`)

**Commit:** `engine: add default_equation field to Equation::Arrayed for EXCEPT support`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
## Subcomponent B: Dimension Mapping Extension

<!-- START_TASK_3 -->
### Task 3: Add DimensionMapping struct and replace Dimension.maps_to

**Verifies:** mdl-full-compat.AC7.1

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs:737-749`
- Modify: All files that reference `Dimension.maps_to`

**Implementation:**

Add a new `DimensionMapping` struct and replace the `maps_to: Option<String>` field on `Dimension`:

```rust
/// Element-level correspondence between two subscript families.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct DimensionMapping {
    /// Target dimension name
    pub target: String,
    /// Element-level correspondence. When empty, positional mapping is assumed.
    /// When present, maps source elements to target elements by name.
    pub element_map: Vec<(String, String)>,
}

pub struct Dimension {
    pub name: String,
    pub elements: DimensionElements,
    /// Dimension mappings. Replaces the previous maps_to field.
    /// Supports both simple (single target) and multi-entry
    /// (element-level correspondence) mappings.
    pub mappings: Vec<DimensionMapping>,
}
```

Migration path for existing code that uses `maps_to`:
- `dim.maps_to = Some(target)` becomes `dim.mappings = vec![DimensionMapping { target, element_map: vec![] }]`
- `dim.maps_to.as_ref()` becomes `dim.mappings.first().map(|m| &m.target)` (for simple single-target lookups)
- Provide a helper method on `Dimension`:

```rust
impl Dimension {
    /// Convenience accessor for the simple single-target mapping case.
    /// Returns the target dimension name if exactly one positional mapping exists.
    pub fn maps_to(&self) -> Option<&str> {
        if self.mappings.len() == 1 && self.mappings[0].element_map.is_empty() {
            Some(&self.mappings[0].target)
        } else {
            None
        }
    }
}
```

This helper minimizes churn at call sites that only need the simple mapping case. However, note that call sites which previously used `maps_to` for ANY mapping (including element-level ones from Phase 6) will now get `None` from the helper for element-level mappings. Audit each call site to determine whether it needs element-level mapping awareness. Call sites in the compiler that do dimension mapping resolution (e.g., `compiler/context.rs`, `compiler/dimensions.rs`) should use the full `mappings` field, not the helper.

Update all files that reference `dim.maps_to` to use either the helper or the full `mappings` field as appropriate. Key locations:
- `mdl/convert/dimensions.rs` -- constructs Dimension with maps_to
- `mdl/writer.rs` -- writes `-> target` for dimension definitions
- `json.rs` -- serializes/deserializes maps_to
- `xmile/dimensions.rs` -- serializes/deserializes maps_to
- `serde.rs` -- protobuf handling
- `compiler/dimensions.rs` -- uses maps_to for dimension matching
- `tests/mdl_equivalence.rs` -- compares maps_to in normalization

**Verification:**
Run: `cargo check -p simlin-engine`
Expected: Compiles

Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: replace Dimension.maps_to with mappings: Vec<DimensionMapping>`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add DataSource metadata struct

**Verifies:** mdl-full-compat.AC7.1

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs`

**Implementation:**

Add a metadata struct that stores parsed arguments from GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT functions. This metadata is set during MDL conversion (Phase 4) when DataProvider resolves external data references:

```rust
/// Metadata for variables backed by external data files.
/// Stores the parsed arguments from GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT
/// so the MDL writer can reconstruct the original function call.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct DataSource {
    /// The kind of data function (Data, Constants, Lookups, Subscript)
    pub kind: DataSourceKind,
    /// Path to the external data file
    pub file: String,
    /// Tab/sheet name (for Excel) or delimiter (for CSV)
    pub tab_or_delimiter: String,
    /// Row or column label for data lookup
    pub row_or_col: String,
    /// Cell label for data lookup
    pub cell: String,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum DataSourceKind {
    Data,
    Constants,
    Lookups,
    Subscript,
}
```

Add an optional `data_source: Option<DataSource>` field to the `Compat` struct (since data source metadata is Vensim-specific compatibility information):

```rust
pub struct Compat {
    pub active_initial: Option<String>,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    pub visibility: Visibility,
    pub data_source: Option<DataSource>,
}
```

Update `Compat::default()` to include `data_source: None`.

**Verification:**
Run: `cargo check -p simlin-engine`
Expected: Compiles

Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: add DataSource metadata struct for external data references`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-7) -->
## Subcomponent C: Serialization

<!-- START_TASK_5 -->
### Task 5: Update JSON serialization for all new fields

**Verifies:** mdl-full-compat.AC4.1, mdl-full-compat.AC7.1, mdl-full-compat.AC7.2

**Files:**
- Modify: `src/simlin-engine/src/json.rs` (lines 65-110 `ArrayedEquation`/`ElementEquation`, line 480 `Dimension`)

**Implementation:**

**ArrayedEquation changes** -- The JSON `ArrayedEquation` struct already has an `equation: Option<String>` field (line 73) used for ApplyToAll. For EXCEPT support, when `default_equation` is `Some`, serialize it as the `equation` field WITH `elements` also present. Currently `equation` and `elements` are mutually exclusive (ApplyToAll vs Arrayed). With EXCEPT, both are present:

```rust
// In From<datamodel Equation> for JSON:
Equation::Arrayed(dims, elements, default_eq) => {
    ArrayedEquation {
        dimensions: dims,
        equation: default_eq,  // default equation (EXCEPT)
        elements: Some(element_equations),
        compat: ...,
    }
}

// In From<JSON> for datamodel Equation:
// If arrayed.elements.is_some() AND arrayed.equation.is_some()
//   -> Equation::Arrayed(dims, elements, Some(equation))
// If arrayed.elements.is_some() AND arrayed.equation.is_none()
//   -> Equation::Arrayed(dims, elements, None)
// If arrayed.elements.is_none() AND arrayed.equation.is_some()
//   -> Equation::ApplyToAll(dims, equation)
```

**Dimension changes** -- Add `mappings` field to JSON `Dimension` struct:

```rust
pub struct Dimension {
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub elements: Vec<String>,
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub size: i32,
    // Keep maps_to for backward compatibility with existing JSON files
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub maps_to: Option<String>,
    // New: element-level dimension mappings
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub mappings: Vec<JsonDimensionMapping>,
}

pub struct JsonDimensionMapping {
    pub target: String,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub element_map: Vec<JsonElementMapEntry>,
}

pub struct JsonElementMapEntry {
    pub source: String,
    pub target: String,
}
```

For From conversions:
- If `datamodel::Dimension` has a single positional mapping (empty element_map), serialize as `maps_to` for backward compat
- If it has element-level mappings, serialize in the `mappings` array
- On deserialization, check `mappings` first; if empty, check `maps_to` and convert to single `DimensionMapping`

**DataSource/Compat changes** -- Add `data_source` field to JSON `Compat` struct with skip_serializing_if None.

**Testing:**

Add JSON roundtrip tests:
- Arrayed equation with default_equation roundtrips (EXCEPT case)
- Dimension with element-level mapping roundtrips
- Dimension with simple maps_to still roundtrips (backward compat)
- DataSource metadata roundtrips through Compat

**Verification:**
Run: `cargo test -p simlin-engine --test json_roundtrip`
Expected: All tests pass including new roundtrip tests

Run: `cargo test -p simlin-engine json`
Expected: All JSON-related tests pass

**Commit:** `engine: JSON serialization for default_equation, dimension mappings, and data source`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Update MDL writer for EXCEPT syntax

**Verifies:** mdl-full-compat.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/mdl/writer.rs` (lines 755, 804, 926, 973)

**Implementation:**

When `Equation::Arrayed` has a `default_equation` (`Some(eq)`), the MDL writer should emit `:EXCEPT:` syntax. The Vensim MDL format for EXCEPT is:

```
var[DimA] = default_equation ~~|
var[element1] = override1 ~~|
var[element2] = override2 ~
    ~ units
    ~ comment |
```

The first entry uses the full dimension name (not a specific element) and contains the default equation. Then `:EXCEPT:` entries follow with specific element overrides.

Modify `write_arrayed_entries()` (line 926) and `write_arrayed_stock_entries()` (line 849):

```rust
fn write_arrayed_entries(
    buf: &mut String,
    ident: &str,
    dims: &[String],
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    default_equation: &Option<String>,
    units: &Option<String>,
    doc: &Option<String>,
) {
    if let Some(default_eq) = default_equation {
        // Write default equation with dimension names
        let dim_display = dims.join(",");
        write!(buf, "{}[{}]=\n\t{}", format_mdl_ident(ident), dim_display, default_eq).unwrap();
        buf.push_str("\n\t~~|");
        // Write :EXCEPT: override entries
        for (element, eq_str, _initial, _gf) in elements {
            let elem_display = element.replace(',', ",");
            write!(buf, "\n{}[{}] :EXCEPT: =\n\t{}", format_mdl_ident(ident), elem_display, eq_str).unwrap();
            // ... separator logic
        }
    } else {
        // Existing behavior: write per-element entries
        // ... current code
    }
}
```

Also update `write_dimension_def()` (line 973) to handle the new `mappings` field:
- For simple mappings (empty element_map): write `-> target` as before
- For element-level mappings: write the Vensim element-level mapping syntax

**Testing:**

Add MDL writer tests that:
- Write an Arrayed equation with default_equation and verify `:EXCEPT:` syntax appears in output
- Roundtrip an EXCEPT equation through MDL write -> parse -> compare

**Verification:**
Run: `cargo test -p simlin-engine mdl::writer`
Expected: All tests pass

Run: `cargo test -p simlin-engine --test mdl_roundtrip`
Expected: All roundtrip tests pass

**Commit:** `engine: MDL writer emits EXCEPT syntax for arrayed equations with default`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Protobuf and XMILE error handling for unsupported constructs

**Verifies:** mdl-full-compat.AC7.3

**Files:**
- Modify: `src/simlin-engine/src/serde.rs` (protobuf serialization)
- Modify: `src/simlin-engine/src/xmile/variables.rs` (XMILE serialization)

**Implementation:**

For protobuf serialization (`serde.rs`):
- When serializing `Equation::Arrayed` with a non-None `default_equation`, return an explicit error instead of silently dropping the default equation
- When serializing `Dimension` with element-level mappings (non-empty `element_map`), return an explicit error
- When serializing `Compat` with `data_source`, return an explicit error

For XMILE serialization (`xmile/variables.rs`):
- Same pattern: explicit errors for EXCEPT equations, element-level dimension mappings, and data source metadata

The error should be descriptive, e.g.: `"Protobuf serialization does not support EXCEPT (default_equation) in Equation::Arrayed -- use sd.json format for full fidelity"`

**Testing:**

Add tests that verify:
- Protobuf serialization of an EXCEPT equation returns an error (not silent data loss)
- XMILE serialization of an EXCEPT equation returns an error

**Verification:**
Run: `cargo test -p simlin-engine serde`
Expected: All tests pass

**Commit:** `engine: return explicit errors for unsupported proto/XMILE constructs`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_TASK_8 -->
### Task 8: File GitHub issues for protobuf and XMILE serialization gaps

**Verifies:** mdl-full-compat.AC7.3

**Files:** None (GitHub issues only)

**Implementation:**

File two GitHub issues:

1. **Protobuf serialization gap**: "Add protobuf support for EXCEPT equations, element-level dimension mappings, and DataSource metadata"
   - Body: Describe the new datamodel constructs, current error behavior, and what proto schema changes would be needed
   - Label: enhancement

2. **XMILE serialization gap**: "Add XMILE support for EXCEPT equations, element-level dimension mappings, and DataSource metadata"
   - Body: Describe the XMILE schema limitations and potential extension points
   - Label: enhancement

Use `gh issue create` CLI commands.

**Verification:**
Run: `gh issue list --label enhancement`
Expected: Both issues appear

**Commit:** No commit (GitHub issues only)
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Final verification -- all tests pass

**Verifies:** mdl-full-compat.AC4.1, mdl-full-compat.AC4.2, mdl-full-compat.AC7.1, mdl-full-compat.AC7.2, mdl-full-compat.AC7.3

**Files:** None (verification only)

**Implementation:**

Run the full test suite:

**Step 1:** Engine unit tests
```bash
cargo test -p simlin-engine
```
Expected: All tests pass

**Step 2:** JSON roundtrip tests
```bash
cargo test -p simlin-engine --test json_roundtrip
```
Expected: All tests pass including new EXCEPT and DimensionMapping roundtrips

**Step 3:** MDL roundtrip tests
```bash
cargo test -p simlin-engine --test mdl_roundtrip
```
Expected: All tests pass including EXCEPT roundtrip

**Step 4:** Simulation tests (no regression)
```bash
cargo test --features file_io --test simulate
```
Expected: All tests pass

No commit (verification only).
<!-- END_TASK_9 -->
