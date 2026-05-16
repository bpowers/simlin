# Vensim Macro Support — Phase 1: Datamodel and serialization foundation

**Goal:** Represent macros in the datamodel with a new `MacroSpec` marker and persist it losslessly through protobuf and JSON across every consumer package.

**Architecture:** Add a `MacroSpec` struct and an `Option<MacroSpec>` field to the `Model` datamodel type, then mirror it through the five existing serialization layers exactly as the `LoopMetadata` nested-struct precedent does: the Rust protobuf path (`project_io.proto` + `serde.rs`), the Rust JSON path (`json.rs`, which also drives the auto-generated JSON schema), the TypeScript mirrors (`json-types.ts` + `datamodel.ts`), and the Python mirrors (`json_types.py` + `json_converter.py`). No import/export or simulation logic is touched in this phase — only representation and round-trip.

**Tech Stack:** Rust (prost protobuf codegen, serde, schemars JSON-schema derive), TypeScript (Jest), Python (cattrs, pytest).

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 1 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC1: Macro definitions parse and represent faithfully
- **macros.AC1.4 Success:** A macro-bearing project round-trips losslessly through protobuf and through JSON -- `MacroSpec` and macro body are identical after deserialize.

**Note on scope within AC1:** Phase 1 establishes the *representation* and proves the *round-trip* half of `macros.AC1.4`. It does so by constructing `MacroSpec` values directly in test code (not via import). The remaining AC1 cases — `AC1.1`, `AC1.2`, `AC1.5`, `AC1.6`, `AC1.7` (MDL import) and `AC1.3` (XMILE import) — are completed in Phases 2 and 5, which build on the representation this phase adds. In Phase 1 the "macro body" referenced by AC1.4 is simply the model's ordinary `variables`, which already round-trips; the new persisted element is `MacroSpec`.

---

## Background: the two Rust `Model` representations

There are **two** distinct Rust structs involved, and `MacroSpec` must persist through **both**:

1. **`datamodel::Model`** (`src/simlin-engine/src/datamodel.rs`) — the in-memory model. `src/simlin-engine/src/serde.rs` converts it to/from **`project_io::Model`** (the prost-generated protobuf type). This is the **protobuf** persistence path (the project DB stores serialized protobuf, so changes must be additive).

2. **`json::Model`** (`src/simlin-engine/src/json.rs`) — a separate serde struct (`#[serde(rename_all = "camelCase")]`). `json.rs` converts it to/from `datamodel::Model`. `json::Model` is the single source of truth for (a) the TypeScript `JsonModel` shape in `src/engine/src/json-types.ts`, (b) the Python `Model` dataclass in `src/pysimlin/simlin/json_types.py`, and (c) the JSON Schema at `docs/simlin-project.schema.json`, which is **auto-generated** by a test in `json_proptest.rs` (gated `#[cfg(all(test, feature = "schema"))]`; `schema` is a default feature, so plain `cargo test -p simlin-engine` regenerates it).

**Generated files — never hand-edit:**
- `src/simlin-engine/src/project_io.gen.rs` — regenerate with `pnpm build:gen-protobufs`.
- `docs/simlin-project.schema.json` — regenerates itself when `cargo test -p simlin-engine` runs the `generate_and_write_schema` test.

**The `LoopMetadata` precedent.** `LoopMetadata` is a nested struct on `Model` that already threads through all five layers. Every task below points at the exact `LoopMetadata` lines to mirror. `MacroSpec` differs from `LoopMetadata` in one way: it is a *singular optional* field (`Option<MacroSpec>`), not a `Vec`. For the singular-optional shape, the precedent is `Model.sim_specs: Option<SimSpecs>` (in `datamodel.rs`) and `json::Model.sim_specs` (`#[serde(skip_serializing_if = "Option::is_none", default)]` in `json.rs`).

The `MacroSpec` shape (from the design plan):

```rust
pub struct MacroSpec {
    pub parameters: Vec<String>,         // formal parameters, in positional calling order
    pub primary_output: String,          // body variable the call-site LHS receives
    pub additional_outputs: Vec<String>, // the ':'-list outputs, usually empty
}
```

All three fields are `Eq`-able (no `f64`), so `MacroSpec` derives `Eq`. `datamodel::Model` itself derives only `Clone, PartialEq` (it transitively contains `f64`); adding an `Option<MacroSpec>` field does not change that.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
## Subcomponent A: Rust protobuf persistence

<!-- START_TASK_1 -->
### Task 1: Define `MacroSpec` and add `Model.macro_spec`

**Verifies:** None (type definition; the round-trip it enables is tested in Tasks 2-5).

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs` (add `MacroSpec` struct near the `Model` struct, ~line 783; add field to `Model`, ~lines 783-792)
- Modify: every other file in the workspace that constructs a `datamodel::Model { ... }` literal — these are discovered by the compiler, not guessed (see Step 2). Known sites include `src/simlin-engine/src/serde.rs` (`From<project_io::Model> for Model`, ~line 2069), `src/simlin-engine/src/json.rs` (`From<json::Model> for datamodel::Model`, ~line 1185), `src/simlin-engine/src/mdl/convert/variables.rs` (~line 88), the XMILE reader under `src/simlin-engine/src/xmile/`, `src/simlin-engine/src/test_common.rs` / `testutils.rs`, and inline `#[cfg(test)]` fixtures in `serde.rs` and `json.rs`.

**Implementation:**

Step 1 — In `datamodel.rs`, immediately before the `pub struct Model` definition, add the `MacroSpec` struct. Mirror the derive attributes of the neighboring `LoopMetadata` struct (`#[cfg_attr(feature = "debug-derive", derive(Debug))]` + `#[derive(Clone, PartialEq, Eq)]`). Give it concise rustdoc explaining that `Some` on `Model.macro_spec` marks the model as a callable macro template, that the model's `variables` are the macro body, and what each field means:

```rust
/// Marks a [`Model`] as a callable macro template rather than an ordinary
/// model, and records its calling convention. A macro definition is an
/// ordinary model whose `variables` are the macro body; this spec names which
/// body variables are the formal parameters and which are the outputs.
/// `Model.macro_spec` is `None` for every non-macro model.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct MacroSpec {
    /// Formal parameter names, in positional calling order. Each names a body
    /// variable that a macro invocation binds an argument to.
    pub parameters: Vec<String>,
    /// The body variable whose value the call-site left-hand side receives.
    pub primary_output: String,
    /// Additional named outputs from Vensim's `:`-list multi-output call
    /// syntax, in declaration order. Empty for ordinary single-output macros.
    pub additional_outputs: Vec<String>,
}
```

Step 2 — Add the field to `Model`, after `pub groups: Vec<ModelGroup>,`:

```rust
    /// `Some` if this model is a callable macro template. See [`MacroSpec`].
    pub macro_spec: Option<MacroSpec>,
```

Step 3 — Make the workspace compile again. Adding a field to `Model` breaks every struct-literal construction of `datamodel::Model`. Run `cargo build --workspace --all-targets` and, for each `error[E0063]: missing field \`macro_spec\``, add `macro_spec: None` to that `Model { ... }` literal. Repeat until the build is clean. Do **not** use `..Default::default()` — `datamodel::Model` does not derive `Default`, and the surrounding code uses explicit field lists. `cargo build --workspace` includes the `pysimlin` Rust crate, so if it constructs any `datamodel::Model` literal the compiler flags it here — there is **no** separate Rust-side `pysimlin` datamodel mirror to hand-edit (pysimlin consumes the engine through the JSON bridge, which Task 5 handles on the Python side).

**Testing:** None. This task only defines a type and adds a field; no behavior to test yet. The compiler is the verifier.

**Verification:**
Run: `cargo build --workspace --all-targets`
Expected: compiles with no errors.

**Commit:** `engine: add MacroSpec type to datamodel`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `MacroSpec` protobuf message and serde conversion

**Verifies:** macros.AC1.4 (protobuf round-trip).

**Files:**
- Modify: `src/simlin-engine/src/project_io.proto` (add `message MacroSpec`; add field 7 to `message Model`, ~lines 317-325)
- Regenerate: `src/simlin-engine/src/project_io.gen.rs` (and any TypeScript server protobuf the script touches) — via `pnpm build:gen-protobufs`, never by hand
- Modify: `src/simlin-engine/src/serde.rs` (add `From` impls for `MacroSpec` near the `LoopMetadata` impls ~line 1989; wire `macro_spec` into both `Model` conversion impls ~lines 1951-1987 and ~2069-2091; add a round-trip test near `test_model_with_loop_metadata_roundtrip` ~line 2093)

**Implementation:**

Step 1 — In `project_io.proto`, add a top-level `message MacroSpec` next to `message LoopMetadata` (~line 299). Nested messages in this file are declared top-level, not inside `Model`:

```proto
message MacroSpec {
  repeated string parameters = 1;
  string primary_output = 2;
  repeated string additional_outputs = 3;
}
```

Then add a field to `message Model`. Fields 1,3,4,5,6 are in use; field 2 was historically skipped; **use field 7** (the next sequential number — consistent with how this file has grown):

```proto
  MacroSpec macro_spec = 7;
```

A singular message field in proto3 is presence-tracked, so prost generates it as `::core::option::Option<MacroSpec>` — no `optional` keyword needed. Verify this in the regenerated `.gen.rs` in Step 2.

Step 2 — Regenerate the bindings: `pnpm build:gen-protobufs`. Then run `git status` and stage every file it changed (at minimum `project_io.gen.rs`; the script also runs `pnpm format`). Confirm `project_io.gen.rs` now has `pub macro_spec: ::core::option::Option<MacroSpec>` on `Model` and a `MacroSpec` message with `#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]`.

Step 3 — In `serde.rs`, add the two `From` impls converting `datamodel::MacroSpec` ⇄ `project_io::MacroSpec`, mirroring the `LoopMetadata` impls at ~line 1989. The body is a field-by-field move (all three fields have identical names and types on both sides).

Step 4 — Wire `macro_spec` into the two `Model` conversion impls:
- In `From<Model> for project_io::Model` (~lines 1951-1987): add `macro_spec: model.macro_spec.map(project_io::MacroSpec::from),` to the constructed `project_io::Model`.
- In `From<project_io::Model> for Model` (~lines 2069-2091): change the placeholder `macro_spec: None` (added in Task 1) to `macro_spec: model.macro_spec.map(MacroSpec::from),`.

**Testing:**
Add `test_model_with_macro_spec_roundtrip` next to `test_model_with_loop_metadata_roundtrip` (`serde.rs` ~line 2093). It must verify **macros.AC1.4 (protobuf half)**: construct a `Model` with `macro_spec: Some(MacroSpec { ... })` populated with non-empty `parameters`, a non-empty `primary_output`, and non-empty `additional_outputs`, plus at least one body `Variable` (so the "macro body" is also exercised). Round-trip it via `Model::from(project_io::Model::from(expected.clone()))` and `assert_eq!`. Follow the exact `cases: &[Model]` + loop shape of the neighboring tests. Also add one case (or a second test) with `macro_spec: None` to confirm a non-macro model still round-trips.

**Verification:**
Run: `cargo test -p simlin-engine serde`
Expected: all `serde` tests pass, including `test_model_with_macro_spec_roundtrip`.

**Commit:** `engine: persist MacroSpec through protobuf`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3) -->
## Subcomponent B: Rust JSON persistence and schema

<!-- START_TASK_3 -->
### Task 3: `json::MacroSpec`, JSON serde, and schema regeneration

**Verifies:** macros.AC1.4 (JSON round-trip).

**Files:**
- Modify: `src/simlin-engine/src/json.rs` (add `json::MacroSpec` struct near `json::LoopMetadata` ~line 551; add `macro_spec` field to `json::Model` ~lines 455-481; add `From` impls both ways near the `LoopMetadata` impls ~lines 1263 and ~1921; wire `macro_spec` into `From<json::Model> for datamodel::Model` ~line 1185 and `From<datamodel::Model> for json::Model` ~line 1818; extend the `json::Model` test fixtures and `test_model_roundtrip` ~line 2693)
- Regenerate: `docs/simlin-project.schema.json` — by running `cargo test -p simlin-engine` (the `generate_and_write_schema` test rewrites it), never by hand
- Possibly modify: `src/simlin-engine/src/json_proptest.rs` (only if it has a `json::Model` generation strategy — see Step 5)

**Implementation:**

Step 1 — In `json.rs`, add the `json::MacroSpec` struct next to `json::LoopMetadata` (~line 551). Mirror `json::LoopMetadata`'s attributes exactly: `#[cfg_attr(feature = "debug-derive", derive(Debug))]`, `#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]`, `#[cfg_attr(feature = "schema", derive(JsonSchema))]`, `#[serde(rename_all = "camelCase")]`. Use the file's existing `is_empty_vec` helper for `additional_outputs`:

```rust
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct MacroSpec {
    pub parameters: Vec<String>,
    pub primary_output: String,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub additional_outputs: Vec<String>,
}
```

Step 2 — Add the field to `json::Model` (~lines 455-481), after `groups`. Use the *optional-singular* serde idiom from `json::Model.sim_specs`:

```rust
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub macro_spec: Option<MacroSpec>,
```

Step 3 — Add the two `From` impls converting `json::MacroSpec` ⇄ `datamodel::MacroSpec`, mirroring the `json::LoopMetadata` impls at ~lines 1263 and ~1921. Field-by-field move.

Step 4 — Wire `macro_spec` into the two `Model` conversion impls:
- In `From<json::Model> for datamodel::Model` (~lines 1185-1215): change the placeholder `macro_spec: None` (from Task 1) to `macro_spec: model.macro_spec.map(|m| m.into()),`.
- In `From<datamodel::Model> for json::Model` (~lines 1818-1856): add `macro_spec: model.macro_spec.map(|m| m.into()),`.
- Adding the field to `json::Model` breaks every `json::Model { ... }` literal in `json.rs`'s own `#[cfg(test)]` fixtures. Run `cargo build -p simlin-engine --all-targets` and add `macro_spec: None` to each one the compiler flags (the one populated test case is set in the Testing step below).

Step 5 — Check `src/simlin-engine/src/json_proptest.rs` for a strategy/generator that builds a `json::Model` (e.g. a `prop_model()` function or a `proptest!` macro over `json::Model`). If one exists, extend it to populate `macro_spec` — a `proptest::option::of(...)` over a `MacroSpec` strategy is ideal, but `Just(None)` is acceptable as a minimal start. If `json::Model` is generated via a derive-based strategy that picks up new fields automatically, no change is needed; note that in the commit message.

**Testing:**
Verify **macros.AC1.4 (JSON half)**. `json.rs`'s `test_model_roundtrip` (~line 2693) currently sets `loop_metadata: vec![]` and does not exercise the optional nested struct. Add a populated `macro_spec` to a `json::Model` round-trip test: either extend `test_model_roundtrip` with a `macro_spec: Some(MacroSpec { ... })` (non-empty `parameters` and `primary_output`, non-empty `additional_outputs`) plus a body variable, or add a focused `test_macro_spec_roundtrip` that goes `json::Model` → `datamodel::Model` → `json::Model` → `serde_json` string → `json::Model` and `assert_eq!`s. Match the existing `test_model_roundtrip` structure.

**Verification:**
Run: `cargo test -p simlin-engine json`
Expected: all `json` tests pass.

Run: `cargo test -p simlin-engine generate_and_write_schema`
Expected: passes; `git diff docs/simlin-project.schema.json` now shows a `"MacroSpec"` entry under `$defs` and a `"macroSpec"` property on the `Model` definition.

**Commit:** `engine: persist MacroSpec through JSON and regenerate schema` (stage `json.rs`, the regenerated `docs/simlin-project.schema.json`, and `json_proptest.rs` if changed)
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 4-5) -->
## Subcomponent C: Consumer mirrors (TypeScript and Python)

<!-- START_TASK_4 -->
### Task 4: TypeScript mirrors

**Verifies:** macros.AC1.4 (TypeScript JSON round-trip).

**Files:**
- Modify: `src/engine/src/json-types.ts` (add `JsonMacroSpec` interface near `JsonLoopMetadata` ~line 325; add `macroSpec?` to `JsonModel` ~lines 355-368)
- Modify: `src/core/datamodel.ts` (add `MacroSpec` interface near `LoopMetadata` ~line 1112; add `macroSpecFromJson`/`macroSpecToJson` near `loopMetadataFromJson`/`loopMetadataToJson` ~line 1119; add `macroSpec?` to the `Model` interface ~lines 1181-1187; wire into `modelFromJson` ~line 1189 and `modelToJson` ~line 1208; the `JsonMacroSpec` import joins the existing `@simlin/engine` import block ~lines 11-41)
- Modify: `src/core/tests/datamodel.test.ts` (add a `MacroSpec` round-trip `describe` block mirroring the `LoopMetadata` block ~lines 557-592; add the converter imports ~lines 30-31 and the `MacroSpec` type import ~line 57)

**Implementation:**

`json-types.ts` is the hand-maintained TypeScript mirror of `json.rs` — keep the field names and optionality identical to the `json::MacroSpec` you wrote in Task 3.

Step 1 — In `json-types.ts`, add `JsonMacroSpec` mirroring `JsonLoopMetadata`'s style (camelCase keys, optional for skip-serialized fields):

```typescript
/**
 * Marks a model as a callable macro template and records its calling convention.
 */
export interface JsonMacroSpec {
  parameters: string[];
  primaryOutput: string;
  additionalOutputs?: string[];
}
```

Then add `macroSpec?: JsonMacroSpec;` to the `JsonModel` interface.

Step 2 — In `datamodel.ts`, add the rich (readonly) `MacroSpec` interface and its two converters, mirroring `LoopMetadata` / `loopMetadataFromJson` / `loopMetadataToJson`:

```typescript
export interface MacroSpec {
  readonly parameters: readonly string[];
  readonly primaryOutput: string;
  readonly additionalOutputs: readonly string[];
}

export function macroSpecFromJson(json: JsonMacroSpec): MacroSpec {
  return {
    parameters: json.parameters,
    primaryOutput: json.primaryOutput,
    additionalOutputs: json.additionalOutputs ?? [],
  };
}

export function macroSpecToJson(spec: MacroSpec): JsonMacroSpec {
  const result: JsonMacroSpec = {
    parameters: [...spec.parameters],
    primaryOutput: spec.primaryOutput,
  };
  if (spec.additionalOutputs.length > 0) {
    result.additionalOutputs = [...spec.additionalOutputs];
  }
  return result;
}
```

Step 3 — Add `readonly macroSpec?: MacroSpec;` to the `Model` interface. In `modelFromJson`, add `macroSpec: json.macroSpec ? macroSpecFromJson(json.macroSpec) : undefined,`. In `modelToJson`, add (mirroring how `loopMetadata` is conditionally attached) `if (model.macroSpec) { result.macroSpec = macroSpecToJson(model.macroSpec); }`.

**Testing:**
Verify **macros.AC1.4 (TypeScript half)**. Add a `describe('MacroSpec', ...)` block to `datamodel.test.ts` mirroring the `describe('LoopMetadata', ...)` block (~lines 557-592): a "should roundtrip correctly" test that builds a `MacroSpec` with non-empty `parameters`, `primaryOutput`, and `additionalOutputs`, runs `macroSpecToJson` then `macroSpecFromJson`, and asserts equality; and a "should omit empty additionalOutputs" test confirming `macroSpecToJson` leaves `additionalOutputs` undefined when the array is empty and `macroSpecFromJson` restores it to `[]`.

**Verification:**
Run: `pnpm build` (rebuilds `@simlin/engine` so `@simlin/core` sees the new `JsonMacroSpec` export, then builds all TS packages)
Run: `pnpm tsc`
Expected: type-checks with no errors.
Run: `pnpm --filter @simlin/core test`
Expected: all `@simlin/core` tests pass, including the new `MacroSpec` block.

(Note: `json-types.ts` is an existing file being edited, not a new file, so the `lib.browser/` `.d.ts` regeneration caveat for *new* engine files does not apply — `pnpm build` covers it.)

**Commit:** `core: mirror MacroSpec in the TypeScript datamodel`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Python mirrors

**Verifies:** macros.AC1.4 (Python JSON round-trip).

**Files:**
- Modify: `src/pysimlin/simlin/json_types.py` (add `MacroSpec` dataclass near `LoopMetadata` ~line 298; add `macro_spec` field to the `Model` dataclass ~lines 308-319)
- Modify: `src/pysimlin/simlin/json_converter.py` (add a `structure_macro_spec` hook near `structure_loop_metadata` ~line 723; register it; read `macroSpec` in `structure_model` ~lines 765-787; add `MacroSpec` to `additional_type_required_fields` ~line 805)
- Modify: the relevant test file under `src/pysimlin/tests/` (add a `MacroSpec` round-trip test — see Testing)

**Implementation:**

The Python dataclasses are not self-serializing; serialization is centralized in `json_converter.py` via `cattrs`. JSON keys are camelCase; dataclass fields are snake_case; the converter bridges them (`structure_*` reads camelCase keys, `_make_omit_default_hook` emits camelCase via `_to_camel_case`).

Step 1 — In `json_types.py`, add the `MacroSpec` dataclass mirroring `LoopMetadata`'s style (all fields defaulted):

```python
@dataclass
class MacroSpec:
    """Marks a model as a callable macro template and records its calling convention."""

    parameters: list[str] = field(default_factory=list)
    primary_output: str = ""
    additional_outputs: list[str] = field(default_factory=list)
```

Then add `macro_spec: MacroSpec | None = None` to the `Model` dataclass (mirroring `sim_specs: SimSpecs | None = None`).

Step 2 — In `json_converter.py`, add a structure hook next to `structure_loop_metadata` (~line 723) and register it:

```python
def structure_macro_spec(d: dict[str, Any], _: type) -> MacroSpec:
    return MacroSpec(
        parameters=d.get("parameters", []),
        primary_output=d.get("primaryOutput", ""),
        additional_outputs=d.get("additionalOutputs", []),
    )

conv.register_structure_hook(MacroSpec, structure_macro_spec)
```

Step 3 — In `structure_model` (~lines 765-787), structure the optional `macroSpec` key and pass it to the `Model(...)` constructor (mirror how `sim_specs` is handled):

```python
    macro_spec = conv.structure(d["macroSpec"], MacroSpec) if d.get("macroSpec") else None
```

Step 4 — Add `MacroSpec` to the `additional_type_required_fields` dict (~line 805) so `parameters` and `primary_output` always serialize even at their defaults (keys are snake_case dataclass field names): `MacroSpec: {"parameters", "primary_output"}`. The generic `_make_omit_default_hook` then handles unstructure (emitting `macroSpec`, `primaryOutput`, `additionalOutputs` in camelCase).

Step 5 — Ensure `MacroSpec` is exported wherever `LoopMetadata` is (check `json_types.py`'s `__all__` if it has one, and any re-export in `simlin/__init__.py`).

**Testing:**
Verify **macros.AC1.4 (Python half)**. Find the existing `Model`/`LoopMetadata` round-trip test in `src/pysimlin/tests/` (likely `test_json_types.py`) and add a `MacroSpec` round-trip test in the same style: build a `Model` with a populated `macro_spec` (non-empty `parameters`, `primary_output`, `additional_outputs`), unstructure it to a dict via the `json_converter`, structure it back, and assert the `MacroSpec` is identical. Confirm the unstructured dict uses the camelCase keys `macroSpec`, `primaryOutput`, `additionalOutputs`.

**Verification:**
Run: `cd src/pysimlin && uv run pytest tests/test_json_types.py -x`
Expected: all tests pass, including the new `MacroSpec` round-trip test.

**Commit:** `pysimlin: mirror MacroSpec in the Python json types`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase 1 completion check

When all five tasks are committed, the following hold:
- `MacroSpec` is a first-class datamodel type with an `Option<MacroSpec>` field on `Model`.
- A macro-bearing `Model` round-trips losslessly through protobuf (`serde.rs` test), through Rust JSON (`json.rs` test), through the TypeScript mirror (`@simlin/core` test), and through the Python mirror (pysimlin test) — satisfying **macros.AC1.4**.
- `docs/simlin-project.schema.json` describes `MacroSpec`.
- `cargo build --workspace`, `pnpm tsc`, and the pre-commit hook all pass.

No import, export, resolution, or simulation behavior changes in this phase — those build on this representation in Phases 2-7.
