# C-LEARN Residual — Phase 1: Lookup-only variable saved-value semantics (#590)

**Goal:** A standalone graphical-function ("lookup-only") variable — one whose entire equation is an inline table with no functional argument — produces the same saved series as genuine Vensim, uniformly across scalar, arrayed (`Equation::Arrayed`), and apply-to-all (A2A) shapes, resolving the inconsistent `gf(0)`-vs-literal-`0` lowering.

**Architecture:** Today the compiler lowers a lookup-only variable inconsistently: the scalar path wraps it as `LOOKUP(self, 0)` → a constant `gf(0)`, while the arrayed and A2A hoisting paths emit a literal `0` and never consult the attached tables. The fix is confined to `compiler::Var::new` (`src/simlin-engine/src/compiler/mod.rs`): detect lookup-only variables and lower all three shapes uniformly to the empirically-determined Vensim rule (leading hypothesis: `gf(Time)`), reusing the per-element table layout (`variable.rs::build_tables`/`reorder_arrayed_element_tables`) and the existing `Lookup` opcode. No new datamodel field, no protobuf change, no new VM/codegen primitive. Because the production salsa-incremental compile path (`db_var_fragment.rs::lower_var_fragment`) reuses `compiler::Var::new`, the fix applies to both the legacy and incremental paths automatically.

**Tech Stack:** Rust (`simlin-engine` crate). Fixtures build `datamodel::Project` directly (the `per_element_gf_tests.rs` pattern), because `TestProject` cannot attach graphical functions.

**Scope:** Phase 1 of 5 from `docs/design-plans/2026-05-19-clearn-residual.md`.

**Codebase verified:** 2026-05-20 (branch `clearn-residual`, off `main`@`2ed93950`).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### clearn-residual.AC1: Lookup-only (graphical-function) variables produce Vensim-matching saved series
- **clearn-residual.AC1.1 Success:** A scalar lookup-only variable (equation is an inline graphical function, no functional argument) produces the saved series determined to match genuine Vensim (per the Phase 1 determination), not a constant `gf(0)`.
- **clearn-residual.AC1.2 Success:** An arrayed (`Equation::Arrayed`) lookup-only variable produces the correct per-element saved series for every element, not literal `0`.
- **clearn-residual.AC1.3 Success:** An apply-to-all lookup-only variable produces the correct saved series for every element.
- **clearn-residual.AC1.4 Success (no regression):** A graphical-function variable that is applied with an argument elsewhere (`var(idx)` → `LOOKUP(var, idx)`) still produces correct applied values.
- **clearn-residual.AC1.5 Edge:** A lookup-only variable whose declared element order is non-alphabetical maps each element to its own table (no positional mis-map; consistent with the `per_element_gf_tests.rs` invariant).
- **clearn-residual.AC1.6 Failure/robustness:** Out-of-range lookup-index semantics for the scalar `Lookup` opcode are unchanged by the fix (the fix changes which expression is fed to the table, not the table's clamp/NaN behavior).

---

## Verified ground truth (read before starting)

Confirmed by investigation on 2026-05-20. Trust these over the design's line numbers.

- **The compiler never sees the `0+0` sentinel string as a signal.** `LOOKUP_SENTINEL = "0+0"` (`src/simlin-engine/src/mdl/mod.rs:41`) is MDL-format-internal — written by the importer and read only by the MDL writer's round-trip check `is_lookup_only_equation` (`src/simlin-engine/src/mdl/writer.rs:831-834`). It is NOT a stable datamodel contract: an XMILE-sourced lookup-only variable carries an empty/absent equation + a `gf`, not `"0+0"`. **Detection must treat "equation is empty-trimmed OR equals `LOOKUP_SENTINEL`" together with "has a graphical function (`tables` non-empty)".**
- **Datamodel distinction** (set at MDL import, `src/simlin-engine/src/mdl/convert/variables.rs`):
  - lookup-only (`MdlEquation::Lookup`, :666-670): `equation = LOOKUP_SENTINEL`, `gf = Some(..)`.
  - WITH LOOKUP (`MdlEquation::WithLookup`, :671-675): `equation = <real input expr>`, `gf = Some(..)`.
  - empty/no-data (`MdlEquation::EmptyRhs`, :676): `equation = LOOKUP_SENTINEL`, `gf = None`.
  So lookup-only ⇔ `gf.is_some()` (i.e. compiler `tables` non-empty) AND equation is empty-or-sentinel; WITH LOOKUP has `tables` non-empty but a real input equation and must keep `gf(input)`.
- **`is_table_only` is a dead end.** The compiler-side `Variable::Var.is_table_only` (`src/simlin-engine/src/variable.rs:104`) is hard-coded `false` everywhere and is NOT present in the datamodel, the protobuf (`src/simlin-engine/src/project_io.proto` `message Aux`/`message Flow`), serde, or json. Do NOT route detection through it (would require a protobuf schema change). Detect via `tables` + equation form instead.
- **Scalar lowering** (`src/simlin-engine/src/compiler/mod.rs`): the `Variable::Var { tables, .. }` arm at `:770`; scalar sub-arm `:781-801`. For any scalar Var with non-empty `tables`, it wraps `Expr::App(BuiltinFn::Lookup(Box::new(Expr::Var(off, loc)), Box::new(main_expr), loc), loc)` where `main_expr` is the lowered equation. For lookup-only, `main_expr` is the lowered `"0+0"` = constant `0` → `gf(0)`. For WITH LOOKUP, `main_expr` is the real input → `gf(input)`.
- **Arrayed lowering** (`expand_arrayed_with_hoisting`, `compiler/mod.rs:1605`): the non-array-producing else-branch (`:1650-1683`) emits `Expr::AssignCurr(off + i, main_expr)` with `main_expr` = constant `0` for lookup-only — no `LOOKUP` wrap, tables ignored.
- **A2A lowering** (`expand_a2a_with_hoisting`, `compiler/mod.rs:1692`): the fallback per-element loop (`:1719-1736`) likewise emits `Expr::AssignCurr(off + i, main_expr=0)` — no `LOOKUP` wrap, tables ignored.
- **Table layout (already correct)**: `variable.rs::build_tables` (`:369-428`) produces `tables.len() == n` for arrayed-with-per-element-gfs (laid out in row-major declared order via `reorder_arrayed_element_tables`, `:346-358`) and `tables.len() == 1` for A2A or arrayed-without-per-element-gfs (single variable-level table). The per-element index `i` in the lowering loops is the same row-major `SubscriptIterator` index used by `build_tables`, so element `i`'s table is at `tables[i]` → `graphical_functions[base_gf + i]`.
- **The `Lookup` opcode** (`src/simlin-engine/src/vm.rs:1507-1528`) pops `lookup_index` then `element_offset`, indexes `graphical_functions[base_gf + element_offset]`, and pushes `NaN` if `element_offset < 0 || element_offset >= table_count`. `codegen.rs::extract_table_info` (`:593-610`) computes `elem_off = off − base_off` from the `Expr::Var(off)` fed to `Lookup`, and `table_count` (`codegen.rs:730-735`) = the table var's table count.
- **Current sim time as an `Expr`**: `Expr::App(BuiltinFn::Time, loc)`. `codegen.rs:836-850` lowers `BuiltinFn::Time` to `Opcode::LoadGlobalVar { off: TIME_OFF }` with `TIME_OFF = 0` (`vm.rs:83`). Precedent: `MdlEquation::Implicit` (`mdl/convert/variables.rs:677-680`) already produces a `Time`-indexed lookup (`equation = "TIME"`, `gf = Some(..)`).
- **`TestProject` cannot attach graphical functions** (`src/simlin-engine/src/test_common.rs:20`; every builder sets `gf: None`). Build `datamodel::Project` directly. The template is `arrayed_gf_project` in `src/simlin-engine/src/per_element_gf_tests.rs:66-131` (a `#[cfg(test)]` module in `src/`, NOT `tests/`); `ramp_gf(base, slope)` (`:33-44`) builds a 2-point identifying table; the file also shows how to compile, run, and assert per-element series, and how to inspect `compiled.modules.get(&compiled.root).context.graphical_functions` / opcodes.
- **No existing test exercises arrayed/A2A lookup-only *simulation* behavior** (greenfield). `per_element_gf_tests.rs` covers explicit `LOOKUP(g[Dim], x)` consumers — a different shape — and is the layout/no-regression reference.

---

<!-- START_TASK_1 -->
### Task 1: Determine the standalone lookup-only saved-value semantics (spike)

**Verifies:** none directly — produces the rule that AC1.1/1.2/1.3 encode. This task decides the `index_expr` used by Tasks 3-4.

**Files:**
- Investigate (temporary instrumentation, reverted before commit): `src/simlin-engine/tests/simulate.rs` (`run_clearn_vs_vdf`, `:1669-1697`).
- Reference: `docs/reference/xmile-v1.0.html` (graphical-function / lookup input semantics).
- Output: a short written determination recorded as a doc comment in the new test module created in Task 3 (and summarized in the commit message).

**Implementation:**
Decide empirically what genuine Vensim saves for a standalone lookup-only variable, choosing among:
- **Candidate A — `gf(Time)`** (leading hypothesis; `index_expr = Expr::App(BuiltinFn::Time, loc)`). Plausible because C-LEARN's `INITIAL TIME = 1850` and these tables are year-indexed historical data.
- **Candidate B — `gf(0)`** (current scalar behavior; `index_expr = Expr::Const(0.0, loc)`).
- **Candidate C — literal `0`** (current arrayed/A2A behavior).

Method: temporarily instrument `run_clearn_vs_vdf` to print, for a few representative scalar lookup-only bases that are NOT suspected VDF-decode artifacts (`global_emissions_from_graph_lookup`, `historical_gdp_lookup`), the engine series under each candidate vs the `Ref.vdf` reference series at several time steps. Run via `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored run_clearn_vs_vdf` (or a temporary `#[ignore]` probe test calling the helper). Cross-check the conclusion against the XMILE spec's description of a graphical function whose input is unspecified.

Avoid the bases the design flags as suspected decode artifacts (`rs_*`, `oc,_bc,_and_bio_aerosol_forcings`, `other_forcings_*`) for the determination — Phase 4 confirms those reference columns separately. Note: the probe base `historical_gdp_lookup` (a lookup-only variable, #590 cluster) is DISTINCT from `historical_gdp` (the #591-c1 `:NA:`-arithmetic boundary base handled in Phase 4); probe the `_lookup` one only.

**Verification:**
The determination is documented with evidence (the candidate-vs-reference comparison) and a chosen `index_expr`. Revert all temporary instrumentation; `git status` is clean of probe code before moving on. If the determination contradicts `gf(Time)`, update Tasks 3-4 expectations and `index_expr` accordingly and note it — do not proceed on an unconfirmed rule.

**Commit:** none (spike). The conclusion is committed as part of Task 3's doc comment.
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-4) -->

<!-- START_TASK_2 -->
### Task 2: Pure lookup-only detection helper + unit tests

**Verifies:** clearn-residual.AC1.4 (the helper is what keeps WITH LOOKUP on `gf(input)` while routing lookup-only to the determined rule).

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs` (add a small private helper `fn is_lookup_only(...) -> bool`).
- Test: `src/simlin-engine/src/compiler/mod.rs` (or the nearest `#[cfg(test)]` module) — unit tests for the helper.

**Implementation:**
Add a pure helper that, given the variable's equation form and whether it has tables, returns whether it is lookup-only. The signal: has a graphical function (`!tables.is_empty()`) AND the equation is empty-trimmed or equals `LOOKUP_SENTINEL` (mirror `mdl::writer::is_lookup_only_equation`'s "empty or sentinel" rule so XMILE-sourced and MDL-sourced lookup-only variables are both detected). It must return `false` for a WITH-LOOKUP variable (has tables but a real input equation) and for an ordinary aux (no tables). Keep it functional/pure (input: equation string or `Ast`/`eqn` view + `tables` emptiness; output: bool) so it is unit-testable without compiling a model.

**Testing:** unit tests cover:
- lookup-only with sentinel equation + tables → `true`.
- lookup-only with empty equation + tables → `true` (XMILE-form).
- WITH LOOKUP (real input equation, e.g. `"some_input"`) + tables → `false` (AC1.4 distinction).
- ordinary aux (real equation, no tables) → `false`.
- sentinel equation but no tables (empty-RHS aux) → `false`.

**Verification:**
Run: `cargo test -p simlin-engine --lib is_lookup_only`
Expected: all pass.

**Commit:** `engine: add pure lookup-only detection helper`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Generality fixtures for standalone + applied lookup-only (RED)

**Verifies:** clearn-residual.AC1.1, clearn-residual.AC1.2, clearn-residual.AC1.3, clearn-residual.AC1.4, clearn-residual.AC1.5

**Files:**
- Test: a NEW `#[cfg(test)]` module `src/simlin-engine/src/lookup_only_tests.rs`, registered in `lib.rs` alongside `per_element_gf_tests` (do NOT extend `per_element_gf_tests.rs` — a dedicated module keeps the `cargo test --lib lookup_only` filter in Task 4 reliable). Every test function name in it MUST contain the substring `lookup_only` (e.g. `scalar_lookup_only_evaluates_at_time`, `arrayed_lookup_only_non_sorted_order`) so the verification filter cannot silently match nothing. Build `datamodel::Project` directly (mirror `arrayed_gf_project` / `ramp_gf` from `per_element_gf_tests.rs`). Record the Task 1 determination as a module doc comment.

**Implementation:**
Build small, C-LEARN-independent inline-GF models and assert the determined standalone-lookup series. Use tables whose values are distinct and identifying so a wrong index (`0`) or a zeroed series is unambiguously detectable. Pick a sim window aligned to the table's x-domain (e.g. tables keyed on `x = [2000, 2001, 2002]`, sim `INITIAL TIME = 2000`, `FINAL TIME = 2002`, `dt = 1`), so `gf(Time)` yields the table's y-values step by step.

Fixtures (each asserts the user-facing variable's series directly):
- **Scalar lookup-only** (AC1.1): one aux `g`, `equation = LOOKUP_SENTINEL`, `gf = Some(table x→y)`. Assert `g` series equals the determined rule (`gf(Time)` → the y-values `[y@2000, y@2001, y@2002]`), NOT a constant `gf(0)`.
- **Arrayed lookup-only** (AC1.2, AC1.5): `g[Dim]` with `Equation::Arrayed`, each element's equation `LOOKUP_SENTINEL` + its own per-element `gf`. Use a **non-alphabetical declared order** (e.g. `Dim = [Z, A, M]`) with each element's table carrying an element-identifying value, and assert each element's series equals its OWN table at `Time` (no positional mis-map).
- **A2A lookup-only** (AC1.3): `g[Dim]` with `Equation::ApplyToAll`, `equation = LOOKUP_SENTINEL`, one variable-level `gf`. Assert every element's series equals the single shared table at `Time`.
- **Applied-lookup no-regression** (AC1.4): in the same or a sibling model, a consumer `out = LOOKUP(g, idx)` (scalar) and/or `out[Dim] = LOOKUP(g[Dim], idx)` referencing a lookup-only `g`, with `idx` a real input distinct from `Time`. Assert `out` equals `gf(idx)` (the applied value), proving the standalone fix does not perturb the applied path.

**Testing:** the assertions above. Before Task 4, these FAIL: the arrayed/A2A standalone cases produce `0` and the scalar standalone case produces a constant `gf(0)`.

**Verification:**
Run: `cargo test -p simlin-engine --lib lookup_only`
Expected: **FAILS** (RED) for the standalone cases (arrayed/A2A = 0, scalar = constant gf(0)); the applied-lookup case may already pass.

**Commit:** `engine: add failing generality fixtures for lookup-only saved values`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Uniform lookup-only lowering (GREEN)

**Verifies:** clearn-residual.AC1.1, clearn-residual.AC1.2, clearn-residual.AC1.3, clearn-residual.AC1.4, clearn-residual.AC1.5, clearn-residual.AC1.6

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs`:
  - Scalar arm `:781-801` (the `Ast::Scalar` case under `Variable::Var`).
  - `expand_arrayed_with_hoisting` non-array-producing else-branch `:1650-1683`.
  - `expand_a2a_with_hoisting` fallback per-element loop `:1719-1736`.

**Implementation:**
Using the Task 2 helper and the Task 1 `index_expr` (e.g. `Expr::App(BuiltinFn::Time, loc)`), lower lookup-only variables uniformly:
- **Scalar:** when the variable is lookup-only, feed `index_expr` to the existing `LOOKUP(Expr::Var(off, loc), index_expr)` wrap instead of the lowered (constant-0) equation. WITH LOOKUP (non-lookup-only) keeps `LOOKUP(self, input)` unchanged.
- **Arrayed (per-element tables, `tables.len() == n`):** for lookup-only element `i`, replace `AssignCurr(off + i, 0)` with `AssignCurr(off + i, Expr::App(BuiltinFn::Lookup(Box::new(Expr::Var(off + i, loc)), Box::new(index_expr.clone()), loc), loc))`. `extract_table_info` computes `elem_off = i`, the VM reads `graphical_functions[base_gf + i]` — element `i`'s own table.
- **A2A (single shared table, `tables.len() == 1`):** for lookup-only, every element must read the one shared table, so wrap the BASE offset, not `off + i`: `AssignCurr(off + i, Expr::App(BuiltinFn::Lookup(Box::new(Expr::Var(off, loc)), Box::new(index_expr.clone()), loc), loc))` → `elem_off = 0`, `table_count = 1`. (Using `off + i` here would make `element_offset >= table_count` for `i > 0` and the VM would push `NaN`.)

Detection of lookup-only inside these branches uses the Task 2 helper (tables non-empty + equation empty/sentinel). Add a concise comment at each site explaining the A2A-vs-arrayed offset distinction (it is non-obvious and bounds-check-critical). Reuse `build_tables`/`reorder_arrayed_element_tables` and the `Lookup` opcode; add no new VM/codegen primitive.

**Testing:**
- Task 3 fixtures pass (GREEN): scalar/arrayed/A2A standalone produce the determined series; non-alphabetical order maps each element to its own table; applied-lookup unchanged.
- AC1.6: the scalar `Lookup` opcode's out-of-range clamp/NaN behavior is unchanged — the fix only changes which expression is fed as the index. Verify by (a) the existing lookup tests still passing (`cargo test -p simlin-engine` lookup/per-element GF tests), and (b) if not already covered, a small assertion that a lookup whose index falls outside the table x-range clamps to the endpoint value (low/high) as before.

**Verification:**
Run: `cargo test -p simlin-engine --lib lookup_only is_lookup_only`
Expected: all pass (GREEN).
Run: `cargo test -p simlin-engine --lib per_element_gf` and `cargo test -p simlin-engine` (default suite)
Expected: green (no regression to the per-element GF layout/applied-lookup tests).

**Commit:** `engine: lower lookup-only variables uniformly to determined saved-value semantics`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase completion criteria

- The determined semantics is documented (Task 1) with evidence and a chosen `index_expr`.
- Tasks 2-4 committed; the generality fixtures pass (RED before Task 4, GREEN after) for scalar, arrayed, and A2A standalone lookup-only, plus the applied-lookup no-regression and non-alphabetical-order cases.
- `cargo test -p simlin-engine` (default, non-ignored) is green, including `per_element_gf_tests`.
- **Ignored C-LEARN gate note:** This phase makes the engine produce the determined-correct value uniformly. Whether each #590 base (`historical_gdp_lookup`, `historical_forestry_lookup`, the arrayed `rs_*`, the scalar `*_from_graph_lookup`/`*_forcings`) then reconciles against `Ref.vdf` — versus being a VDF-decode artifact whose reference column is mis-decoded — is determined in Phase 4. Do NOT prune `EXPECTED_VDF_RESIDUAL` here. Running the `--ignored` `clearn_residual_exactness` after this phase will report any reconciled bases as `shrank`; that is expected and is closed in Phase 4. Optionally record the observed `shrank` set in the commit body.

## No special-casing (hard constraint)

No change keys on a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list. Detection is the general "has-gf + empty/sentinel-equation" rule; the index is the general determined rule; the fixtures are small models independent of C-LEARN. The only C-LEARN contact is the Task 1 read-only measurement against `Ref.vdf`, which produces a documented number, not a code branch.
