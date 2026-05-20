# C-LEARN Residual: General Vensim Import & Simulation Primitives

## Summary

This work fixes a cluster of general defects in Simlin's Vensim (`MDL`) import and
simulation pipeline. The defects were *surfaced* by driving the numeric output of
one large reference model (C-LEARN) to match its genuine-Vensim baseline
(`Ref.vdf`), but the fixes themselves are model-agnostic primitives: each one
improves import or simulation of *any* large Vensim model, and a hard constraint
of this design is that no change may key on a C-LEARN variable name, file path, or
the known residual list. C-LEARN is used only as an end-to-end measurement gate,
not as a special case in the code.

The approach repairs four engine/importer defects plus one forward-looking
capability, each behind its own generality-proving test. (1) Lookup-only
(graphical-function) variables are lowered inconsistently across scalar, arrayed,
and apply-to-all shapes; the fix lowers all three uniformly to Vensim's
empirically-determined saved-value semantics, reusing the per-element table layout
that already exists rather than adding new VM opcodes. (2) An import-time formatter
rewrites `RAMP FROM TO` into a linear-only builtin before macro resolution runs,
pre-empting a user macro of the same name; the fix removes that rewrite so the call
flows to the existing, correct compile-time macro path. (3) A trivial passthrough
macro over `INITIAL` (`:MACRO: INIT(x) = INITIAL(x)`) is expanded into a synthetic
module whose value is mis-ordered and mis-propagated; the fix collapses such a
passthrough directly to the proven `LoadInitial` opcode at the call site. (4) The
remaining divergence is honestly attributed — separating true engine error from
VDF-reader decode artifacts, NaN-vs-`:NA:` representation, and benign near-zero
cross-simulator noise — and the `EXPECTED_VDF_RESIDUAL` carve-out is tightened to
exactly what genuinely remains, enforced by the `clearn_residual_exactness` guard.
(5, separable) Native targets gain a filesystem `DataProvider` so external-data
functions (`GET XLS/DIRECT ...`) resolve; C-LEARN does not need this, so it serves
other models and may be cut without affecting residual closure.

## Definition of Done

Close C-LEARN's tracked numeric residual against the genuine-Vensim reference
(`test/xmutil_test_models/Ref.vdf`, GitHub #590/#591) by fixing the underlying
engine and importer defects as **general primitives** — code that improves
import/simulation of *any* large Vensim model and never keys on C-LEARN
variable names, the C-LEARN file, or `Ref.vdf` — and by honestly attributing any
irreducible remainder rather than masking it.

### Primary deliverables

1. **Arrayed / apply-to-all inline graphical-function (lookup-only) variables
   simulate correctly.** The compiler applies the attached per-element graphical
   function to arrayed and apply-to-all lookup-only variables (the missing half
   of the lowering the scalar path already performs), and a standalone scalar
   lookup-only variable matches Vensim's "never read" semantics. (#590)

2. **A user-defined macro that shadows a builtin is no longer pre-empted by an
   import-time builtin rewrite.** `RAMP FROM TO` (and any sibling macro the
   formatter currently special-cases) resolves through the existing, correct
   compile-time macro path, so both the linear and exponential branches run as
   the model selects. (#591 cluster 2)

3. **A user-macro `INITIAL` recurrence produces correct values at every step.**
   The synthetic-module path for an element-wise `INITIAL` macro (e.g.
   `:MACRO: INIT(x) = INITIAL(x)`) is fixed — both the Initials topological
   ordering and the flows-phase propagation of the module's output — and/or a
   trivial passthrough macro collapses to the proven `LoadInitial` opcode.
   `SAMPLE UNTIL` and the dependent emissions chain follow without dedicated
   work. (#591 cluster 1)

4. **The remaining residual is honestly attributed and either fixed or
   documented.** VDF-reader decode artifacts, NaN-vs-`:NA:` representation, and
   benign near-zero cross-simulator noise are each identified and resolved or
   recorded with a reason. `EXPECTED_VDF_RESIDUAL` in
   `src/simlin-engine/tests/simulate.rs` is tightened to exactly what genuinely
   remains (ideally near-empty), the `clearn_residual_exactness` guard is updated
   to match, and #590/#591 are updated or closed accordingly.

5. **(Forward-looking; not required for C-LEARN.) Native targets resolve Vensim
   external-data functions.** `simlin-cli` and other native (filesystem-capable)
   builds resolve `GET XLS/DIRECT DATA/LOOKUPS/CONSTANTS/SUBSCRIPT` through a
   filesystem data provider, validated by a small non-C-LEARN fixture. Exposing
   this through libsimlin/WASM is a tracked follow-up, not part of this work.
   NOTE: investigation proved C-LEARN needs **no** external data (all its lookup
   data is inline); this phase serves *other* large Vensim models and is cleanly
   separable from residual closure.

### Success criteria

- Every engine/importer fix lands via test-driven development with tests that
  prove **generality** — unit/fixture tests that do not depend on C-LEARN names
  or files — in addition to the C-LEARN end-to-end gate.
- `simulates_clearn` (`--ignored`, runtime class) passes with a materially
  smaller `EXPECTED_VDF_RESIDUAL`; `clearn_residual_exactness` proves the
  exclusion set is exactly the genuine remainder (neither over- nor under-broad).
- No special-casing: a grep of the change set finds no branch keyed on a C-LEARN
  variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list.
- `cargo test --workspace` is green within the 3-minute cap; the pre-commit hook
  passes (Rust fmt/clippy/tests, TS lint/types, WASM build, TS tests, Python
  bindings).

### Explicitly out of scope

- libsimlin / WASM exposure of external data (tracked follow-up: the right FFI
  surface for supplying data is its own design question).
- Changing the VDF binary format spec, or re-architecting the LTM subsystem.
- Any C-LEARN-specific code path or per-model patch.
- "Fixing" NA-arithmetic (confirmed already correct) or `SAMPLE UNTIL`
  (confirmed structurally correct; resolves downstream of deliverable 3).

### Assumptions / decisions carried in

- The external-data phase (deliverable 5) is included per the explicit interest
  in general primitives for other large Vensim models, despite C-LEARN not
  needing it. It is flagged throughout as cleanly cuttable.
- Vensim's `:NA:` is the finite sentinel `crate::float::NA = -2^109`; this design
  does not change that representation.

## Acceptance Criteria

### clearn-residual.AC1: Lookup-only (graphical-function) variables produce Vensim-matching saved series
- **clearn-residual.AC1.1 Success:** A scalar lookup-only variable (equation is an inline graphical function, no functional argument) produces the saved series determined to match genuine Vensim (per the Phase 1 determination), not a constant `gf(0)`.
- **clearn-residual.AC1.2 Success:** An arrayed (`Equation::Arrayed`) lookup-only variable produces the correct per-element saved series for every element, not literal `0`.
- **clearn-residual.AC1.3 Success:** An apply-to-all lookup-only variable produces the correct saved series for every element.
- **clearn-residual.AC1.4 Success (no regression):** A graphical-function variable that is applied with an argument elsewhere (`var(idx)` → `LOOKUP(var, idx)`) still produces correct applied values.
- **clearn-residual.AC1.5 Edge:** A lookup-only variable whose declared element order is non-alphabetical maps each element to its own table (no positional mis-map; consistent with the `per_element_gf_tests.rs` invariant).
- **clearn-residual.AC1.6 Failure/robustness:** Out-of-range lookup-index semantics for the scalar `Lookup` opcode are unchanged by the fix (the fix changes which expression is fed to the table, not the table's clamp/NaN behavior).

### clearn-residual.AC2: User macros sharing a builtin name resolve via the macro path
- **clearn-residual.AC2.1 Success:** A model defining `:MACRO: RAMP FROM TO(...)` invoked with the exponential selector (`islinear = 0`) produces the exponential trajectory from the macro body, not a linear ramp.
- **clearn-residual.AC2.2 Success (no regression):** The same invocation with the linear selector (`islinear = 1`) produces the linear trajectory (existing `simulates_macro_clearn_ramp_from_to_mdl` behavior preserved).
- **clearn-residual.AC2.3 Success:** After import, `RAMP FROM TO(...)` is a resolvable call (not a pre-linearized `RAMP(...)` string), so compile-time macro resolution applies.
- **clearn-residual.AC2.4 Edge:** Nonpositive endpoints exercise the macro's `linear` selector (forced linear when `xfrom>0 :AND: xto>0` is false) per the macro definition.
- **clearn-residual.AC2.5 Failure/guard:** No `xmile_compat.rs` formatter special-case rewrites a name a model defines as a macro ahead of macro resolution (verified for the audited builtin-named macros, e.g. `SSHAPE`).

### clearn-residual.AC3: User-macro `INITIAL` recurrences produce correct multi-step values
- **clearn-residual.AC3.1 Success:** A model with `:MACRO: INIT(x) = INITIAL(x)` and a scalar `INITIAL`-captured value holds that value constant across all saved steps (matching the opcode path).
- **clearn-residual.AC3.2 Success:** An element-wise `INITIAL` recurrence routed through the passthrough macro produces correct per-element values at t0 and at every subsequent step (no drop to `0` at t≥1, no spurious `:NA:`).
- **clearn-residual.AC3.3 Success (no regression):** A bare `INITIAL(expr)` with no user macro still compiles to `LoadInitial` and behaves as today (`helper_recurrence.mdl` stays green).
- **clearn-residual.AC3.4 Edge:** The passthrough collapse fires only for a genuine `out = BUILTIN(param)` body; a non-passthrough macro that merely shares a builtin name still expands as a macro (not mis-collapsed to the opcode).
- **clearn-residual.AC3.5 Success (downstream):** A `SAMPLE UNTIL` fed by a corrected `INITIAL` value samples until the correct time without any dedicated `SAMPLE UNTIL` change.

### clearn-residual.AC4: The residual is honestly attributed and the gate is tightened
- **clearn-residual.AC4.1 Success:** After Phases 1-3, `EXPECTED_VDF_RESIDUAL` is reduced to the proven remainder and `simulates_clearn` passes (`--ignored`).
- **clearn-residual.AC4.2 Success:** `clearn_residual_exactness` passes — the live failing set equals the reduced `EXPECTED_VDF_RESIDUAL` exactly (no `grew` / no `shrank`).
- **clearn-residual.AC4.3 Success:** Every still-excluded base carries a one-line sourced reason (engine-genuine-tracked / VDF-decode-artifact / benign-near-zero / NaN-vs-`:NA:` / boundary), and #590 / #591 are updated to reflect the final disposition.
- **clearn-residual.AC4.4 Failure/guard:** The comparator's 1% tolerance, per-series floor, and matched-variable floor are unchanged for every non-excluded variable (the carve-out remains a documented exclusion, never a tolerance loosening; the gate cannot pass vacuously).
- **clearn-residual.AC4.5 Edge:** A NaN-vs-`:NA:` series (e.g. `slr_inches_from_2000`) is handled by the documented NaN-skip mechanism and does not enter the failure set.

### clearn-residual.AC5: Native targets resolve external-data functions (forward-looking)
- **clearn-residual.AC5.1 Success:** On a native (`file_io`) build, a model using `GET DIRECT DATA` (or `GET DIRECT LOOKUPS`/`CONSTANTS`) imported via the CLI resolves values from a companion file located relative to the model path.
- **clearn-residual.AC5.2 Success:** The resolved external data drives simulation (the fixture asserts a downstream value that reflects the external data, not a zeroed series).
- **clearn-residual.AC5.3 Edge:** A missing or unreadable data file produces a clear diagnostic rather than a silent `0+0` zeroing.
- **clearn-residual.AC5.4 Constraint:** WASM / libsimlin builds remain on the null provider (behavior unchanged); a tracked follow-up issue captures their data-supply API.

### clearn-residual.AC6: Cross-cutting correctness and hygiene
- **clearn-residual.AC6.1:** No change keys on a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list (grep-verifiable).
- **clearn-residual.AC6.2:** Each engine fix (Phases 1-3) ships with at least one generality-proving test independent of C-LEARN.
- **clearn-residual.AC6.3:** `cargo test --workspace` passes within the 3-minute cap and the pre-commit hook passes (Rust fmt/clippy/tests, TS lint/types, WASM build, TS tests, Python bindings).
- **clearn-residual.AC6.4:** New end-to-end C-LEARN tests stay `#[ignore]` (runtime class) and run via `--ignored`.

## Glossary

- **System dynamics (SD)**: Modeling discipline based on stocks, flows, and
  feedback loops that Simlin compiles and simulates; the broad domain of all the
  model files referenced here.
- **C-LEARN**: A large (~53k-line) real-world Vensim climate-policy model used in
  this repo as an end-to-end import/simulation reference. It is the model whose
  numeric residual this work closes; per the design it is a measurement gate only,
  never a code special-case.
- **Vensim**: A widely-used commercial SD modeling tool. Simlin imports Vensim
  models and aims to reproduce Vensim's simulation results.
- **MDL**: Vensim's native text model file format (`.mdl`). The Simlin importer
  parses MDL and converts it to Simlin's datamodel.
- **XMILE**: The open XML interchange standard for SD models (spec at
  `docs/reference/xmile-v1.0.html`). Simlin's internal datamodel is XMILE-aligned;
  `xmile_compat.rs` bridges MDL into it.
- **VDF**: Vensim Data Format — the binary file Vensim writes with a simulation
  run's saved variable series. `Ref.vdf` is C-LEARN's genuine-Vensim output, the
  numeric reference the gate compares against; `vdf.rs` is Simlin's reader for it.
- **Graphical function / lookup**: A piecewise-linear table mapping an input value
  to an output (Vensim "lookup"). A *lookup-only* variable is one whose entire
  equation is an inline table with no functional argument; the importer marks it
  with the `LOOKUP_SENTINEL` (`"0+0"`).
- **Apply-to-all (A2A)**: An arrayed variable where one equation applies uniformly
  to every subscript element, as opposed to a per-element (`Equation::Arrayed`)
  variable with a distinct expression per element. The lookup-only lowering must be
  correct for scalar, arrayed, and A2A shapes.
- **`:NA:` sentinel**: Vensim's "not available" marker. In Simlin it is the *finite*
  value `float.rs::NA` (`-2^109 ≈ -6.49e32`), not IEEE NaN; it participates in
  ordinary IEEE arithmetic (e.g. `NA+NA == -2^110`), which already matches Vensim
  and is explicitly left unchanged by this work. Distinct from NaN, which is why a
  NaN-vs-`:NA:` representation mismatch is its own residual category.
- **`RAMP FROM TO`**: A Vensim builtin-named construct that ramps between endpoints
  with a selectable linear or exponential branch. C-LEARN defines it as a user
  `:MACRO:`; the import-time linear-only rewrite that pre-empted the macro is what
  Phase 2 removes.
- **`INITIAL` / `LoadInitial`**: Vensim's `INITIAL(expr)` captures a value at the
  start of the run and holds it constant. Simlin compiles it to the `LoadInitial`
  opcode, which reads from an initial-value buffer snapshotted at t=0 — the proven
  path that Phase 3 routes the passthrough macro onto.
- **Macro / `:MACRO:` / macro-shadows precedence**: Vensim user-defined macros
  (`:MACRO: name(args) = body`). When a user macro shares a builtin's name, Simlin's
  resolution gives the in-model macro precedence over the builtin
  ("macro-shadows-everything"). The `#554 self-call exception` collapses a
  renamed-builtin self-call back to the opcode; Phase 3 extends that collapse from
  inside the macro body to the call site.
- **`SAMPLE UNTIL`**: A Vensim function that holds a sampled value until a condition
  time is reached. Here it is confirmed structurally correct and resolves downstream
  of the `INITIAL` fix, so it needs no dedicated work.
- **`GET DIRECT/XLS DATA/LOOKUPS/CONSTANTS/SUBSCRIPT`**: Vensim functions that pull
  values from external files (CSV/Excel) rather than inline equations. Resolving
  them is the subject of the forward-looking Phase 5.
- **DataProvider**: Simlin's trait for supplying external data to GET-DATA
  functions. `FilesystemDataProvider` (native, `file_io` feature) reads companion
  files relative to the model; WASM/libsimlin currently use a null provider.
- **`EXPECTED_VDF_RESIDUAL`**: The explicit carve-out list (in
  `tests/simulate.rs`) of variable bases allowed to diverge from `Ref.vdf`. This
  work *shrinks* it to the genuine remainder rather than loosening tolerances. The
  `clearn_residual_exactness` guard asserts the live failing set equals this list
  exactly (neither grew nor shrank), so the gate cannot pass vacuously.
- **salsa / incremental compilation**: The query-based incremental recompilation
  framework underlying the engine's compiler; relevant because lookup-only lowering
  and macro resolution run within it.
- **Stock / flow**: A stock is an accumulator integrated over time; a flow is its
  rate of change. The Initials phase and flows phase of the VM correspond to
  computing initial stock values versus per-step rates — the two phases where the
  `INITIAL`-macro value was being mis-ordered and mis-propagated.

## Architecture

This work closes C-LEARN's tracked numeric residual (#590/#591) against the
genuine-Vensim reference (`test/xmutil_test_models/Ref.vdf`) by repairing a small
set of **general** defects in the Vensim import and simulation path. Root-cause
investigation decomposed the residual into five concerns; none requires
C-LEARN-specific code.

**1. Lookup-only variable lowering** (`src/simlin-engine/src/compiler/mod.rs`).
A variable whose equation is the graphical-function sentinel `0+0` (an inline
table with no functional argument; `mdl/mod.rs::LOOKUP_SENTINEL`) is lowered
inconsistently: the scalar path wraps it `LOOKUP(self, 0) -> gf(0)`
(`compiler/mod.rs:~770-801`), while the arrayed and apply-to-all paths
(`expand_arrayed_with_hoisting:~1605`, `expand_a2a_with_hoisting:~1692`) leave it
literally `0`. Neither matches genuine Vensim's saved value for a standalone
lookup. The fix determines Vensim's rule empirically, then lowers all three
shapes uniformly to match it, reusing the per-element table layout that already
exists.

**2. Import-time macro shadowing** (`src/simlin-engine/src/mdl/xmile_compat.rs`).
The MDL->datamodel expression formatter rewrites `RAMP FROM TO(...)` into a
linear-only `RAMP(...)` string (`xmile_compat.rs:422-434`) *before* compile-time
macro resolution runs, discarding the exponential branch and pre-empting the
in-model `:MACRO: RAMP FROM TO`. The fix removes the formatter special-case so
the call survives to the existing, correct macro path
(`builtins_visitor.rs:626-638`, which already knows `RAMP FROM TO` is a macro).

**3. User-macro `INITIAL` value handling** (`src/simlin-engine/src/builtins_visitor.rs`
and the VM runlist). C-LEARN defines `:MACRO: INIT(x) = INITIAL(x)`; the importer
renames Vensim `INITIAL`/`active initial`/`reinitial` to `INIT`
(`xmile_compat.rs:545-547`), so every `INITIAL` call collides with the macro and,
under macro-shadows precedence, expands to a per-element synthetic module whose
value is mis-ordered in the Initials run and mis-propagated in the flows phase.
The fix collapses the trivial passthrough macro to the proven `LoadInitial`
opcode at the call site (extending the #554 self-call exception from the macro
body to the call site).

**4. Measurement fidelity** (`src/simlin-engine/src/vdf.rs`,
`src/simlin-engine/tests/simulate.rs`). Independent of engine correctness, the
harness's VDF reader mis-decodes some reference columns, NaN-vs-`:NA:`
representation differs on a few series, and some series are benign near-zero
cross-simulator noise. Each inflates the *measured* residual. The fix classifies
every remaining divergent base and either repairs it or documents it precisely,
then tightens the gate.

**5. External-data resolution** (`src/simlin-engine/src/data_provider/`,
`src/simlin-cli/`). Forward-looking and independent: native targets gain a
filesystem `DataProvider` so `GET XLS/DIRECT ...` functions resolve. C-LEARN has
no external data; this serves other large Vensim models.

**Gate.** `simulates_clearn` (`tests/simulate.rs:1654`) compares against
`Ref.vdf` with a hardened 1% comparator (`classify_vdf_ident`,
`ensure_vdf_results_excluding`) and a tracked `EXPECTED_VDF_RESIDUAL` carve-out
(`simulate.rs:1563-1622`); `clearn_residual_exactness` (`simulate.rs:1722`)
proves the carve-out is exactly the genuine remainder. As phases 1-4 land, bases
are pruned from `EXPECTED_VDF_RESIDUAL` and the exactness guard enforces the
shrinkage. The `:NA:` sentinel (`float.rs::NA = -2^109`) and its ordinary-IEEE
arithmetic are an invariant this work preserves, not changes.

## Existing Patterns

Investigation found mature primitives this design **extends rather than replaces**:

- **Per-element graphical functions** already exist end to end:
  `variable.rs::build_tables` + `reorder_arrayed_element_tables` lay out one
  `Table` per element at its declared dimension index; `vm.rs` `Lookup`
  (`:1508-1520`) and `LookupArray` (`Opcode::LookupArray`, #580) evaluate
  `graphical_functions[base_gf + element_offset]`; `per_element_gf_tests.rs`
  pins the layout. Phase 1's gap is only that the arrayed/A2A *lowering* paths
  don't wrap the per-element expression in `Lookup` the way the scalar path does.

- **Macro-shadows-everything precedence** (`builtins_visitor.rs:626-638`) already
  resolves an in-model macro named like a builtin (`RAMP FROM TO`, `SSHAPE`) to
  the macro; the **#554 self-call exception**
  (`is_enclosing_macro_renamed_builtin_self_call`, `:601-637`) already collapses
  a renamed-builtin self-call *inside* a macro body to the opcode. Phase 2
  removes the formatter special-case that pre-empts this precedence; Phase 3
  extends the #554 collapse from the macro body to the *call site*.

- **`INITIAL` recurrence via opcode** is proven correct: `helper_recurrence.mdl`
  (`simulate.rs:2314-2375`) exercises `ecc[tNext] = INITIAL(ecc[tPrev]*2)`
  through `LoadInitial` (`vm.rs:1355-1363`, snapshot `vm.rs:1151-1155`,
  `init_referenced` runlist `model.rs:1166-1180`). Phase 3 routes the C-LEARN
  macro-`INIT` onto this same path.

- **`DataProvider` trait + `FilesystemDataProvider`**
  (`src/simlin-engine/src/data_provider/`, feature `file_io`) and
  `mdl/convert/external_data.rs` already implement GET-DATA resolution;
  `compat.rs::open_vensim_with_data` already threads a provider. Phase 5 wires
  it into `simlin-cli::open_model` (`main.rs:176-216`), which already enables
  `file_io` and holds the model `file_path`.

- **The VDF comparator + carve-out gate** (`ensure_vdf_results_excluding`,
  `classify_vdf_ident`, `EXPECTED_VDF_RESIDUAL`, `clearn_residual_exactness`) is
  the existing hardened measurement surface. Phase 4 *tightens* it (prunes the
  exclusion) rather than loosening tolerances.

- **Test fixtures**: the `TestProject` builder (`testutils.rs`), the metasd macro
  corpus (`tests/metasd_macros.rs`, gated on `file_io`), and `test/test-models/`
  Vensim fixtures are the model for new generality-proving fixtures.

All work is test-driven (root `CLAUDE.md`), with functional-core/imperative-shell
separation where applicable.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Lookup-only variable saved-value semantics (#590)
**Goal:** A standalone graphical-function (lookup-only) variable produces the
same saved series as genuine Vensim, for scalar, arrayed, and apply-to-all
shapes — resolving the inconsistent `gf(0)`-vs-`0` lowering.

**Components:**
- A short empirical determination of genuine-Vensim's rule for a standalone
  lookup variable's saved column (hypothesis: `gf(Time)`, given `INITIAL TIME =
  1850` and year-indexed tables; candidates also `0` and "VDF-decode artifact"),
  by instrumenting `run_clearn_vs_vdf` (`tests/simulate.rs`) against `Ref.vdf`.
  This decides the target semantics before any code change.
- Uniform lookup-only lowering in `src/simlin-engine/src/compiler/mod.rs`:
  `expand_arrayed_with_hoisting` and `expand_a2a_with_hoisting` apply the
  per-element table to match the determined semantics, and the scalar path
  (`:~770-801`) is reconciled to the same rule. Reuses
  `variable.rs::build_tables`/`reorder_arrayed_element_tables` and the
  `Lookup`/`LookupArray` opcodes (no new VM/codegen primitives).
- Generality fixtures (non-C-LEARN): small arrayed and apply-to-all inline-GF
  models built with `TestProject`, asserting the standalone-lookup column and
  the applied-lookup (`LOOKUP(var, idx)`) column independently.

**Dependencies:** None.

**Done when:** The determined semantics is documented; the new fixtures pass;
the `historical_gdp_lookup`/`historical_forestry_lookup`/`rs_*` arrayed bases and
the scalar `*_from_graph_lookup`/`*_forcings` bases either reconcile in
`simulates_clearn` or are demonstrably VDF-decode artifacts deferred to Phase 4.
Covers `clearn-residual.AC1`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Stop import-time linearization of shadowed macros (#591-c2)
**Goal:** A user-defined macro that shares a builtin's name is resolved by the
macro path, not silently replaced by an import-time builtin rewrite — so
`RAMP FROM TO` runs its exponential branch when the model selects it.

**Components:**
- Remove the `"ramp from to"` special-case in
  `src/simlin-engine/src/mdl/xmile_compat.rs:422-434` and reconcile the
  `:555` `"ramp from to" => "RAMP_FROM_TO"` name mapping, so the invocation
  survives import as a call and resolves through `builtins_visitor.rs:626-638`
  (which already expands the in-model `RAMP FROM TO` macro correctly).
- Audit the remaining `xmile_compat.rs` formatter special-cases (`sshape`, etc.)
  for the same hazard — any name that can be an in-model macro must not be
  rewritten ahead of macro resolution.
- Rewrite `test_ramp_from_to_transforms_args` (`xmile_compat.rs:~2031-2054`,
  which currently encodes the buggy linear-only output) and add a regression
  fixture with `islinear = 0` (exponential branch), complementing the existing
  linear-only `simulates_macro_clearn_ramp_from_to_mdl` (`simulate.rs:3181`).
- End-to-end check that the resolved macro-module output propagates correctly
  (see Additional Considerations: Phase 2/3 coupling).

**Dependencies:** None (independent of Phase 1).

**Done when:** The `islinear=0` fixture matches the exponential trajectory; the
`im_3_emissions` / `im_3_emissions_vs_rs` / `im_3_ff_co2` /
`relative_emissions_to_equity` / `relative_emissions_to_equity_target` bases
reconcile in `simulates_clearn`; the formatter audit is recorded. Covers
`clearn-residual.AC2`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Correct user-macro `INITIAL` recurrences (#591-c1)
**Goal:** An element-wise `INITIAL` expressed through a trivial passthrough macro
(`:MACRO: INIT(x) = INITIAL(x)`) produces correct values at every step, matching
the proven opcode path.

**Components:**
- Call-site passthrough collapse in `src/simlin-engine/src/builtins_visitor.rs`:
  a single-output macro whose body is exactly `out = BUILTIN(param)` (a renamed
  builtin self-call) compiles the call site directly to that builtin's opcode
  (here `LoadInitial`), bypassing `expand_module_function` (`:480-579`). This
  generalizes the existing #554 self-call exception from the macro body to the
  call site, and must only fire for genuine passthroughs (not for a macro that
  merely shares a name).
- Contingent sub-component (only if a *non-passthrough* `INITIAL`-recurrence
  macro-module remains divergent after the collapse): fix the synthetic-module
  Initials topological ordering and flows-phase output propagation
  (`model.rs::module_output_deps:236` / `all_deps:301-486`, `vm.rs` Initials vs
  flows `LoadInitial` at `:1355-1363`). Treated as deeper general correctness,
  scoped in only if needed.
- Generality fixture (non-C-LEARN): a `:MACRO: INIT(x)=INITIAL(x)` plus an
  element-wise `INITIAL` recurrence with a multi-step value assertion (note:
  `ensure_results` skips implicit module vars, `test_helpers.rs:81-87`, so the
  fixture must assert the user-facing variable's series directly).

**Dependencies:** None for the primary collapse; shares macro-module concerns
with Phase 2.

**Done when:** The fixture holds the recurrence value across all saved steps;
`last_set_target_year`, `last_active_target_year`,
`time_from_target_to_ultimate_target`, `target_emissions_for_rate`,
`ultimate_target_value_from_rate`, `depth_at_bottom`,
`emissions_with_stopped_growth` reconcile in `simulates_clearn` (`SAMPLE UNTIL`
follows downstream). No NA-arithmetic change is made. Covers `clearn-residual.AC3`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Residual attribution and gate tightening
**Goal:** Every remaining divergent base is honestly classified and either fixed
or documented; `EXPECTED_VDF_RESIDUAL` shrinks to exactly the genuine remainder.

**Components:**
- Re-measure C-LEARN after Phases 1-3; classify each remaining divergent base as
  engine-genuine / VDF-decode-artifact (`src/simlin-engine/src/vdf.rs`
  name-collision / OT-block) / benign near-zero (`diffusion_flux`,
  `co2eq_gap_closing_percentage`) / NaN-vs-`:NA:` representation (#591-c3,
  `slr_inches_from_2000`) / boundary (`historical_gdp`).
- Fix engine-genuine divergences and tractable VDF-reader decode bugs; for the
  rest, record a precise, sourced attribution.
- Tighten `EXPECTED_VDF_RESIDUAL` (`tests/simulate.rs:1563-1622`) to the proven
  remainder and update `clearn_residual_exactness` (`:1722`); update or close
  #590 / #591 with the final disposition of each base.

**Dependencies:** Phases 1-3 (must re-measure after the engine fixes).

**Done when:** `simulates_clearn` and `clearn_residual_exactness` both pass with
the reduced exclusion set; every still-excluded base has a one-line sourced
reason (not "unknown"). Covers `clearn-residual.AC4`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Native external-data provider wiring (forward-looking; cuttable)
**Goal:** Native (filesystem-capable) targets resolve Vensim external-data
functions, so other large Vensim models that depend on `GET XLS/DIRECT ...`
import with real data.

**Components:**
- Wire `FilesystemDataProvider` (rooted at the model file's parent directory)
  into `src/simlin-cli/src/main.rs::open_model` (`:176-216`, currently
  `open_vensim(&contents)` at `:187`) and any other native entry point, via the
  existing `compat.rs::open_vensim_with_data` + `mdl/convert/external_data.rs`
  resolution for `GET DIRECT/XLS DATA/LOOKUPS/CONSTANTS/SUBSCRIPT`.
- A small non-C-LEARN fixture: a tiny Vensim model using `GET DIRECT DATA` (or
  lookups) plus a companion CSV under `test/`, asserting resolved values
  (mirrors `tests/metasd_macros.rs`, `file_io`).
- A tracked follow-up issue for exposing data supply through libsimlin/WASM (the
  FFI surface is out of scope here).

**Dependencies:** None (independent). May be cut without affecting Phases 1-4.

**Done when:** The fixture imports and simulates with data resolved from the
filesystem on a native build; WASM/libsimlin remain on the null provider; the
follow-up issue is filed. Covers `clearn-residual.AC5`.
<!-- END_PHASE_5 -->

## Additional Considerations

**Phase 2/3 coupling (user-macro -> module output).** Both phases touch how a
user macro that shares a builtin name is handled. `RAMP FROM TO` (Phase 2) is a
non-`INITIAL` macro, so its module output is a regular flows-phase value and is
expected to propagate correctly; Phase 2 includes an end-to-end check of that. If
that check surfaces a *general* (non-`INITIAL`) module-output-propagation bug, it
is handled together with Phase 3's contingent sub-component rather than papered
over inline.

**Phase 1 / Phase 4 interplay.** Some #590 bases (notably the arrayed
`rs_*[oecd_us]` columns and `oc,_bc,_and_bio_aerosol_forcings` /
`other_forcings_smooth_plus_rcp85`) are suspected VDF-reader *decode* artifacts,
not engine output. Phase 1 makes the engine correct; Phase 4 confirms which
columns are reference-side artifacts and attributes them. A base may therefore be
"engine-correct" yet still excluded with a VDF-decode reason — that is honest
accounting, not a tolerance loosening.

**No special-casing (hard constraint).** No fix may key on a C-LEARN variable
name, the C-LEARN `.mdl`/`.vdf` path, or the residual list. The reviewer should
be able to grep the change set and find none. Each engine fix ships with a
generality-proving fixture independent of C-LEARN.

**Test-time budget.** `simulates_clearn` / `clearn_residual_exactness` stay
`#[ignore]` (runtime class: ~53k lines / ~5s just to parse on release) and run
via `--ignored`. The new per-phase generality fixtures must be tiny so the capped
`cargo test --workspace` stays under the 3-minute cap (root `CLAUDE.md`).

**No bytecode back-compat.** VM bytecode is never serialized; only protobufs
require versioning. None of these phases touch the protobuf schema.

**NA-arithmetic is intentionally untouched.** `2*NA == NA+NA == -2^110 ==
-1.298e33` already matches Vensim under ordinary IEEE arithmetic on the finite
sentinel (`float.rs::NA`). Special-casing it would be incorrect.

**Phase count.** Five phases, within the 8-phase implementation-plan limit. A
single implementation plan suffices.
