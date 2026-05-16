# Vensim Macro Support — Phase 7: Hero validation, corpus, and consumer integration

**Goal:** Validate the macro implementation against the C-LEARN hero model and the metasd corpus, confirm the bundled fixture suite is wired and green, and make the `src/diagram` consumer macro-aware (filter macro-marked models out of the model-reference list; confirm macro-bearing projects open without crashing).

**Architecture:** Three independent strands. **(a) C-LEARN:** C-LEARN's four macros must parse, register, and expand with no macro-specific errors (verified by compiling C-LEARN and inspecting diagnostics — C-LEARN's *non-macro* blockers are explicitly out of scope), and focused isolation models invoking each invoked macro (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`) with known inputs must compute the macro's defined behavior. **(b) metasd corpus:** a tiered harness over the 14 macro-using metasd models — an *expansion tier* (every model expands its macros with no macro-attributable diagnostics, runnable for all 14 with no prerequisites) and a *simulation tier* (full simulation matches Vensim DSS reference output, runnable only for models that both have a checked-in reference output and have no unrelated blockers). **(c) diagram:** macro-marked models are ordinary entries in `project.models` after import, so the diagram receives them automatically through `projectFromJson` — the work is to skip them in `getAvailableModels` (so they never appear as a selectable module-reference target) and to confirm a macro-bearing project opens and renders without throwing.

**Tech Stack:** Rust — `src/simlin-engine/tests/`. TypeScript — `src/diagram/`. The Vensim DSS reference outputs for newly-authored fixtures are a documented prerequisite (a setup task, not implementation work — per the design's "Test prerequisites" note); C-LEARN's reference output (`test/xmutil_test_models/Ref.vdf`) already exists in-repo.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 7 of 7 — the final phase.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC6: Validation corpus and consumer floor
- **macros.AC6.1 Success:** All six `test/test-models/tests/macro_*` fixtures are wired into the active test suite and pass.
- **macros.AC6.2 Success:** C-LEARN's macros (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`, `INIT`) parse, register, and expand with no macro-specific errors.
- **macros.AC6.3 Success:** Focused models invoking C-LEARN's `SAMPLE UNTIL`, `SSHAPE`, and `RAMP FROM TO` with known inputs match Vensim DSS reference output.
- **macros.AC6.4 Success:** All 14 macro-using metasd models pass the expansion tier; those without unrelated blockers match Vensim DSS reference output.
- **macros.AC6.5 Success:** The diagram opens every macro-bearing fixture without crashing.
- **macros.AC6.6 Success:** Macro-marked models don't appear as standalone, navigable models in the diagram's model list.

---

## Current state (verified 2026-05-14)

**C-LEARN:** `test/xmutil_test_models/` contains `C-LEARN v77 for Vensim.mdl` (1.4 MB; macros `SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO` — all invoked, some arrayed over `[COP]` — plus `INIT`, defined but never invoked) **and `Ref.vdf` (1.86 MB, the Vensim reference output)**. `simulates_clearn()` (`tests/simulate.rs:908-938`) is `#[ignore]`d; it already does the full path — `open_vensim` → `compile_vm` → `Vm::run_to_end` → `VdfFile::parse("../../test/xmutil_test_models/Ref.vdf").to_results_via_records()` → `ensure_vdf_results` — and its `#[ignore]` comment says the blocker is macro expansion. The design explicitly scopes C-LEARN's *non-macro* blockers (circular dependencies, dimension mismatches, unit errors) and *full* end-to-end C-LEARN simulation **out**, "tracked separately."

**metasd:** `test/metasd/` has 15 subdirectories; the 14 that contain `:MACRO:`-using `.mdl` files (per a repo-wide `:MACRO:` grep) are `bathtub-statistics`, `beer-game`, `covid19-us-homer`, `critical-slowing`, `early-warnings-catastrophe`, `FREE`, `industrial-dynamics`, `interpolating-arrays`, `pink-noise`, `scientific-revolution`, `social-network-valuation`, `theil-statistics`, `thyroid-dynamics`, `wonderland`. **Most have only `.mdl` + `PROVENANCE.md` — no checked-in reference output.** Only `social-network-valuation/` (6 `.vdf` files), `FREE/` (`all_data2.vdf`), and `WRLD3-03/` (the non-macro 15th dir) carry `.vdf`s. The only existing `test/metasd/` test is `simulates_wrld3_03` (`simulate.rs:~865`); there is no corpus loop.

**Corpus-harness patterns:** the established template is a `static &[&str]` path list + a loop test that accumulates `Vec<(String, String)> failures` and asserts it empty — `incremental_compilation_covers_all_models` (`simulate.rs:~970`), `mdl_to_mdl_roundtrip` (`mdl_roundtrip.rs:245`). Failing/excluded entries are commented out with multi-line reason comments (`TEST_SDEVERYWHERE_MODELS`, `simulate.rs:~531`). Comparison: `ensure_results` / `ensure_vdf_results` (`tests/test_helpers.rs`) — absolute `2e-3`, or relative `max_val * 5e-6` floored at `2e-3` for Vensim-sourced data; `$⁚`-prefixed implicit-module internals are skipped. The `../../test/...` path prefix is relative to `src/simlin-engine/`.

**Diagnostic API:** `collect_all_diagnostics(&db, &sync) -> Vec<Diagnostic>` (`db.rs:~2255`); `Diagnostic { model, variable: Option<String>, error: DiagnosticError, severity }`; `DiagnosticSeverity` ∈ `{ Error, Warning }`. `roundtrip.rs` is the precedent for "compile, collect diagnostics, inspect them." There is no macro-specific `ErrorCode` variant — after Phases 1-6 a *correctly* macro-using model produces **zero** macro-attributable diagnostics, so a macro-attributable diagnostic is identified by its `variable`/equation referencing a known macro name or macro-instance, or by being a registry-build error.

**`src/diagram` model list:** `getAvailableModels(project, currentModelName)` (`src/diagram/module-details-utils.ts:69-112`) returns `{ projectModels, stdlibModels }` — two flat `string[]` lists. Its loop (lines 91-109) over `project.models.keys()` already filters self-references, cycle-creating models, and `STDLIB_PREFIX`-prefixed names — **but has no macro filter**. Its **sole consumer** is `ModuleDetails.tsx:109`, which renders the `<select data-testid="model-ref-select">` module-reference dropdown. `module-navigation.ts` holds `STDLIB_MODEL_NAMES`, `STDLIB_PREFIX = 'stdlib\u{205A}'`, `isStdlibModel()` (the precedent for model-name-based gating), and the drill-in `modelStack` navigation helpers — there is **no other flat model-list UI**; navigation is a drill-in stack. After Phase 1, the `@simlin/core` `Model` type carries `macroSpec?` and the diagram receives it through `projectFromJson` automatically (`Editor.tsx:538`, `engine.serializeJson(undefined, true)`).

**Diagram "opens without crashing":** the open path is `Editor.openInitialProject`/`openEngineProject` → `projectFromJson(...)` → `setState({ activeProject })` → `render()` → `getCanvas()` → `<Canvas project model={getModel()} view={getView()} />`. Macro-marked models sit in `project.models` but are not rendered unless navigated to (which AC6.6 prevents). The established test pattern is `src/diagram/tests/editor-open-project.test.ts` — `Object.create(Editor.prototype)`, a `makeFakeEngine` providing `serializeJson`, a `makeEditor(props)` helper, and assertions that `openEngineProject()` resolves with `state.activeProject` defined (never a thrown exception). Diagram tests: Jest + ts-jest + jsdom; pure-logic tests in `module-details-utils.test.ts` (`@jest-environment node`, `makeAux`/`makeModel`/`makeProject` helpers); component tests use `@testing-library/react`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: Validation corpus and consumer integration

<!-- START_TASK_1 -->
### Task 1: C-LEARN hero validation

**Verifies:** macros.AC6.2, macros.AC6.3.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (the C-LEARN macro-expansion test; the `simulates_clearn()` `#[ignore]` comment; wire the focused-model tests)
- Create: focused `.mdl` fixtures + their expected-output files under `test/test-models/tests/macro_clearn_*/` (one directory per C-LEARN macro)

**Implementation:**

1. **macros.AC6.2 — C-LEARN macro expansion.** Add an `#[ignore]`d test (C-LEARN is 1.4 MB — `#[ignore]` per `docs/dev/rust.md` test-time-budget rules, with a documented opt-in command) that `open_vensim`s `../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`, asserts its four macros (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`, `INIT`) imported as macro-marked `Model`s with correct `MacroSpec`s, syncs + compiles via the salsa path, and collects diagnostics (`collect_all_diagnostics`). Assert **no diagnostic is macro-attributable** — none reference a macro name or a macro-instance variable, and none are macro-registry-build errors. C-LEARN's known *non-macro* blockers (circular dependencies, dimension mismatches, unit errors) are expected and explicitly allowed — the assertion is specifically that macro handling introduced no errors. Per Phase 3's documented-limitation note, a **non-time `$` reference** (`FOO$` for a non-time variable — deprioritized and unsupported) surfaces as an ordinary unknown-variable / unresolved-reference diagnostic; it is *not* macro-attributable (no macro name, no macro-instance variable, not a registry-build error), so if C-LEARN contains one it is an allowed non-macro blocker like the others, not a test failure — the macro-attributable classifier must not mistake it for a macro error.

2. **macros.AC6.3 — focused C-LEARN-macro models.** For each of the three *invoked* C-LEARN macros (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`), author a small focused `.mdl` model that defines that macro (copy its `:MACRO:` block verbatim from C-LEARN's `.mdl`) and invokes it with **known constant inputs**, plus an expected-output file (`output.tab`). Compute the expected output by applying the macro's body formula (read from the copied `:MACRO:` block) to the known inputs, documenting the arithmetic in the fixture's `README.md`. Wire each as a `simulate_mdl_path` test in `simulate.rs`. **Prefer a Vensim DSS reference `.vdf`** if one is provided (a prerequisite setup task per the design's "Test prerequisites" note) — if a reference `.vdf` is present alongside the focused `.mdl`, compare against it via `ensure_vdf_results`; otherwise the formula-derived `output.tab` is the gate. (`INIT` is not invoked anywhere in C-LEARN and needs no focused model — AC6.2's "parse, register, and expand" covers it; this matches the design's `macros.AC1.7` "defined but never invoked" case.)

3. **Update `simulates_clearn()`.** Its `#[ignore]` comment currently blames macro expansion. After Phases 1-6, macro expansion works — update the comment to reflect that the macros now expand and that full C-LEARN simulation is blocked (if at all) only on the design's explicitly-out-of-scope non-macro issues. If C-LEARN in fact fully simulates against `Ref.vdf` after Phases 1-6, un-`#[ignore]` it; if non-macro blockers remain, keep it `#[ignore]`d, update the comment to name them, and ensure they are tracked (spawn the `track-issue` agent — the design says these are "tracked separately").

**Testing:**
- **macros.AC6.2:** the C-LEARN macro-expansion test passes — the four macros import as macro-marked models and the compile produces no macro-attributable diagnostics.
- **macros.AC6.3:** the three focused `macro_clearn_*` fixture tests pass — each invokes a C-LEARN macro with known inputs and matches the computed (or Vensim-DSS-reference) output.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate -- --ignored` (the C-LEARN expansion test)
Run: `cargo test -p simlin-engine --test simulate macro_clearn` (the focused-model tests)
Expected: all pass.

**Commit:** `engine: validate C-LEARN macros against reference behavior`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: metasd corpus harness and fixture-suite confirmation

**Verifies:** macros.AC6.1, macros.AC6.4.

**Files:**
- Create: `src/simlin-engine/tests/metasd_macros.rs` (a new integration-test file — declared automatically by Cargo as a `tests/` file)
- Possibly modify: `src/simlin-engine/tests/simulate.rs` (only if the AC6.1 confirmation finds a `macro_*` fixture not yet wired)

**Implementation:**

1. **macros.AC6.1 — confirm the bundled fixture suite.** Verify that all six `test/test-models/tests/macro_*` fixtures are wired into the active test suite and passing: the `.mdl` variants in `simulate.rs` (Phase 3) and `mdl_roundtrip.rs` (Phase 6), and the four `.xmile` variants in `simulate.rs` (Phase 5). This is a confirmation step — run the suite and inspect; if any fixture is not wired (a gap left by an earlier phase), wire it. No new harness is needed for AC6.1 — it is satisfied by the earlier phases' wiring; this task makes the satisfaction explicit and complete.

2. **macros.AC6.4 — the tiered metasd corpus harness.** In the new `metasd_macros.rs`, build a corpus list of **every** macro-using `.mdl` file under `test/metasd/`. A `:MACRO:` grep finds **17 files across the 14 macro-using directories** — 12 directories contribute one file each, plus `scientific-revolution` (`scirev7.mdl` *and* `scirev8.mdl`) and `social-network-valuation` (`groupon 1.mdl`, `groupon 2.mdl`, `groupon 3.mdl`). The full list (paths relative to repo root):
   - `test/metasd/bathtub-statistics/integration3.mdl`
   - `test/metasd/beer-game/RealBeer4-Sterman13.mdl`
   - `test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl`
   - `test/metasd/critical-slowing/critical-slowing.mdl`
   - `test/metasd/early-warnings-catastrophe/catastropeWarning2.mdl`
   - `test/metasd/FREE/FREE6/FREE6-original/free 6.mdl`
   - `test/metasd/industrial-dynamics/IDch15/IDch15d.mdl`
   - `test/metasd/interpolating-arrays/InterpolatingArrays.mdl`
   - `test/metasd/pink-noise/PinkNoise2010.mdl`
   - `test/metasd/scientific-revolution/scirev7.mdl`, `test/metasd/scientific-revolution/scirev8.mdl`
   - `test/metasd/social-network-valuation/groupon 1.mdl`, `test/metasd/social-network-valuation/groupon 2.mdl`, `test/metasd/social-network-valuation/groupon 3.mdl`
   - `test/metasd/theil-statistics/Theil_2011.mdl`
   - `test/metasd/thyroid-dynamics/thyroid-2008-d.mdl`
   - `test/metasd/wonderland/Wonderland3.mdl`

   (AC6.4's "all 14 macro-using metasd models" maps to these 14 *directories* — every directory is represented, and the multi-file directories contribute all their macro-using files, so the harness covers 17 files.) Each list entry is annotated with its tier status — model the annotation as a small struct or as parallel commented sections, in the style of `TEST_SDEVERYWHERE_MODELS`. Two tiers:
   - **Expansion tier (all 14):** for each model, `open_vensim` → sync → compile → `collect_all_diagnostics`; assert **no macro-attributable diagnostic** (a failed macro-call resolution, a missing macro model, a macro arity error, or a registry-build error). A model with *unrelated, non-macro* blockers still passes the expansion tier as long as none of its diagnostics are macro-attributable. Accumulate `(model, macro-diagnostic)` failures and assert the failure vec is empty.
   - **Simulation tier (subset):** for each model that **both** has a checked-in Vensim reference output (a sibling `.vdf` / `output.tab` / `.dat`) **and** has no unrelated blockers, additionally run the VM to completion and compare against the reference via `ensure_vdf_results` / `ensure_results`. Annotate every model that is *not* simulation-tier-eligible with its reason (`no reference output checked in` — a documented prerequisite per the design; or `unrelated blocker: <description>` — file the blocker via the `track-issue` agent). The harness asserts: all 14 pass the expansion tier; every simulation-tier-eligible model matches its reference output.
   - Large metasd models (e.g. `covid19-us-homer`) should be `#[ignore]`d individually or the whole harness `#[ignore]`d with a documented opt-in command if it exceeds the per-test budget; measure and decide per `docs/dev/rust.md`.

**Testing:**
- **macros.AC6.1:** confirmed — all six `macro_*` fixtures are wired (`.mdl` in `simulate.rs` + `mdl_roundtrip.rs`, `.xmile` in `simulate.rs`) and the suite is green.
- **macros.AC6.4:** the expansion-tier test passes for all 14 macro-using metasd models (no macro-attributable diagnostics); the simulation-tier test passes for every simulation-tier-eligible model; every non-eligible model is annotated with a documented reason.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate` (AC6.1 — the wired `macro_*` fixtures)
Run: `cargo test -p simlin-engine --test mdl_roundtrip` (AC6.1 — the round-trip wiring)
Run: `cargo test -p simlin-engine --test metasd_macros` (plus `-- --ignored` if the heavy models are gated)
Expected: all pass; the metasd expansion tier is green for all 14 models.

**Commit:** `engine: add the metasd macro corpus harness`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Make the diagram macro-aware

**Verifies:** macros.AC6.5, macros.AC6.6.

**Files:**
- Modify: `src/diagram/module-details-utils.ts` (`getAvailableModels` ~lines 91-109 — add the macro filter)
- Modify: `src/diagram/module-navigation.ts` (add an `isMacroModel` helper, mirroring `isStdlibModel`)
- Modify: `src/diagram/tests/module-details-utils.test.ts` (add `getAvailableModels` macro-filter tests; extend the `makeModel` helper to accept an optional `macroSpec`)
- Modify: `src/diagram/tests/editor-open-project.test.ts` (add a macro-bearing-project open test)

**Implementation:**

1. **macros.AC6.6 — filter macro-marked models from the model list.** In `getAvailableModels`'s loop over `project.models.keys()` (`module-details-utils.ts:91-109`), skip any `name` whose `project.models.get(name)?.macroSpec` is set (a macro-marked model must never appear as a selectable module-reference target). Add an `isMacroModel(model: Model): boolean` helper to `module-navigation.ts` next to `isStdlibModel` — `model.macroSpec !== undefined` — and use it in the filter, so the macro check is a named, reusable predicate. (After Phase 1, the `@simlin/core` `Model` type has the optional `macroSpec` field; `module-details-utils.ts` already imports `Model` from `@simlin/core/datamodel`.) Confirm `getAvailableModels` is the only model-*list* surface — the investigation found navigation is otherwise a drill-in stack with no flat list; if any other model-list site exists, filter it there too.

2. **macros.AC6.5 — confirm macro-bearing projects open without crashing.** Macro-marked models are ordinary `project.models` entries, so the open path (`projectFromJson` → `setState({ activeProject })` → render) should handle them transparently — but verify it. No production change is expected here; if a test surfaces a real crash (e.g. a model-list-rendering site that does not tolerate a `macroSpec` field, or the editor choking on a macro model's synthesized port variables / materialized `Variable::Module`), fix it minimally.

**Testing:**
- **macros.AC6.6** (`module-details-utils.test.ts`, pure-logic): extend `makeModel` to take an optional `macroSpec`; build a `project` whose `models` include a `main` model, an ordinary submodel, and a macro-marked model; assert `getAvailableModels(project, 'main').projectModels` contains the ordinary submodel but **not** the macro-marked model. Add an `isMacroModel` unit test.
- **macros.AC6.5** (`editor-open-project.test.ts`, following the existing `Object.create(Editor.prototype)` + `makeFakeEngine` + `openEngineProject` pattern): construct a `validProjectJson` whose `models` array includes a macro-marked model (with a `macroSpec` and synthesized port variables) alongside a `main` model; assert `openEngineProject()` resolves, `state.activeProject` is defined, and no exception is thrown. Optionally add a `ModuleDetails` component test (`@testing-library/react`) asserting the macro-marked model name does not appear in the rendered `<select data-testid="model-ref-select">`.

**Verification:**
Run: `pnpm build` (rebuilds `@simlin/core`/`@simlin/engine` so the `macroSpec` field is visible to `@simlin/diagram`)
Run: `pnpm --filter @simlin/diagram test`
Run: `pnpm tsc`
Expected: all pass — macro-marked models are filtered from `getAvailableModels`, and a macro-bearing project opens without crashing.

**Commit:** `diagram: filter macro-marked models from the model list`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 7 completion check

When all three tasks are committed:
- C-LEARN's four macros parse, register, and expand with no macro-specific errors; focused models invoking `SAMPLE UNTIL`, `SSHAPE`, and `RAMP FROM TO` with known inputs compute the macros' defined behavior (the design's "Done when": C-LEARN's macros validate against reference output).
- A tiered metasd corpus harness passes the expansion tier for all 14 macro-using models and the simulation tier for every model with a checked-in reference output and no unrelated blockers; non-eligible models are annotated with documented reasons (the design's "Done when").
- All six bundled `test/test-models/tests/macro_*` fixtures are confirmed wired into `simulate.rs` and `mdl_roundtrip.rs` and passing.
- The diagram filters macro-marked models out of `getAvailableModels` so they never appear as a selectable module-reference target, and a macro-bearing project opens without crashing (the design's "Done when").
- `macros.AC6.1`–`AC6.6` are verified.

This is the final phase. With Phases 1-7 complete, Vensim macros are a first-class, persistent concept in the Simlin engine: parsed, represented, persisted, simulated, round-tripped through both `.mdl` and XMILE, and surfaced safely to the diagram — meeting the design's Definition of Done. Out-of-scope items (C-LEARN's non-macro blockers and full end-to-end C-LEARN simulation; native macro authoring/editing UX; fixing xmutil's C++ macro parser) remain tracked separately.
