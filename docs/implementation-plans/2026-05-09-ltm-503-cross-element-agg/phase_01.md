# LTM Cross-Element Aggregate Scoring — Phase 1: Arrayed-target partial equations

**Goal:** Link scores into per-element-equation (`Ast::Arrayed`) targets carry meaningful per-element partial equations derived from each element's own equation, instead of a `"0"`-derived placeholder.

**Architecture:** The link-score equation generator (`generate_auxiliary_to_auxiliary_equation` / `generate_stock_to_flow_equation` in `ltm_augment.rs`) currently only understands `Ast::Scalar` and `Ast::ApplyToAll` targets; an `Ast::Arrayed` target falls through to `_ => "0"`. We make those generators return a `datamodel::Equation` (not a `String`), and for an `Ast::Arrayed` target they build an `Equation::Arrayed` over the target's dimension whose per-element slot equation is the standard link-score form with that element's own partial (computed by running the existing `build_partial_equation_shaped` machinery on that element's expression text). To carry an `Arrayed` link-score equation through the LTM pipeline, `LtmSyntheticVar.equation` changes from `String` to `datamodel::Equation`.

**Tech Stack:** Rust; `simlin-engine` crate; salsa incremental-compilation tracked functions; the `datamodel` IR; the existing per-reference-shape (`RefShape`) PREVIOUS-wrapping machinery from the 2026-04-25 per-ref element-graph work.

**Scope:** Phase 1 of 6 from `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`.

**Codebase verified:** 2026-05-09 (codebase-investigator).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-503-cross-element-agg.AC1: Arrayed-target link scores carry real per-element partials
- **ltm-503-cross-element-agg.AC1.1 Success:** For a per-element-equation aux `mp[NYC] = (pop[NYC] - pop[Boston]) * 0.01`, `mp[Boston] = (pop[Boston] - pop[NYC]) * 0.01`, the link score `$⁚ltm⁚link_score⁚population[nyc]→migration_pressure` is an `Equation::Arrayed` over the target dimension whose `nyc` slot partial is (the canonical form of) `(pop[nyc] - PREVIOUS(pop[boston])) * 0.01` and whose `boston` slot partial is `(PREVIOUS(pop[boston]) - pop[nyc]) * 0.01` -- no `"0"` placeholder.
- **ltm-503-cross-element-agg.AC1.2 Success:** For the same model, `$⁚ltm⁚link_score⁚population[boston]→migration_pressure` is `Equation::Arrayed`; `nyc` slot partial `(PREVIOUS(pop[nyc]) - pop[boston]) * 0.01`, `boston` slot partial `(pop[boston] - PREVIOUS(pop[nyc])) * 0.01`.
- **ltm-503-cross-element-agg.AC1.3 Success:** A stock-to-flow link score into a per-element-equation arrayed flow yields per-element partials referencing the flow's actual equation contents, not `"0"` (regression sibling to `test_stock_to_flow_link_score_handles_apply_to_all`).
- **ltm-503-cross-element-agg.AC1.4 Success:** In the `cross_element_ltm` fixture simulation, `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in` has magnitude approximately 1 in the NYC slot at every step >= 2 (since `migration_in[NYC] = MAX(-migration_pressure[Boston], 0)` and `migration_pressure[Boston] < 0` throughout) and is identically 0 in the Boston slot. Pre-fix this slot carried a `"0"`-partial-derived value far from 1.

> Note: this phase also performs the `LtmSyntheticVar.equation: String -> datamodel::Equation` type change that AC7.1 (Phase 6) requires be *documented*. No AC is "verified" by the refactor task alone — its bar is "all existing tests stay green".

---

## Context for the implementer (read before starting)

You have minimal domain context; here is what you need.

### What "LTM" is and what a "link score" is

Simlin instruments a system-dynamics model with synthetic auxiliary variables that compute, per timestep, each causal link's contribution to its target's change ("link score") and each feedback loop's strength ("loop score"). All synthetic variables use a `$` prefix and U+205A (`⁚`, TWO DOT PUNCTUATION) separator: e.g. `$⁚ltm⁚link_score⁚{from}→{to}` (the `→` is U+2192). A `[elem]` subscript may follow the name when the link score is element-specific.

A link score `x → z` is, conceptually, "ceteris paribus, how much of `Δz` came from `x`?" — it re-evaluates `z`'s equation with `x` live and every other input frozen at its `PREVIOUS()` value (the "partial equation"), then takes a signed ratio. The generated equation has the form (this is the existing scalar/A2A shape — verify exact spelling in code):

```
if (TIME = INITIAL_TIME) then 0
else if (({to} - PREVIOUS({to})) = 0) OR (({from_src} - PREVIOUS({from_src})) = 0) then 0
else ABS(SAFEDIV((({partial}) - PREVIOUS({to})), ({to} - PREVIOUS({to})), 0))
   * SIGN(SAFEDIV((({partial}) - PREVIOUS({to})), ({from_src} - PREVIOUS({from_src})), 0))
```

where `{partial}` is `build_partial_equation_shaped(...)` of `{to}`'s equation, `{to}` is the target var ref, `{from_src}` is `shape_aware_source_ref(from, shape)`.

### `Ast::Arrayed` vs `Ast::ApplyToAll`

An arrayed (subscripted) equation has two representations in the IR:
- `Ast::ApplyToAll(Vec<Dimension>, Expr)` — one shared formula evaluated for every element of the dimension(s). XMILE: `<eqn>population * 0.02</eqn>` on a `[Region]` variable.
- `Ast::Arrayed(Vec<Dimension>, HashMap<CanonicalElementName, Expr>, Option<Expr>, bool)` — the per-element form. XMILE: `<element subscript="NYC"><eqn>...</eqn></element>` blocks. Field 1 is the per-element expression map; field 2 is an optional default (EXCEPT) expression; field 3 is `apply_default_to_missing`.

The bug: `generate_auxiliary_to_auxiliary_equation` and `generate_stock_to_flow_equation` handle `Ast::ApplyToAll` but not `Ast::Arrayed` (fall through to `"0"`). There is already a regression test for the *A2A* variant of this same fall-through (`test_stock_to_flow_link_score_handles_apply_to_all`); the `Arrayed` variant was never covered.

### Verified code locations (from codebase-investigator, 2026-05-09)

| Symbol | Location | Notes |
|---|---|---|
| `Ast` enum | `src/simlin-engine/src/ast/mod.rs:29-40` | `Arrayed(Vec<Dimension>, HashMap<CanonicalElementName, Expr>, Option<Expr>, bool)` |
| `CanonicalElementName` | `src/simlin-engine/src/common.rs:43` | `CanonicalElementName(String)`, ctor `from_raw` |
| `datamodel::Equation` | `src/simlin-engine/src/datamodel.rs:190-211` | `Arrayed(Vec<DimensionName>, Vec<(ElementName, String, Option<String>, Option<GraphicalFunction>)>, Option<String>, bool)` |
| `datamodel::Equation` -> `Ast` conversion | `src/simlin-engine/src/variable.rs::parse_equation` (lines 381-463; `Arrayed` at 423-461) | filters out elements whose AST is `None` |
| `lower_variable` (keeps both `ast` and `eqn`) | `src/simlin-engine/src/model.rs:552` | a reconstructed arrayed var carries `ast: Some(Ast::Arrayed)` AND `eqn: Some(Equation::Arrayed)` |
| `generate_auxiliary_to_auxiliary_equation` | `src/simlin-engine/src/ltm_augment.rs:640` | private, returns `String` today; defective `_ => "0"` at lines ~654-681 |
| `generate_stock_to_flow_equation` | `src/simlin-engine/src/ltm_augment.rs:788` | private, returns `String` today; defective `_ => "0"` at lines ~804-823; uses `shape_aware_source_ref(stock, shape)` |
| `generate_link_score_equation` (dispatcher) | `src/simlin-engine/src/ltm_augment.rs:607` | private; dispatches flow→stock / stock→flow / aux→aux |
| `generate_link_score_equation_for_link` | `src/simlin-engine/src/ltm_augment.rs:595` | pub(crate); the public entry the salsa fns call |
| `build_partial_equation_shaped` | `src/simlin-engine/src/ltm_augment.rs:387` | `pub(crate) fn build_partial_equation_shaped(equation_text: &str, deps: &HashSet<Ident<Canonical>>, live_source: &Ident<Canonical>, live_shape: &RefShape, source_dim_elements: &[Vec<String>]) -> String` — **takes equation TEXT**; parses with `Expr0::new`, computes `other_deps`, calls `wrap_non_matching_in_previous`, returns `print_eqn`; on parse failure returns `equation_text.to_lowercase()` |
| `wrap_non_matching_in_previous` | `src/simlin-engine/src/ltm_augment.rs:163` | the recursive PREVIOUS-wrapper |
| `shape_aware_source_ref` | `src/simlin-engine/src/ltm_augment.rs:739` | builds the source-ref string given a `RefShape` |
| `link_score_var_name` | `src/simlin-engine/src/ltm_augment.rs:458` | `pub(crate) fn link_score_var_name(from: &str, to: &str, shape: &RefShape) -> String`; `FixedIndex(elems)` -> `{from}[{elems.join(",")}]→{to}` |
| `RefShape` | **defined in** `src/simlin-engine/src/db_analysis.rs:76-94` (a `#[path] mod` of `db.rs`); re-exported `pub use db_analysis::RefShape;` at `db.rs:24` | variants `Bare`, `FixedIndex(Vec<String>)` (canonical-lowercase elem names per dim), `Wildcard`, `DynamicIndex` |
| `ReferenceSite` | `src/simlin-engine/src/db_analysis.rs:109-113` | `pub(crate) struct ReferenceSite { pub shape: RefShape, pub target_element: Option<String> }`; `target_element` is `Some(elem)` only inside `Ast::Arrayed` per-element exprs |
| `collect_reference_shapes` | `src/simlin-engine/src/db_analysis.rs:219` | already walks `Ast::Arrayed` per-element exprs (calls `collect_reference_sites` then dedupes) |
| `LtmSyntheticVar` | `src/simlin-engine/src/db.rs:1972-1977` | `pub struct LtmSyntheticVar { pub name: String, pub equation: String, pub dimensions: Vec<String> }` — **`equation` is `String`** |
| `LtmVariablesResult` | `src/simlin-engine/src/db.rs:1988-1992` | `pub vars: Vec<LtmSyntheticVar>, pub loop_partitions: HashMap<String, Option<usize>>` |
| `parse_ltm_equation` | `src/simlin-engine/src/db_ltm.rs:65` | `pub(super) fn parse_ltm_equation(var_name: &str, equation: &str, var_dimensions: &[String], dims, units_ctx, module_idents) -> ParsedVariableResult`; lines 73-77 build `Equation::Scalar` or `Equation::ApplyToAll` from `var_dimensions` — never `Arrayed` today |
| `compile_ltm_equation_fragment` | `src/simlin-engine/src/db_ltm.rs:433` | `pub(super) fn compile_ltm_equation_fragment(db, var_name, equation: &str, var_dimensions: &[String], model, project) -> Option<VarFragmentResult>`; `var_size` from `var_dimensions` at lines ~592-602; compiler `Var::new` does A2A expansion |
| `compile_ltm_var_fragment` | `src/simlin-engine/src/db_ltm.rs:258` | `#[salsa::tracked(returns(ref))]`; `lsv = link_score_equation_text(...)`; `compile_ltm_equation_fragment(db, &lsv.name, &lsv.equation, &lsv.dimensions, ...)` |
| `link_score_equation_text` | `src/simlin-engine/src/db.rs:2037` | `#[salsa::tracked]`; `LtmSyntheticVar` constructors at `db.rs:2125` (module branch), `db.rs:2154` (standard branch) |
| `link_score_equation_text_shaped` | `src/simlin-engine/src/db_ltm.rs:300` | `#[salsa::tracked]`; `LtmSyntheticVar` constructors at `db_ltm.rs:376` (module branch), `db_ltm.rs:416` (standard branch: `equation = generate_link_score_equation_for_link(...)`) |
| Other production `LtmSyntheticVar` ctors | `db_ltm.rs:2818` (`try_cross_dimensional_link_scores`), `db_ltm.rs:3053` (loop scores), `db_ltm.rs:3105` (pathway scores), `db_ltm.rs:3117` (composite scores) | total **8** production ctors (the 4 above + these 4) |
| Production `.equation` readers | `db_ltm.rs:134` (`parse_ltm_var_with_ids`), `db_ltm.rs:191` (`model_ltm_implicit_var_info`), `db_ltm.rs:269` (`compile_ltm_var_fragment`), `db_ltm.rs:821,827` (parent-lsv lookup in `compile_ltm_equation_fragment`), `db.rs:5014,5031,5044,5055` (`assemble_module` Pass 3) | ~6 sites |
| `compute_layout` (LTM var sizing) | `src/simlin-engine/src/db.rs:~3088-3108` | sizes from `.dimensions` only (`size = if dimensions.is_empty() { 1 } else { product }`) — never from `.equation`. An `Equation::Arrayed` LTM var with `dimensions` set is allocated identically to an `Equation::ApplyToAll` one |
| `parse_link_offsets` | `src/simlin-engine/src/ltm_finding.rs:318` | reads `.name` and `.dimensions` only — never `.equation` |
| `enumerate_shapes` / `emit_per_shape_link_scores` | `src/simlin-engine/src/db_ltm.rs:~2838` / `~2882` | already emit one `LtmSyntheticVar` per distinct `RefShape` with `dimensions = target_dims`; for `migration_pressure` as referenced by `migration_in` they yield `FixedIndex(["boston"])` and `FixedIndex(["nyc"])` |
| `test_stock_to_flow_link_score_handles_apply_to_all` | `src/simlin-engine/src/db_ltm_tests.rs:334-371` | pins the **`Equation::ApplyToAll`** variant only, via the non-shaped `link_score_equation_text` (i.e. `RefShape::Bare`); `Ast::Arrayed` is not covered |
| `cross_element_ltm` fixture | `test/cross_element_ltm/cross_element.stmx` | Region={NYC,Boston}, euler 0..50 dt 1. `population` per-element stock (NYC=1000, Boston=500; inflows `births`, `migration_in`; outflow `migration_out`). `births` A2A flow `population * 0.02`. `migration_pressure` per-element aux: NYC `(population[NYC] - population[Boston]) * 0.01`, Boston `(population[Boston] - population[NYC]) * 0.01`. `migration_out` A2A flow `MAX(migration_pressure, 0)`. `migration_in` per-element flow: NYC `MAX(migration_pressure[Boston] * -1, 0)`, Boston `MAX(migration_pressure[NYC] * -1, 0)`. `total_population` scalar aux `SUM(population[*])`. **No `ltm_results.tsv`** — exercised by structural tests only. |

**DISCREPANCY vs design (note for the implementer):** the design's prose writes `migration_in[NYC] = MAX(-migration_pressure[Boston], 0)`; the actual fixture equation is `MAX(migration_pressure[Boston] * -1, 0)` (semantically equivalent, textually different). String-match assertions must use the `* -1` spelling or compare parsed ASTs.

### Testing conventions you must follow

- **TDD, mandatory.** Write the failing test, run it and confirm it fails for the right reason, then implement the minimal change, then confirm it passes. `docs/dev/workflow.md`, root `CLAUDE.md` ("ALL work must follow test-driven development targeting 95%+ code coverage").
- **Where the tests go:**
  - Equation-string assertions on the new per-element partials: unit tests in `src/simlin-engine/src/ltm_augment.rs`'s `#[cfg(test)] mod tests` (AST-direct style: build an `Expr2`/equation text, call the generator, `assert_eq!` on canonicalized equation strings). Existing siblings: `test_partial_equation_share_bare_shape`, `test_partial_equation_migration_pressure_fixed_nyc`. Helpers there: `deps_set(&["..."])`, `region_dim_elements()`.
  - Salsa-level link-score-equation assertions (`link_score_equation_text` / `link_score_equation_text_shaped`): `src/simlin-engine/src/db_ltm_tests.rs` (the `TestProject` builder -> `sync_from_datamodel` -> tracked-fn style; sibling: `test_stock_to_flow_link_score_handles_apply_to_all`).
  - End-to-end on the `cross_element_ltm` fixture: `src/simlin-engine/tests/simulate_ltm.rs` (compile-and-simulate, then assert on `Results` offsets; helpers `find_link_score_offset` ~`simulate_ltm.rs:1960`, `find_cross_dimensional_offsets` ~`simulate_ltm.rs:2384`; `compile_ltm_incremental` / `compile_ltm_incremental_with_partitions` ~`simulate_ltm.rs:34-68`; `load_xmile_model("../../test/cross_element_ltm/cross_element.stmx")`). `simulate_ltm.rs` is `required-features = ["file_io"]`.
- **Canonicalization rules for equation-string assertions** (these are what `print_eqn` produces): identifiers and element-names lowercased; parsed builtin names lowercased (`SUM` round-trips as `sum`); synthesized `PREVIOUS` stays uppercase; binary operators get a single space each side; parens reintroduced for precedence. So AC1.1's expected `nyc` slot partial, written canonically, is `(population[nyc] - PREVIOUS(population[boston])) * 0.01`.
- **Test-time budget:** each new unit test under ~2s on a debug build; `cargo test --workspace` must stay under the 3-minute wall-clock cap (enforced by `scripts/pre-commit` and CI). Use small fixtures (the `cross_element_ltm` model is already tiny). Do not skip silently — fail loudly if a fixture is missing.
- **`TestProject` builder** (the struct/impl is in `src/simlin-engine/src/test_common.rs`; the free `x_*` helpers below are in `src/simlin-engine/src/testutils.rs`): `.new(name)`, `.with_sim_time(start, stop, dt)`, `.named_dimension("Region", &["NYC","Boston"])`, `.array_stock("population[Region]", "100", &["births"], &[], None)`, `.array_flow("births[Region]", "population * 0.1", None)`, `.array_aux(...)`, `.array_with_ranges(name, vec![(elem, eqn)])` (per-element ⇒ `Equation::Arrayed`), `.build_datamodel()`. To enable LTM: `use salsa::Setter; source_project.set_ltm_enabled(&mut db).to(true)` (or `set_project_ltm_enabled(&mut db, sync.project, true)`). If `TestProject` cannot express the variable you need, build the `datamodel::Project` literally (helpers `x_stock`/`x_flow`/`x_aux`/`x_model` in `src/simlin-engine/src/testutils.rs` build literal `datamodel::Variable`s, so you can set `equation: Equation::Arrayed(...)`), or parse an inline XMILE string.
- **Commits:** `engine: <lowercase description>` (no period, <60 chars), body explains *why*, no emoji, no `Co-Authored-By`, never `--no-verify`. Lean on the pre-commit hook (it runs fmt/clippy/tests).

---

## Tasks

<!-- START_TASK_1 -->
### Task 1: Refactor `LtmSyntheticVar.equation` from `String` to `datamodel::Equation`

This is a prerequisite, behavior-preserving refactor. Phase 1's feature work (Tasks 2-4) needs an LTM synthetic var to be able to hold an `Equation::Arrayed`; today its `equation` field is a `String`.

**Files:**
- Modify: `src/simlin-engine/src/db.rs:1972-1977` (the `LtmSyntheticVar` struct), and the 2 ctors at `db.rs:2125` / `db.rs:2154`, the ~4 `.equation` readers at `db.rs:5014,5031,5044,5055`.
- Modify: `src/simlin-engine/src/db_ltm.rs` — the 4 ctors at `db_ltm.rs:376`, `416`, `2818`, `3053`, `3105`, `3117` (note: that's 6 lines listed — `376`/`416` are the shaped link-score ctors, `2818` cross-dimensional, `3053` loop, `3105` pathway, `3117` composite; reconcile against the code); the 3 signatures `parse_ltm_equation` (`db_ltm.rs:65`), `compile_ltm_equation_fragment` (`db_ltm.rs:433`), `compile_ltm_var_fragment` (`db_ltm.rs:258`); the readers at `db_ltm.rs:134,191,269,821,827`.
- Modify: `src/simlin-engine/src/ltm_augment.rs` — `generate_link_score_equation_for_link` (`:595`), `generate_link_score_equation` (`:607`), `generate_auxiliary_to_auxiliary_equation` (`:640`), `generate_stock_to_flow_equation` (`:788`), the flow→stock generator — change their return type from `String` to `datamodel::Equation` (returning `Equation::Scalar(s)` for scalar targets and `Equation::ApplyToAll(target_dims, s)` for A2A targets; `Equation::Arrayed` is added in Task 2). Pure mechanical for now. **The loop-score / pathway / composite generators (`generate_loop_score_equation` `:965`, `generate_loop_score_variables` `:486`, `create_aux_variable` `:991`, and the pathway/composite emission in `model_ltm_variables`) build equation *text* and currently produce `datamodel::Variable`s / strings that get matched into an `LtmSyntheticVar` at `db_ltm.rs:3053` (`var.get_equation()` → `Equation::Scalar(eq) => eq.clone()`) — so the `datamodel::Equation` carry-through for those lives at the `LtmSyntheticVar`-construction site (`db_ltm.rs:3053`, `:3105`, `:3117`), not necessarily inside the generators themselves. Change whichever is minimal; the oracle is "no behavior change, all existing tests pass with identical expected values."
- Modify: ~30 `#[cfg(test)]` `LtmSyntheticVar` constructors in `src/simlin-engine/src/ltm_post.rs`, `src/simlin-engine/src/ltm_finding.rs`, `src/simlin-engine/src/db_ltm_tests.rs` (`equation: String::new()` -> `equation: datamodel::Equation::Scalar(String::new())`, etc.); and the few test assertions that inspect `.equation` as a string (e.g. `lsv.equation.contains("population")`) need to destructure the `datamodel::Equation` first (consider a tiny test helper `equation_text(&Equation) -> String` that concatenates the scalar/A2A string or the per-element strings).

**Implementation contract:**
- `LtmSyntheticVar.equation` becomes `datamodel::Equation`.
- `LtmSyntheticVar.dimensions: Vec<String>` is **retained** as-is (layout sizing in `compute_layout` and `parse_link_offsets` both key off it; it is redundant with the `Equation` variant's dims but the design keeps it).
- Constructors: build the `Equation` variant consistent with `dimensions` — `dimensions.is_empty()` ⇒ `Equation::Scalar`, non-empty ⇒ `Equation::ApplyToAll(dimensions.clone(), ...)` (this is exactly the logic currently in `parse_ltm_equation:73-77`, just moved to where the var is built). `generate_link_score_equation_for_link` should return the appropriate variant directly so the ctor stores it verbatim.
- `parse_ltm_equation` / `compile_ltm_equation_fragment` / `compile_ltm_var_fragment` and the readers: take/pass `&datamodel::Equation` instead of `(&str, &[String])`. `parse_ltm_equation`'s current `Equation::Scalar`/`ApplyToAll` construction is deleted (the equation arrives already-typed); for an `Equation::Arrayed` it must produce an `Ast::Arrayed` (the existing `datamodel::Equation` -> `Ast` conversion in `variable.rs::parse_equation` already does this — route through the same `datamodel::Variable::Aux { equation, .. }` -> parse path).
- `assemble_module` Pass 3 (`db.rs:5014-5055`): if it pattern-matches the equation *string* to choose a compile path, adapt to match on the `Equation` variant; behavior unchanged.
- **No behavior change.** Every existing test must still pass with identical expected values. `migration_in`'s link score is still the `"0"`-partial form (just now represented as `Equation::ApplyToAll(["Region"], "if (TIME = INITIAL_TIME) ... (0) ...")`); Task 2 fixes it.

**Steps:**
1. Change the `LtmSyntheticVar` struct field type.
2. Fix the 8 production constructors and the ~6 production `.equation` readers, plus the 3 helper signatures (`parse_ltm_equation`, `compile_ltm_equation_fragment`, `compile_ltm_var_fragment`) and `assemble_module` Pass 3.
3. Change `generate_link_score_equation_for_link` / `generate_link_score_equation` / `generate_auxiliary_to_auxiliary_equation` / `generate_stock_to_flow_equation` / the flow→stock generator to return `datamodel::Equation`. For the loop-score / pathway / composite vars, carry a `datamodel::Equation` through wherever the `LtmSyntheticVar` is constructed (`db_ltm.rs:3053/:3105/:3117`) — those generators may keep returning equation text.
4. Fix the ~30 `#[cfg(test)]` `LtmSyntheticVar` constructors and the `.equation`-inspecting test assertions.
5. `cargo build --workspace` — must compile clean. `cargo clippy --workspace --all-targets -- -D warnings` — clean. `cargo test -p simlin-engine` — green. `cargo test -p simlin-engine --features file_io --test simulate_ltm` — green.
6. Commit: `engine: change LtmSyntheticVar.equation to datamodel::Equation` (body: this is the representation prerequisite for arrayed-target link-score equations; no behavior change).

**Verifies:** None (behavior-preserving refactor; the bar is "all existing tests stay green").

**Verification:**
- Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Expected: no errors, no warnings.
- Run: `cargo test -p simlin-engine && cargo test -p simlin-engine --features file_io --test simulate_ltm`
  Expected: all tests pass, no expected-value changes.
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-4) -->

<!-- START_TASK_2 -->
### Task 2: Generate per-element partial equations for `Ast::Arrayed` link-score targets

**Verifies:** ltm-503-cross-element-agg.AC1.1, ltm-503-cross-element-agg.AC1.2

**Files:**
- Create (new helper) + Modify: `src/simlin-engine/src/ltm_augment.rs` — add `build_arrayed_link_score_equation(...)` (name your call); modify `generate_auxiliary_to_auxiliary_equation` (`:640`) and `generate_stock_to_flow_equation` (`:788`) so an `Ast::Arrayed` target goes through it instead of the `_ => "0"` fall-through.
- Test: `src/simlin-engine/src/ltm_augment.rs` `#[cfg(test)] mod tests` (unit, AST/equation-text-direct).

**Implementation contract:**

When the link-score *target* var's AST is `Ast::Arrayed(target_dims, per_elem_map, default_expr, apply_default_to_missing)`:

- For each `(element_key, expr)` in `per_elem_map`:
  - Get that element's equation text. Either `print_eqn`/`expr2_to_string` on `expr`, or — preferably — the corresponding `(element, eqn_text, _, _)` from the target's `eqn: Some(Equation::Arrayed(..))` (a reconstructed var carries both; the raw `datamodel` text avoids a round-trip). Match keys case-insensitively (`CanonicalElementName`).
  - Compute `deps_e` from `expr` the same way the scalar path computes `deps` (`identifier_set(ast, ..)` over that element's AST).
  - `partial_e = build_partial_equation_shaped(elem_eqn_text, &deps_e, from_ident, shape, source_dim_elements)`. The `shape` is the *one* `RefShape` this link-score var was emitted for (`emit_per_shape_link_scores` already emits one var per distinct shape; within a var the shape is constant). For an element whose equation does not reference `from` with that shape, `build_partial_equation_shaped` naturally wraps the (non-matching) refs in `PREVIOUS`, so the slot equation evaluates to ≈0 — correct (that source-element's effect on that target-element flows through a *different* link-score var / shape, and must not be double-counted here).
  - Wrap `partial_e` in the standard link-score form (the same `if (TIME = INITIAL_TIME) ... ABS(SAFEDIV(...)) * SIGN(SAFEDIV(...))` shape the scalar path builds), with `to_q` = the target var name (bare — within an `Equation::Arrayed` slot a bare self-reference resolves element-wise) and `from_source_q` = `shape_aware_source_ref(from_ident, shape)` (constant across slots).
- Build the default slot equation analogously from `default_expr` if present.
- Return `datamodel::Equation::Arrayed(target_dims_names, vec_of_(element, slot_eqn, None, None), default_slot_eqn, apply_default_to_missing)`.
- Scalar and `Ast::ApplyToAll` targets keep returning `Equation::Scalar(...)` / `Equation::ApplyToAll(target_dims, ...)` exactly as Task 1 left them.
- Factor the "wrap a partial in the link-score guard form" into a small shared helper if it isn't already, so the scalar/A2A/Arrayed paths don't duplicate the format string.

**Testing:** TDD. Tests must verify each AC listed:
- AC1.1: build the 2-region model `mp[NYC] = (population[NYC] - population[Boston]) * 0.01`, `mp[Boston] = (population[Boston] - population[NYC]) * 0.01` (`Ast::Arrayed`); call the aux→aux generator for the link `population → migration_pressure` with `shape = RefShape::FixedIndex(["nyc"])`; assert the result is `Equation::Arrayed` over `["Region"]` whose `nyc` slot's `{partial}` substring is the canonical `(population[nyc] - PREVIOUS(population[boston])) * 0.01` and whose `boston` slot's `{partial}` substring is the canonical `(PREVIOUS(population[boston]) - population[nyc]) * 0.01`. (Assert on the partial substring or parse-and-compare; do not hard-code the full guard string brittlely — but DO assert there is no literal `(0)` partial in any slot.)
- AC1.2: same model, `shape = RefShape::FixedIndex(["boston"])`; `nyc` slot partial canonical `(PREVIOUS(population[nyc]) - population[boston]) * 0.01`, `boston` slot partial canonical `(population[boston] - PREVIOUS(population[nyc])) * 0.01`.
- Add a guard test: a scalar target and an `Ast::ApplyToAll` target still produce `Equation::Scalar` / `Equation::ApplyToAll` respectively (no regression of the existing shapes) — or rely on the existing `ltm_augment.rs` tests for that.

**Verification:**
- Run: `cargo test -p simlin-engine ltm_augment`
  Expected: the new tests pass; existing `ltm_augment` tests still pass.

**Commit:** `engine: build per-element partials for arrayed-target link scores`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Regression test — stock-to-flow link score into a per-element-equation arrayed flow

**Verifies:** ltm-503-cross-element-agg.AC1.3

**Files:**
- Test: `src/simlin-engine/src/db_ltm_tests.rs` — add `test_stock_to_flow_link_score_handles_arrayed` (sibling to `test_stock_to_flow_link_score_handles_apply_to_all` at `:334-371`).
- Modify (only if the test fails): `src/simlin-engine/src/ltm_augment.rs` `generate_stock_to_flow_equation` — Task 2 should already have wired the `Ast::Arrayed` path here; this task confirms it via the salsa-level entry point and fixes any gap.

**Implementation contract:** No new production logic expected — Task 2 covered `generate_stock_to_flow_equation`. This task adds the salsa-level regression test that mirrors the existing A2A one. If it surfaces a gap (e.g. the stock→flow path didn't actually get the `Ast::Arrayed` arm), fix it minimally.

**Testing:** TDD. Build a model with a stock and a per-element-equation (`Equation::Arrayed`) arrayed flow that references the stock — e.g. `population[Region]` stock (`Region = {NYC, Boston, LA}`), `births[Region]` flow with per-element equations `<NYC: population[NYC] * 0.03>`, `<Boston: population[Boston] * 0.02>`, `<LA: population[LA] * 0.01>`. If `TestProject` cannot express a per-element-equation flow, build the `datamodel::Project` literally (`x_stock`/`x_flow`/`x_model` from `testutils.rs`, setting the flow's `equation: Equation::Arrayed(...)`). Enable LTM, call `link_score_equation_text` (or `link_score_equation_text_shaped`) for the link `population → births`, and assert: the returned `LtmSyntheticVar.equation` is `Equation::Arrayed`; its per-element slot equations reference `population` (the flow's actual equation contents) and contain no literal `(0)` partial. Mirror the existing test's structure (`LtmLinkId::new`, `sync_from_datamodel`, etc.).

**Verification:**
- Run: `cargo test -p simlin-engine test_stock_to_flow_link_score`
  Expected: both `..._handles_apply_to_all` and the new `..._handles_arrayed` pass.

**Commit:** `engine: cover stock-to-arrayed-flow link scores (regression test)`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Integration test — `migration_pressure[boston]→migration_in` link score on the `cross_element_ltm` fixture

**Verifies:** ltm-503-cross-element-agg.AC1.4 (and contributes to AC6.1 — no scalar/A2A regression — though AC6.1's full check is Phase 6)

**Files:**
- Test: `src/simlin-engine/tests/simulate_ltm.rs` — add `test_cross_element_link_score_migration_in_arrayed_partials` (or similar). Reuse `compile_ltm_incremental` / `compile_ltm_incremental_with_partitions`, `load_xmile_model("../../test/cross_element_ltm/cross_element.stmx")`, and `find_link_score_offset` / `find_cross_dimensional_offsets` to locate the var by name-prefix.

**Implementation contract:** No new production logic — Task 2 fixed `generate_auxiliary_to_auxiliary_equation`, which is the path for the `migration_pressure → migration_in` link (`migration_pressure` is an aux, `migration_in` a flow whose equation references no stock — routed to the aux→aux generator). This task verifies the end-to-end value.

**Testing:** TDD — write this test before/while implementing Task 2 if you can, but at the latest add it now and confirm it goes red against `main`'s behavior (the var carries a `"0"`-partial-derived value far from 1) and green after Task 2. Compile the `cross_element_ltm` fixture with LTM enabled, build a `Vm`, run it, get `Results`. Find the offset and `dimensions` of the synthetic var named `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in` (it is dimensioned over `Region`; NYC = element index 0, Boston = index 1 — confirm element order from the dimension definition). Assert: for every step `t >= 2` (`t == 1` is the unstable first post-initial step — skip it, matching `ensure_ltm_results`), `|value at offset + nyc_index|` is within `1e-3` of `1.0`, and `value at offset + boston_index == 0.0` exactly. (Reason: `migration_pressure[Boston] = (500 - 1000) * 0.01 = -5 < 0` throughout, so `migration_in[NYC] = MAX(5, 0) = 5` and the partial w.r.t. live `migration_pressure[boston]` exactly equals `migration_in[NYC]` ⇒ `ABS(SAFEDIV(Δ, Δ)) = 1`; `migration_pressure[NYC] = +5 > 0` so `migration_in[Boston] = MAX(-5, 0) = 0` constantly ⇒ that slot is identically 0.)

Also: confirm no golden-data drift — `simulates_population_ltm` (the only LTM golden test, `test/logistic_growth_ltm/`, a scalar model) still passes with unchanged expected values; `test_arrayed_population_ltm_exhaustive` / `test_arrayed_population_ltm_discovery` / `test_cross_element_ltm_exhaustive` / `test_cross_element_ltm_edge_set_truthful` / `measurement_postscript_*` still pass (these are pure-A2A or only assert on edges/structure that Phase 1 doesn't change — but if any breaks, investigate; Phase 1 should not change them).

**Verification:**
- Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm`
  Expected: the new test passes; `simulates_population_ltm`, `test_cross_element_ltm_*`, `test_arrayed_population_ltm_*`, `measurement_postscript_*` all still pass.
- Run: `cargo test --workspace`
  Expected: green within the 3-minute wall-clock cap (this is the phase's "Done when" gate).

**Commit:** `engine: verify arrayed-target link scores on the cross-element fixture`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase 1 done-when checklist

- [ ] `LtmSyntheticVar.equation` is `datamodel::Equation`; all 8 production constructors, ~6 readers, 3 helper signatures, and ~30 test constructors updated; no behavior change (all pre-existing tests green).
- [ ] `generate_auxiliary_to_auxiliary_equation` and `generate_stock_to_flow_equation` produce `Equation::Arrayed` per-element partials for `Ast::Arrayed` targets (no `"0"` placeholder).
- [ ] AC1.1, AC1.2 — unit tests on the per-element partial strings pass.
- [ ] AC1.3 — `test_stock_to_flow_link_score_handles_arrayed` passes.
- [ ] AC1.4 — the `cross_element_ltm` integration test passes; `simulates_population_ltm` and the existing cross-element/arrayed structural tests still pass.
- [ ] `cargo test --workspace` green within the 3-minute cap; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt -- --check` clean. (Run `git commit` and let the pre-commit hook gate it.)
