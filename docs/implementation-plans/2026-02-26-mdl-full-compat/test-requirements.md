# Test Requirements: mdl-full-compat

Maps each acceptance criterion to specific automated tests and their locations in the codebase.

## Conventions

- **Test file paths** are relative to the repository root (`/home/bpowers/src/simlin/`).
- **Automated** means CI-runnable with `cargo test`. All tests in this document are automated unless explicitly noted.
- **Feature flags** required for specific tests are noted in parentheses.
- **Tolerance** for simulation comparisons: 2e-3 absolute or 5e-6 relative for SDEverywhere models; 1% relative for C-LEARN VDF comparison.

---

## AC1: SDEverywhere models simulate via MDL path

### AC1.1: All sdeverywhere test models parse and convert to datamodel without errors

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | Iterates all entries in `TEST_SDEVERYWHERE_MODELS`. Each model is loaded via `simulate_path()` which triggers the full MDL parse and conversion pipeline. A parse or conversion failure causes a panic before simulation begins. (`file_io`, `ext_data`) |
| Integration | `src/simlin-engine/tests/mdl_equivalence.rs` :: `test_mdl_equivalence` (parameterized over `EQUIVALENT_MODELS`) | Parses each MDL via both the native Rust parser and xmutil, compares the resulting `datamodel::Project` structures. (`xmutil`) |
| Integration | `src/simlin-engine/tests/mdl_equivalence.rs` :: `test_clearn_equivalence` | Parses C-LEARN MDL via both parsers and compares project structures. Currently `#[ignore]`; to be enabled in Phase 7. (`xmutil`) |

### AC1.2: All sdeverywhere test models simulate with results matching expected output within tolerance

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | After parse/convert, `simulate_path()` runs each model through both the interpreter and the VM, then calls `ensure_results()` which loads the `.dat` expected output file and compares every variable's time series against the reference. (`file_io`, `ext_data`) |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_except` | Dedicated test for except models via the MDL path. Currently `#[ignore]`; to be enabled in Phase 3. (`file_io`) |

---

## AC2: C-LEARN model works end-to-end

### AC2.1: C-LEARN simulation completes without not_simulatable error

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_clearn` (new) | Loads `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` via `open_vensim_with_data()` with `FilesystemDataProvider`. Creates a `Simulation`, calls `run_to_end()`. To be added in Phase 7. (`file_io`) |
| Integration | `src/simlin-engine/tests/mdl_equivalence.rs` :: `test_clearn_equivalence` | Verifies C-LEARN parses without panic via both parsers. To be enabled in Phase 7. (`xmutil`) |

### AC2.2: C-LEARN simulation results match VDF reference data within 1% relative error

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_clearn` (new) | After simulation, loads `test/xmutil_test_models/Ref.vdf`, converts to `Results` using `to_results_with_model()`, calls `ensure_vdf_results()` with 1% relative tolerance. Follows `simulates_wrld3_03()` pattern. (`file_io`) |

---

## AC3: External data via DataProvider

### AC3.1: DataProvider trait compiles for both native and WASM targets

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/data_provider.rs` :: `tests` module (new) | Verifies `DataProvider` trait and `NullDataProvider` compile and can be instantiated. Phase 4. |
| Build | (verification command) | `cargo check -p simlin-engine --target wasm32-unknown-unknown` confirms WASM compilation. Phase 4. |

### AC3.2: FilesystemDataProvider loads CSV data files and produces correct lookup tables

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/data_provider.rs` :: `tests` module (new) | Tests `FilesystemDataProvider::load_data()` with a test CSV file, verifying correct (time, value) pair extraction. Phase 4. (`file_io`) |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `directconst`, `directdata`, `directlookups`, `directsubs` models all use CSV data files. (`file_io`, `ext_data`) |

### AC3.3: FilesystemDataProvider loads Excel (XLS) data files via calamine crate

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/data_provider.rs` :: `tests` module (new) | Tests `load_data()` with `test/sdeverywhere/models/directdata/data.xlsx`. Phase 4. (`file_io`, `ext_data`) |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `directdata` and `extdata` models use Excel data. (`file_io`, `ext_data`) |

### AC3.4: GET DIRECT DATA variables simulate as time-indexed lookups with correct interpolation

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/mdl/convert/` :: `tests` module | Converts MDL snippet with GET DIRECT DATA using mock DataProvider, verifies `GraphicalFunction` with correct data points. Phase 4. |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `directdata`, `directlookups`, `directsubs`, `extdata` exercise GET DIRECT paths. (`file_io`, `ext_data`) |

### AC3.5: NullDataProvider returns clear error when data file is referenced but no provider configured

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/data_provider.rs` :: `tests` module (new) | Calls `NullDataProvider.load_data("file.csv", ...)` and asserts the `Err` contains the filename string. Phase 4. |

---

## AC4: EXCEPT support

### AC4.1: Equation::Arrayed with default_equation roundtrips through JSON serialization

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/json.rs` :: `tests` module | Tests `From<Equation>` and `From<ArrayedEquation>` conversions for the case with both `equation` (default) and `elements` (overrides). Phase 2. |
| Integration | `src/simlin-engine/tests/json_roundtrip.rs` | Roundtrip: constructs `Equation::Arrayed` with `default_equation`, serializes to JSON, deserializes, compares. Phase 2. |

### AC4.2: MDL writer emits :EXCEPT: syntax for Arrayed equations with default_equation

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/mdl/writer.rs` :: `tests` module | Writes `Equation::Arrayed` with default_equation to MDL, asserts output contains `:EXCEPT:`. Phase 2. |
| Integration | `src/simlin-engine/tests/mdl_roundtrip.rs` | Roundtrip: writes EXCEPT equation to MDL, re-parses, verifies same structure. Phase 2. |

### AC4.3: EXCEPT equations compile and simulate correctly

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/mdl/convert/variables.rs` :: `tests` module | Converts MDL with `:EXCEPT:`, verifies correct `default_equation`, override elements, per-element substitution. Phase 3. |
| Unit | `src/simlin-engine/src/variable.rs` :: `tests` module | Tests `Equation::Arrayed` with `default_equation` expansion to `Ast::Arrayed` covering all dimension elements. Phase 3. |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_except` | Loads except model, simulates, compares against `.dat`. Phase 3. (`file_io`) |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `except`, `except2`, `longeqns`, `ref` models. Phase 3/7. (`file_io`) |

---

## AC5: Missing builtins

### AC5.1: QUANTUM, SSHAPE, RAMP_FROM_TO produce correct values

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/vm.rs` or `src/simlin-engine/src/builtins.rs` :: `tests` | SSHAPE test values: `SSHAPE(0)~0`, `SSHAPE(0.5)=50`, `SSHAPE(1)~100`. Phase 5. |
| Unit | `src/simlin-engine/src/mdl/xmile_compat.rs` :: `tests` | RAMP FROM TO name mapping and QUANTUM inline expansion verification. Phase 5. |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `quantum/quantum.xmile`. Phase 5. (`file_io`) |

### AC5.2: DELAY FIXED, SAMPLE IF TRUE, NPV simulate correctly

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `delayfixed`, `delayfixed2`, `sample`, `npv` models compared against `.dat` reference. Phase 5. (`file_io`) |
| Unit (conditional) | `src/simlin-engine/src/builtins_visitor.rs` :: `tests` | If NPV is a stdlib model: verifies `.stmx` compiles and `builtins_visitor` expands correctly. Phase 5. |

### AC5.3: GET DATA BETWEEN TIMES retrieves correct values

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `getdata/getdata.xmile` compared against `.dat` reference. Phase 5. (`file_io`, `ext_data`) |
| Unit | `src/simlin-engine/src/builtins.rs` or `src/simlin-engine/src/compiler/codegen.rs` :: `tests` | Tests translation of GET DATA BETWEEN TIMES to lookup variant based on mode parameter. Phase 5. |

### AC5.4: VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, ALLOCATE AVAILABLE produce correct array outputs

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/compiler/codegen.rs` or `src/simlin-engine/src/interpreter.rs` :: `tests` | VECTOR SELECT with SUM/MIN/MAX actions, all-zeros, partial selection. Phase 6. |
| Unit | `src/simlin-engine/src/compiler/codegen.rs` or `src/simlin-engine/src/interpreter.rs` :: `tests` | VECTOR ELM MAP with known offsets, VECTOR SORT ORDER ascending/descending. Phase 6. |
| Unit | `src/simlin-engine/src/compiler/codegen.rs` or `src/simlin-engine/src/interpreter.rs` :: `tests` | ALLOCATE AVAILABLE: supply > demand, supply < demand, width smoothing. Phase 6. |
| Unit | `src/simlin-engine/src/compiler/codegen.rs` :: `tests` | SUM(IF THEN ELSE(...)) pattern: `If` expressions inside array iteration. Phase 6. |
| Integration | `src/simlin-engine/tests/simulate.rs` :: `simulates_arrayed_models_correctly` | `mapping`, `multimap`, `subscript`, `vector`, `allocate`, `sumif` models. Phase 6. (`file_io`) |

---

## AC6: No panics

### AC6.1: Compiler handles missing/sparse array element keys without panic

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/compiler/mod.rs` :: `tests` | Uses `TestProject` with sparse element keys, asserts `Err` not panic. Phase 1. |

### AC6.2: MDL conversion of any valid MDL file completes without panic

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/mdl/xmile_compat.rs` :: `tests` | `Expr::Na` formats to `"NAN"` not `":NA:"`. Phase 1. |
| Unit | `src/simlin-engine/src/mdl/convert/variables.rs` :: `tests` | `build_equation_rhs_with_context()` handles Data/Implicit/EmptyRhs types. Phase 1. |
| Unit | `src/simlin-engine/src/mdl/convert/dimensions.rs` :: `tests` | Dimension aliases preserve original casing. Phase 1. |
| Unit | `src/simlin-engine/src/builtins_visitor.rs` :: `tests` | Stdlib expansion provides subscript context for `Ast::Arrayed` equations. Phase 1. |
| Integration | `src/simlin-engine/tests/mdl_equivalence.rs` :: `test_clearn_equivalence` | C-LEARN exercises nearly every MDL feature; zero diffs means no panics. Phase 1/7. (`xmutil`) |

---

## AC7: Serialization

### AC7.1: sd.json format includes all new datamodel fields

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/json.rs` :: `tests` | JSON `ArrayedEquation` serializes `equation` field for EXCEPT case. Phase 2. |
| Unit | `src/simlin-engine/src/json.rs` :: `tests` | JSON `Dimension` serializes `mappings` array with element_map. Phase 2. |
| Unit | `src/simlin-engine/src/json.rs` :: `tests` | JSON `Compat` serializes `data_source` with DataSourceKind, file path, cell reference. Phase 2. |

### AC7.2: sd.json roundtrip preserves all new fields

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Integration | `src/simlin-engine/tests/json_roundtrip.rs` | Roundtrip for EXCEPT equation with default_equation. Phase 2. |
| Integration | `src/simlin-engine/tests/json_roundtrip.rs` | Roundtrip for element-level DimensionMapping. Phase 2. |
| Integration | `src/simlin-engine/tests/json_roundtrip.rs` | Roundtrip for DataSource. Phase 2. |
| Integration | `src/simlin-engine/tests/json_roundtrip.rs` | Backward compatibility: old `maps_to` JSON format still loads. Phase 2. |

### AC7.3: Protobuf and XMILE serialization return explicit errors for unsupported constructs

| Test Type | Test File | Description |
|-----------|-----------|-------------|
| Unit | `src/simlin-engine/src/serde.rs` :: `tests` | Protobuf serialization of Equation::Arrayed with default_equation returns error. Phase 2. |
| Unit | `src/simlin-engine/src/serde.rs` :: `tests` | Protobuf serialization of Dimension with element-level mappings returns error. Phase 2. |
| Unit | `src/simlin-engine/src/xmile/variables.rs` :: `tests` | XMILE serialization of EXCEPT equation returns error. Phase 2. |
| Process | GitHub issues | Phase 2 Task 8 files tracking issues for protobuf/XMILE serialization gaps. |

---

## Summary: Test Commands

```bash
# AC6, AC4.1-4.2, AC7 (unit tests)
cargo test -p simlin-engine

# AC1, AC2, AC3, AC4.3, AC5 (simulation integration)
cargo test --features file_io,ext_data --test simulate

# AC1.1, AC2.1, AC6.2 (MDL equivalence)
cargo test --features file_io,xmutil -p simlin-engine --test mdl_equivalence

# AC4.2, AC7.2 (roundtrip tests)
cargo test -p simlin-engine --test mdl_roundtrip
cargo test -p simlin-engine --test json_roundtrip

# AC3.1 (WASM compilation)
cargo check -p simlin-engine --target wasm32-unknown-unknown
```

## Verification Matrix

| AC | Test Type | Automated | Phase(s) |
|----|-----------|-----------|----------|
| AC1.1 | Integration | Yes | 1-7 |
| AC1.2 | Integration | Yes | 1-7 |
| AC2.1 | Integration | Yes | 7 |
| AC2.2 | Integration | Yes | 7 |
| AC3.1 | Unit + Build | Yes | 4 |
| AC3.2 | Unit + Integration | Yes | 4 |
| AC3.3 | Unit + Integration | Yes | 4 |
| AC3.4 | Unit + Integration | Yes | 4 |
| AC3.5 | Unit | Yes | 4 |
| AC4.1 | Unit + Integration | Yes | 2 |
| AC4.2 | Unit + Integration | Yes | 2 |
| AC4.3 | Unit + Integration | Yes | 3 |
| AC5.1 | Unit + Integration | Yes | 5 |
| AC5.2 | Integration | Yes | 5 |
| AC5.3 | Unit + Integration | Yes | 5 |
| AC5.4 | Unit + Integration | Yes | 6 |
| AC6.1 | Unit | Yes | 1 |
| AC6.2 | Unit + Integration | Yes | 1 |
| AC7.1 | Unit | Yes | 2 |
| AC7.2 | Integration | Yes | 2 |
| AC7.3 | Unit + Process | Yes | 2 |

All 21 acceptance sub-criteria are covered by automated tests. No human-only verification is required.
