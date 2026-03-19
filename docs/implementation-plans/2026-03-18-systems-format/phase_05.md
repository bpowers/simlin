# Systems Format Support - Phase 5: Systems Format Writer

**Goal:** Reconstruct the `.txt` format from the datamodel by reading module structure, stripping synthesized variables, and recovering original declaration order via chain walking.

**Architecture:** A `writer.rs` module inspects all `Variable::Module` instances with systems-format model names, extracts source stock / rate expression / destination stock from module input bindings, reconstructs flow types from `model_name`, walks the `remaining` chain to recover declaration order, and emits explicit flow type syntax (`Rate(...)`, `Conversion(...)`, `Leak(...)`). Synthesized helpers (aux variables for rate/capacity, waste flows, modules) are stripped from output. Integration via `to_systems()` in `compat.rs`.

**Tech Stack:** Rust

**Scope:** 7 phases from original design (phase 5 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- Writer pattern in `compat.rs`: `to_xmile(project) -> Result<String>` delegates to `xmile::project_to_xmile`. `to_systems` follows same pattern.
- MDL writer uses `MdlWriter { buf: String }` struct with `write!` macro accumulation.
- Module identification: `model_name.starts_with("stdlib⁚systems_")` with `⁚` being U+205A.
- Systems module model names: `"stdlib⁚systems_rate"`, `"stdlib⁚systems_conversion"`, `"stdlib⁚systems_leak"`
- Rate modules bind: `available`, `requested`, `dest_capacity`. Leak/Conversion bind: `available`, `rate`, `dest_capacity`.
- Chain walking: module bound to stock directly (src = stock ident) was last-declared/highest-priority; follow `.remaining` references to find the chain. Reverse to get original order.
- Waste flows: appear in some stock's `outflows` but never in any stock's `inflows`.
- Infinite stocks: `Equation::Scalar("inf()")`.
- Synthesized helpers: aux idents that are the `src` of a module reference.
- Design says writer outputs explicit type syntax always: `Rate(...)`, `Conversion(...)`, `Leak(...)`.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### systems-format.AC4: Round-trip writer reconstructs original format
- **systems-format.AC4.1 Success:** Writer strips all module variables from output
- **systems-format.AC4.2 Success:** Writer strips waste flows from output
- **systems-format.AC4.3 Success:** Writer reconstructs flow type (Rate/Conversion/Leak) from module model_name
- **systems-format.AC4.4 Success:** Writer recovers original declaration order from module remaining chain
- **systems-format.AC4.5 Success:** Writer identifies infinite stocks and produces `[Name]` syntax
- **systems-format.AC4.6 Success:** Round-trip (parse -> translate -> write -> parse -> translate -> simulate) produces matching output for all fixtures

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Implement the writer module

**Verifies:** systems-format.AC4.1, AC4.2, AC4.3, AC4.4, AC4.5

**Files:**
- Create: `src/simlin-engine/src/systems/writer.rs`
- Modify: `src/simlin-engine/src/systems/mod.rs` -- add `mod writer;` and `pub use writer::project_to_systems;`
- Modify: `src/simlin-engine/src/compat.rs` -- add `to_systems()` entry point

**Implementation:**

`writer.rs` provides:
```rust
pub fn project_to_systems(project: &Project) -> Result<String>
```

**Writer algorithm:**

1. **Find the main model** (first model in `project.models`, or model named "main").

2. **Collect all systems modules**: iterate `model.variables`, filter `Variable::Module` where `model_name` contains `"systems_rate"`, `"systems_conversion"`, or `"systems_leak"`. Build a map from module ident to `&Module`.

3. **Identify synthesized variables to strip**:
   - Module idents (all systems modules found in step 2)
   - Waste flow idents: collect all flow idents that appear in any stock's `outflows` but in no stock's `inflows`
   - Helper aux idents: for each module, collect all `src` values from its `references` -- these are the synthesized rate/capacity aux variables. Exclude stock idents (which are real variables, not synthesized).
   - Flow idents for actual transfers (e.g., `"{source}_to_{dest}"`) -- these are synthesized by the translator

4. **Group modules by source stock**: for each module, find the `ModuleReference` where `dst == "available"`. If `src` is a plain stock name (no `.` separator), this module is the head of the chain for that stock. If `src` contains `.remaining`, find the referenced module and note it as a predecessor.

5. **Walk chains to recover declaration order**: For each source stock:
   a. Find the "head" module (the one whose `available` src is the stock name directly -- this was the last-declared/highest-priority flow).
   b. Walk forward through the chain: find modules whose `available.src` references `{head}.remaining`, then modules referencing that module's `.remaining`, etc.
   c. Reverse the chain order to get original declaration order.

6. **For each flow (in recovered declaration order), extract**:
   - Source stock: from the chain grouping
   - Destination stock: find the flow variable whose equation references `{module}.actual` or `{module}.outflow`; find which stock has this flow in its `inflows`
   - Flow type: from `model_name` -- `systems_rate` -> `Rate`, `systems_conversion` -> `Conversion`, `systems_leak` -> `Leak`
   - Rate expression: from the module reference where `dst` is `"requested"` (Rate) or `"rate"` (Leak/Conversion), look up the aux variable with that `src` ident and extract its equation text

7. **Identify stock properties**:
   - Infinite: `equation == "inf()"`
   - Initial value: equation text (default "0" can be omitted)
   - Maximum: determined from the `dest_capacity` module reference -- if the `src` aux has equation `"{max_expr} - {stock}"`, extract the max expression. If `src` is `"inf()"`, no maximum.

8. **Emit output**: Write each stock declaration and flow line. Use explicit type syntax:
   - `[StockName]` for infinite stocks
   - `StockName` or `StockName(initial)` or `StockName(initial, max)` for regular stocks
   - `Source > Dest @ Rate(expr)` / `Source > Dest @ Conversion(expr)` / `Source > Dest @ Leak(expr)`

   Stock names in output should use the original casing. Since canonical idents are lowercase with underscores, and the original casing is not preserved in the datamodel, the writer outputs canonicalized names. This is acceptable per the design ("explicit flow type syntax to avoid ambiguity").

**compat.rs addition:**
```rust
pub fn to_systems(project: &Project) -> Result<String> {
    systems::project_to_systems(project)
}
```

**Testing:**

Unit tests in `writer.rs` with `#[cfg(test)] mod tests`:
- AC4.1: Build a translated project, write it, verify no module variable names appear in output
- AC4.2: Build a project with conversion waste flows, verify waste flow names don't appear in output
- AC4.3: Test each flow type reconstruction from module model_name
- AC4.4: Build a multi-outflow project, verify output order matches original declaration order (not reversed priority order)
- AC4.5: Build a project with infinite stock, verify `[stockname]` syntax in output

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems::writer
```

**Commit:** `engine: implement systems format writer with module-based reconstruction`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Round-trip integration tests

**Verifies:** systems-format.AC4.6

**Files:**
- Create: `src/simlin-engine/tests/systems_roundtrip.rs`
- Modify: `src/simlin-engine/Cargo.toml` -- add `[[test]]` section

**Implementation:**

Add to `Cargo.toml`:
```toml
[[test]]
name = "systems_roundtrip"
required-features = ["file_io", "testing"]
```

`systems_roundtrip.rs` tests the full round-trip: parse -> translate -> write -> parse -> translate -> simulate, comparing simulation output against the same expected CSV used in Phase 4.

**Important:** Use the shared `ensure_results` helper extracted in Phase 4 (or duplicate it). The function takes exactly **2 arguments** (`ensure_results(expected: &Results, results: &Results)`). See Phase 4 Task 2 for the extraction approach.

```rust
fn roundtrip_systems_file(txt_path: &str, csv_path: &str, rounds: u64) {
    let contents = std::fs::read_to_string(txt_path).unwrap();

    // First pass: parse -> translate -> write
    let model1 = simlin_engine::systems::parse(&contents).unwrap();
    let project1 = simlin_engine::systems::translate(&model1, rounds).unwrap();
    let written = simlin_engine::compat::to_systems(&project1).unwrap();

    // Second pass: parse written output -> translate -> simulate
    let model2 = simlin_engine::systems::parse(&written).unwrap();
    let project2 = simlin_engine::systems::translate(&model2, rounds).unwrap();

    // Simulate the round-tripped model
    let expected = simlin_engine::compat::load_csv(csv_path, b',').unwrap();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    ensure_results(&expected, &results);
}
```

Test each example file:
```rust
#[test]
fn roundtrip_hiring() {
    roundtrip_systems_file(
        "test/systems-format/hiring.txt",
        "test/systems-format/hiring_output.csv",
        5,
    );
}
// ... similar for links, maximums, projects, extended_syntax
```

**Testing notes:**
- The round-trip may not produce byte-for-byte identical `.txt` output (e.g., different whitespace, canonical vs original casing), but the simulation output must match.
- If round-trip tests fail, the likely cause is loss of information during write (e.g., wrong flow type, wrong order, wrong rate expression).

**Verification:**

Run:
```bash
cargo test --features "file_io,testing" --test systems_roundtrip
```

Expected: All 5 tests pass.

**Commit:** `test: add systems format round-trip integration tests`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Verify end-to-end with compat.rs API

**Files:**
- Modify: `src/simlin-engine/tests/systems_roundtrip.rs` -- add compat API test

**Implementation:**

Add a test that uses the high-level `open_systems` and `to_systems` APIs:

```rust
#[test]
fn compat_open_and_write_systems() {
    let contents = std::fs::read_to_string("test/systems-format/hiring.txt").unwrap();
    let project = simlin_engine::compat::open_systems(&contents).unwrap();
    let written = simlin_engine::compat::to_systems(&project).unwrap();
    // Verify written output can be re-parsed
    let project2 = simlin_engine::compat::open_systems(&written).unwrap();
    // Verify project2 compiles
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    assert!(compile_project_incremental(&db, sync.project, "main").is_ok());
}
```

**Verification:**

Run:
```bash
cargo test --features "file_io,testing" --test systems_roundtrip compat_
```

**Commit:** `test: add systems compat API round-trip test`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
