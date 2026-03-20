# Systems Format Support - Human Test Plan

## Prerequisites
- Environment set up via `./scripts/dev-init.sh`
- All automated tests passing: `cargo test -p simlin-engine` (run from `src/simlin-engine`)
- Python `systems` package installed for fixture regeneration (only if re-validating ground truth)

## Phase 1: stdlib.gen.rs Manual Inspection

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-engine/src/stdlib.gen.rs` | File exists and is non-empty |
| 2 | Search for `systems_rate` model definition | Model named `stdlib\u{205A}systems_rate` with variables: `available`, `requested`, `dest_capacity`, `actual`, `remaining` |
| 3 | Search for `systems_leak` model definition | Model named `stdlib\u{205A}systems_leak` with variables: `available`, `rate`, `dest_capacity`, `actual`, `remaining` |
| 4 | Search for `systems_conversion` model definition | Model named `stdlib\u{205A}systems_conversion` with variables: `available`, `rate`, `dest_capacity`, `outflow`, `waste`, `remaining` |
| 5 | Verify Unicode model name prefix `\u{205A}` (TWO DOT PUNCTUATION) is used consistently | All three systems stdlib models use the same `stdlib\u{205A}` namespace prefix |
| 6 | Check that variable equations use `int()` for truncation in leak and conversion models | The `actual`/`outflow` equations in leak and conversion should apply integer truncation via `int()` |

## Phase 2: Python Fixture Verification

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open a Python REPL with the `systems` package available | Package imports successfully |
| 2 | Run: `import systems; m = systems.Model("hiring.txt"); m.simulate(5)` | Simulation completes, returns state dictionary |
| 3 | Compare stock values at round 5 against `test/systems-format/hiring_output.csv` last row | Values match to within floating-point precision |
| 4 | Repeat for `links.txt`, `maximums.txt`, `projects.txt`, `extended_syntax.txt` | All five fixture CSVs match Python output |
| 5 | Run `scripts/gen-systems-fixtures.py` | Script regenerates all five CSV fixtures; `git diff` shows no changes |

## Phase 3: End-to-End Round-Trip Integrity

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --test systems_roundtrip -- --nocapture` | All 5 round-trip tests pass |
| 2 | Inspect the written output printed by each test | Human-readable systems format: bracket syntax for infinite stocks, `Rate()/Conversion()/Leak()` prefixes, no internal identifiers (`_outflows`, `_remaining`, `_waste`, `_effective`) |
| 3 | Copy written hiring.txt output, parse via `simlin_engine::systems::parse()` | Parse succeeds, producing a `SystemsModel` with 8 stocks and 7 flows |

## Phase 4: Layout Visual Rendering (AC6.5)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Start the Simlin app (`pnpm dev` from `src/app`) | App starts and is accessible |
| 2 | Import `hiring.txt` from `test/systems-format/hiring.txt` | Model loads without errors |
| 3 | Observe the diagram canvas | All 8 stocks visible as rectangles, flow arrows connecting source/dest, module elements with connecting lines |
| 4 | Check module element labels | Labels should be human-friendly, not raw internal identifiers |
| 5 | Verify infinite stocks are visually distinguishable | Different styling or annotation for infinite stocks |
| 6 | Pan and zoom | All elements correctly positioned; no overlapping labels or misaligned connectors |

## Phase 5: ALLOCATE BY PRIORITY Integration

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --test simulate -- simulates_allocate` | Both `simulates_allocate_mdl` and `simulates_allocate_xmile` pass |
| 2 | Run `cargo test -p simlin-engine --test compiler_vector -- allocate_by_priority` | All 3 allocate_by_priority tests pass |
| 3 | Open an allocate model in the Simlin app | Model loads and simulates without errors |

## End-to-End: Full Pipeline

1. Create a new `.txt` file: `[Pool] > Workers(0, 10) @ 3\nWorkers > Trained @ Conversion(0.8)\nTrained > [Done] @ Leak(0.2)`
2. Parse, translate, compile, simulate for 5 rounds
3. Verify: Pool is infinite, Workers has initial=0/max=10, 3 flows (Rate, Conversion, Leak), waste flows exist, simulation completes
4. Write back to systems format, re-parse, re-simulate -- identical results

## Manual Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| stdlib.gen.rs model names | Generated code correctness requires inspecting Unicode model names | Phase 1 |
| Python fixture generation | CSV fixtures are ground truth; automated tests compare against them but cannot verify generation | Phase 2 |
| AC6.5 visual rendering | Automated tests verify structure but cannot assess visual quality | Phase 4 |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1-AC1.11 | parser.rs unit tests (15+ functions) | -- |
| AC2.1-AC2.7 | translate.rs unit tests (10+ functions) | -- |
| AC3.1-AC3.6 | simulate_systems.rs (5 models, VM+interpreter) | Phase 2 |
| AC4.1-AC4.5 | writer.rs unit tests (7 functions) | Phase 3 |
| AC4.6 | systems_roundtrip.rs (5 tests) | Phase 3 |
| AC5.1-AC5.2 | compiler_vector.rs (3 functions) | Phase 5 |
| AC5.3 | simulate.rs (2 allocate tests) | Phase 5 |
| AC5.4 | mdl/writer.rs (native test) | -- |
| AC6.1-AC6.4 | layout/mod.rs unit tests | Phase 4 |
| AC6.5 | layout.rs integration test | Phase 4 |
| AC7.1-AC7.2 | ast.rs + parser.rs tests | -- |
