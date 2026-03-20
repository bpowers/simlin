# Test Requirements: Systems Format Support

This document maps every acceptance criterion from the systems format design plan to specific automated tests or documented human verification. Each entry includes the criterion ID, test type, expected file path, implementation phase, and a brief description.

## AC1: Parser handles all valid syntax constructs

**Phase:** 2 (Systems Format Parser)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC1.1 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"Name"` stock-only line; assert stock with initial=Int(0), max=Inf |
| AC1.2 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"Name(10)"` and `"Name(10, 20)"`; assert correct initial and max Expr values |
| AC1.3 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"[Name]"`; assert is_infinite=true and initial=Inf |
| AC1.4 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"A > B @ 5"`; assert FlowType::Rate |
| AC1.5 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"A > B @ 0.5"`; assert FlowType::Conversion (implicit decimal detection) |
| AC1.6 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"A > B @ Rate(5)"`, `"A > B @ Conversion(0.5)"`, `"A > B @ Leak(0.2)"`; assert correct FlowType for each explicit prefix |
| AC1.7 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"A > B @ Recruiters * 3"` and `"A > B @ Developers / (Projects+1)"`; assert correct Expr trees with BinOp and Ref nodes |
| AC1.8 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse input containing `"# comment"` lines intermixed with valid lines; assert comments are ignored and model is correct |
| AC1.9 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"Name"` and `"Name(5)"` without flow syntax; assert stocks are created but flows vec is empty |
| AC1.10 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse `"a > b @ 5\nb(2) > c @ 3"`; assert stock b has initial=Int(2) (later parameterization wins over default) |
| AC1.11 | Unit | `src/simlin-engine/src/systems/parser.rs` `#[cfg(test)] mod tests` | Parse input with conflicting stock params (e.g., `"a(5) > b @ 1\na(10) > c @ 2"`); assert error result |

**Additional parser coverage (Phase 2):**

| Test | Type | Test Location | Description |
|------|------|---------------|-------------|
| parse_hiring | Unit | parser.rs tests | Parse `hiring.txt` via `include_str!`; assert correct stock count, flow count, flow types |
| parse_links | Unit | parser.rs tests | Parse `links.txt`; assert formula references in rate expressions |
| parse_maximums | Unit | parser.rs tests | Parse `maximums.txt`; assert (initial, max) stock params |
| parse_extended_syntax | Unit | parser.rs tests | Parse `extended_syntax.txt`; assert stock-only lines, explicit Rate, formula max |
| parse_projects | Unit | parser.rs tests | Parse `projects.txt`; assert complex formulas with division and parentheses |

---

## AC2: Translation produces correct datamodel structure

**Phase:** 3 (Translator)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC2.1 | Unit | `src/simlin-engine/src/systems/translate.rs` tests | Translate Rate/Conversion/Leak flows; assert correct `model_name` for each |
| AC2.2 | Unit | translate.rs tests | Check module `references` for correct `src`/`dst` bindings |
| AC2.3 | Unit | translate.rs tests | Translate Conversion; assert waste flow in source outflows, not in any inflows |
| AC2.4 | Unit | translate.rs tests | Translate multi-outflow stock; assert chaining via `.remaining` references |
| AC2.5 | Unit | translate.rs tests | Translate flows declared [B, C]; assert reversed chain order |
| AC2.6 | Unit | translate.rs tests | Translate infinite stock; assert equation `"inf()"` |
| AC2.7 | Unit | translate.rs tests | Assert SimSpecs: start=0.0, dt=Dt(1.0), sim_method=Euler |

**Additional translation coverage:**

| Test | Type | Description |
|------|------|-------------|
| translate_*_compiles | Unit | Parse/translate each example file; verify `compile_project_incremental` succeeds |
| translate_conversion_detection | Unit | `@ 1.0` produces `systems_conversion` (decimal = Conversion) |
| translate_dynamic_max | Unit | `EngRecruiter(1, Recruiter)` produces capacity aux referencing stock name |

---

## AC3: Simulation output matches Python systems package

**Phase:** 4 (Simulation Integration Tests)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC3.1 | Integration | `tests/simulate_systems.rs::simulates_hiring` | Simulate `hiring.txt` for 5 rounds; compare against Python CSV |
| AC3.2 | Integration | `tests/simulate_systems.rs::simulates_links` | Simulate `links.txt`; validates formula references |
| AC3.3 | Integration | `tests/simulate_systems.rs::simulates_maximums` | Simulate `maximums.txt`; validates dest capacity limiting |
| AC3.4 | Integration | `tests/simulate_systems.rs::simulates_projects` | Simulate `projects.txt`; validates complex formulas |
| AC3.5 | Integration | `tests/simulate_systems.rs::simulates_extended_syntax` | Simulate `extended_syntax.txt`; validates all flow types |
| AC3.6 | Integration | All simulate_systems tests | Each test runs both interpreter and VM paths; AC3.6 satisfied when AC3.1-AC3.5 pass |

---

## AC4: Round-trip writer reconstructs original format

**Phase:** 5 (Systems Format Writer)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC4.1 | Unit | `src/simlin-engine/src/systems/writer.rs` tests | Write translated project; verify no module idents in output |
| AC4.2 | Unit | writer.rs tests | Write Conversion project; verify no waste flow idents in output |
| AC4.3 | Unit | writer.rs tests | Write each flow type; verify `Rate(...)`, `Conversion(...)`, `Leak(...)` syntax |
| AC4.4 | Unit | writer.rs tests | Write multi-outflow; verify original declaration order (not reversed) |
| AC4.5 | Unit | writer.rs tests | Write infinite stock; verify `[stockname]` syntax |
| AC4.6 | Integration | `tests/systems_roundtrip.rs` (5 tests) | Parse -> translate -> write -> parse -> translate -> simulate; compare against CSV |

---

## AC5: ALLOCATE BY PRIORITY works as native builtin

**Phase:** 6 (ALLOCATE BY PRIORITY Builtin)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC5.1 | Unit | In-source test module | TestProject with `allocate_by_priority`; compile and simulate via VM+interpreter |
| AC5.2 | Unit | In-source test module | Equivalent TestProject with `allocate_available` + rectangular profiles; verify identical results |
| AC5.3 | Integration | `tests/simulate.rs` (existing) | Existing `simulates_allocate*` tests continue to pass |
| AC5.4 | Unit | mdl/writer.rs tests | XMILE with `allocate_by_priority` round-trips to MDL `ALLOCATE BY PRIORITY` |

---

## AC6: Layout engine generates module diagram elements

**Phase:** 7 (Module Diagram Generation)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC6.1 | Unit | `src/simlin-engine/src/layout/mod.rs` tests | Model with Variable::Module; assert ViewElement::Module in output |
| AC6.2 | Unit | layout/mod.rs tests | Assert module has finite, non-zero x/y (SFDP positioned) |
| AC6.3 | Unit | layout/mod.rs tests | Module with stock dependencies; assert connectors exist |
| AC6.4 | Unit | layout/mod.rs tests | Assert module participates in label placement |
| AC6.5 | Integration | `tests/layout.rs::test_systems_format_layout_with_modules` | Translate hiring.txt, generate layout; verify completeness |

---

## AC7: Left-to-right formula evaluation

**Phase:** 2 (parsing) and 3 (translation)

| Criterion | Type | Test Location | Description |
|-----------|------|---------------|-------------|
| AC7.1 | Unit | `src/simlin-engine/src/systems/ast.rs` tests | Expr tree for `a + b * c` -> `to_equation_string()` produces `"(a + b) * c"` |
| AC7.2 | Unit | parser.rs tests | Parse `"A > B @ (a + b) / 2"`; assert Paren node in Expr tree |

---

## Phase 1 Foundation Tests (Stdlib Modules)

Phase 1 provides foundational verification enabling AC2.1 and AC3.*

| Test | Type | Test Location | Description |
|------|------|---------------|-------------|
| systems_rate_basic | Unit | `systems_stdlib_tests.rs` | available=10, requested=7 -> actual=7, remaining=3 |
| systems_rate_limited_by_available | Unit | `systems_stdlib_tests.rs` | available=3, requested=7 -> actual=3, remaining=0 |
| systems_rate_limited_by_dest_capacity | Unit | `systems_stdlib_tests.rs` | available=10, requested=7, dest_capacity=5 -> actual=5 |
| systems_rate_zero_available | Unit | `systems_stdlib_tests.rs` | available=0, requested=7 -> actual=0, remaining=0 |
| systems_leak_basic | Unit | `systems_stdlib_tests.rs` | available=100, rate=0.1 -> actual=10, remaining=90 |
| systems_leak_with_truncation | Unit | `systems_stdlib_tests.rs` | available=15, rate=0.2 -> actual=3, remaining=12 |
| systems_leak_limited_by_dest | Unit | `systems_stdlib_tests.rs` | available=100, rate=0.5, dest_capacity=10 -> actual=10 |
| systems_leak_zero_available | Unit | `systems_stdlib_tests.rs` | available=0, rate=0.5 -> actual=0, remaining=0 |
| systems_conversion_basic | Unit | `systems_stdlib_tests.rs` | available=10, rate=0.5 -> outflow=5, waste=5, remaining=0 |
| systems_conversion_truncation | Unit | `systems_stdlib_tests.rs` | available=7, rate=0.3 -> outflow=2, waste=5, remaining=0 |
| systems_conversion_limited | Unit | `systems_stdlib_tests.rs` | available=10, rate=0.5, dest_capacity=3 -> outflow=3, waste=7 |
| systems_conversion_full | Unit | `systems_stdlib_tests.rs` | available=10, rate=1.0 -> outflow=10, waste=0, remaining=0 |

---

## Test File Index

| File | Type | Phase | Criteria |
|------|------|-------|----------|
| `src/simlin-engine/src/systems_stdlib_tests.rs` | Unit | 1 | Foundation |
| `src/simlin-engine/src/systems/parser.rs` (tests) | Unit | 2 | AC1.*, AC7.2 |
| `src/simlin-engine/src/systems/ast.rs` (tests) | Unit | 3 | AC7.1 |
| `src/simlin-engine/src/systems/translate.rs` (tests) | Unit | 3 | AC2.* |
| `src/simlin-engine/tests/simulate_systems.rs` | Integration | 4 | AC3.* |
| `src/simlin-engine/src/systems/writer.rs` (tests) | Unit | 5 | AC4.1-AC4.5 |
| `src/simlin-engine/tests/systems_roundtrip.rs` | Integration | 5 | AC4.6 |
| `src/simlin-engine/src/` (allocate tests) | Unit | 6 | AC5.1, AC5.2, AC5.4 |
| `src/simlin-engine/tests/simulate.rs` (existing) | Integration | 6 | AC5.3 |
| `src/simlin-engine/src/layout/mod.rs` (tests) | Unit | 7 | AC6.1-AC6.4 |
| `src/simlin-engine/tests/layout.rs` | Integration | 7 | AC6.5 |

---

## Human Verification

| Item | Justification | Approach |
|------|---------------|----------|
| stdlib.gen.rs model names | Generated code correctness needs manual inspection | Inspect after `pnpm rebuild-stdlib`; confirm Unicode model names and variable structure |
| Python fixture generation | CSV fixtures are ground truth from Python package | Run `scripts/gen-systems-fixtures.py`; spot-check against manual Python REPL |
| AC6.5 visual rendering | Automated tests verify structure, not visual quality | Load systems model in app; verify modules render as labeled boxes with connectors |
