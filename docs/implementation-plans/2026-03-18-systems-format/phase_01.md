# Systems Format Support - Phase 1: Stdlib Modules for Flow Types

**Goal:** Define systems_rate, systems_leak, and systems_conversion as stdlib modules that encapsulate the three flow type semantics from the systems format.

**Architecture:** Three new `.stmx` files in `stdlib/` (repo root), each defining a pure-auxiliary module with input variables (available, rate/requested, dest_capacity) and output variables (actual/outflow, remaining, waste). Modules are embedded into the engine via `pnpm rebuild-stdlib` which regenerates `src/simlin-engine/src/stdlib.gen.rs`. Tests use `TestProject` with manual `Variable::Module` construction to verify correct behavior.

**Tech Stack:** XMILE (.stmx), Rust (tests)

**Scope:** 7 phases from original design (phase 1 of 7)

**Codebase verified:** 2026-03-18

**Key codebase findings:**
- Stdlib .stmx files are at `stdlib/` (repo root), NOT `src/simlin-engine/stdlib/` as stated in the design plan
- Regeneration: `pnpm rebuild-stdlib` runs `scripts/gen-stdlib.sh`
- Module model names use Unicode colon: `stdlib⁚<name>` (U+205A)
- Module output references use Unicode interpunct: `module_name·output_name` (U+00B7)
- All existing stdlib modules use `Compat::default()` for all variables
- `INT` is an available engine builtin (confirmed in `builtins.rs`)
- The `TestProject` builder in `src/simlin-engine/src/test_common.rs` does not have a `module()` method; modules must be added as raw `Variable::Module` via `build_datamodel()` customization or using `testutils::x_module()`
- The `x_module()` helper in `testutils.rs` sets `model_name` equal to `ident` by default; for stdlib modules, `model_name` must be `stdlib⁚<name>` (different from `ident`)

---

## Acceptance Criteria Coverage

This phase is foundational infrastructure. It does not directly verify design-level acceptance criteria, but enables:

- **systems-format.AC2.1:** Each systems flow produces a stdlib module instance with correct model_name
- **systems-format.AC3.1-AC3.6:** Simulation output matches Python systems package (depends on modules computing correct values)

Phase 1 tests verify each module's numeric behavior in isolation with representative inputs matching the Python `systems` package semantics.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Create systems_rate.stmx, systems_leak.stmx, systems_conversion.stmx

**Files:**
- Create: `stdlib/systems_rate.stmx`
- Create: `stdlib/systems_leak.stmx`
- Create: `stdlib/systems_conversion.stmx`

**Implementation:**

All three modules are pure-auxiliary XMILE models (no stocks or flows needed -- they compute values within a single time step). Follow the pattern in `stdlib/smth1.stmx` for XML structure: standard XMILE header, `<model name="stdlib⁚systems_rate">`, `<variables>` section with `<aux>` elements, and a minimal `<views>` section with aux view elements.

**systems_rate.stmx** -- Rate flow: moves `min(requested, min(available, dest_capacity))` units.

Input variables (with default equations):
- `available` (default: `0`) -- remaining source stock value
- `requested` (default: `0`) -- rate expression value
- `dest_capacity` (default: `INF`) -- max_dest - current_dest, or INF if no max

Output variables:
- `actual` with equation: `MIN(requested, MIN(available, dest_capacity))`
- `remaining` with equation: `available - actual`

**systems_leak.stmx** -- Leak flow: moves `min(floor(available * rate), dest_capacity)` units non-destructively.

Input variables:
- `available` (default: `0`)
- `rate` (default: `0`)
- `dest_capacity` (default: `INF`)

Output variables:
- `actual` with equation: `MIN(INT(available * rate), dest_capacity)`
- `remaining` with equation: `available - actual`

**systems_conversion.stmx** -- Conversion flow: drains entire source, adds `floor(src * rate)` to dest, remainder vanishes.

Input variables:
- `available` (default: `0`)
- `rate` (default: `0`)
- `dest_capacity` (default: `INF`)

Output variables:
- `outflow` with equation: `MIN(INT(available * rate), dest_capacity)`
- `waste` with equation: `available - outflow`
- `remaining` with equation: `0`

The `remaining = 0` for conversion is because conversion always drains the entire source stock (the waste flow handles the unconverted portion).

View sections should include `<aux>` view elements with simple grid-like coordinates for each variable. Use coordinates starting at (100, 100) with 100px spacing. No connectors needed in the view since these are simple aux-to-aux references.

**Verification:**
The files should be valid XML. Verification happens in Task 2 (regeneration) and Task 3 (tests).

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Regenerate stdlib.gen.rs and verify compilation

**Files:**
- Modify: `src/simlin-engine/src/stdlib.gen.rs` (generated, not hand-edited)

**Step 1: Regenerate**

Run:
```bash
pnpm rebuild-stdlib
```

This runs `scripts/gen-stdlib.sh` which:
1. Builds `simlin-cli` (`cargo build -p simlin-cli --release`)
2. Runs `target/release/simlin gen-stdlib --stdlib-dir stdlib --output src/simlin-engine/src/stdlib.gen.rs`
3. Runs `cargo fmt -p simlin-engine`

Expected: Command succeeds. The generated `stdlib.gen.rs` now contains functions `systems_rate()`, `systems_leak()`, `systems_conversion()` returning `Model` structs. The `MODEL_NAMES` array grows from 6 to 9 entries (6 existing: delay1, delay3, npv, smth1, smth3, trend -- note `npv` is hand-maintained in `gen_stdlib.rs` and has no `.stmx` file, but is still counted in `MODEL_NAMES`). The `get()` function has new match arms.

**Step 2: Verify compilation**

Run:
```bash
cargo build -p simlin-engine
```

Expected: Builds without errors.

**Step 3: Verify module names in generated code**

Manually inspect `src/simlin-engine/src/stdlib.gen.rs` to confirm:
- Model names are `stdlib⁚systems_rate`, `stdlib⁚systems_leak`, `stdlib⁚systems_conversion`
- Each model has the correct input and output aux variables
- Equations match what was defined in the .stmx files

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Write unit tests for all three stdlib modules

**Verifies:** Module numeric behavior matches Python `systems` package semantics for Rate, Leak, and Conversion flow types.

**Files:**
- Create: `src/simlin-engine/src/systems_stdlib_tests.rs`
- Modify: `src/simlin-engine/src/lib.rs` -- add `#[cfg(test)] #[path = "systems_stdlib_tests.rs"] mod systems_stdlib_tests;`

**Testing approach:**

Tests create a `datamodel::Project` with a main model containing:
1. Aux variables for the inputs (e.g., `available_val` with a constant equation)
2. A `Variable::Module` referencing the stdlib module (e.g., `model_name: "stdlib⁚systems_rate"`)
3. `ModuleReference` bindings connecting the input auxes to module inputs
4. Aux variables that read module outputs (e.g., equation `"my_module·actual"`)

Then compile via `compile_project_incremental` and run via `Vm` to verify output values.

Use the helper pattern from `testutils.rs`:
- `x_aux(ident, eqn, units)` for creating aux variables
- For `Variable::Module`, construct directly (the `x_module` helper sets `model_name = ident`, but we need `model_name = "stdlib⁚systems_rate"` while `ident` is something like `"rate_module"`)

**Module construction pattern** (since `x_module` doesn't support different `model_name`):

```rust
use crate::datamodel::{Module, ModuleReference, Variable, Compat};

fn systems_module(ident: &str, model_name: &str, refs: &[(&str, &str)]) -> Variable {
    Variable::Module(Module {
        ident: ident.to_string(),
        model_name: model_name.to_string(),
        documentation: "".to_string(),
        units: None,
        references: refs
            .iter()
            .map(|(src, dst)| ModuleReference {
                src: src.to_string(),
                dst: dst.to_string(),
            })
            .collect(),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}
```

**Test cases for systems_rate:**

1. **Basic rate transfer:** available=10, requested=7, dest_capacity=INF -> actual=7, remaining=3
2. **Rate limited by available:** available=3, requested=7, dest_capacity=INF -> actual=3, remaining=0
3. **Rate limited by dest_capacity:** available=10, requested=7, dest_capacity=5 -> actual=5, remaining=5
4. **Zero available:** available=0, requested=7, dest_capacity=INF -> actual=0, remaining=0

**Test cases for systems_leak:**

1. **Basic leak:** available=100, rate=0.1, dest_capacity=INF -> actual=INT(10.0)=10, remaining=90
2. **Leak with truncation:** available=15, rate=0.2, dest_capacity=INF -> actual=INT(3.0)=3, remaining=12
3. **Leak limited by dest_capacity:** available=100, rate=0.5, dest_capacity=10 -> actual=10, remaining=90
4. **Zero available:** available=0, rate=0.5, dest_capacity=INF -> actual=0, remaining=0

**Test cases for systems_conversion:**

1. **Basic conversion:** available=10, rate=0.5, dest_capacity=INF -> outflow=INT(5.0)=5, waste=5, remaining=0
2. **Conversion with truncation:** available=7, rate=0.3, dest_capacity=INF -> outflow=INT(2.1)=2, waste=5, remaining=0
3. **Conversion limited by dest_capacity:** available=10, rate=0.5, dest_capacity=3 -> outflow=3, waste=7, remaining=0
4. **Full conversion (rate=1.0):** available=10, rate=1.0, dest_capacity=INF -> outflow=10, waste=0, remaining=0

For each test, build the project, compile incrementally, run VM, and assert output values within 1e-6 tolerance (per `TestProject` convention).

**Verification:**

Run:
```bash
cargo test -p simlin-engine systems_stdlib_tests
```

Expected: All tests pass.

**Commit:**

```bash
git add stdlib/systems_rate.stmx stdlib/systems_leak.stmx stdlib/systems_conversion.stmx \
    src/simlin-engine/src/stdlib.gen.rs src/simlin-engine/src/systems_stdlib_tests.rs \
    src/simlin-engine/src/lib.rs
git commit -m "engine: add systems format stdlib modules (rate, leak, conversion)"
```

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
