# Test Requirements: MDL Roundtrip Fidelity

## Automated Tests

| AC | Criterion | Test Type | Test Location | Phase |
|---|---|---|---|---|
| AC1.1 | mark2.mdl roundtrip produces exactly 2 views with names `*1 housing` and `*2 investments` | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- construct StockFlow with two Group elements, write to MDL, assert two view headers with correct names | 3 |
| AC1.1 | mark2.mdl roundtrip produces exactly 2 views (end-to-end) | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- parse mark2.mdl, write back, split output on `*N name` pattern, assert exactly 2 views with names `1 housing` and `2 investments` | 6 |
| AC1.2 | Each view contains the correct elements (unordered set comparison) | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- construct StockFlow with `[Group("1 housing"), Aux, Stock, Group("2 investments"), Aux, Flow]`, write to MDL, assert elements partition correctly between views | 3 |
| AC1.2 | Each view contains the correct elements (end-to-end) | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- extract element lines from each view section of roundtripped mark2.mdl, compare as unordered sets against original | 6 |
| AC1.3 | Single-view models produce a single view as before | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- construct StockFlow with NO Group elements, write to MDL, assert single view with default name | 3 |
| AC1.4 | Each view has its own font specification line matching the original | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- StockFlow with `font = Some("192-192-192,0,Verdana|10||...")` emits that font; StockFlow with `font = None` emits hardcoded default | 3 |
| AC1.4 | Font spec preserved through parse-write roundtrip | unit | `src/simlin-engine/src/mdl/view/mod.rs` or `convert.rs` (test module) -- parse MDL view with font line `$192-192-192,0,Verdana|10||0-0-0|...`, verify `StockFlow.font` preserves it; parse MDL without font line, verify `StockFlow.font == None` | 2 |
| AC1.4 | Font line matches original in roundtripped mark2.mdl | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- assert each view section contains font line matching `Verdana|10` | 6 |
| AC2.1 | Stock elements preserve original width/height/bits | unit (parse) | `src/simlin-engine/src/mdl/view/convert.rs` (test module) -- parse stock element line with width=53, height=32, bits=131, verify `compat == Some(ViewElementCompat { width: 53.0, height: 32.0, bits: 131 })` | 2 |
| AC2.1 | Stock elements emit preserved dimensions in writer | unit (write) | `src/simlin-engine/src/mdl/writer.rs` (test module) -- stock with `compat = Some(ViewElementCompat { width: 53.0, height: 32.0, bits: 131 })` emits element line containing `53,32,3,131` | 3 |
| AC2.2 | Aux, flow, cloud, and alias elements preserve original dimensions and bits | unit (parse) | `src/simlin-engine/src/mdl/view/convert.rs` (test module) -- parse each element type with non-default dimensions, verify compat fields match | 2 |
| AC2.2 | Aux, flow, cloud, and alias elements emit preserved dimensions in writer | unit (write) | `src/simlin-engine/src/mdl/writer.rs` (test module) -- each element type with compat emits preserved dimensions; flow tests both `compat` (valve) and `label_compat` (attached label) | 3 |
| AC2.3 | Elements without compat data use hardcoded defaults without error | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- each element type with `compat: None` emits hardcoded defaults (stock: `40,20,3,3`; aux: `40,20,8,3`; flow valve: `6,8,34,3`; cloud: `10,8,0,3`; alias: `40,20,8,2`) | 3 |
| AC3.1 | Lookup invocations emit as `table_name ( input )` not `LOOKUP(table_name, input)` | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- construct AST with lookup call, walk through MdlPrintVisitor, assert output is `table name ( Time )` | 4 |
| AC3.1 | Lookup syntax correct in roundtripped mark2.mdl | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- assert output contains native lookup call syntax and does NOT contain `LOOKUP(` | 6 |
| AC3.2 | Explicit lookup range bounds are preserved | unit | `src/simlin-engine/src/mdl/convert/variables.rs` (test module) -- lookup with explicit `[(0,0)-(2,5)]` range has `y_scale = {min: 0.0, max: 5.0}` (from file, not computed from data) | 2 |
| AC3.2 | Lookup bounds preserved in roundtripped mark2.mdl | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- assert lookup definitions preserve explicit bounds | 6 |
| AC3.3 | Lookups without explicit bounds still compute bounds from data | unit | `src/simlin-engine/src/mdl/convert/variables.rs` (test module) -- lookup WITHOUT explicit y_range has y_scale computed from actual data points | 2 |
| AC4.1 | Short equations use inline format with spaces around `=` | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- variable with equation `0.03` and name `average repayment rate` emits `average repayment rate = 0.03` | 5 |
| AC4.1 | Inline format present in roundtripped mark2.mdl | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- assert at least one short equation uses inline format | 6 |
| AC4.2 | Long equations use multiline format with backslash line continuations | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- long equation (>80 chars) contains `\\\r\n\t\t` continuation; short equations do NOT | 5 |
| AC4.3 | Variable name casing on equation LHS matches original | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- variable with canonical ident and view element name emits original casing on LHS | 4 |
| AC4.3 | Variable casing correct in roundtripped mark2.mdl | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- assert at least one variable has original casing | 6 |
| AC4.4 | Ungrouped variables are ordered deterministically (alphabetically by ident) | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- model with ungrouped variables [c, a, b] emits in order [a, b, c] | 5 |
| AC4.5 | Grouped variables retain sector-based ordering | unit | `src/simlin-engine/src/mdl/writer.rs` (test module) -- grouped variables appear in group order, not alphabetical | 5 |
| AC5.1 | `mdl_roundtrip` test is registered in Cargo.toml and runs with `cargo test` | infrastructure | `src/simlin-engine/Cargo.toml` -- verify `[[test]] name = "mdl_roundtrip"` entry exists | 6 |
| AC5.2 | Format test roundtrips mark2.mdl and asserts per-view element lines match as unordered sets | integration | `src/simlin-engine/tests/mdl_roundtrip.rs` -- `mdl_format_roundtrip` test | 6 |
| AC5.3 | Existing roundtrip and simulation tests continue to pass | regression | `cargo test -p simlin-engine` -- all existing tests pass alongside new format tests | 6 |

## Human Verification

| AC | Criterion | Verification Approach | Justification |
|---|---|---|---|
| Design DoD #4 | Vensim compatibility: Roundtripped mark2.mdl is openable by Vensim with diagrams that look the same as the original | 1. Run `simlin convert --to mdl` on `test/bobby/vdf/econ/mark2.mdl` to produce roundtripped output. 2. Open the roundtripped MDL file in Vensim. 3. Visually compare the `1 housing` and `2 investments` views against the original. 4. Verify: (a) both views present and selectable, (b) element positions/sizes/connections look the same, (c) labels readable and positioned correctly, (d) no Vensim warnings on open, (e) simulation runs and produces same results. | Vensim's MDL parser has undocumented tolerances and rendering heuristics that cannot be captured by automated field-by-field comparison. Visual equivalence in the actual target application is the authoritative test. |
