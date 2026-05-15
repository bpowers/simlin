# Vensim Macro Support — Phase 6: MDL export and round-trip

**Goal:** Make the MDL writer emit `:MACRO: … :END OF MACRO:` blocks for macro-marked models and reconstruct multi-output invocations back into the `:` call syntax, then wire the macro fixtures into the `mdl_roundtrip.rs` harness so the full `.mdl` → datamodel → `.mdl` loop is verified.

**Architecture:** Three pieces. **(a)** `project_to_mdl` currently rejects any project with more than one model and any `Variable::Module`; both gates relax to allow macro-marked models and macro-module instances (while still rejecting ordinary multi-model projects and ordinary submodule instances — a general MDL module-export overhaul is out of scope). **(b)** The MDL writer emits each macro-marked `datamodel::Model` as a `:MACRO:` block — header from `MacroSpec`, body from the model's variables *minus* the synthesized port variables (the ports are reconstructed from the header parameter list, so emitting them as body equations would be redundant and would lose the `can_be_module_input` flag on re-import). Single-output invocations need no special work — they are already plain equation text, and the writer's RHS formatter passes unknown function calls through verbatim. **(c)** A multi-output invocation, materialized by Phase 4 as a `Variable::Module` plus binding `Aux`es, is detected and reconstructed into `total = add3(a, b, c : minv, maxv)` text; the cluster's module and binding auxes are suppressed from normal per-variable emission.

**Tech Stack:** Rust — `src/simlin-engine/src/mdl/writer.rs`, `src/simlin-engine/src/mdl/mod.rs`, and `src/simlin-engine/tests/mdl_roundtrip.rs`. The `:MACRO:` block syntax is documented in-repo at `docs/reference/vensim-macros.md`. No external dependencies.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 6 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC4: Round-trip and export
- **macros.AC4.1 Success:** A macro-bearing `.mdl` file round-trips with definitions emitted as `:MACRO:` blocks and invocations preserved.
- **macros.AC4.3 Success:** A multi-output macro round-trips through `.mdl` with the `:` call syntax reconstructed.

(`macros.AC4.2`, `macros.AC4.4`, `macros.AC4.5` are XMILE round-trip — Phase 5.)

---

## Current state (verified 2026-05-14)

**`project_to_mdl` rejects everything macro-related.** `src/simlin-engine/src/mdl/mod.rs:43-66`:
```rust
pub fn project_to_mdl(project: &Project) -> Result<String> {
    if project.models.len() != 1 {
        return Err(... "MDL format supports only a single model" ...);
    }
    let model = &project.models[0];
    for var in &model.variables {
        if matches!(var, Variable::Module(_)) {
            return Err(... "MDL format does not support Module variables" ...);
        }
    }
    let writer = MdlWriter::new();
    writer.write_project(project)
}
```
Both gates must relax for macro-marked models/modules. `compat::to_mdl` (`compat.rs:36-38`) is a one-line wrapper over `project_to_mdl` — fixing `project_to_mdl` fixes both.

**Pre-existing MDL-writer gaps (out of scope, already tracked).** The MDL writer today writes only the main model and drops *all* `Variable::Module` instances (`write_variable_entry`'s `Variable::Module(_) => return`). Phase 6 adds *macro-specific* writer support — `:MACRO:` blocks and multi-output reconstruction — but a general overhaul of MDL module export is **out of scope**. Two unrelated MDL serialization gaps found during the design investigation are tracked separately as GitHub **#538** and **#539**. An engineer who encounters one of these while wiring the round-trip harness should recognize it as pre-existing and tracked — *not* a regression introduced by macro support.

**The MDL writer (`src/simlin-engine/src/mdl/writer.rs`):**
- `write_project` (`writer.rs:2459-2470`) writes `{UTF-8}\n`, then `write_equations_section(model, project)` for `project.models[0]` only, then sketch + settings. `&datamodel::Project` is in scope here.
- `write_equations_section` (`writer.rs:2526-2595`) assembles: dimension defs → grouped variables (with `****` markers) → ungrouped variables (alphabetical) → `.Control` header + sim specs → `\\\---///` terminator. **No `:MACRO:` emission point exists.** `&Project` is in scope here too.
- `write_variable_entry` (`writer.rs:851-921`) dispatches per variable: `Variable::Stock` → `write_stock_variable`; `Variable::Module(_) => return` (line 867, emits nothing); `Flow`/`Aux` → `write_single_entry`. **It receives only `var` + `display_names` — not `&Project`/`&Model`** — so it cannot look up `module.model_name → macro_spec`. Multi-output reconstruction must therefore happen in `write_equations_section` (which has `&Project`).
- The RHS formatter: `equation_to_mdl` (`writer.rs:738-760`) parses the equation string to `Expr0` and walks it with `MdlPrintVisitor` (`expr0_to_mdl`, `writer.rs:622-736`). The `App` arm (`writer.rs:645-671`) → `xmile_to_mdl_function_name` (`writer.rs:194-216`), whose fall-through `_ => underbar_to_space(xmile_name).to_uppercase()` passes **unknown function calls through verbatim** (uppercased, `_`→space).
- Variable-entry emitters: `write_single_entry` (`writer.rs:1239-1292`), `write_stock_variable`/`write_stock_entry` (`writer.rs:978-1067`), `write_units_and_comment` (`writer.rs:1357-1366`, the `\n\t~\tunits\n\t~\tcomment\n\t|` trailer).

**Single-output macro invocations already round-trip.** A single-output invocation is an ordinary `Variable::Aux` with `equation: Scalar("mymacro(a, b)")`. `write_variable_entry` → `write_single_entry` → `equation_to_mdl` parses it (the parser parses *any* `ident(...)` as `Expr0::App` — no builtin-table validation) → `MdlPrintVisitor` emits `MYMACRO(a, b)` (uppercased). Pinned today by the `function_unknown_uppercased` writer test. So Phase 6 needs **no** special single-output-invocation code — only the `:MACRO:` definition emission and the relaxed `project_to_mdl` gate.

**The `:MACRO:` block syntax** (`docs/reference/vensim-macros.md`): a top-level block in the equation section. Single-output header `:MACRO: macroname(in1, in2)`; multi-output header `:MACRO: macroname(in1, in2, in3 : out1, out2)`; body = ordinary `name = rhs ~ units ~ comment |` equations; `:END OF MACRO:` terminator. Every well-formed `macro_*` fixture places its `:MACRO:` blocks **immediately after the `{UTF-8}` line, before the main model's equations and the `.Control` section** — so the writer should emit all `:MACRO:` blocks right after `{UTF-8}\n`. Multiple back-to-back `:MACRO:` blocks are allowed.

**The materialized multi-output cluster** (Phase 4, per `phase_04.md` Task 1): `total = add3(a, b, c : minv, maxv)` becomes a `Variable::Module` (deterministic serialization-stable `ident`, `model_name` = the macro's `Model.name`, input-only `ModuleReference`s with `dst = "{module_ident}.{param}"`, `src` = the arg) plus binding `Aux`es — a primary-output aux `Aux { ident: <lhs>, equation: Scalar("{module_ident}.{primary_output}") }` and one additional-output aux per `:`-list entry `Aux { ident: <output_binding>, equation: Scalar("{module_ident}.{additional_output}") }`. Reconstruction is unambiguous **only with the macro model's `MacroSpec` as a cross-reference** (to classify each binding aux as primary vs additional and to recover positional arg order — `Module.references` ordering is not guaranteed positional, so each `dst`'s post-`.` segment must be matched against `MacroSpec.parameters[i]`). `Module` and `ModuleReference` are at `datamodel.rs:344-361`.

**`mdl_roundtrip.rs`:**
- Header comment (`mdl_roundtrip.rs:12-22`) lists excluded categories, first being `macros (:MACRO:) -- the writer rejects them`.
- `TEST_MDL_MODELS` (`mdl_roundtrip.rs:23-82`) — a flat `static &[&str]` of repo-relative paths; `resolve_path` prepends `../../`.
- `mdl_to_mdl_roundtrip()` (`mdl_roundtrip.rs:245-295`) drives `parse_mdl → project_to_mdl → parse_mdl → assert_semantic_equivalence`.
- `assert_semantic_equivalence` / `assert_model_equivalence` (`mdl_roundtrip.rs:134-214`) compares: `sim_specs`, `dimensions`, model **count**, and per-model (paired by **zip index**) variable count + per-variable (sorted by ident) name + equation text. **It does NOT compare `Model.macro_spec`, `Model.name`, groups, views, variable type, `Compat`, or `ModuleReference`s** — so after Phase 1 it neither breaks on the new `macro_spec` field nor verifies it.
- Writer tests live in `src/simlin-engine/src/mdl/writer_tests.rs` (`assert_mdl` for equation-text tests; `make_aux`/`make_model`/`make_project` for full-project tests). `project_to_mdl_rejects_multiple_models` and `project_to_mdl_rejects_module_variable` (`writer_tests.rs:1280-1314`) assert the current reject-everything behavior and must be rewritten to reject only the *non-macro* cases.

**Fixtures:** all 6 `test/test-models/tests/macro_*` directories have a `.mdl` file. Phase 4 authors `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl` (the `:`-multi-output fixture) and `test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl` (a stockless apply-to-all invocation — note: an arrayed *single-output* invocation stays as `ApplyToAll` equation text, not a materialized module, so it round-trips via the ordinary equation-text path, not Task 2's reconstruction).

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: MDL macro export and round-trip

<!-- START_TASK_1 -->
### Task 1: Emit `:MACRO:` blocks for macro-marked models

**Verifies:** macros.AC4.1 (single-output: definitions emitted as `:MACRO:` blocks, invocations preserved).

**Files:**
- Modify: `src/simlin-engine/src/mdl/mod.rs` (`project_to_mdl` ~lines 43-66 — relax the `models.len()` gate)
- Modify: `src/simlin-engine/src/mdl/writer.rs` (`write_project` ~lines 2459-2470 and/or `write_equations_section` ~lines 2526-2595 — add `:MACRO:` block emission)
- Modify: `src/simlin-engine/src/mdl/writer_tests.rs` (rewrite `project_to_mdl_rejects_multiple_models` ~line 1280; add `:MACRO:`-emission writer tests)

**Implementation:**

1. **Relax the `models.len()` gate in `project_to_mdl`.** Instead of rejecting `project.models.len() != 1`, reject only when there is more than one **non-macro** model (`project.models.iter().filter(|m| m.macro_spec.is_none()).count() != 1`). Ordinary multi-model XMILE projects are still rejected (MDL has no general multi-model representation); macro-marked extra models are allowed.

2. **Emit `:MACRO:` blocks.** In `write_project` (or at the top of `write_equations_section`), after `{UTF-8}\n` and **before** the dimension definitions / main-model variables, iterate `project.models` in order and, for each model with `macro_spec.is_some()`, emit a `:MACRO:` block:
   - **Header:** `:MACRO: <macro name>(<param>, <param>, ...)` from `MacroSpec.parameters`. If `MacroSpec.additional_outputs` is non-empty, emit the multi-output header form `:MACRO: <name>(<params> : <additional_outputs>)`. Use the MDL display-name form for the macro name and parameter names (the writer's existing `format_mdl_ident` / `display_name_for_ident` / `underbar_to_space` helpers — match how the main model's variable names are emitted).
   - **Body:** for each variable in the macro `Model.variables` whose ident is **not** in `MacroSpec.parameters`, emit it via the existing `write_variable_entry`. This reuses the existing Stock/Flow/Aux emission verbatim — a macro-body stock round-trips the same way a main-model stock does; a macro body that calls another macro is an `Aux` whose equation is plain call text and round-trips like any unknown-function call. **Excluding the `MacroSpec.parameters` variables is required:** those are Phase 2's synthesized port variables; they are reconstructed from the header on re-import, and emitting them as `<param> = 0` body equations would make the re-imported macro treat them as ordinary body variables (losing `compat.can_be_module_input` and diverging from the original).
   - **Terminator:** `:END OF MACRO:`.
   - Emit the macro blocks in a **deterministic order** (`project.models` order — stable across passes, which the round-trip harness's zip-index model pairing relies on).
   - Single-output invocations in the main model need no special handling — they are already equation text and the existing per-variable emission handles them.

3. **Rewrite `project_to_mdl_rejects_multiple_models`** so it asserts an ordinary two-*non-macro*-model project is still rejected, and add a positive case: a project with one `main` model + one macro-marked model is accepted and produces a `:MACRO:` block.

**Testing:**
- **macros.AC4.1** (writer unit tests in `writer_tests.rs`, full-project style): build a `datamodel::Project` with a `main` model and a single-output macro-marked `Model` (with `MacroSpec` and synthesized port variables) → `project_to_mdl` → assert the output contains a `:MACRO: <NAME>(<params>)` header, the body equation(s), `:END OF MACRO:`, and that the synthesized port variables do **not** appear as body equations; assert the main model's invocation equation is preserved as call text.
- A macro with a multi-equation body (a helper variable) and a macro with a body stock both emit correct `:MACRO:` blocks.
- The rewritten rejection test passes (ordinary multi-model still rejected; macro-marked extra model accepted).

**Verification:**
Run: `cargo test -p simlin-engine mdl::writer`
Expected: all pass, including the new `:MACRO:`-emission tests.

**Commit:** `engine: emit :MACRO: blocks from the MDL writer`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Reconstruct multi-output invocations

**Verifies:** macros.AC4.3.

**Files:**
- Modify: `src/simlin-engine/src/mdl/mod.rs` (`project_to_mdl` ~lines 43-66 — relax the `Variable::Module` gate)
- Modify: `src/simlin-engine/src/mdl/writer.rs` (`write_equations_section` ~lines 2526-2595 — detect and reconstruct the multi-output cluster)
- Modify: `src/simlin-engine/src/mdl/writer_tests.rs` (rewrite `project_to_mdl_rejects_module_variable` ~line 1300; add multi-output-reconstruction tests)

**Implementation:**

1. **Relax the `Variable::Module` gate in `project_to_mdl`.** Reject a `Variable::Module` only when its `model_name` does **not** resolve to a macro-marked model in `project.models`. An ordinary submodule instance is still rejected (a general MDL module-export overhaul is out of scope); a macro-module instance materialized by Phase 4 is allowed.

2. **Detect and reconstruct the multi-output cluster** in `write_equations_section` (it has `&Project`). Before the per-variable emission loop:
   - Build a `name → &MacroSpec` lookup over `project.models` (the macro-marked models).
   - For each `Variable::Module` in the main model whose `model_name` resolves to a macro-marked model: look up its `MacroSpec`. Recover the call arguments — for each `ModuleReference`, the `dst` is `{module_ident}.{param_name}`; match each `param_name` against `MacroSpec.parameters[i]` to get positional order, and the `src` is the argument's name. Collect the binding auxes — scan the main model's `Aux` variables for a `Scalar` equation that is exactly a module-output reference into this module instance (`{module_ident}.{output}` — an **ASCII period**: per Phase 4's authoritative separator note, the materialized binding-aux equation text is the datamodel `.` form, *not* the canonical `·`, since `project_to_mdl` operates on datamodel equation strings). Classify each binding aux: the one whose referenced output equals `MacroSpec.primary_output` is the LHS variable; the ones matching `MacroSpec.additional_outputs[j]` are the `:`-list output bindings in order `j`.
   - Emit the reconstructed invocation: `<lhs ident> = <MACRO NAME>(<arg1>, ..., <argN> : <addout_binding_1>, ..., <addout_binding_M>)` followed by the standard `~ units ~ comment |` trailer (use the primary-output binding aux's units/documentation). Use the existing MDL ident-formatting helpers.
   - **Suppress** the `Variable::Module` and all of its binding auxes from the normal per-variable emission loop (collect their idents into a skip-set the loop checks). `write_variable_entry`'s existing `Variable::Module(_) => return` stays as a harmless fallback.

3. **Rewrite `project_to_mdl_rejects_module_variable`** so it asserts an *ordinary* (non-macro) `Variable::Module` is still rejected, and add a positive case: a `Variable::Module` whose `model_name` is a macro-marked model is accepted and reconstructed as a `:` invocation.

**Testing:**
- **macros.AC4.3** (writer unit tests, full-project style): build a `datamodel::Project` with a multi-output macro-marked `Model` (non-empty `MacroSpec.additional_outputs`) and a materialized multi-output cluster (a `Variable::Module` + a primary-output binding aux + additional-output binding auxes, exactly as Phase 4 produces) → `project_to_mdl` → assert the output contains the reconstructed `<lhs> = <MACRO>(<args> : <bindings>)` `:` call syntax with the arguments in positional order and the output bindings in `:`-list order, and that the `Variable::Module` and binding auxes do **not** appear as separate `<module>`/aux entries.
- The rewritten rejection test passes (ordinary module still rejected; macro-module accepted).

**Verification:**
Run: `cargo test -p simlin-engine mdl::writer`
Expected: all pass, including the multi-output-reconstruction tests.

**Commit:** `engine: reconstruct multi-output macro invocations in the MDL writer`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Wire macro fixtures into the `mdl_roundtrip.rs` harness

**Verifies:** macros.AC4.1, macros.AC4.3 (the full `.mdl` → datamodel → `.mdl` round-trip).

**Files:**
- Modify: `src/simlin-engine/tests/mdl_roundtrip.rs` (remove the macro-exclusion comment line; add the 8 fixtures to `TEST_MDL_MODELS`; extend `assert_model_equivalence`)

**Implementation:** Tests + harness only — if the round-trip surfaces a real writer gap, fix it in `writer.rs`/`mod.rs`, but after Tasks 1-2 the expectation is that macro fixtures round-trip.

1. **Extend `assert_model_equivalence`** (`mdl_roundtrip.rs`) to also compare `Model.macro_spec` (via `PartialEq` — `MacroSpec` derives it) and `Model.name`. Without this, the harness would pass even if the writer dropped a `:MACRO:` block entirely, because the body variables live in the macro model and a missing model would only change the model *count* (which is checked) — but a *malformed* `MacroSpec` would slip through. Comparing `macro_spec` makes the macro round-trip actually verified. Confirm the existing non-macro fixtures still pass (the `main` model is named `"main"` and has `macro_spec: None` on both passes, so the new comparisons are no-ops for them).

2. **Remove** the `//   - macros (:MACRO:) -- the writer rejects them` line from the `TEST_MDL_MODELS` header comment (`mdl_roundtrip.rs:12-22`).

3. **Add the 8 macro fixtures to `TEST_MDL_MODELS`:** the 6 bundled `test/test-models/tests/macro_*/test_macro_*.mdl` fixtures plus the 2 Phase-4-authored fixtures `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl` and `test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl`. Each then flows through `mdl_to_mdl_roundtrip()`'s `parse_mdl → project_to_mdl → parse_mdl → assert_semantic_equivalence` automatically.

**Testing:**
- **macros.AC4.1:** the 6 `macro_*` fixtures plus `macro_arrayed` round-trip — `parse_mdl → project_to_mdl → parse_mdl` yields a semantically equivalent project (same models, same `MacroSpec`s, same variables/equations). `macro_cross_reference` (one macro calls another) and `macro_multi_macros` (two macros) exercise multiple `:MACRO:` blocks; `macro_stock` exercises a macro-body stock; `macro_trailing_definition` exercises a macro block emitted before its invocation.
- **macros.AC4.3:** `macro_multi_output` round-trips — the `:` multi-output call syntax is reconstructed and re-parses to the same materialized cluster (`Variable::Module` + binding auxes) and the same macro `MacroSpec` with non-empty `additional_outputs`.

**Verification:**
Run: `cargo test -p simlin-engine --test mdl_roundtrip`
Expected: `mdl_to_mdl_roundtrip` passes with all 8 macro fixtures included; no regression in the existing non-macro fixtures.

**Commit:** `engine: wire macro fixtures into the MDL round-trip harness`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 6 completion check

When all three tasks are committed:
- `project_to_mdl` accepts macro-bearing projects (macro-marked extra models and macro-module instances) while still rejecting ordinary multi-model projects and ordinary submodule instances.
- The MDL writer emits each macro definition as a `:MACRO: … :END OF MACRO:` block (header from `MacroSpec`, body from the model's non-port variables); single-output invocations are preserved as equation text; multi-output invocations are reconstructed into the `:` call syntax from the materialized `Variable::Module` + binding auxes.
- The `mdl_roundtrip.rs` macro exclusion is removed, all 6 bundled `macro_*` fixtures plus the 2 Phase-4 fixtures round-trip, and `assert_model_equivalence` now verifies `macro_spec` (the design's "Done when": `.mdl` round-trip preserves every macro fixture; multi-output reconstruction is verified; the exclusion is removed).
- `macros.AC4.1` and `macros.AC4.3` are verified.

The hero-model and corpus validation, and the diagram/consumer integration, are Phase 7.
