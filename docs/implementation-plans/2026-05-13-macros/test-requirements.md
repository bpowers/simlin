# Vensim Macro Support — Test Requirements

**Generated:** 2026-05-14
**Source:** `docs/design-plans/2026-05-13-macros.md` (the authoritative acceptance-criteria list) and the seven implementation-plan phase files `phase_01.md` … `phase_07.md` in this directory (the per-task test-to-AC mapping the planning worked out).

This file maps every acceptance criterion in the macro design (`macros.AC1.1` … `macros.AC6.6`) to the automated test(s) that verify it — test type, expected file path, the test name or fixture the plan names, the phase + task that produces it, and the `cargo test` / `pnpm test` verification command — or to documented human verification where a criterion genuinely cannot be automated. It is consumed by the `test-analyst` agent during execution to validate coverage.

**AC count discrepancy noted up front.** The task brief refers to "36 acceptance criteria," but the design plan's `## Acceptance Criteria` section enumerates **37**: AC1 has 7 cases (1.1–1.7), AC2 has 8 (2.1–2.8), AC3 has 5 (3.1–3.5), AC4 has 5 (4.1–4.5), AC5 has 6 (5.1–5.6), AC6 has 6 (6.1–6.6) — 7+8+5+5+6+6 = 37. This file covers all 37; the count is called out again in the coverage summary. This is a judgment call: the design's literal enumeration is treated as authoritative over the brief's round number.

**Project testing conventions (recap).**
- Engine tests are Rust. Unit tests are inline `#[cfg(test)]` modules in the source file under test; integration tests are files under `src/simlin-engine/tests/` (`simulate.rs`, `mdl_roundtrip.rs`, and the new `metasd_macros.rs`). Run via `cargo test -p simlin-engine …`.
- Diagram tests are TypeScript (Jest, under `src/diagram/tests/`). Run via `pnpm --filter @simlin/diagram test`.
- TypeScript datamodel-mirror tests live in `src/core/tests/`; run via `pnpm --filter @simlin/core test`. Python-mirror tests live in `src/pysimlin/tests/`; run via `uv run pytest` from `src/pysimlin/`.
- Fixture-driven integration tests compare simulation output against checked-in `output.tab` / `.vdf` reference files. Wiring a fixture into `simulate.rs`'s `TEST_MODELS` array runs it through `simulate_path_with`, which asserts: import → simulate against `output.tab` → protobuf round-trip → byte-stable XMILE round-trip. So for an `.xmile` fixture, "the automated test" is the `TEST_MODELS` entry plus `simulate_path_with`'s built-in assertions — not a separately-named function.
- Some tests are `#[ignore]`d (large corpus / hero models that exceed the per-test time budget) with a documented opt-in command (`cargo test … -- --ignored`). These are noted as **automated-but-gated**.

---

## macros.AC1: Macro definitions parse and represent faithfully

### macros.AC1.1
**AC text:** *Success: A `.mdl` `:MACRO:` block imports as a macro-marked `Model` whose `MacroSpec.parameters` matches the header's input list in order, with body variables matching the body equations.*

**Automated test:** Integration / converter-level, two layers.
- **Primary:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/mdl/convert/mod.rs` (or a sibling `convert/macro_tests.rs`), produced by **Phase 2, Task 5** and **Phase 2, Task 7**. Task 5 adds a focused test converting an inline `:MACRO: EXPRESSION MACRO(input, parameter)` `.mdl` via `convert_mdl(...)` and asserting `MacroSpec.parameters == ["input", "parameter"]`, `primary_output == "expression_macro"`, body aux `expression_macro` present with equation text byte-identical to the parameter names, plus synthesized port variables `input`/`parameter` with `compat.can_be_module_input == true`. Task 7 extends this to all six `test/test-models/tests/macro_*` `.mdl` fixtures via `convert_mdl(include_str!(...))`, asserting each macro-marked model's `MacroSpec` and body.
- Verification command: `cargo test -p simlin-engine convert`.

### macros.AC1.2
**AC text:** *Success: A `:MACRO:` header with a `:` output list (`add3(a,b,c : minval, maxval)`) imports with `MacroSpec.additional_outputs` populated in order.*

**Automated test:** Integration / converter-level, with supporting parser-unit tests.
- **Parser support:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/mdl/parser.rs` near `test_parse_macro_start`, and reader tests in `src/simlin-engine/src/mdl/reader.rs`, produced by **Phase 2, Task 2** — assert `:MACRO: add3(a, b, c : minval, maxval)` parses to a `SectionEnd::MacroStart` with `outputs.len() == 2` (names `minval`/`maxval` in order) and the reader builds a `MacroDef.outputs` with those names. Command: `cargo test -p simlin-engine mdl::parser` and `cargo test -p simlin-engine mdl::reader`.
- **Import-level (the AC itself):** inline `#[cfg(test)]` test in `src/simlin-engine/src/mdl/convert/mod.rs`, produced by **Phase 2, Task 5** — converts a `.mdl` with `:MACRO: add3(a, b, c : minval, maxval)` and asserts the macro model's `MacroSpec.additional_outputs == ["minval", "maxval"]` in order, `parameters == ["a", "b", "c"]`, and that `minval`/`maxval` are *not* synthesized as port variables (they are body-computed outputs). Command: `cargo test -p simlin-engine convert`.

### macros.AC1.3
**AC text:** *Success: An XMILE `<macro>` element imports as a macro-marked `Model`; an expression-form `<eqn>` is normalized into a macro-named body variable.*

**Automated test:** Integration / XMILE-reader-level, plus fixture wiring.
- **Reader unit test:** inline `#[cfg(test)]` test in `src/simlin-engine/src/xmile/mod.rs` (or a focused test using `open_xmile` on an in-test XMILE string), produced by **Phase 5, Task 1** — an XMILE string with `<macro name="MYMACRO"><parm>a</parm><parm>b</parm><eqn>a * b</eqn></macro>` imports as a `datamodel::Project` containing a macro-marked `Model` with `MacroSpec.parameters == ["a", "b"]`, a body variable `mymacro` carrying the normalized `<eqn>` `a * b`, and synthesized port variables `a`/`b`. A second test covers a `<variables>`-body macro; a third asserts a non-empty `<sim_specs>` returns the documented-limitation error. Command: `cargo test -p simlin-engine xmile`.
- **Fixture wiring:** **Phase 5, Task 4** wires the four `.xmile` macro fixtures into `src/simlin-engine/tests/simulate.rs`'s `TEST_MODELS` array — `test/test-models/tests/macro_expression/test_macro_expression.xmile`, `.../macro_multi_expression/test_macro_multi_expression.xmile`, `.../macro_stock/test_macro_stock.xmile` (uncommented at lines 27-29) and `.../macro_multi_macros/test_macro_multi_macros.xmile` (added new). `simulate_path_with` then exercises the `<macro>` → macro-marked-model import for each. Command: `cargo test -p simlin-engine --test simulate`.

### macros.AC1.4
**AC text:** *Success: A macro-bearing project round-trips losslessly through protobuf and through JSON -- `MacroSpec` and macro body are identical after deserialize.*

**Automated test:** Unit / serde round-trip, across all five serialization layers — **Phase 1** in full.
- **Protobuf (Rust):** `test_model_with_macro_spec_roundtrip` in `src/simlin-engine/src/serde.rs`, near `test_model_with_loop_metadata_roundtrip` — **Phase 1, Task 2**. Constructs a `Model` with `macro_spec: Some(MacroSpec{...})` (non-empty `parameters`, `primary_output`, `additional_outputs`) plus a body `Variable`, round-trips via `Model::from(project_io::Model::from(expected.clone()))`, `assert_eq!`. Includes a `macro_spec: None` case. Command: `cargo test -p simlin-engine serde`.
- **JSON (Rust):** an extended `test_model_roundtrip` or focused `test_macro_spec_roundtrip` in `src/simlin-engine/src/json.rs` — **Phase 1, Task 3**. Goes `json::Model` → `datamodel::Model` → `json::Model` → `serde_json` string → `json::Model` and `assert_eq!`s. Also regenerates `docs/simlin-project.schema.json` with a `MacroSpec` entry (verified by `cargo test -p simlin-engine generate_and_write_schema`). Command: `cargo test -p simlin-engine json`.
- **TypeScript mirror:** a `describe('MacroSpec', …)` block in `src/core/tests/datamodel.test.ts` — **Phase 1, Task 4**. A "should roundtrip correctly" test (`macroSpecToJson` → `macroSpecFromJson`, assert equality) and a "should omit empty additionalOutputs" test. Command: `pnpm --filter @simlin/core test`.
- **Python mirror:** a `MacroSpec` round-trip test in `src/pysimlin/tests/test_json_types.py` — **Phase 1, Task 5**. Builds a `Model` with a populated `macro_spec`, unstructures via `json_converter`, structures back, asserts identity; confirms camelCase keys. Command: `cd src/pysimlin && uv run pytest tests/test_json_types.py -x`.

### macros.AC1.5
**AC text:** *Success: `$`-suffixed time references in a macro body (`TIME STEP$`, `Time$`) import as the canonical time identifiers.*

**Automated test:** Unit (helper-level) + integration (import-level) — **Phase 2, Task 6**.
- **Helper-level:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/mdl/convert/helpers.rs` — build `Expr` trees with `Var("Time$")`, `Var("TIME STEP$")`, `Var("Initial Time$")`, and a nested `c + MYMACRO(Time$, x)` shape; assert the `$`-time rewrite produces `time`, `time_step`, `initial_time`, rewrites the nested occurrence, and leaves non-time `Var`s (`"foo$"`, `"input"`) untouched.
- **Import-level (the AC):** inline `#[cfg(test)]` test in `src/simlin-engine/src/mdl/convert/mod.rs` — `convert_mdl` a `.mdl` whose macro body uses `Time$` and `TIME STEP$` (e.g. `MYMACRO = input * Time$ / TIME STEP$`); assert the imported macro model's body equation text references `time` and `time_step`.
- Verification command: `cargo test -p simlin-engine convert`.

### macros.AC1.6
**AC text:** *Failure: A `:MACRO:` block with no `:END OF MACRO:` reports a clear parse error.*

**Automated test:** Integration / error-case — **Phase 2, Task 7**.
- Inline `#[cfg(test)]` test in `src/simlin-engine/src/mdl/convert/mod.rs` (or `convert/macro_tests.rs`) — asserts `open_vensim` / `convert_mdl` on a `.mdl` containing `:MACRO: BAD(x)` … with no `:END OF MACRO:` returns `Err`, specifically `ConvertError::Reader(ReaderError::EofInsideMacro)` whose `Display` is the clear message `"reader error: unexpected end of file inside macro"` (through `open_vensim`: `"Failed to parse MDL: reader error: unexpected end of file inside macro"`). The same test also asserts a stray `:END OF MACRO:` with no opening `:MACRO:` yields `ReaderError::UnmatchedMacroEnd`.
- Verification command: `cargo test -p simlin-engine convert`.

### macros.AC1.7
**AC text:** *Edge: A macro defined but never invoked (C-LEARN's `INIT`) imports as a valid macro-marked model and is preserved.*

**Automated test:** Integration / converter-level — **Phase 2, Task 5** and **Phase 2, Task 7**.
- Task 5 adds a focused test: `convert_mdl` a `.mdl` that defines a macro but never invokes it; assert the macro-marked `Model` is still present in `project.models` with the correct `MacroSpec` and synthesized port variables.
- Task 7 covers the `macro_trailing_definition` fixture (defined after its call site, exercising definition-order leniency) and confirms its preservation.
- The C-LEARN `INIT` case specifically (a real never-invoked macro) is additionally covered by **Phase 7, Task 1**'s C-LEARN macro-expansion test, which asserts all four C-LEARN macros — including `INIT` — import as macro-marked `Model`s with correct `MacroSpec`s (automated-but-gated, `#[ignore]`d).
- Verification command: `cargo test -p simlin-engine convert` (Phase 2); `cargo test -p simlin-engine --test simulate -- --ignored` (Phase 7 C-LEARN).

---

## macros.AC2: Single-output invocations simulate with correct Vensim semantics

### macros.AC2.1
**AC text:** *Success: A stockless single-output macro invocation (`macro_expression`) simulates and matches the fixture's `output.tab`.*

**Automated test:** Integration / fixture simulation — **Phase 3, Task 4** (with an end-to-end smoke in **Phase 3, Task 3**).
- `simulates_macro_expression_mdl` in `src/simlin-engine/tests/simulate.rs` — calls `simulate_mdl_path("../../test/test-models/tests/macro_expression/test_macro_expression.mdl")`, which does `open_vensim` → `compile_vm` → run → `ensure_results` against the fixture's `output.tab`.
- Phase 3 Task 3 additionally adds an inline end-to-end smoke in `src/simlin-engine/src/builtins_visitor.rs`'s `#[cfg(test)] mod tests`: a trivial `:MACRO: M(a, b)` / `M = a * b` invoked `y = M(5, 1.1)`, compiled and run, asserting `y == 5.5`.
- Verification command: `cargo test -p simlin-engine --test simulate` (fixture); `cargo test -p simlin-engine builtins_visitor` (smoke).

### macros.AC2.2
**AC text:** *Success: A stock-bearing macro invocation (`macro_stock`) simulates with correct per-invocation integration, matching the 11-step `output.tab`.*

**Automated test:** Integration / fixture simulation — **Phase 3, Task 4**.
- `simulates_macro_stock_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_stock/test_macro_stock.mdl")`, comparing against the fixture's 11-step `output.tab`.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.3
**AC text:** *Success: The same macro invoked at multiple call sites produces independent per-invocation state -- two stock-bearing invocations don't share a stock.*

**Automated test:** Integration / focused inline-`.mdl` simulation — **Phase 3, Task 4** (Group 2).
- A focused `#[test] fn` in `src/simlin-engine/tests/simulate.rs` — an inline `.mdl` defining a stock-bearing macro `M(rate, init) = INTEG(rate, init)` invoked twice (`x = M(1, 0)`, `y = M(2, 10)`); compiled via `compile_vm`, run, with hand-computed assertions that `x` integrates `1`/step from `0` and `y` integrates `2`/step from `10` — proving they do not share a stock. The plan does not pin a function name; expected name follows the file's `simulates_*` convention.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.4
**AC text:** *Success: A macro invoked with an expression-valued argument (`MYMACRO(a + b, t)`) simulates correctly, with the argument evaluated in the caller's context.*

**Automated test:** Integration / focused inline-`.mdl` simulation — **Phase 3, Task 4** (Group 2).
- A focused `#[test] fn` in `src/simlin-engine/tests/simulate.rs` — an inline `.mdl` with `M(in, p) = in * p` invoked `y = M(a + b, t)` over constants `a`, `b`, `t`; hand-computed assertion `y == (a + b) * t`, confirming the argument is evaluated in the caller's context.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.5
**AC text:** *Success: A multi-equation macro body with macro-local helpers (`macro_multi_expression`) simulates correctly; helper names don't leak into the caller's namespace.*

**Automated test:** Integration / fixture simulation — **Phase 3, Task 4**.
- `simulates_macro_multi_expression_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_multi_expression/test_macro_multi_expression.mdl")`. The plan additionally requires the test to assert the `main` model has no `intermediate` variable (the macro-local helper does not leak into the caller's namespace), beyond `ensure_results`'s expected-columns check.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.6
**AC text:** *Success: A macro that calls another macro (`macro_cross_reference`) expands recursively and simulates correctly.*

**Automated test:** Integration / fixture simulation — **Phase 3, Task 4**.
- `simulates_macro_cross_reference_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_cross_reference/test_macro_cross_reference.mdl")`. ("Expands recursively" here means a macro body that invokes another macro — `EXPRESSION MACRO` body calls `SECOND MACRO` — not self-recursion, which is rejected per AC5.2.)
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.7
**AC text:** *Success: A macro body referencing global time via the `$` escape simulates with the global time values.*

**Automated test:** Integration / focused inline-`.mdl` simulation — **Phase 3, Task 4** (Group 2).
- A focused `#[test] fn` in `src/simlin-engine/tests/simulate.rs` — an inline `.mdl` with a macro body referencing `Time$` (e.g. `M(x) = x + Time$`) invoked `y = M(10)`; hand-computed assertion `y == 10 + time` at each step. This exercises Phase 2's `$`-time translation plus Phase 3's global-time-resolves-inside-a-module behavior.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC2.8
**AC text:** *Edge: A macro invocation nested inside a larger expression (`y = c + MYMACRO(x, t)`) expands and simulates correctly.*

**Automated test:** Integration / focused inline-`.mdl` simulation — **Phase 3, Task 4** (Group 2).
- A focused `#[test] fn` in `src/simlin-engine/tests/simulate.rs` — an inline `.mdl` with `y = c + M(x, t)` (a macro call inside a larger expression); hand-computed assertion that `y` equals `c` plus the macro's value.
- Verification command: `cargo test -p simlin-engine --test simulate`.

---

## macros.AC3: Multi-output and arrayed invocation

### macros.AC3.1
**AC text:** *Success: A multi-output invocation (`total = add3(a,b,c : minv, maxv)`) materializes as a module instance; `total` receives the primary output and `minv`/`maxv` become model variables holding the additional outputs.*

**Automated test:** Integration / converter-level structure test — **Phase 4, Task 1**.
- Inline `#[cfg(test)]` test in `src/simlin-engine/src/mdl/convert/` (the same module as Phase 2's macro-conversion tests) — `convert_mdl(include_str!(".../macro_multi_output/test_macro_multi_output.mdl"))`; asserts the main model contains exactly one `Variable::Module` whose `model_name` is the `ADD3` macro's model with input `ModuleReference`s wiring `in1`/`in2`/`in3` to ports `a`/`b`/`c`; asserts `total` is a `Variable::Aux` reading `<module>.<primary_output>` (ASCII-period datamodel form); asserts `the min` / `the max` are `Variable::Aux`es reading `<module>.minval` / `<module>.maxval`. The fixture `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl` is authored by this task.
- Verification command: `cargo test -p simlin-engine convert`.

### macros.AC3.2
**AC text:** *Success: `minv` and `maxv` are referenceable by subsequent equations and carry the correct values.*

**Automated test:** Integration / fixture simulation — **Phase 4, Task 1**.
- `simulates_macro_multi_output_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_multi_output/test_macro_multi_output.mdl")`. The authored fixture includes a **downstream equation** that references the additional outputs (e.g. `spread = the max - the min`); the fixture's hand-computed `output.tab` includes `total`, `the min`, `the max`, and `spread`, so the passing test proves `the min`/`the max` are referenceable by a subsequent equation and carry correct values.
- Verification command: `cargo test -p simlin-engine --test simulate simulates_macro_multi_output_mdl`.

### macros.AC3.3
**AC text:** *Success: The metasd multi-output models (`THEIL`, `SSTATS`) expand and simulate.*

**Automated test:** Integration / corpus-model — **Phase 4, Task 3** (early gate) and **Phase 7, Task 2** (comprehensive tier). Automated-but-gated for the heavy models.
- **Phase 4, Task 3:** focused tests in `src/simlin-engine/tests/simulate.rs` — import `test/metasd/theil-statistics/Theil_2011.mdl` via `open_vensim`, assert the `THEIL` multi-output invocation materialized (a `Variable::Module` plus binding `Aux`es for the primary output and all 13 `:`-list outputs), compile via `compile_vm`, run to the end. Same for `SSTATS` in `test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl` (a `Variable::Module` + 1 primary + 10 additional binding auxes per invocation). Heavy models are `#[ignore]`d with a documented opt-in command; `Theil_2011.mdl` may stay a regular test if it fits the per-test budget. If the COVID model has unrelated non-macro blockers, the assertion narrows to "the `SSTATS` materialization succeeded with no macro-specific diagnostics" and the blocker is filed via `track-issue`.
- **Phase 7, Task 2:** `THEIL` and `SSTATS` are also covered by the `metasd_macros.rs` tiered corpus harness (expansion tier for all 14 directories; simulation tier where a reference output exists).
- Verification command: `cargo test -p simlin-engine --test simulate -- --ignored` (gated); `cargo test -p simlin-engine --test metasd_macros` (corpus harness, plus `-- --ignored` if gated).

### macros.AC3.4
**AC text:** *Success: An arrayed invocation (`y[Dim] = MYMACRO(x[Dim], …)`) expands into one independent macro instance per dimension element.*

**Automated test:** Integration / fixture simulation + expansion-level structure assertion — **Phase 4, Task 2**, building on **Phase 3, Task 3**.
- `simulates_macro_arrayed_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_arrayed/test_macro_arrayed.mdl")`. The authored fixture `test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl` is a stockless macro `:MACRO: SCALE(x, k)` invoked apply-to-all `out[Region] = SCALE(inp[Region], factor)` over a small 2-3-element dimension; the hand-computed `output.tab` has one column per `out[Region]` element.
- Additionally, a `convert/`- or expansion-level assertion (per the plan) that the arrayed invocation produced one synthetic `Variable::Module` per element (subscript-suffixed idents), not a single shared instance.
- The supporting `contains_stdlib_call` macro-awareness predicate is unit-tested in **Phase 3, Task 3** (a focused test in `src/simlin-engine/src/builtins_visitor.rs` asserting it returns `true` for an arrayed macro `App`).
- Verification command: `cargo test -p simlin-engine --test simulate macro_arrayed`.

### macros.AC3.5
**AC text:** *Edge: An arrayed invocation of a stock-bearing macro gives each element its own persistent stock.*

**Automated test:** Integration / focused inline-`.mdl` simulation — **Phase 4, Task 2**.
- A focused inline-`.mdl` test in `src/simlin-engine/tests/simulate.rs` — a stock-bearing macro `:MACRO: ACCUM(rate)` with body `ACCUM = INTEG(rate, 0)`, arrayed invocation `total[Region] = ACCUM(rate[Region])` with `rate[Region]` differing per element; `open_vensim` + `compile_vm` + run + hand-computed assertions that each `total[element]` integrates *its own* `rate[element]` independently (e.g. `rate = [1, 3]` over 4 steps at dt=1 → `total[r1] = 0,1,2,3,4` and `total[r2] = 0,3,6,9,12`), proving per-element persistent state.
- Verification command: `cargo test -p simlin-engine --test simulate macro_arrayed`.

---

## macros.AC4: Round-trip and export

### macros.AC4.1
**AC text:** *Success: A macro-bearing `.mdl` file round-trips with definitions emitted as `:MACRO:` blocks and invocations preserved.*

**Automated test:** Unit (writer) + integration (round-trip harness) — **Phase 6, Task 1** and **Phase 6, Task 3**.
- **Writer unit tests:** full-project-style tests in `src/simlin-engine/src/mdl/writer_tests.rs` — **Phase 6, Task 1**. Build a `datamodel::Project` with a `main` model and a single-output macro-marked `Model` → `project_to_mdl` → assert the output contains a `:MACRO: <NAME>(<params>)` header, the body equation(s), `:END OF MACRO:`, that synthesized port variables do *not* appear as body equations, and that the main model's invocation equation is preserved as call text. Includes the rewritten `project_to_mdl_rejects_multiple_models` (ordinary multi-model still rejected; macro-marked extra model accepted). Command: `cargo test -p simlin-engine mdl::writer`.
- **Round-trip harness (the AC end-to-end):** **Phase 6, Task 3** wires the 6 bundled `macro_*` fixtures plus `macro_arrayed` into `TEST_MDL_MODELS` in `src/simlin-engine/tests/mdl_roundtrip.rs`; `mdl_to_mdl_roundtrip()` drives `parse_mdl → project_to_mdl → parse_mdl → assert_semantic_equivalence` for each. Task 3 also extends `assert_model_equivalence` to compare `Model.macro_spec` and `Model.name`, so the macro round-trip is actually verified (not silently passed). Command: `cargo test -p simlin-engine --test mdl_roundtrip`.

### macros.AC4.2
**AC text:** *Success: A macro-bearing XMILE file round-trips with `<macro>` elements and `simlin:` extensions; the `<uses_macros>` header option is emitted.*

**Automated test:** Integration / XMILE-writer tests + fixture round-trip — **Phase 5, Tasks 2, 3, 4**.
- **Writer unit tests (definition + header option):** inline `#[cfg(test)]` tests in `src/simlin-engine/src/xmile/mod.rs` — **Phase 5, Task 2**. A `datamodel::Project` with a single-output macro-marked `Model` → `to_xmile` → assert the output contains a `<macro name="...">` element with expected `<parm>`s and body, and a `<uses_macros recursive_macros="false" option_filters="false"/>` header option. Plus a `to_xmile` → `open_xmile` round-trip asserting the macro-marked model survives with the same `MacroSpec` and body.
- **`simlin:` extension round-trip:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/xmile/mod.rs` — **Phase 5, Task 3**. A multi-output macro project → `to_xmile` → `open_xmile` → assert the round-tripped project has the same `MacroSpec.additional_outputs` and the same materialized `Variable::Module` + binding `Aux`es; a second `to_xmile` is byte-identical to the first.
- **Fixture round-trip (the AC end-to-end):** **Phase 5, Task 4** wires the four `.xmile` macro fixtures into `TEST_MODELS` in `src/simlin-engine/tests/simulate.rs`; `simulate_path_with` performs a byte-stable XMILE round-trip assertion (serialize → re-parse → re-serialize → `assert_eq!`) for each.
- Verification command: `cargo test -p simlin-engine xmile` (writer / extension); `cargo test -p simlin-engine --test simulate` (fixture round-trip).

### macros.AC4.3
**AC text:** *Success: A multi-output macro round-trips through `.mdl` with the `:` call syntax reconstructed.*

**Automated test:** Unit (writer) + integration (round-trip harness) — **Phase 6, Task 2** and **Phase 6, Task 3**.
- **Writer unit tests:** full-project-style tests in `src/simlin-engine/src/mdl/writer_tests.rs` — **Phase 6, Task 2**. Build a `datamodel::Project` with a multi-output macro-marked `Model` and a materialized multi-output cluster (`Variable::Module` + primary-output binding aux + additional-output binding auxes) → `project_to_mdl` → assert the output contains the reconstructed `<lhs> = <MACRO>(<args> : <bindings>)` `:` call syntax with arguments in positional order and output bindings in `:`-list order, and that the `Variable::Module` / binding auxes do *not* appear as separate entries. Includes the rewritten `project_to_mdl_rejects_module_variable` (ordinary module still rejected; macro-module accepted). Command: `cargo test -p simlin-engine mdl::writer`.
- **Round-trip harness (the AC end-to-end):** **Phase 6, Task 3** adds `test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl` (the Phase-4-authored `:`-multi-output fixture) to `TEST_MDL_MODELS` in `src/simlin-engine/tests/mdl_roundtrip.rs`; the round-trip re-parses the reconstructed `:` syntax to the same materialized cluster and the same `MacroSpec` with non-empty `additional_outputs`. Command: `cargo test -p simlin-engine --test mdl_roundtrip`.

### macros.AC4.4
**AC text:** *Success: A cross-format conversion (`.mdl` → datamodel → `.xmile`) preserves macro definitions and invocations.*

**Automated test:** Integration / cross-format conversion test — **Phase 5, Task 4**.
- A dedicated test in `src/simlin-engine/tests/simulate.rs` — `open_vensim`s a single-output macro `.mdl` fixture (e.g. `test/test-models/tests/macro_expression/test_macro_expression.mdl`), converts the resulting `datamodel::Project` to XMILE via `to_xmile`, re-imports via `open_xmile`, and asserts the macro definition (the macro-marked `Model` + its `MacroSpec`) and the invocation are preserved — the cross-format-round-tripped project's macro models and invocation equations match those of the directly-imported `.mdl` project. (The plan notes "MDL→XMILE conversion is untested anywhere" today; this test is net-new coverage.) The plan does not pin a function name.
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC4.5
**AC text:** *Edge: A single-output-only model exports as standards-clean XMILE with no extensions; multi-output triggers the `simlin:` extension.*

**Automated test:** Integration / XMILE-writer tests — **Phase 5, Task 2** (standards-clean half) and **Phase 5, Task 3** (multi-output-triggers half).
- **Standards-clean (Phase 5, Task 2):** an inline `#[cfg(test)]` test in `src/simlin-engine/src/xmile/mod.rs` — assert the `to_xmile` output of a single-output-only macro project contains **no** `simlin:`-prefixed macro-extension element (specifically no macro-additional-output extension; other pre-existing `simlin:` elements like `simlin:loop-metadata` may still be present).
- **Multi-output triggers the extension (Phase 5, Task 3):** an inline `#[cfg(test)]` test in `src/simlin-engine/src/xmile/mod.rs` — a `datamodel::Project` with a multi-output macro (non-empty `MacroSpec.additional_outputs`) and a multi-output invocation → `to_xmile` → assert the output contains the `simlin:` additional-outputs extension on `<macro>` and the `simlin:` multi-output-invocation extension; and (contrast) confirm a single-output project does not.
- Verification command: `cargo test -p simlin-engine xmile`.

---

## macros.AC5: Error handling and edge cases

### macros.AC5.1
**AC text:** *Failure: A macro invoked with the wrong number of arguments reports an arity-mismatch diagnostic naming the macro.*

**Automated test:** Unit / expansion-level — **Phase 3, Task 3**.
- An inline `#[cfg(test)]` test in `src/simlin-engine/src/builtins_visitor.rs`'s `mod tests` (or `macro_expansion_tests.rs`) — a macro declared with 2 parameters, invoked with 3 args (and separately with 1 arg); compile fails with `ErrorCode::BadBuiltinArgs`; the test asserts the error span covers the macro call so the macro is identified in context.
- Note: the multi-output materialization path also has an arity check — **Phase 4, Task 1** adds a `convert/`-level test that `convert_mdl` of a `.mdl` invoking `ADD3` with the wrong argument or `:`-output count returns a `ConvertError` naming `ADD3`. This is supplementary coverage of the same AC for the multi-output form.
- Verification command: `cargo test -p simlin-engine builtins_visitor` (single-output); `cargo test -p simlin-engine convert` (multi-output path).

### macros.AC5.2
**AC text:** *Failure: A directly or mutually recursive macro is rejected with a cycle-detection error rather than expanding without termination.*

**Automated test:** Unit (registry-build) + integration (end-to-end compile failure) — **Phase 3, Task 1** and **Phase 3, Task 2**.
- **Registry-build unit test:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/module_functions.rs` — **Phase 3, Task 1**. A macro `A` whose body calls `A` → `MacroRegistry::build` returns `Err` with `CircularDependency`; a macro `A` calling `B` calling `A` → `Err`; a macro `A` calling `B` with no cycle (the `macro_cross_reference` shape) → `build` succeeds. Command: `cargo test -p simlin-engine module_functions`.
- **End-to-end compile failure:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/model.rs`'s `#[cfg(test)]` module — **Phase 3, Task 2**. `convert_mdl` a `.mdl` with a directly recursive macro and a `main` invocation, compile, assert the compile fails with a cycle-detection error (`CircularDependency`) whose message names the macro; repeat for a mutually recursive `A`/`B` pair. Command: `cargo test -p simlin-engine model::`.

### macros.AC5.3
**AC text:** *Failure: Two macros sharing a name, or a macro name colliding with a model name, report a registry-build diagnostic.*

**Automated test:** Unit (registry-build) + integration (end-to-end compile failure) — **Phase 3, Task 1** and **Phase 3, Task 2**.
- **Registry-build unit test:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/module_functions.rs` — **Phase 3, Task 1**. Two macro models named `FOO` → `MacroRegistry::build` returns `Err` naming `FOO`; a macro named `main` alongside a `main` model → `build` returns `Err` naming the collision. Command: `cargo test -p simlin-engine module_functions`.
- **End-to-end compile failure:** inline `#[cfg(test)]` tests in `src/simlin-engine/src/model.rs`'s `#[cfg(test)]` module — **Phase 3, Task 2**. `convert_mdl` a `.mdl` with two `:MACRO:` blocks of the same name → assert the compile fails with a duplicate-name error naming the macro; a `.mdl` with a macro named `main` → assert the compile fails with a collision error. Command: `cargo test -p simlin-engine model::`.

### macros.AC5.4
**AC text:** *Success: A macro shadowing a builtin (`SSHAPE`, `RAMP FROM TO`) resolves to the macro; the builtin is not invoked.*

**Automated test:** Unit (expansion-level) + integration (simulation-level confirmation) — **Phase 3, Task 3** and **Phase 3, Task 4**.
- **Expansion-level (Phase 3, Task 3):** an inline `#[cfg(test)]` test in `src/simlin-engine/src/builtins_visitor.rs`'s `mod tests` — a `.mdl` defining `:MACRO: SSHAPE(x, p)` with body `SSHAPE = x + p`, invoked `y = SSHAPE(3, 4)` → compiles and simulates with `y == 7` (the macro's definition), proving the macro shadowed the `SSHAPE` builtin. Repeated for a `RAMP FROM TO` macro. Command: `cargo test -p simlin-engine builtins_visitor`.
- **Simulation-level confirmation (Phase 3, Task 4):** a focused test in `src/simlin-engine/tests/simulate.rs` — a macro shadowing `SSHAPE` invoked in a model that also uses other builtins; assert the simulated value matches the macro's definition, not the builtin's. Command: `cargo test -p simlin-engine --test simulate`.

### macros.AC5.5
**AC text:** *Success: A macro defined after its first use (`macro_trailing_definition`) still resolves and simulates.*

**Automated test:** Integration / fixture simulation — **Phase 3, Task 4** (with import-level coverage in **Phase 2, Task 7**).
- `simulates_macro_trailing_definition_mdl` in `src/simlin-engine/tests/simulate.rs` — `simulate_mdl_path(".../macro_trailing_definition/test_macro_trailing_definition.mdl")`. The fixture defines the macro after its call site; the passing simulation proves it resolves and simulates (definition-order leniency).
- Phase 2 Task 7 additionally covers the import-level half (the macro model is produced regardless of definition order).
- Verification command: `cargo test -p simlin-engine --test simulate`.

### macros.AC5.6
**AC text:** *Failure: A call to a name that is neither a macro, a stdlib function, nor a builtin reports an "unknown function or macro" error.*

**Automated test:** Unit / expansion-level — **Phase 3, Task 3**.
- An inline `#[cfg(test)]` test in `src/simlin-engine/src/builtins_visitor.rs`'s `mod tests` — a `.mdl` with `y = NOTAFUNCTION(x)` (a name that is neither macro, stdlib, nor builtin) → compile fails with `ErrorCode::UnknownBuiltin`.
- Verification command: `cargo test -p simlin-engine builtins_visitor`.

---

## macros.AC6: Validation corpus and consumer floor

### macros.AC6.1
**AC text:** *Success: All six `test/test-models/tests/macro_*` fixtures are wired into the active test suite and pass.*

**Automated test:** Integration / fixture wiring across `simulate.rs` and `mdl_roundtrip.rs` — produced incrementally by **Phases 3, 5, 6** and **confirmed explicitly by Phase 7, Task 2**.
- The six `.mdl` fixtures are wired into `src/simlin-engine/tests/simulate.rs` as `simulates_macro_*_mdl` tests (**Phase 3, Task 4**) and into `src/simlin-engine/tests/mdl_roundtrip.rs`'s `TEST_MDL_MODELS` (**Phase 6, Task 3**). The four `.xmile` fixtures are wired into `simulate.rs`'s `TEST_MODELS` array (**Phase 5, Task 4**).
- **Phase 7, Task 2** is the explicit confirmation step: run the suite and inspect; if any `macro_*` fixture is not wired (a gap left by an earlier phase), wire it. No new harness is needed — AC6.1 is satisfied by the earlier phases' wiring, and Phase 7 makes the satisfaction complete and explicit.
- Verification command: `cargo test -p simlin-engine --test simulate` and `cargo test -p simlin-engine --test mdl_roundtrip`.

### macros.AC6.2
**AC text:** *Success: C-LEARN's macros (`SAMPLE UNTIL`, `SSHAPE`, `RAMP FROM TO`, `INIT`) parse, register, and expand with no macro-specific errors.*

**Automated test:** Integration / hero-model expansion — **Phase 7, Task 1**. Automated-but-gated (`#[ignore]`d; C-LEARN is 1.4 MB).
- An `#[ignore]`d test in `src/simlin-engine/tests/simulate.rs` — `open_vensim`s `../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`, asserts its four macros imported as macro-marked `Model`s with correct `MacroSpec`s, syncs + compiles via the salsa path, collects diagnostics via `collect_all_diagnostics`, and asserts **no diagnostic is macro-attributable** (none reference a macro name or macro-instance variable, none are macro-registry-build errors). C-LEARN's known non-macro blockers (circular dependencies, dimension mismatches, unit errors, and any non-time `$` reference) are expected and explicitly allowed. An early-gate version of this expansion check is also in **Phase 4, Task 3**.
- Verification command: `cargo test -p simlin-engine --test simulate -- --ignored`.

### macros.AC6.3
**AC text:** *Success: Focused models invoking C-LEARN's `SAMPLE UNTIL`, `SSHAPE`, and `RAMP FROM TO` with known inputs match Vensim DSS reference output.*

**Automated test:** Integration / focused fixture simulation — **Phase 7, Task 1**.
- Three `simulate_mdl_path` tests in `src/simlin-engine/tests/simulate.rs` (matching `macro_clearn`), one per invoked C-LEARN macro. Each test's fixture is a small focused `.mdl` under `test/test-models/tests/macro_clearn_*/` that defines the macro (its `:MACRO:` block copied verbatim from C-LEARN) and invokes it with known constant inputs, paired with an expected-output file. The plan: prefer a Vensim DSS reference `.vdf` if one is provided (a documented prerequisite setup task) and compare via `ensure_vdf_results`; otherwise the formula-derived `output.tab` (computed by applying the macro's body formula to the known inputs, arithmetic documented in the fixture `README.md`) is the gate. `INIT` is not invoked in C-LEARN and needs no focused model — AC6.2 covers it.
- **Test-prerequisite dependency:** the Vensim DSS reference `.vdf` for these focused fixtures is a documented setup task ("Test prerequisites" in the design), not implementation work. The automated test still exists and gates either way — against the reference `.vdf` if present, or against the formula-derived `output.tab`. So this is fully automated; only the *strength* of the reference (DSS-validated vs formula-derived) depends on the prerequisite.
- Verification command: `cargo test -p simlin-engine --test simulate macro_clearn`.

### macros.AC6.4
**AC text:** *Success: All 14 macro-using metasd models pass the expansion tier; those without unrelated blockers match Vensim DSS reference output.*

**Automated test:** Integration / tiered corpus harness — **Phase 7, Task 2**. Partially automated-but-gated (heavy models `#[ignore]`d).
- A new integration-test file `src/simlin-engine/tests/metasd_macros.rs` with a tiered corpus harness over the 17 macro-using `.mdl` files across the 14 macro-using `test/metasd/` directories (the plan enumerates the exact path list). Two tiers:
  - **Expansion tier (all 14 directories / 17 files):** `open_vensim` → sync → compile → `collect_all_diagnostics`; assert no macro-attributable diagnostic; accumulate `(model, macro-diagnostic)` failures and assert the failure vec is empty. Runnable for all 14 with no prerequisites.
  - **Simulation tier (subset):** for each model that **both** has a checked-in Vensim reference output **and** has no unrelated blockers, run the VM to completion and compare via `ensure_vdf_results` / `ensure_results`. Every non-eligible model is annotated with its reason (`no reference output checked in` — a documented prerequisite; or `unrelated blocker: <description>` — filed via `track-issue`).
  - Heavy models (e.g. `covid19-us-homer`) are `#[ignore]`d individually or the whole harness gated, measured per `docs/dev/rust.md`.
- **Test-prerequisite dependency:** the simulation tier's coverage depends on Vensim DSS reference outputs being generated for the metasd models — a documented setup task. The expansion tier (the "all 14 pass" half of the AC) is fully automated with no prerequisite; the simulation tier (the "those without unrelated blockers match reference output" half) is automated for whichever models have a checked-in reference, and the harness self-documents which models are not yet simulation-tier-eligible and why.
- Verification command: `cargo test -p simlin-engine --test metasd_macros` (plus `-- --ignored` if heavy models are gated).

### macros.AC6.5
**AC text:** *Success: The diagram opens every macro-bearing fixture without crashing.*

**Automated test:** e2e / diagram component (Jest) — **Phase 7, Task 3**.
- A test in `src/diagram/tests/editor-open-project.test.ts` following the existing `Object.create(Editor.prototype)` + `makeFakeEngine` + `openEngineProject` pattern — constructs a `validProjectJson` whose `models` array includes a macro-marked model (with a `macroSpec` and synthesized port variables) alongside a `main` model; asserts `openEngineProject()` resolves, `state.activeProject` is defined, and no exception is thrown. Optionally a `ModuleDetails` component test (`@testing-library/react`) asserting the macro-marked model name does not appear in the rendered `<select data-testid="model-ref-select">`.
- No production change is expected; if a test surfaces a real crash, the plan directs a minimal fix.
- Verification command: `pnpm --filter @simlin/diagram test`.

### macros.AC6.6
**AC text:** *Success: Macro-marked models don't appear as standalone, navigable models in the diagram's model list.*

**Automated test:** Unit / diagram pure-logic (Jest) — **Phase 7, Task 3**.
- A test in `src/diagram/tests/module-details-utils.test.ts` (`@jest-environment node`, pure-logic) — extends the `makeModel` helper to take an optional `macroSpec`; builds a `project` whose `models` include a `main` model, an ordinary submodel, and a macro-marked model; asserts `getAvailableModels(project, 'main').projectModels` contains the ordinary submodel but **not** the macro-marked model. Plus an `isMacroModel` unit test (the named, reusable predicate added to `src/diagram/module-navigation.ts`).
- Verification command: `pnpm --filter @simlin/diagram test`.

---

## Coverage Summary

All **37** acceptance criteria (the design plan's literal enumeration; the brief's "36" is a minor undercount — see the header note) map to an automated test. **0** require human verification.

| AC | Test type | Phase.Task | Verification command |
|----|-----------|------------|----------------------|
| macros.AC1.1 | Integration (converter) | P2.T5, P2.T7 | `cargo test -p simlin-engine convert` |
| macros.AC1.2 | Unit (parser) + Integration (converter) | P2.T2, P2.T5 | `cargo test -p simlin-engine mdl::parser`, `cargo test -p simlin-engine convert` |
| macros.AC1.3 | Integration (XMILE reader) + Integration (fixture) | P5.T1, P5.T4 | `cargo test -p simlin-engine xmile`, `cargo test -p simlin-engine --test simulate` |
| macros.AC1.4 | Unit (serde round-trip, ×5 layers) | P1.T2–T5 | `cargo test -p simlin-engine serde`, `… json`, `pnpm --filter @simlin/core test`, `uv run pytest tests/test_json_types.py` |
| macros.AC1.5 | Unit (helper) + Integration (converter) | P2.T6 | `cargo test -p simlin-engine convert` |
| macros.AC1.6 | Integration (error case) | P2.T7 | `cargo test -p simlin-engine convert` |
| macros.AC1.7 | Integration (converter) + gated hero | P2.T5, P2.T7, P7.T1 | `cargo test -p simlin-engine convert` |
| macros.AC2.1 | Integration (fixture) + Unit (smoke) | P3.T4, P3.T3 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.2 | Integration (fixture) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.3 | Integration (focused inline `.mdl`) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.4 | Integration (focused inline `.mdl`) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.5 | Integration (fixture) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.6 | Integration (fixture) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.7 | Integration (focused inline `.mdl`) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC2.8 | Integration (focused inline `.mdl`) | P3.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC3.1 | Integration (converter structure) | P4.T1 | `cargo test -p simlin-engine convert` |
| macros.AC3.2 | Integration (fixture) | P4.T1 | `cargo test -p simlin-engine --test simulate simulates_macro_multi_output_mdl` |
| macros.AC3.3 | Integration (corpus, gated for heavy) | P4.T3, P7.T2 | `cargo test -p simlin-engine --test simulate -- --ignored`, `… --test metasd_macros` |
| macros.AC3.4 | Integration (fixture) + Unit (predicate) | P4.T2, P3.T3 | `cargo test -p simlin-engine --test simulate macro_arrayed` |
| macros.AC3.5 | Integration (focused inline `.mdl`) | P4.T2 | `cargo test -p simlin-engine --test simulate macro_arrayed` |
| macros.AC4.1 | Unit (writer) + Integration (round-trip harness) | P6.T1, P6.T3 | `cargo test -p simlin-engine mdl::writer`, `… --test mdl_roundtrip` |
| macros.AC4.2 | Integration (XMILE writer + fixture round-trip) | P5.T2, P5.T3, P5.T4 | `cargo test -p simlin-engine xmile`, `… --test simulate` |
| macros.AC4.3 | Unit (writer) + Integration (round-trip harness) | P6.T2, P6.T3 | `cargo test -p simlin-engine mdl::writer`, `… --test mdl_roundtrip` |
| macros.AC4.4 | Integration (cross-format conversion) | P5.T4 | `cargo test -p simlin-engine --test simulate` |
| macros.AC4.5 | Integration (XMILE writer) | P5.T2, P5.T3 | `cargo test -p simlin-engine xmile` |
| macros.AC5.1 | Unit (expansion) + Integration (converter, multi-output path) | P3.T3, P4.T1 | `cargo test -p simlin-engine builtins_visitor`, `… convert` |
| macros.AC5.2 | Unit (registry-build) + Integration (compile failure) | P3.T1, P3.T2 | `cargo test -p simlin-engine module_functions`, `… model::` |
| macros.AC5.3 | Unit (registry-build) + Integration (compile failure) | P3.T1, P3.T2 | `cargo test -p simlin-engine module_functions`, `… model::` |
| macros.AC5.4 | Unit (expansion) + Integration (simulation) | P3.T3, P3.T4 | `cargo test -p simlin-engine builtins_visitor`, `… --test simulate` |
| macros.AC5.5 | Integration (fixture) | P3.T4, P2.T7 | `cargo test -p simlin-engine --test simulate` |
| macros.AC5.6 | Unit (expansion) | P3.T3 | `cargo test -p simlin-engine builtins_visitor` |
| macros.AC6.1 | Integration (fixture wiring, confirmed) | P3.T4, P5.T4, P6.T3, P7.T2 | `cargo test -p simlin-engine --test simulate`, `… --test mdl_roundtrip` |
| macros.AC6.2 | Integration (hero expansion, gated) | P7.T1, P4.T3 | `cargo test -p simlin-engine --test simulate -- --ignored` |
| macros.AC6.3 | Integration (focused fixtures) | P7.T1 | `cargo test -p simlin-engine --test simulate macro_clearn` |
| macros.AC6.4 | Integration (tiered corpus harness, partially gated) | P7.T2 | `cargo test -p simlin-engine --test metasd_macros` |
| macros.AC6.5 | e2e (diagram component, Jest) | P7.T3 | `pnpm --filter @simlin/diagram test` |
| macros.AC6.6 | Unit (diagram pure-logic, Jest) | P7.T3 | `pnpm --filter @simlin/diagram test` |

**Tally:**
- **Automated:** 37 / 37.
- **Human-verified:** 0 / 37.
- **Of the automated:** 3 ACs (`macros.AC3.3`, `macros.AC6.2`, `macros.AC6.4`) are wholly or partly **automated-but-gated** — their tests exist and run, but the heavy hero/corpus models are `#[ignore]`d and require an explicit `-- --ignored` opt-in command (per `docs/dev/rust.md` test-time-budget rules). `macros.AC1.7`'s C-LEARN-specific coverage and `macros.AC3.3`'s early gate are likewise gated, but each of those ACs is *also* covered by a non-gated test, so the AC itself is verified by the regular suite.

**Judgment calls made in this mapping (flagged per the task brief):**

1. **AC count: 37, not 36.** The design plan literally enumerates 37 AC cases (7+8+5+5+6+6). The brief says "36." This file covers all 37 and treats the design's enumeration as authoritative. If the intended count is 36, the most likely candidate for the "extra" is a perceived overlap between `macros.AC2.6` ("a macro that calls another macro ... expands recursively") and `macros.AC5.2` ("a directly or mutually recursive macro is rejected") — but these are genuinely distinct (cross-macro call vs. self/mutual recursion) and the design lists them separately, so both are kept.

2. **`.xmile` fixture tests are `TEST_MODELS` entries, not named functions.** For `macros.AC1.3` and `macros.AC4.2`, the "automated test" for the fixture half is the `TEST_MODELS` array entry in `simulate.rs` plus `simulate_path_with`'s built-in import / simulate / protobuf-round-trip / byte-stable-XMILE-round-trip assertions — there is no separately-named `#[test] fn` per `.xmile` fixture. The phase plans (Phase 5 Task 4, the round-trip-safety-net note) make this explicit; the mapping reflects it rather than inventing function names.

3. **`macros.AC4.4` and `macros.AC4.5` are writer/conversion tests, not fixture-round-trip tests.** There is no checked-in `.xmile` reference for the cross-format output and no standalone XMILE round-trip harness (`tests/roundtrip.rs` only parses, never re-emits). So AC4.4's automated test is a dedicated `.mdl → to_xmile → open_xmile` conversion test in `simulate.rs`, and AC4.5's is a string-content assertion on `to_xmile` output in `xmile/mod.rs`'s inline tests — both as the phase plans specify.

4. **AC5.1, AC5.2, AC5.3, AC5.4 each map to two tests at different layers.** The phase plans deliberately verify the detection logic at the unit layer (`module_functions.rs` registry-build tests, `builtins_visitor.rs` expansion tests) *and* the end-to-end surfacing at the integration layer (`model.rs` compile-failure tests, `simulate.rs` simulation tests). Both are listed; either alone would leave a coverage gap the plans explicitly close.

5. **`macros.AC6.3` and `macros.AC6.4` simulation tier depend on a documented test prerequisite (Vensim DSS reference outputs) — but are still classified automated, not human-verified.** The design's "Test prerequisites" note marks generating the reference `.vdf`s as a setup task, not implementation work. The automated tests exist and gate regardless: against the DSS reference if present, or against a formula-derived `output.tab` (AC6.3) / with the expansion tier as the always-runnable floor (AC6.4). The prerequisite affects the *strength* of the reference, not whether the criterion is automated. No AC is downgraded to human verification on this basis.

6. **`macros.AC6.1` has no single owning test — it is satisfied by accumulated wiring across Phases 3/5/6 and explicitly confirmed by Phase 7 Task 2.** The mapping lists all the contributing wiring sites plus the Phase 7 confirmation step rather than pointing at one test, because that is how the plan structures it.
