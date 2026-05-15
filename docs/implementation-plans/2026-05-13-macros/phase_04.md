# Vensim Macro Support — Phase 4: Multi-output and arrayed invocation

**Goal:** Support Vensim's `:` multi-output macro call form (`total = add3(a, b, c : minv, maxv)`) by materializing it at MDL-import time as an explicit `Variable::Module` plus binding auxiliary variables, and confirm that arrayed (apply-to-all) macro invocation rides the engine's existing per-element unrolling.

**Architecture:** The two halves are asymmetric. **(a) Multi-output** is the only macro form that cannot be expressed as plain equation text (a call returns several named values at once), so it is *materialized at import* in `src/simlin-engine/src/mdl/convert/`: a multi-output invocation becomes an explicit `datamodel::Variable::Module` (pointing at the macro's model, with the call arguments wired as input `ModuleReference`s) plus one binding `Variable::Aux` per output — the LHS aux reads `<module>.<primary_output>`, and each `:`-list aux reads `<module>.<additional_output>`. The module-output reference resolves through the existing `get_submodel_offset` machinery, which is fully general — a binding aux's equation text `module_instance.output_name` (ASCII period at the datamodel layer — see the authoritative Separator note in Current state below) canonicalizes to the `·` (U+00B7) form and resolves like any other reference. **(b) Arrayed invocation** needs no new mechanism: Phase 3 already made `BuiltinVisitor`'s recognition (`walk()` and `contains_stdlib_call`) macro-aware, and the `instantiate_implicit_modules` apply-to-all path builds one independent synthetic module per dimension element regardless of whether the module-function is a stdlib function or a macro. Phase 4 therefore *verifies* arrayed invocation with fixtures rather than building new machinery.

**Tech Stack:** Rust — the MDL converter (`src/simlin-engine/src/mdl/convert/`), with fixture files under `test/test-models/tests/`. No external dependencies.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 4 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC3: Multi-output and arrayed invocation
- **macros.AC3.1 Success:** A multi-output invocation (`total = add3(a,b,c : minv, maxv)`) materializes as a module instance; `total` receives the primary output and `minv`/`maxv` become model variables holding the additional outputs.
- **macros.AC3.2 Success:** `minv` and `maxv` are referenceable by subsequent equations and carry the correct values.
- **macros.AC3.3 Success:** The metasd multi-output models (`THEIL`, `SSTATS`) expand and simulate.
- **macros.AC3.4 Success:** An arrayed invocation (`y[Dim] = MYMACRO(x[Dim], …)`) expands into one independent macro instance per dimension element.
- **macros.AC3.5 Edge:** An arrayed invocation of a stock-bearing macro gives each element its own persistent stock.

---

## Current state (verified 2026-05-14)

**The `·` module-output reference convention is fully general (the crux of multi-output materialization):**
- `canonicalize()` (`common.rs:321`, the replacement at `common.rs:343-346`) turns a `.` in an identifier segment into U+00B7. The MDL lexer (`lexer/mod.rs:307`) accepts `.` as an identifier-continue char, so `module.output` lexes as one identifier and lowers (`ast/expr1.rs:73`) to a canonicalized `Var`. So a `datamodel::Equation::Scalar("module_instance.output_name")` is a valid reference — the period auto-canonicalizes to `·`.
- `get_submodel_offset` (`compiler/context.rs:405-455`) and `get_submodel_metadata` (`compiler/context.rs:372-403`) split on U+00B7 and recurse into the named submodule — **for any module, regardless of how it was created** (stdlib-synthesized or explicitly authored). `get_offset` (`context.rs:124`) → `get_submodel_offset`. Dependency resolution also handles `·`: `all_deps` (`model.rs:365-414`), `module_output_deps` (`model.rs:203-248`). So a binding `Aux { equation: Scalar("add3_call.minval") }` resolves to the `add3_call` module's `minval` output.

**Separator (authoritative for Phases 4-6).** The materialized binding-aux equation text uses an **ASCII period** — `format!("{}.{}", module_ident, output)` — and the datamodel layer stores exactly that ASCII `.` form. `canonicalize()` converts it to U+00B7 (`·`) only later, when the equation string is parsed during compilation. So: the *datamodel* (`Equation::Scalar` text — what `convert_mdl` produces and what `convert/`-level tests inspect) uses `.`; the *compiled AST / VM* layer uses `·`. Phase 3's `BuiltinVisitor` rewrite emits `·` directly because it operates on the post-parse AST — a distinct layer, not a contradiction. Phase 5 (XMILE round-trip) and Phase 6 (MDL reconstruction) both read the datamodel form, so both match against `.`.

**The datamodel `Module` shape and the `BuiltinVisitor` precedent:**
- `datamodel.rs:344-370`: `ModuleReference { src: String, dst: String }`; `Module { ident, model_name, documentation, units, references: Vec<ModuleReference>, ai_state, uid, compat }`; `Variable::Module(Module)`.
- `BuiltinVisitor` (`builtins_visitor.rs:389-541`) is the precedent for minting a `Variable::Module` programmatically: input `ModuleReference`s have `dst = format!("{}.{}", module_ident, input_name)` and `src =` the caller-side argument (a hoisted-arg ident). `parse_var` (`variable.rs:644-669`) turns every `ModuleReference` into a `ModuleInput`.
- **`ModuleReference`s are inputs-only by compile time** — `build_module_inputs` (`db.rs:3232-3256`) strips any reference whose `dst` does not start with the module's own prefix (i.e. output-direction `<connect>`s are dropped). A parent reads a submodule's *output* purely via `·`-equation text, never via an output reference. **So Phase 4's materialized `Variable::Module` carries only input `ModuleReference`s; the outputs are realized as separate binding `Aux`es whose equation text is `<module>.<output>`.**

**The MDL convert path:**
- `MdlItem::Macro` is currently ignored at `convert/mod.rs:248` (Phase 2 changes this — each `MacroDef` becomes a macro-marked `datamodel::Model` with a `MacroSpec` and synthesized port variables, added to `project.models`).
- A regular invocation equation: `build_project` (`convert/variables.rs:28-106`) → `build_variable` (`convert/variables.rs:798-854`) → `build_equation` (`convert/variables.rs:859-939`), which formats the RHS via `self.formatter.format_expr(expr)`. The `XmileFormatter`'s `App` arm — `format_call_ctx` (`xmile_compat.rs:219-498`) — currently formats a multi-arg `CallKind::Symbol` call as plain text `MACRONAME(arg1, arg2, ...)` (`xmile_compat.rs:479-497`); the lookup-invocation special case is `args.len() == 1` only. After Phase 2 adds the 6th `output_bindings` field to `Expr::App`, Phase 2 places a `debug_assert!(output_bindings.is_empty(), ...)` in `format_call_ctx`'s `App` arm — **so Phase 4 must materialize multi-output invocations in the converter *before* anything formats them.** `build_variable` returns `Result<Option<Variable>, ConvertError>` (one variable) — multi-output materialization produces *several* datamodel variables, so it cannot be a plain `build_variable` call.
- `ConvertError` (`convert/types.rs:11-26`): `Reader | View | InvalidRange | CyclicDimensionDefinition | Import | Other`. No macro-specific variant; Phase 4 uses `Other(String)` (or adds a variant, updating the `Display`/`Error`/`From` impls in the same file).

**The apply-to-all path is generic (arrayed invocation rides it for free):**
- `instantiate_implicit_modules` (`builtins_visitor.rs:574-690`): the `Ast::ApplyToAll` arm (`:588-621`) and `Ast::Arrayed` arm (`:622-688`) build one `BuiltinVisitor::new_with_subscript_context` per subscript, so each dimension element gets its own subscript-suffixed synthetic module (`$⁚{var}⁚{n}⁚{func}⁚{subscript_suffix}`). The only stdlib-specific gate was `contains_stdlib_call` (`builtins_visitor.rs:30-56`) → `is_stdlib_module_function` — **Phase 3 (Task 3) makes `contains_stdlib_call` macro-aware**, so an arrayed macro invocation now enters the per-element expansion path. Nothing else in the A2A path is stdlib-coupled. Existing arrayed-module tests: `test_arrayed_smooth1`, `test_arrayed_delay1_numerical_values` in `builtins_visitor.rs`'s `mod tests`.

**The real-world multi-output / arrayed corpus models (verified by a repo-wide scan):**
- **Multi-output `:` form** — exactly two `.mdl` files in the whole repo use it: `test/metasd/theil-statistics/Theil_2011.mdl` — `:MACRO: THEIL(historical,simulated:R2,MAPE,RMSPE,RMSE,MSE,SSE,Dif Mea,Dif Var,Dif Cov,Um,Us,Uc,Count)` (2 inputs, 13 outputs), invoked at `Theil_2011.mdl:518-519`; and `test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl` — `:MACRO: SSTATS(historical,simulated:R2,MAE,MAE over Mean,MAPE,RMSE,MSE,Um,Us,Uc,Count)` (2 inputs, 10 outputs), with two invocations. The six bundled `test/test-models/tests/macro_*` fixtures are **all single-output** — Phase 4 must author its own focused multi-output fixture.
- **Arrayed invocation** — C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) has ~8 arrayed `[COP]` / `[COP,Target]` macro invocations of `SAMPLE UNTIL`, `RAMP FROM TO`, `SSHAPE` (some nested inside `IF THEN ELSE`); all four C-LEARN macros are single-output. C-LEARN exercises arrayed invocation but not the `:` multi-output form.

**Test surfaces (recap):** `convert_mdl(source) -> Result<Project, ConvertError>` (`convert/mod.rs:315-318`, in-crate test-only). `open_vensim(&str) -> common::Result<Project>` (public). `simulate.rs`: `compile_vm`, `simulate_mdl_path`. New MDL fixtures wire in as `#[test] fn simulates_<name>_mdl() { simulate_mdl_path("../../test/.../X.mdl"); }`. Large-model tests must be `#[ignore]`d with a documented opt-in command per `docs/dev/rust.md` test-time-budget rules.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: Multi-output materialization and arrayed-invocation verification

<!-- START_TASK_1 -->
### Task 1: Materialize multi-output invocations in the MDL converter

**Verifies:** macros.AC3.1, macros.AC3.2.

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (the variable-building loop in `build_project` ~lines 28-106, and/or `build_variable` ~lines 798-854) — and/or a new `convert/` step
- Possibly modify: `src/simlin-engine/src/mdl/convert/types.rs` (a `ConvertError` variant, if `Other(String)` is not preferred)
- Create: `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl` and `test/test-models/tests/macro_multi_output/output.tab`
- Modify: `src/simlin-engine/tests/simulate.rs` (wire the new fixture)
- Test: `src/simlin-engine/src/mdl/convert/` inline `#[cfg(test)]` tests (the same module Phase 2's macro-conversion tests live in)

**Implementation:**

A multi-output invocation `total = add3(a, b, c : minv, maxv)` is parsed (Phase 2 Task 3) into an equation whose RHS is an `Expr::App` with a non-empty `output_bindings` field (`[minv, maxv]`). Phase 4 detects this and, instead of letting `build_equation` format it as text, materializes it.

1. **Ordering.** The materialization needs the called macro's `MacroSpec` (its `parameters`, `primary_output`, `additional_outputs`). Phase 2 builds every macro-marked model into `project.models` (with `model.macro_spec` populated). Build a `name -> &MacroSpec` lookup over those macro-marked models, and run multi-output materialization at a point where both the lookup and the main model's variable list are available — either by ordering Phase 2's macro-model-building step before the main-model variable loop, or by running materialization as a step after `build_project`. The implementor picks the cleanest insertion given the actual `convert/` pass structure; the contract is: every multi-output invocation in the main model is materialized, and no multi-output `Expr::App` ever reaches `build_equation`/the formatter.

2. **Detect.** For each main-model symbol whose equation's top-level RHS is an `Expr::App` with non-empty `output_bindings`: this is a multi-output invocation. (A multi-output call cannot legally be a sub-expression — it is always the whole RHS.) Look up the called name in the macro `MacroSpec` map. If it is not a known macro → `ConvertError` ("multi-output call to unknown macro `<name>`"). Arity: `args.len()` must equal `MacroSpec.parameters.len()` and `output_bindings.len()` must equal `MacroSpec.additional_outputs.len()` — otherwise a `ConvertError` naming the macro and the expected/actual counts.

3. **Materialize.** Emit, in place of the plain `total` aux:
   - **One `Variable::Module`** — a deterministic, collision-safe, *serialization-stable* `ident`: **`{lhs}_macro`** — the LHS variable's canonical ident with a literal `_macro` suffix. If `{lhs}_macro` already collides with an existing symbol in the model, append the lowest numeric disambiguator that makes it unique (`{lhs}_macro_2`, `{lhs}_macro_3`, …). This is deliberately *not* the `$⁚` compile-time-synthetic prefix — this module is materialized at import, so it is serialized and round-tripped, and its ident must be stable and human-readable. `model_name` is the called macro's `Model.name`. `references` is one input `ModuleReference` per call argument: `dst = format!("{}.{}", module_ident, MacroSpec.parameters[i])`, `src =` the argument. For a simple `Var` argument, `src` is that variable's canonical name directly. For an expression-valued argument, hoist it into a synthetic `Aux` with a deterministic, serialization-safe name and use that aux's ident as `src`. (The real-world corpus — `THEIL`, `SSTATS` — uses only simple-`Var` arguments, so the hoisting path is for generality; if it proves involved, it may be deferred to a tracked follow-up, but simple-`Var` arguments must work.)
   - **The primary-output binding aux** — `Variable::Aux { ident: <lhs canonical ident>, equation: Scalar(format!("{}.{}", module_ident, MacroSpec.primary_output)), .. }`. This replaces the would-be plain `total` aux; `total` now reads the module's primary output.
   - **One additional-output binding aux per `:`-list entry** — for each `i`, `Variable::Aux { ident: <output_bindings[i] canonical ident>, equation: Scalar(format!("{}.{}", module_ident, MacroSpec.additional_outputs[i])), .. }`. The call-site name (`output_bindings[i]`, e.g. `minv`) becomes the variable ident; the macro's internal output name (`MacroSpec.additional_outputs[i]`, e.g. `minval`) is what it reads from the module. These are brand-new model variables not present in `self.symbols`.

4. **Author the fixture.** `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl`: a **stockless** multi-output macro so the expected output is constant over time and the `output.tab` is trivial to hand-compute. For example a macro `:MACRO: ADD3(a, b, c : minval, maxval)` with body `ADD3 = a + b + c`, `minval = MIN(a, MIN(b, c))`, `maxval = MAX(a, MAX(b, c))`; an invocation `total = ADD3(in1, in2, in3 : the min, the max)`; constant inputs (`in1`, `in2`, `in3`); and a **downstream equation** that references the additional outputs, e.g. `spread = the max - the min` (this is what proves AC3.2). Hand-compute `output.tab` with `total`, `the min`, `the max`, `spread` (all constant). Document the arithmetic in the fixture's `README.md` or a comment.

5. **Wire into `simulate.rs`** as `#[test] fn simulates_macro_multi_output_mdl() { simulate_mdl_path("../../test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl"); }`.

**Testing:**
- **macros.AC3.1** (`convert/`-level structure test, `convert_mdl(include_str!(".../test_macro_multi_output.mdl"))`): assert the main model contains exactly one `Variable::Module` whose `model_name` is the `ADD3` macro's model, with input `ModuleReference`s wiring `in1`/`in2`/`in3` to the `a`/`b`/`c` ports; assert `total` is a `Variable::Aux` whose equation reads `<module>.<primary_output>` (ASCII period — the datamodel form, per the authoritative separator note above); assert `the min` and `the max` are `Variable::Aux`es reading `<module>.minval` and `<module>.maxval` respectively.
- **macros.AC3.2** (simulation, via the wired `simulate.rs` fixture test): the fixture's `simulate_mdl_path` test passes — `total`, `the min`, `the max`, and the downstream `spread` all match the hand-computed `output.tab`, proving `the min`/`the max` are referenceable by a subsequent equation and carry correct values.
- Arity-mismatch: a `convert/`-level test that `convert_mdl` of a `.mdl` invoking `ADD3` with the wrong number of arguments or the wrong number of `:`-outputs returns a `ConvertError` naming `ADD3`.

**Verification:**
Run: `cargo test -p simlin-engine convert`
Run: `cargo test -p simlin-engine --test simulate simulates_macro_multi_output_mdl`
Expected: all pass.

**Commit:** `engine: materialize multi-output macro invocations at import`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Verify arrayed macro invocation

**Verifies:** macros.AC3.4, macros.AC3.5.

**Files:**
- Create: `test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl` and `test/test-models/tests/macro_arrayed/output.tab`
- Modify: `src/simlin-engine/tests/simulate.rs` (wire the new fixture; add the focused arrayed-stock test)
- Possibly modify: `src/simlin-engine/src/builtins_visitor.rs` — only if a gap is found (see below)

**Implementation:** This task is primarily **verification**. Phase 3 made both `BuiltinVisitor::walk()` and `contains_stdlib_call` macro-aware, and the `instantiate_implicit_modules` apply-to-all path (`builtins_visitor.rs:588-688`) builds one independent subscript-suffixed synthetic module per dimension element with no stdlib coupling beyond that recognition step. So an arrayed macro invocation should already expand into one independent macro instance per element. Author the fixtures and tests; **if a test surfaces a real gap** (something arrayed-macro-specific that does not ride the existing path), fix it in `builtins_visitor.rs` and document what was missing.

1. **Author the arrayed fixture.** `test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl`: a **stockless** macro invoked apply-to-all over a small dimension — e.g. a dimension `Region` with 2-3 elements, a macro `:MACRO: SCALE(x, k)` with body `SCALE = x * k`, an arrayed input `inp[Region]` with a different constant per element, and an arrayed invocation `out[Region] = SCALE(inp[Region], factor)`. Stockless ⇒ constant output ⇒ trivial hand-computed `output.tab` with one column per `out[Region]` element. Wire into `simulate.rs` as `simulates_macro_arrayed_mdl`.

2. **Author the arrayed-stock focused test (inline `.mdl`).** AC3.5's per-element-independent-stock case is hard to express as a trivial `output.tab` file, so test it as an inline-`.mdl` test in `simulate.rs` (`open_vensim` + `compile_vm` + run + hand-computed assertions): a stock-bearing macro `:MACRO: ACCUM(rate)` with body `ACCUM = INTEG(rate, 0)`, an arrayed invocation `total[Region] = ACCUM(rate[Region])` where `rate[Region]` differs per element. Over the run, assert each `total[element]` integrates *its own* `rate[element]` independently (e.g. with `rate = [1, 3]` over 4 steps at dt=1: `total[r1] = 0,1,2,3,4` and `total[r2] = 0,3,6,9,12`) — proving each element got its own persistent stock.

**Testing:**
- **macros.AC3.4** (via the wired `simulate.rs` fixture test): `simulates_macro_arrayed_mdl` passes — each `out[Region]` element equals `inp[element] * factor`, confirming one independent macro instance per dimension element. Additionally, a `convert/`- or expansion-level assertion that the arrayed invocation produced one synthetic `Variable::Module` per element (subscript-suffixed idents), not a single shared instance.
- **macros.AC3.5** (the inline arrayed-stock test): each element's stock integrates independently, as hand-computed — proving per-element persistent state.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate macro_arrayed`
Run: `cargo test -p simlin-engine --test simulate` (full file — confirm no regression)
Expected: all pass.

**Commit:** `engine: verify arrayed macro invocation`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Early validation against the multi-output and arrayed corpus models

**Verifies:** macros.AC3.3.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (add the metasd `THEIL` / `SSTATS` and C-LEARN macro-expansion tests)

**Implementation:** Tests only — early validation that the Phase 4 mechanisms work on real-world models (the comprehensive 14-model corpus harness and Vensim-reference-output comparison is Phase 7; this task is a focused early gate). These models are large, so `#[ignore]` the heavy ones with a documented opt-in command (`// Run with: cargo test --release -- --ignored <name>`) per `docs/dev/rust.md` test-time-budget rules; measure `Theil_2011.mdl` and keep it as a regular test only if it compiles+runs within the per-test budget.

**Testing:**
- **macros.AC3.3 — `THEIL`:** import `test/metasd/theil-statistics/Theil_2011.mdl` via `open_vensim`; assert the `THEIL` multi-output invocation materialized (the datamodel has a `Variable::Module` for the `THEIL` macro plus binding `Aux`es for the primary output and all 13 `:`-list outputs); compile via `compile_vm` and run to the end. The model "expands and simulates."
- **macros.AC3.3 — `SSTATS`:** same for `test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl` (note the spaces in the path). Assert both `SSTATS` invocations materialized (each: a `Variable::Module` + 1 primary + 10 additional binding auxes). Compile and run. If this large real-world COVID model turns out to have *unrelated, non-macro* blockers that prevent it reaching a runnable VM, narrow the assertion to "the `SSTATS` multi-output materialization succeeded and produced no macro-specific compile diagnostics" and file the unrelated blocker via the `track-issue` agent (it is then in scope for Phase 7's tiered corpus harness, not Phase 4).
- **C-LEARN macro expansion** (the design's Phase 4 "Done when: C-LEARN's four macros ... expand without macro-specific errors"): import `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`; assert its four macros (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`, `INIT`) imported as macro-marked `Model`s with correct `MacroSpec`s; compile far enough to confirm macro expansion succeeds — assert that the compile diagnostics (if any) contain **no macro-specific errors** (`UnknownBuiltin` for a macro name, `BadModelName` for a macro's model, an arity error on a macro call), even though C-LEARN has known unrelated blockers (circular dependencies, dimension mismatches, unit errors — explicitly out of scope per the design). This confirms the arrayed `[COP]` `SAMPLE UNTIL` / `RAMP FROM TO` / `SSHAPE` invocations expand. `#[ignore]` this test (C-LEARN is ~53k lines / 1.4 MB) with a documented opt-in command. Full C-LEARN reference-output validation is Phase 7.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate -- --ignored` (the gated corpus tests)
Run: `cargo test -p simlin-engine --test simulate` (the non-ignored subset, e.g. `Theil_2011.mdl` if it fits the budget)
Expected: the multi-output macros in `THEIL`/`SSTATS` materialize and the models expand and simulate; C-LEARN's macros expand with no macro-specific errors.

**Commit:** `engine: early-validate multi-output and arrayed macros against corpus models`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 4 completion check

When all three tasks are committed:
- A multi-output invocation (`total = add3(a,b,c : minv, maxv)`) materializes at import as an explicit `Variable::Module` plus binding `Aux`es — `total` receives the primary output, and the `:`-list names become model variables holding the additional outputs, referenceable by subsequent equations (the design's "Done when": new multi-output fixtures pass).
- An arrayed macro invocation expands into one independent macro instance per dimension element, including per-element persistent stocks (the design's "Done when": an arrayed-invocation fixture passes).
- The metasd multi-output models `THEIL` and `SSTATS` expand and simulate; C-LEARN's four macros — including the arrayed `[COP]` invocations — expand with no macro-specific errors (the design's "Done when").
- `macros.AC3.1`–`AC3.5` are verified.

XMILE round-trip is Phase 5; MDL export (including reconstructing the `:` call syntax from the materialized module) is Phase 6; the full corpus harness and Vensim-reference-output validation are Phase 7.
