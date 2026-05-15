# Vensim Macro Support — Phase 5: XMILE import and export

**Goal:** Round-trip macros through the XMILE format — flesh out the `xmile::Macro` type, make the XMILE reader produce macro-marked `datamodel::Model`s from `<macro>` elements, and make the XMILE writer emit `<macro>` elements, the `<uses_macros>` header option, and `simlin:`-namespaced extensions for the multi-output forms that standard XMILE cannot express.

**Architecture:** XMILE handling is **hybrid and asymmetric**: reading uses `quick_xml::de` + serde `Deserialize` derives (so a `<macro>` sibling of `<model>` deserializes into the already-present `xmile::File.macros` field once `xmile::Macro` has fields); writing uses a hand-written `ToXml` trait + `quick_xml::Writer` (the `Serialize` derives are not used for XMILE output). A macro is a top-level `<macro>` element, a sibling of `<model>` — so the bridge between `xmile::File.macros` and the macro-marked entries of `datamodel::Project.models` lives at the `File ↔ Project` level, while the macro *body* (variables/views) reuses the existing per-`Model` conversion. Standard XMILE `<macro>` expresses a single-output macro (`<parm>` inputs, `<eqn>`/`<variables>` body); Vensim's `:`-multi-output form has no XMILE equivalent, so a multi-output macro's additional-output ports and a multi-output invocation's bindings round-trip through `simlin:`-namespaced extension elements. A single-output-only project therefore exports as standards-clean XMILE with no extensions.

**Tech Stack:** Rust — `src/simlin-engine/src/xmile/`. The XMILE `<macro>` specification is in-repo at `docs/reference/xmile-v1.0.html` (section 4.8). No external dependencies.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 5 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC1: Macro definitions parse and represent faithfully
- **macros.AC1.3 Success:** An XMILE `<macro>` element imports as a macro-marked `Model`; an expression-form `<eqn>` is normalized into a macro-named body variable.

### macros.AC4: Round-trip and export
- **macros.AC4.2 Success:** A macro-bearing XMILE file round-trips with `<macro>` elements and `simlin:` extensions; the `<uses_macros>` header option is emitted.
- **macros.AC4.4 Success:** A cross-format conversion (`.mdl` → datamodel → `.xmile`) preserves macro definitions and invocations.
- **macros.AC4.5 Edge:** A single-output-only model exports as standards-clean XMILE with no extensions; multi-output triggers the `simlin:` extension.

(`macros.AC4.1` and `macros.AC4.3` are MDL round-trip — Phase 6.)

---

## Current state (verified 2026-05-14)

**`xmile::Macro` is an empty stub, but `File` is already wired to deserialize `<macro>` siblings:**
- `xmile::Macro` (`xmile/mod.rs:334-338`) is `pub struct Macro { /* TODO */ }` — empty, with `Deserialize`/`Serialize` derived but no fields and no `ToXml` impl.
- `xmile::File` (`xmile/mod.rs:56-77`) already has `#[serde(rename = "macro", default)] pub macros: Vec<Macro>` — so `<macro>` elements already deserialize into `file.macros` once `Macro` has fields.

**XMILE is hybrid — read via serde derive, write via hand-written `ToXml`:**
- Reading: `project_from_reader` (`xmile/mod.rs:824-834`) → `quick_xml::de::from_reader` into `File` → `convert_file_to_project` → `datamodel::Project::from(file)`.
- Writing: `project_to_xmile` (`xmile/mod.rs:802-822`) → `project.clone().into()` (`From<datamodel::Project> for File`) → `file.write_xml()` (the `ToXml` trait, `xmile/mod.rs:25-32`). Write helpers: `write_tag_start`, `write_tag_start_with_attrs`, `write_tag_end`, `write_tag`, `write_tag_with_attrs` (`xmile/mod.rs:398-445`).
- `compat.rs`: `open_xmile` (`:76-78`) and `to_xmile` (`:32-34`) are thin pass-throughs.

**The XMILE `<macro>` element** (`docs/reference/xmile-v1.0.html` §4.8): a top-level child of `<xmile>`, sibling of `<model>`. **Required:** a `name` attribute and one `<eqn>` child. **Optional children:** `<parm>` (a formal parameter; optional `default` attribute; `<parm>`s appear before `<eqn>`), `<format>`, `<doc>`, `<sim_specs>` (only valid with `<variables>`), `<variables>` (same content model as a `<model>`'s `<variables>`), `<views>`; optional `namespace` attribute. §4.8.1 — expression-form: the body is a single `<eqn>` over the `<parm>`s. §4.8.2 — with-variables: extra stocks/flows/auxes in `<variables>`, each macro invocation getting its own independent instances. A macro is *invoked* like a builtin function call: `MACRONAME(arg1, arg2)`. XMILE has **no** native multi-output call syntax — the `:`-form is Vensim-specific.

**The four write-side gaps Phase 5 must fill:**
1. `File::write_xml` (`xmile/mod.rs:79-126`) does not iterate `self.macros` — hook point is between the `for model in self.models.iter()` loop (~line 121) and `write_tag_end(writer, "xmile")` (~line 125). `xmlns:simlin` is **already** declared on the root `<xmile>` (line 96).
2. `Header::write_xml` (`xmile/mod.rs:447-474`) emits only `<name>`/`<vendor>`/`<product>` — it writes **no** `<options>`/features at all. Emitting `<uses_macros>` is net-new code.
3. `From<datamodel::Project> for File` (`xmile/mod.rs:188-247`) hard-codes `macros: vec![]` (~line 244).
4. `From<File> for datamodel::Project` (`xmile/mod.rs:129-186`) never reads `file.macros` — `datamodel::Project.models` is populated solely from `file.models`.

**The `simlin:` extension pattern** (read with a serde rename to the namespace-*stripped* local name — quick-xml strips the `simlin:` prefix on deserialize; write by hand with the `simlin:`-prefixed tag). Reference implementations to mirror: `XmileLoopMetadata` (`xmile/model.rs:20-36`, with `#[serde(rename = "@name")]` etc.; written at `xmile/model.rs:106-119` via `write_tag_with_attrs(writer, "simlin:loop-metadata", ...)`) and the more complete `DataSourceElement` (`xmile/variables.rs:21-102` — has `from_datamodel`/`to_datamodel`/`write_xml`, deserialized via `#[serde(rename = "data_source", ...)]`).

**`Feature::UsesMacros`** (`xmile/mod.rs:510-513`) exists: `UsesMacros { recursive_macros: Option<bool>, option_filters: Option<bool> }`, read inside `Options.features` (`#[serde(rename = "$value")]`, `xmile/mod.rs:542`) on `Header.options`. **Latent bug:** the two fields lack `#[serde(rename = "@...")]`, so they currently deserialize as child *elements*, not the spec-required *attributes* — Phase 5 (Task 2) fixes this as part of emitting `<uses_macros>` correctly.

**The per-`Model` bridges** (`xmile/model.rs`): `From<xmile::Model> for datamodel::Model` (`:126-205`) and `From<datamodel::Model> for xmile::Model` (`:207-289`) — reusable for the macro *body* (a macro body is structurally an ordinary model's `variables`/`views`), but a macro's `name`/`<parm>`s/`<eqn>`/`MacroSpec`/additional-output extensions live on the `Macro` type, not `Model`. After Phase 1, `datamodel::Model` has a `macro_spec: Option<MacroSpec>` field these impls will need to default/populate.

**Fixture reality (corrected):** the `.xmile` macro fixtures **do** contain `<macro>` elements. `grep -rl "<macro" test/` returns exactly four files: `test/test-models/tests/macro_expression/test_macro_expression.xmile`, `.../macro_multi_expression/test_macro_multi_expression.xmile`, `.../macro_multi_macros/test_macro_multi_macros.xmile` (two `<macro>` elements), `.../macro_stock/test_macro_stock.xmile` (a stock-bearing macro body). The `.stmx` files and the `macro_cross_reference`/`macro_trailing_definition` directories have **no** `<macro>` element (xmutil dropped it / those dirs have no `.xmile`). The xmutil-emitted `<macro>` shape has *both* `<parm>` children and a `<variables>` body, with `<eqn>` holding the macro name (e.g. `macro_expression.xmile` lines 47-77: `<macro name="EXPRESSION MACRO"><eqn>EXPRESSION MACRO</eqn><parm>input</parm><parm>parameter</parm><variables><aux name="EXPRESSION MACRO"><eqn>input*parameter</eqn>...</variables></macro>`). The real `<macro>` element also carries a `<doc>` child and a `<views>` child (a macro-body diagram) — `<doc>` is modeled by `xmile::Macro` (Task 1), `<views>` is **deliberately not** (see Task 1's "No `views` field" note). **No fixture anywhere uses `<uses_macros>`.** Of these four files, `simulate.rs`'s `TEST_MODELS` static array (`tests/simulate.rs:22-36`) already lists exactly **three** as commented-out entries (lines 27-29: `macro_expression`, `macro_multi_expression`, `macro_stock`) — `macro_multi_macros` is **not** present in the array at all. So Task 4 *uncomments* three and *adds* a fourth new entry.

**Round-trip safety net:** `simulate_path_with` (`tests/simulate.rs:189-251`) already performs a byte-stable XMILE round-trip assertion — it serializes the project via `project_to_xmile`, re-parses, re-serializes, and `assert_eq!`s the two serializations. So **wiring a macro `.xmile` fixture into `simulate.rs`'s `TEST_MODELS` automatically exercises an XMILE→datamodel→XMILE byte-stable round-trip.** There is no standalone XMILE round-trip harness (unlike `mdl_roundtrip.rs`); `tests/roundtrip.rs` only parses XMILE, never re-emits. MDL→XMILE conversion is untested anywhere.

**Documented limitation (per the design):** XMILE per-macro `<sim_specs>` — a macro running with its own dt/stop time — is **not supported**. `xmile::Macro` carries an optional `sim_specs` field for parse-completeness/round-trip, but a `<macro>` with a non-empty `<sim_specs>` is rejected with a clear error at conversion time.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: XMILE macro read/write

<!-- START_TASK_1 -->
### Task 1: Flesh out `xmile::Macro` and the XMILE reader

**Verifies:** macros.AC1.3.

**Files:**
- Modify: `src/simlin-engine/src/xmile/mod.rs` (replace the empty `Macro` stub ~lines 334-338; extend `From<File> for datamodel::Project` ~lines 129-186)
- Possibly modify: `src/simlin-engine/src/mdl/convert/` and/or `src/simlin-engine/src/datamodel.rs` (extract Phase 2's port-variable-synthesis logic into a shared helper if it is not already callable from outside `mdl/convert/`)

**Implementation:**

1. **Define `xmile::Macro`** as a serde-`Deserialize` struct mirroring the established `xmile::Model`/`xmile::Var` serde patterns. **First, drop the `Eq` derive from the existing stub:** it currently derives `#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]` — it can derive `Eq` only because it is empty. The new `variables`/`sim_specs` fields transitively contain `f64` (which is not `Eq`), so keeping `Eq` will not compile. Change the derive to match `xmile::Model` exactly — `#[derive(Clone, PartialEq, Deserialize, Serialize)]` (no `Eq`), keeping the existing `#[cfg_attr(feature = "debug-derive", derive(Debug))]` line above it. Then add the fields:
   - `#[serde(rename = "@name")] name: String`
   - `#[serde(rename = "parm", default)] parms: Vec<Parm>` — a `Parm` struct with `#[serde(rename = "$text")] name: String` and `#[serde(rename = "@default", skip_serializing_if = "Option::is_none", default)] default: Option<String>`
   - `#[serde(rename = "eqn")] eqn: Option<String>` — the expression-form body / primary-output expression (`<eqn>`)
   - `variables: Option<Variables>` — the multi-equation body (reuses the existing `xmile::Variables` type)
   - `sim_specs: Option<SimSpecs>` — for parse-completeness; a non-empty value is rejected at conversion (the documented limitation)
   - `doc: Option<String>`, `#[serde(rename = "@namespace")] namespace: Option<String>` — round-tripped for fidelity
   - **No `views` field — deliberate.** The xmutil-emitted `<macro>` carries a `<views>` child (a macro-body diagram), but macro models are non-navigable (AC6.6), so a macro body's views are inert. `xmile::Macro` deliberately does **not** model `<views>`: `quick_xml::de` silently ignores the unknown element on read, and the Task 2 writer never emits one. This is a documented intentional non-round-trip — it does **not** break Task 4's byte-stable round-trip (the `<views>` is dropped on the *first* `project_to_xmile` too, so both serializations in `simulate_path_with`'s `assert_eq!` agree), but an engineer diffing a wired fixture's emitted output against the original `.xmile` file will see the macro `<views>` is gone, and that is expected.
   - The `simlin:`-namespaced additional-output extension field is added in **Task 3** (leave it out here; Task 3's serde field uses `#[serde(default)]` so Task 1's reader compiles without it).

2. **Bridge `xmile::Macro` → a macro-marked `datamodel::Model`.** In `From<File> for datamodel::Project`, after building `models` from `file.models`, convert each `file.macros` entry to a macro-marked `datamodel::Model` and append it to `project.models`. Conversion:
   - **Body:** if `<variables>` is present, the body is those variables (reuse the existing `xmile::Variables` → `datamodel::Variable` conversion). If `<variables>` is absent (expression-form, §4.8.1), **normalize the `<eqn>` into a body variable named after the macro** — a `Variable::Aux { ident: <canonical macro name>, equation: Scalar(<eqn text>), .. }` (this is the AC1.3 "expression-form `<eqn>` is normalized into a macro-named body variable" requirement).
   - **`primary_output`:** the canonical macro name. (For the with-`<variables>` xmutil shape where `<eqn>` is literally the macro name, this is consistent. If `<eqn>` is an expression that does *not* name an existing body variable, also normalize it into a macro-named body variable as above and use that as the primary output.)
   - **Port variables + `MacroSpec`:** **call the shared `Model::new_macro` function that Phase 2 Task 5 created** (the named, reusable `pub(crate)` helper in `datamodel.rs` or a small shared module — `Model::new_macro(macro_name, parameters, additional_outputs, body_variables) -> Model`). The XMILE reader's job is just to build the inputs: the macro name (the `<macro name>` attribute, canonicalized), the parameter names (the `<parm>` names, canonical, in order), `additional_outputs` (empty here — Task 3 populates it from the `simlin:` extension), and `body_variables` (built from `<variables>`/`<eqn>` per the **Body** bullet above). `Model::new_macro` then synthesizes the port variables (placeholder `Equation::Scalar("0")`, `compat.can_be_module_input = true`, kind by body usage) and attaches the `MacroSpec` — identically to the MDL path. Do not re-implement port synthesis in `xmile/`.
   - **Per-macro `<sim_specs>`:** if `macro.sim_specs` is `Some` and non-empty, return a clear error (per-macro `<sim_specs>` is the documented unsupported limitation) — use the XMILE layer's existing error type.

**Testing:**
- **macros.AC1.3** (`xmile/` inline `#[cfg(test)]` tests, or a focused test using `open_xmile` on an in-test XMILE string): an XMILE string with a `<macro name="MYMACRO"><parm>a</parm><parm>b</parm><eqn>a * b</eqn></macro>` (expression-form, no `<variables>`) imports as a `datamodel::Project` containing a macro-marked `Model` whose `MacroSpec.parameters == ["a", "b"]`, with a body variable named `mymacro` carrying the equation `a * b` (the normalized `<eqn>`) and synthesized port variables `a`/`b` (`can_be_module_input == true`).
- A `<macro>` with a `<variables>` body imports with the `<variables>` as the body and the `<eqn>`-named variable as `primary_output`.
- A `<macro>` with a non-empty `<sim_specs>` returns the documented-limitation error.

**Verification:**
Run: `cargo test -p simlin-engine xmile`
Expected: all pass, including the new `<macro>` reader tests.

**Commit:** `engine: read XMILE <macro> elements as macro-marked models`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: XMILE writer for `<macro>` elements and the `<uses_macros>` header option

**Verifies:** macros.AC4.2 (definition + header-option emission), macros.AC4.5 (single-output standards-clean).

**Files:**
- Modify: `src/simlin-engine/src/xmile/mod.rs` (`impl ToXml for Macro`; `File::write_xml` ~lines 79-126; `Header::write_xml` ~lines 447-474 — or the header path in `File::write_xml`; `From<datamodel::Project> for File` ~lines 188-247; `Feature::UsesMacros` ~lines 510-513)

**Implementation:**

1. **`impl ToXml<XmlWriter> for Macro`** — hand-written, mirroring `Model::write_xml` and the write-helper style. Emit `<macro name="...">`, then the `<parm>` children (with `default` attributes where present), then `<eqn>`, then `<variables>` (looping `Var::write_xml` like `Model::write_xml` does), then `<doc>` if present, then `</macro>`. The `simlin:` additional-output extension emission is added in Task 3.

2. **`File::write_xml`** — add `for macro in self.macros.iter() { macro.write_xml(writer)?; }` between the models loop and `write_tag_end(writer, "xmile")`.

3. **`From<datamodel::Project> for File`** — replace `macros: vec![]` with a real population: partition `project.models` by `model.macro_spec.is_some()`; macro-marked models become `file.macros` entries (`datamodel::Model` → `xmile::Macro`: name from `model.name`, `<parm>`s from `MacroSpec.parameters`, `<variables>` from the body variables *excluding the synthesized port variables* — the port variables are reconstructed from the `<parm>`s on re-import, so emitting them in `<variables>` too would be redundant and break round-trip stability; `<eqn>` = the `MacroSpec.primary_output` name), non-macro models stay in `file.models`.

4. **Emit `<uses_macros>`** — when `project.models` contains at least one macro, emit the `<uses_macros recursive_macros="false" option_filters="false"/>` header option (Simlin does not support recursive macros, and emits both attributes as fixed `"false"` — a deterministic emission, which keeps the byte-stable round-trip stable). This requires writing the `<header>`'s `<options>` block, which `Header::write_xml` does not do today — add the minimal options-writing code. **Also fix the `Feature::UsesMacros` serde field renames** to `#[serde(rename = "@recursive_macros")]` / `#[serde(rename = "@option_filters")]` so the *reader* parses the spec-correct attribute form that the writer now emits (without this, read and write disagree and the round-trip is lossy).

5. **AC4.5 — standards-clean for single-output:** a macro with empty `MacroSpec.additional_outputs` emits a plain standard `<macro>` with **no** `simlin:`-namespaced child elements. (The `simlin:` additional-output extension — Task 3 — is emitted only when `additional_outputs` is non-empty.)

**Testing:**
- **macros.AC4.2** (definition): a `datamodel::Project` with a single-output macro-marked `Model` (built in-test, or imported from a `.xmile`/`.mdl` fixture) → `to_xmile` → assert the output string contains a `<macro name="...">` element with the expected `<parm>`s and body, and a `<uses_macros recursive_macros="false" option_filters="false"/>` header option.
- **macros.AC4.5** (single-output standards-clean): assert the `to_xmile` output of a single-output-only macro project contains **no** `simlin:`-prefixed macro-extension element (it may still contain other pre-existing `simlin:` elements like `simlin:loop-metadata`; assert specifically that no macro-additional-output extension is present).
- Round-trip: `to_xmile` → `open_xmile` → assert the macro-marked model survives with the same `MacroSpec` and body.

**Verification:**
Run: `cargo test -p simlin-engine xmile`
Expected: all pass.

**Commit:** `engine: write XMILE <macro> elements and the uses_macros option`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `simlin:` extensions for multi-output macros — read and write

**Verifies:** macros.AC4.2 (`simlin:` extensions), macros.AC4.5 (multi-output triggers the extension).

**Files:**
- Modify: `src/simlin-engine/src/xmile/mod.rs` (`xmile::Macro` — add the additional-output extension field; `Macro` reader bridge and `ToXml` impl; the multi-output-invocation extension)
- Possibly create: a small `simlin:`-extension type (mirroring `DataSourceElement` in `xmile/variables.rs`)

**Implementation:**

Standard XMILE `<macro>` has no concept of multiple *output* ports, and no concept of a multi-output *invocation*. Both round-trip through `simlin:`-namespaced extension elements (the established pattern: deserialize via a serde rename to the namespace-stripped local name; serialize by hand with the `simlin:`-prefixed tag). Design the two extension elements; the contract is round-trip fidelity and that single-output projects never emit them.

1. **Additional-output ports on a macro definition.** A multi-output macro imported from `.mdl` has a non-empty `MacroSpec.additional_outputs`. Add a `simlin:`-namespaced child element on `<macro>` (e.g. `<simlin:additional-outputs>` listing the additional output port names in order). Wire it: a serde field on `xmile::Macro` (`#[serde(rename = "additional-outputs", default)]`), emitted in `Macro::write_xml` (`write_tag_with_attrs(writer, "simlin:additional-outputs", ...)`), populated from / into `MacroSpec.additional_outputs` in the `Macro ↔ Model` bridge. Emitted **only** when `additional_outputs` is non-empty.

2. **Multi-output invocations.** After Phase 4, a multi-output invocation `total = add3(a,b,c : minv, maxv)` is materialized in the datamodel as a `Variable::Module` (with input-only `ModuleReference`s, `model_name` = the macro's model) plus binding `Variable::Aux`es (the LHS aux reads `<module>.<primary_output>`, each `:`-list aux reads `<module>.<additional_output>` — ASCII period, the datamodel separator form per Phase 4's authoritative separator note; the XMILE layer reads the datamodel, so it matches against `.`). Standard XMILE `<module>` references a `<model>`, not a `<macro>` — so this materialized cluster round-trips through a `simlin:`-namespaced extension that records the invocation (the macro name, the argument wiring, and the output bindings) faithfully enough that the reader reconstructs exactly the same `Variable::Module` + binding `Aux`es. Wire it read + write. Emitted **only** for modules whose `model_name` resolves to a macro-marked model.

3. The reader side of both extensions reconstructs the datamodel shape (the macro `Model`'s `additional_outputs`; the `Variable::Module` + binding auxes) so an XMILE→datamodel→XMILE round-trip is byte-stable.

**Testing:**
- **macros.AC4.5** (multi-output triggers the extension): a `datamodel::Project` with a multi-output macro (non-empty `MacroSpec.additional_outputs`) and a multi-output invocation → `to_xmile` → assert the output contains the `simlin:` additional-outputs extension on the `<macro>` and the `simlin:` multi-output-invocation extension; and (contrast) confirm a single-output project does **not**.
- **macros.AC4.2** (`simlin:` extension round-trip): a multi-output macro project → `to_xmile` → `open_xmile` → assert the round-tripped project has the same macro `MacroSpec.additional_outputs` and the same materialized `Variable::Module` + binding `Aux`es as the original; and a second `to_xmile` of the round-tripped project is byte-identical to the first (byte-stable).
- Multi-output cross-format: `open_vensim` a multi-output `.mdl` (a focused `:`-form fixture, or `test/metasd/theil-statistics/Theil_2011.mdl`) → `to_xmile` → `open_xmile` → assert the multi-output macro and its invocation survived.

**Verification:**
Run: `cargo test -p simlin-engine xmile`
Expected: all pass, including the multi-output `simlin:`-extension round-trip tests.

**Commit:** `engine: round-trip multi-output macros through simlin: XMILE extensions`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (task 4) -->
## Subcomponent B: Fixture wiring and cross-format verification

<!-- START_TASK_4 -->
### Task 4: Wire the `.xmile` macro fixtures and the cross-format test

**Verifies:** macros.AC1.3 (the fixtures import and simulate), macros.AC4.2 (round-trip), macros.AC4.4 (cross-format).

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (uncomment the three commented-out macro `.xmile` fixtures at `TEST_MODELS` lines 27-29 and add a fourth new entry for `macro_multi_macros`; add a cross-format test)

**Implementation:** Tests only — if a test surfaces a real gap, fix it in `xmile/`, but after Tasks 1-3 the expectation is that the `.xmile` macro fixtures import, simulate, and round-trip.

1. **Wire all four `.xmile` macro fixtures into `TEST_MODELS`** (`simulate.rs` lines 22-36). Three are *already present* as commented-out entries at lines 27-29 — **uncomment** them: `test/test-models/tests/macro_expression/test_macro_expression.xmile`, `.../macro_multi_expression/test_macro_multi_expression.xmile`, `.../macro_stock/test_macro_stock.xmile`. The fourth, `test/test-models/tests/macro_multi_macros/test_macro_multi_macros.xmile` (two `<macro>` elements), is **not** in the array at all — **add** it as a new entry alongside the other macro fixtures. Because `simulate_path_with` runs each `TEST_MODELS` entry through import → simulate → protobuf round-trip → **byte-stable XMILE round-trip** (`tests/simulate.rs:189-251`), wiring these four fixtures in exercises, for each: the Task 1 reader (the `<macro>` imports as a macro-marked model), Phase 3's expansion (the invocation simulates and matches `output.tab`), and the Task 2 writer's byte-stability (`project_to_xmile` is stable across a re-parse). The `.stmx` variants stay out (no `<macro>` element); `macro_cross_reference`/`macro_trailing_definition` have no `.xmile`.

2. **Cross-format `.mdl` → datamodel → `.xmile` test (macros.AC4.4):** add a test that `open_vensim`s a single-output macro `.mdl` fixture (e.g. `test/test-models/tests/macro_expression/test_macro_expression.mdl`), converts the resulting `datamodel::Project` to XMILE via `to_xmile`, re-imports via `open_xmile`, and asserts the macro definition (the macro-marked `Model` + its `MacroSpec`) and the invocation are preserved — i.e. the cross-format-round-tripped project's macro models and invocation equations match those of the directly-imported `.mdl` project.

**Testing:**
- **macros.AC1.3 / macros.AC4.2:** the four wired `.xmile` fixture tests pass — each imports, simulates to its `output.tab`, and round-trips byte-stably through XMILE.
- **macros.AC4.4:** the cross-format test passes — `.mdl` → datamodel → `.xmile` → datamodel preserves the macro definitions and invocations.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate`
Expected: all pass, including the four newly-wired macro `.xmile` fixtures and the cross-format test.

**Commit:** `engine: wire XMILE macro fixtures and the cross-format conversion test`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

---

## Phase 5 completion check

When all four tasks are committed:
- `xmile::Macro` is a real type; an XMILE `<macro>` element imports as a macro-marked `datamodel::Model`, and an expression-form `<eqn>` is normalized into a macro-named body variable (the design's "Done when": the `.xmile` macro fixtures import).
- The XMILE writer emits `<macro>` elements, the `<uses_macros>` header option, and `simlin:`-namespaced extensions for multi-output additional outputs and invocation bindings; a single-output-only project exports as standards-clean XMILE with no extensions.
- A macro-bearing XMILE file round-trips byte-stably (the four wired fixtures, via `simulate_path_with`'s round-trip assertion); a cross-format `.mdl` → datamodel → `.xmile` conversion preserves macro definitions and invocations (the design's "Done when").
- `macros.AC1.3`, `macros.AC4.2`, `macros.AC4.4`, `macros.AC4.5` are verified.

MDL export (the `:MACRO:` writer and reconstructing the `:` call syntax) is Phase 6; the corpus harness is Phase 7.
