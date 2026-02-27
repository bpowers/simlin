# MDL Full Compatibility -- Phase 4: DataProvider Infrastructure and External Data

**Goal:** Pluggable external data loading from CSV/Excel files via a DataProvider trait, resolving GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT during MDL conversion so data-backed variables become regular lookup tables or constants.

**Architecture:** A sync `DataProvider` trait abstracts data file access. `FilesystemDataProvider` (feature-gated) handles CSV and Excel files for native builds. `NullDataProvider` returns errors for WASM/browser contexts. During MDL conversion, GET DIRECT functions are parsed from the opaque equation strings and resolved via DataProvider between dimension building and variable conversion. Data variables become ordinary lookup-backed Aux variables with `GraphicalFunction` tables.

**Tech Stack:** Rust (simlin-engine crate), calamine crate (Excel, feature-gated), csv crate (behind `file_io` feature)

**Scope:** 7 phases from original design (phase 4 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC3: External data via DataProvider
- **mdl-full-compat.AC3.1 Success:** `DataProvider` trait compiles for both native and WASM targets
- **mdl-full-compat.AC3.2 Success:** `FilesystemDataProvider` loads CSV data files and produces correct lookup tables
- **mdl-full-compat.AC3.3 Success:** `FilesystemDataProvider` loads Excel (XLS) data files via `calamine` crate (feature-gated)
- **mdl-full-compat.AC3.4 Success:** GET DIRECT DATA variables simulate as time-indexed lookups with correct interpolation
- **mdl-full-compat.AC3.5 Failure:** `NullDataProvider` returns clear error when data file is referenced but no provider configured

---

## Reference Files

- `/home/bpowers/src/simlin/src/simlin-engine/src/compat.rs` -- `open_vensim()` (line 58), `load_dat`/`load_csv` (line 67+)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/convert/mod.rs` -- `convert_mdl()` (line 296), pipeline (line 125)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/convert/variables.rs` -- `MdlEquation::Data` handling (line 733)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/builtins.rs` -- `SymbolClass::GetXls` (line 346)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/normalizer.rs` -- `read_get_xls()` (line 340)
- `/home/bpowers/src/simlin/src/simlin-engine/Cargo.toml` -- features (file_io, etc.)
- `/home/bpowers/src/simlin/src/libsimlin/src/lib.rs` -- FFI surface
- `/home/bpowers/src/simlin/src/libsimlin/src/project.rs` -- `simlin_project_open_vensim` (line 516)

Test models:
- `test/sdeverywhere/models/directdata/` -- GET DIRECT DATA with CSV and Excel, `data.xlsx`, `e_data.csv`
- `test/sdeverywhere/models/directconst/` -- GET DIRECT CONSTANTS with CSV, `data/` subdir
- `test/sdeverywhere/models/directlookups/` -- GET DIRECT LOOKUPS with CSV, `lookup_data.csv`
- `test/sdeverywhere/models/directsubs/` -- GET DIRECT SUBSCRIPT, `b_subs.csv`, `c_subs.csv`
- `test/sdeverywhere/models/extdata/` -- implicit external data

External research:
- calamine 0.33.0: Pure Rust Excel reader (XLS/XLSX), WASM-compatible
- `open_workbook::<Xlsx<_>>(path)` for file, `open_workbook_from_rs(Cursor::new(bytes))` for WASM
- `Data::as_f64()` handles both Int and Float; no built-in A1 address parser
- csv crate already in Cargo.toml behind `file_io` feature

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: DataProvider Trait and Implementations

<!-- START_TASK_1 -->
### Task 1: Define DataProvider trait and NullDataProvider

**Verifies:** mdl-full-compat.AC3.1, mdl-full-compat.AC3.5

**Files:**
- Create: `src/simlin-engine/src/data_provider.rs`
- Modify: `src/simlin-engine/src/lib.rs` (add `mod data_provider; pub use data_provider::*;`)

**Implementation:**

Create a new file for the DataProvider abstraction:

```rust
use crate::common::Result;

/// Trait for resolving external data references during compilation.
/// Native builds use FilesystemDataProvider; WASM callers provide
/// pre-loaded data via an adapter implementing this trait.
pub trait DataProvider {
    /// Load a time-indexed data series from an external file.
    /// Returns (time, value) pairs suitable for interpolation.
    fn load_data(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_or_col_label: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load a constant value from an external file.
    fn load_constant(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64>;

    /// Load a lookup table from an external file.
    fn load_lookup(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load dimension element names from an external file.
    fn load_subscript(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>>;
}

/// Default provider that returns errors for all operations.
/// Used when no data files are available (WASM default).
pub struct NullDataProvider;

impl DataProvider for NullDataProvider {
    fn load_data(&self, file: &str, _: &str, _: &str, _: &str) -> Result<Vec<(f64, f64)>> {
        Err(format!(
            "external data file '{}' referenced but no DataProvider configured",
            file
        ).into())
    }

    fn load_constant(&self, file: &str, _: &str, _: &str, _: &str) -> Result<f64> {
        Err(format!(
            "external data file '{}' referenced but no DataProvider configured",
            file
        ).into())
    }

    fn load_lookup(&self, file: &str, _: &str, _: &str, _: &str) -> Result<Vec<(f64, f64)>> {
        Err(format!(
            "external data file '{}' referenced but no DataProvider configured",
            file
        ).into())
    }

    fn load_subscript(&self, file: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>> {
        Err(format!(
            "external data file '{}' referenced but no DataProvider configured",
            file
        ).into())
    }
}
```

**Testing:**

Add tests verifying:
- `NullDataProvider.load_data("file.csv", ...)` returns an error containing the filename
- Same for load_constant, load_lookup, load_subscript

**Verification:**
Run: `cargo test -p simlin-engine data_provider`
Expected: All tests pass

**Commit:** `engine: add DataProvider trait and NullDataProvider`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement FilesystemDataProvider for CSV

**Verifies:** mdl-full-compat.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/data_provider.rs`

**Implementation:**

Add a `FilesystemDataProvider` that resolves file paths relative to a base directory and reads CSV files. Gate it behind the `file_io` feature (which already enables the `csv` crate).

```rust
#[cfg(feature = "file_io")]
pub struct FilesystemDataProvider {
    /// Base directory for resolving relative file paths
    base_dir: std::path::PathBuf,
}

#[cfg(feature = "file_io")]
impl FilesystemDataProvider {
    pub fn new(base_dir: impl Into<std::path::PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    fn resolve_path(&self, file: &str) -> std::path::PathBuf {
        self.base_dir.join(file)
    }
}
```

For CSV data loading, implement the Vensim cell-addressing conventions:
- The `tab_or_delimiter` parameter: for CSV, this is the delimiter character (`,` or `\t`)
- The `row_or_col_label` parameter: column letter(s) identifying the time column (e.g., `"A"`)
- The `cell_label` parameter: cell reference for the start of data (e.g., `"B2"`)

The Vensim CSV convention:
- Row 1 contains headers
- The time column is identified by `row_or_col_label`
- Data starts at `cell_label` row and runs down
- Multiple series can share a time column

Implement `parse_cell_ref()` to convert A1-style references to (row, col) indices:
```rust
fn col_index(col: &str) -> usize {
    col.bytes().fold(0usize, |acc, b| acc * 26 + (b - b'A' + 1) as usize) - 1
}

fn parse_cell_ref(s: &str) -> (usize, usize) {
    let split = s.find(|c: char| c.is_ascii_digit()).unwrap();
    let col = col_index(&s[..split]);
    let row: usize = s[split..].parse::<usize>().unwrap() - 1;
    (row, col)
}
```

For `load_data`: Read CSV with the given delimiter, find the time column, extract (time, value) pairs from the data column starting at cell_label.

For `load_constant`: Read a single cell value at the specified location.

For `load_lookup`: Same as load_data but the result is treated as lookup (x, y) pairs rather than time-indexed.

For `load_subscript`: Read a range of cells and return the string values as dimension element names.

**Testing:**

Create test CSV files in the test directory and write tests verifying:
- `load_data` reads correct (time, value) pairs from a CSV
- `load_constant` reads a single numeric value
- `load_subscript` reads element names from a column range
- Delimiter parameter is respected

**Verification:**
Run: `cargo test --features file_io -p simlin-engine data_provider`
Expected: All tests pass

**Commit:** `engine: implement FilesystemDataProvider for CSV files`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add Excel support to FilesystemDataProvider

**Verifies:** mdl-full-compat.AC3.3

**Files:**
- Modify: `src/simlin-engine/Cargo.toml` (add calamine dependency behind feature flag)
- Modify: `src/simlin-engine/src/data_provider.rs`

**Implementation:**

Add calamine as an optional dependency:

```toml
[features]
ext_data = ["dep:calamine"]

[dependencies]
calamine = { version = "0.33", optional = true }
```

In `FilesystemDataProvider`, detect file format by extension:
- `.csv`, `.tsv`: use CSV reader
- `.xls`, `.xlsx`, `.xlsm`: use calamine
- If `tab_or_delimiter` is a single character, treat as CSV delimiter; otherwise treat as Excel sheet name

```rust
#[cfg(feature = "file_io")]
impl FilesystemDataProvider {
    fn is_excel_file(path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("xls" | "xlsx" | "xlsm")
        )
    }
}
```

For Excel reading with calamine:
- Open workbook with `calamine::open_workbook_auto(path)`
- Select sheet by `tab_or_delimiter` name
- Parse cell references the same way as CSV
- Read data using `range.get_value((row, col))` and `Data::as_f64()`

The `directdata` test model references `?data` which is resolved to `data.xlsx` via the MDL settings section. Handle the `?` prefix lookup in the conversion step (Task 5), not here.

**Testing:**

Use the existing `test/sdeverywhere/models/directdata/data.xlsx` file for integration tests:
- `load_data` from an Excel sheet returns correct values
- `load_constant` from Excel returns correct value
- Error on missing sheet name

**Verification:**
Run: `cargo test --features file_io,ext_data -p simlin-engine data_provider`
Expected: All tests pass

**Commit:** `engine: add Excel support to FilesystemDataProvider via calamine`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
## Subcomponent B: MDL Conversion Integration

<!-- START_TASK_4 -->
### Task 4: Thread DataProvider through open_vensim and convert pipeline

**Verifies:** mdl-full-compat.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/compat.rs:58` (`open_vensim` signature)
- Modify: `src/simlin-engine/src/mdl/convert/mod.rs:44-81,296` (`ConversionContext`, `convert_mdl`)
- Modify: `src/simlin-engine/src/mdl/mod.rs` (if `parse_mdl` exists, update signature)

**Implementation:**

Add an optional DataProvider parameter to the conversion pipeline:

```rust
// compat.rs
pub fn open_vensim(contents: &str) -> Result<Project> {
    open_vensim_with_data(contents, None)
}

pub fn open_vensim_with_data(
    contents: &str,
    data_provider: Option<&dyn DataProvider>,
) -> Result<Project> {
    mdl::parse_mdl_with_data(contents, data_provider)
}
```

Thread the DataProvider through:
1. `mdl::parse_mdl_with_data(contents, data_provider)` -- new entry point
2. `ConversionContext::new_with_data(source, data_provider)` -- stores provider reference
3. `ConversionContext` gains field: `data_provider: Option<&'a dyn DataProvider>`

The existing `open_vensim(&str)` remains backward-compatible (passes `None`).

**Testing:**

Verify `open_vensim("simple model text")` still works with no DataProvider.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All existing tests pass (no behavioral change)

**Commit:** `engine: thread DataProvider through open_vensim and convert pipeline`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Parse and resolve GET DIRECT functions during conversion

**Verifies:** mdl-full-compat.AC3.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/mod.rs` (add resolution pass)
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (handle resolved data in equation building)

**Implementation:**

Add a new pass in the conversion pipeline between Pass 2 (build_dimensions) and Pass 3 (mark_variable_types). This "Pass 2.7: resolve_external_data" pass:

1. Iterate all equations in `self.items`
2. For `MdlEquation::Data(lhs, Some(expr))` where expr text starts with `{GET DIRECT`:
   a. Parse the opaque string to extract function name and arguments
   b. Call the appropriate DataProvider method
   c. Transform the variable:
      - GET DIRECT DATA: Replace with lookup-backed variable (GraphicalFunction with time/value pairs)
      - GET DIRECT CONSTANTS: Replace equation with constant value
      - GET DIRECT LOOKUPS: Replace with lookup-backed variable
3. For `MdlEquation::SubscriptDef` referencing GET DIRECT SUBSCRIPT:
   a. Extract the opaque string from the dimension definition
   b. Call `data_provider.load_subscript()`
   c. Replace the dimension's element list with the loaded names

The opaque string format from the normalizer is: `{GET DIRECT DATA('file','delim','col','row')}`. Parse by stripping braces, extracting the function name, and splitting the quoted arguments.

Handle the `?data` alias pattern: the MDL settings section (Pass 6+ area) may contain `30:?data=data.xlsx`. Parse these settings earlier and use them to resolve `?` prefixed file references. Check if the settings parser in `settings.rs` already handles this.

When DataProvider is None and GET DIRECT functions are encountered, store the `DataSource` metadata on the variable's `Compat` struct (from Phase 2) and emit an appropriate error or warning.

**Testing:**

Create a minimal MDL snippet with a GET DIRECT DATA equation. With a mock DataProvider that returns known data, verify:
- The equation is replaced with a lookup-backed variable
- The GraphicalFunction contains the correct (time, value) pairs
- The variable simulates correctly

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: resolve GET DIRECT DATA/CONSTANTS/LOOKUPS during MDL conversion`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Resolve GET DIRECT SUBSCRIPT during dimension building

**Verifies:** mdl-full-compat.AC3.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/dimensions.rs` (inside `build_dimensions()`)

**Implementation:**

GET DIRECT SUBSCRIPT appears in dimension definitions like:
```
DimB: GET DIRECT SUBSCRIPT('b_subs.csv', ',', 'A2', 'A', '')
```

In `build_dimensions()` Phase 1a (line 19-29), when a `SubscriptDef` is encountered, check if its elements contain a GET DIRECT SUBSCRIPT reference. If so:

1. Parse the opaque string to extract file, delimiter, first_cell, last_cell arguments
2. Call `data_provider.load_subscript(file, delimiter, first_cell, last_cell)`
3. Replace the dimension's element list with the returned names
4. Continue with normal dimension building using the resolved elements

The `raw_subscript_defs` map stores element lists -- the loaded names replace whatever the parser found.

If DataProvider is None and GET DIRECT SUBSCRIPT is encountered, return a clear error.

**Testing:**

With a mock DataProvider that returns `["B1", "B2", "B3"]` for a subscript load, verify:
- A dimension defined with GET DIRECT SUBSCRIPT gets the correct element names
- Variables subscripted by that dimension can be built

**Verification:**
Run: `cargo test -p simlin-engine mdl::convert`
Expected: All tests pass

**Commit:** `engine: resolve GET DIRECT SUBSCRIPT during dimension building`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-9) -->
## Subcomponent C: FFI and Test Enablement

<!-- START_TASK_7 -->
### Task 7: Update libsimlin FFI for DataProvider

**Verifies:** mdl-full-compat.AC3.1

**Files:**
- Modify: `src/libsimlin/src/project.rs` (around line 516)

**Implementation:**

Add a new FFI function that accepts a DataProvider configuration alongside the model data. The simplest approach for the FFI boundary is a base directory path for FilesystemDataProvider:

```rust
/// Open a Vensim MDL model with external data file support.
/// `data_dir` is an optional path to the directory containing data files.
/// If NULL, a NullDataProvider is used (data file references will error).
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_vensim_with_data(
    data: *const u8,
    len: usize,
    data_dir: *const u8,
    data_dir_len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject
```

When `data_dir` is non-null, create a `FilesystemDataProvider` with that base directory. When null, use `NullDataProvider`.

Keep the existing `simlin_project_open_vensim` unchanged for backward compatibility.

**Testing:**

The FFI is tested through the TypeScript engine wrapper. Add a basic test that the new function compiles and can be called with NULL data_dir.

**Verification:**
Run: `cargo build -p libsimlin`
Expected: Builds without errors

**Commit:** `libsimlin: add simlin_project_open_vensim_with_data FFI function`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Enable and run directdata, directconst, directlookups, directsubs test models

**Verifies:** mdl-full-compat.AC3.2, mdl-full-compat.AC3.3, mdl-full-compat.AC3.4

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (uncomment test models, add DataProvider setup)

**Implementation:**

The test models require DataProvider integration in the test harness. The simulate.rs test currently calls `open_vensim(contents)`. For data models, it needs to call `open_vensim_with_data(contents, Some(&provider))` where `provider` is a `FilesystemDataProvider` pointing to the test model's directory.

1. Modify the `simulate_path()` helper to detect data-dependent models and provide a `FilesystemDataProvider`
2. Uncomment the test models from `TEST_SDEVERYWHERE_MODELS`:
   - `directdata/directdata.xmile`
   - `directconst/directconst.xmile`
   - `directlookups/directlookups.xmile`
   - `directsubs/directsubs.xmile`
   - `extdata/extdata.xmile`

Note: The XMILE versions of these models (produced by xmutil) may handle data differently than the native MDL path. The native MDL path resolves data during conversion; the XMILE path may have the data already embedded. Investigate how xmutil handles GET DIRECT DATA in the XMILE output.

3. Add MDL-path-specific tests for the data models that use `open_vensim_with_data()` directly

**Testing:**

Run each data model through:
1. MDL parse + convert with FilesystemDataProvider
2. Simulate via interpreter
3. Simulate via VM
4. Compare against `.dat` expected output

**Verification:**
Run: `cargo test --features file_io,ext_data --test simulate -- direct`
Expected: directdata, directconst, directlookups, directsubs tests pass

Run: `cargo test --features file_io,ext_data --test simulate`
Expected: All tests pass (no regressions)

**Commit:** `engine: enable external data test models`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Final verification

**Verifies:** mdl-full-compat.AC3.1, mdl-full-compat.AC3.2, mdl-full-compat.AC3.3, mdl-full-compat.AC3.4, mdl-full-compat.AC3.5

**Files:** None (verification only)

**Implementation:**

Run the full test suite:

```bash
cargo test -p simlin-engine
cargo test --features file_io,ext_data --test simulate
cargo test -p simlin-engine --test mdl_roundtrip
cargo build -p libsimlin
```

Verify:
- NullDataProvider returns clear errors (AC3.5)
- FilesystemDataProvider loads CSV (AC3.2) and Excel (AC3.3)
- DataProvider trait compiles for WASM: `cargo check -p simlin-engine --target wasm32-unknown-unknown` (AC3.1)
- GET DIRECT DATA variables simulate correctly (AC3.4)

No commit (verification only).
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_C -->
