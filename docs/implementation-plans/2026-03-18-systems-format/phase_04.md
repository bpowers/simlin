# Systems Format Support - Phase 4: Simulation Integration Tests

**Goal:** Verify simulation output matches the Python `systems` package by generating reference CSV outputs and comparing against simlin's VM and interpreter results.

**Architecture:** A Python script generates expected CSV outputs from the `systems` package for each example file. These CSVs become test fixtures in `test/systems-format/`. An integration test file `tests/simulate_systems.rs` follows the existing `simulate.rs` pattern: parse -> translate -> compile -> simulate (both VM and interpreter) -> compare against CSV. The `ensure_results` function iterates expected keys only, so extra simlin variables (modules, flows, etc.) don't cause failures.

**Tech Stack:** Rust (integration tests), Python (fixture generation)

**Scope:** 7 phases from original design (phase 4 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- Integration tests use `[[test]]` sections in `Cargo.toml` with `required-features = ["file_io", "testing"]`
- `simulate_path_with()` in `tests/simulate.rs` (lines 325-404) is the canonical pattern: loads model, runs both interpreter and VM paths, compares against expected CSV/tab output
- `load_csv(file_path, delimiter)` in `compat.rs` forces first column to `"time"`, canonicalizes headers via `Ident::new()`, parses all values as f64
- `ensure_results(expected, results)` iterates expected keys only, uses `2e-3` absolute epsilon for non-Vensim data
- Python `systems` package: `Model.render(results, sep=',', pad=False)` outputs CSV with blank first column header (round number), only stocks with `show=True` (infinite stocks excluded)
- Default round count in Python: `--rounds 10`. AC3.1 specifies 5 rounds for hiring.txt.
- Python CSV output values are integers (from `math.floor` operations), loaded as f64 by `load_csv`

---

## Acceptance Criteria Coverage

This phase implements and tests:

### systems-format.AC3: Simulation output matches Python systems package
- **systems-format.AC3.1 Success:** `hiring.txt` simulation matches Python output for 5 rounds
- **systems-format.AC3.2 Success:** `links.txt` simulation matches (tests formula references in flow rates)
- **systems-format.AC3.3 Success:** `maximums.txt` simulation matches (tests destination capacity limiting)
- **systems-format.AC3.4 Success:** `projects.txt` simulation matches (tests complex formulas with division and parentheses)
- **systems-format.AC3.5 Success:** `extended_syntax.txt` simulation matches (tests Rate, Leak, Conversion, formula references, stock maximums)
- **systems-format.AC3.6 Success:** Both VM and interpreter paths produce identical results

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Generate expected CSV fixtures from Python systems package

**Files:**
- Create: `scripts/gen-systems-fixtures.py` -- Python script to generate CSVs
- Create: `test/systems-format/hiring.txt` -- copy from `third_party/systems/examples/`
- Create: `test/systems-format/hiring_output.csv`
- Create: `test/systems-format/links.txt`
- Create: `test/systems-format/links_output.csv`
- Create: `test/systems-format/maximums.txt`
- Create: `test/systems-format/maximums_output.csv`
- Create: `test/systems-format/projects.txt`
- Create: `test/systems-format/projects_output.csv`
- Create: `test/systems-format/extended_syntax.txt`
- Create: `test/systems-format/extended_syntax_output.csv`

**Implementation:**

Create `scripts/gen-systems-fixtures.py`:

```python
#!/usr/bin/env python3
"""Generate expected CSV outputs for systems format test fixtures.

Usage: python3 scripts/gen-systems-fixtures.py

Reads .txt files from third_party/systems/examples/ and generates
CSV outputs in test/systems-format/ for integration tests.
"""
import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'third_party', 'systems'))

from systems import parse

FIXTURES = {
    'hiring': 5,
    'links': 5,
    'maximums': 5,
    'projects': 5,
    'extended_syntax': 5,
}

def generate_csv(model_name, rounds):
    src = os.path.join('third_party', 'systems', 'examples', f'{model_name}.txt')
    with open(src) as f:
        txt = f.read()
    model = parse.parse(txt)
    results = model.run(rounds=rounds)
    csv_output = model.render(results, sep=',', pad=False)
    return csv_output

def main():
    out_dir = os.path.join('test', 'systems-format')
    os.makedirs(out_dir, exist_ok=True)
    for name, rounds in FIXTURES.items():
        # Copy source .txt
        src = os.path.join('third_party', 'systems', 'examples', f'{name}.txt')
        dst_txt = os.path.join(out_dir, f'{name}.txt')
        with open(src) as f:
            txt = f.read()
        with open(dst_txt, 'w') as f:
            f.write(txt)
        # Generate CSV
        csv = generate_csv(name, rounds)
        dst_csv = os.path.join(out_dir, f'{name}_output.csv')
        with open(dst_csv, 'w') as f:
            f.write(csv)
        print(f'Generated {dst_csv} ({rounds} rounds)')

if __name__ == '__main__':
    main()
```

Run the script to generate all fixtures:
```bash
python3 scripts/gen-systems-fixtures.py
```

**CSV format note:** The Python output uses an empty first column header (round number). `load_csv` forces the first column to `"time"` regardless. Stock names in headers will be canonicalized by `Ident::new()` to match the translator's canonical identifiers.

**Verification:**
```bash
ls test/systems-format/
# Should show: hiring.txt, hiring_output.csv, links.txt, links_output.csv, etc.
head test/systems-format/hiring_output.csv
# Should show CSV with round numbers and stock values
```

**Commit:** `test: add systems format fixtures generated from Python systems package`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Write simulation integration tests

**Verifies:** systems-format.AC3.1, AC3.2, AC3.3, AC3.4, AC3.5, AC3.6

**Files:**
- Create: `src/simlin-engine/tests/simulate_systems.rs`
- Modify: `src/simlin-engine/Cargo.toml` -- add `[[test]]` section

**Implementation:**

Add to `Cargo.toml`:
```toml
[[test]]
name = "simulate_systems"
required-features = ["file_io", "testing"]
```

`simulate_systems.rs` follows the pattern from `simulate.rs`:

```rust
use std::rc::Rc;

use simlin_engine::compat::{load_csv, open_systems};
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::interpreter::Simulation;
use simlin_engine::project::Project;
use simlin_engine::vm::Vm;
```

**Important: `ensure_results` accessibility.** The `ensure_results` function in `tests/simulate.rs` is private and cannot be called from separate integration test files. Before writing test logic, either:
- (a) Extract `ensure_results` (and its helpers: `IGNORABLE_COLS`, `is_implicit_module_var`) from `simulate.rs` into a shared `tests/test_helpers.rs` module that both `simulate.rs` and `simulate_systems.rs` can `mod test_helpers;` include, or
- (b) Define a local `ensure_results` function in `simulate_systems.rs` that duplicates the core comparison logic (iterate expected variable keys, compare with `2e-3` absolute epsilon).

Option (a) is preferred to avoid code duplication. The function takes exactly **2 arguments** (`ensure_results(expected: &Results, results: &Results)`) -- do NOT pass a label/path as a third argument.

Main test helper:
```rust
fn simulate_systems_file(txt_path: &str, csv_path: &str, rounds: u64) {
    // Parse and translate
    let contents = std::fs::read_to_string(txt_path)
        .unwrap_or_else(|e| panic!("Failed to read {txt_path}: {e}"));
    let systems_model = simlin_engine::systems::parse(&contents)
        .unwrap_or_else(|e| panic!("Failed to parse {txt_path}: {e}"));
    let datamodel_project = simlin_engine::systems::translate(&systems_model, rounds)
        .unwrap_or_else(|e| panic!("Failed to translate {txt_path}: {e}"));

    // Load expected results
    let expected = load_csv(csv_path, b',')
        .unwrap_or_else(|e| panic!("Failed to load {csv_path}: {e}"));

    // Run interpreter path
    let project = Rc::new(Project::from(datamodel_project.clone()));
    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("Interpreter creation failed for {txt_path}: {e}"));
    let results1 = sim.run_to_end();
    ensure_results(&expected, &results1);

    // Run VM path
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("VM compilation failed for {txt_path}: {e:?}"));
    let mut vm = Vm::new(compiled)
        .unwrap_or_else(|e| panic!("VM creation failed for {txt_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM execution failed for {txt_path}: {e}"));
    let results2 = vm.into_results();
    ensure_results(&expected, &results2);
}
```

The comparison uses the standard absolute epsilon (`2e-3`) for non-Vensim data. Since the systems format produces integer outputs (from floor operations), differences should be 0.0 (exact match).

Individual test functions:
```rust
#[test]
fn simulates_hiring() {
    simulate_systems_file(
        "test/systems-format/hiring.txt",
        "test/systems-format/hiring_output.csv",
        5,
    );
}

#[test]
fn simulates_links() {
    simulate_systems_file(
        "test/systems-format/links.txt",
        "test/systems-format/links_output.csv",
        5,
    );
}

// ... similar for maximums, projects, extended_syntax
```

**Testing notes:**
- AC3.6: Both VM and interpreter paths are exercised in `simulate_systems_file` and compared against the same expected output.
- The `ensure_results` comparison uses the standard absolute epsilon (2e-3) for non-Vensim data. Since systems format values are integers, differences should be 0.0 (exact match).
- If any test fails, it indicates a mismatch between simlin's translation/simulation and the Python reference. Debug by printing the full results and comparing column by column.

**Verification:**

Run:
```bash
cargo test --features "file_io,testing" --test simulate_systems
```

Expected: All 5 tests pass.

**Commit:** `test: add systems format simulation integration tests`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
