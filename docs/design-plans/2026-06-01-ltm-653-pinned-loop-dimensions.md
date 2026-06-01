# LTM Pinned-Loop Dimension Classification (GH #653)

## Summary

Pinned feedback loops (the `SetLoopName` / LOOPSCORE escape hatch from #648) cannot be
scored on arrayed models: `model_pinned_loops` builds every pin as a *scalar* `Loop`
(no dimension classification), so the generated `loop_score` equation mixes scalar and
arrayed link-score references, fails to compile, and silently stubs to constant 0 (the
GH #466 silent-degradation class). This is Gap 2 of GH #653, and it makes pinning --
the workflow designed for exactly the large arrayed models where exhaustive enumeration
is impossible -- unusable on those models.

Gap 1 of #653 (pins blocked by the discovery-instrumentation slot-limit failure) was
resolved by the work that landed after the issue was filed: the 2026-06-01 commit
cluster (`099c5659`..`477af2ab`) cut C-LEARN's LTM layout from 171k to 29,764 slots
(well under the 65,536 u16 ceiling) and made the discovery DFS complete all 251
C-LEARN steps in 0.08s. Pins ride the normal discovery instrumentation; **no
"pinned-only instrumentation mode" is needed** and none is built here. Residual
slot-headroom concerns for even larger models stay tracked in #654.

The fix routes pinned cycles through the same dimension classification the exhaustive
enumerator already applies (`classify_cycle` -> PureScalar / PureSameElementA2A /
CrossElementOrMixed), and -- because the same root deficiency exists latently in the
enumerator's own A2A-collapse branch -- builds the dimension-aware loop-score equation
generation as shared machinery used by **both** the pin path and the enumerated path.

## Definition of Done

### Primary deliverables

1. **A pinned pure-A2A loop produces a per-element loop score.** Pinning a cycle whose
   variables are all arrayed over the same dimensions with Bare (apply-to-all)
   references yields one `$⁚ltm⁚loop_score⁚pin{n}` variable arrayed over those
   dimensions, with one correct score slot per element -- in both exhaustive and
   discovery mode.

2. **A pinned per-element-equation loop (the C-LEARN / MDL-importer shape) produces a
   per-element loop score.** When the cycle's variables carry `Equation::Arrayed`
   per-element equations with literal-element (FixedIndex) references, the pin's
   element-level instances are detected as a diagonal family and emitted as one
   `Equation::Arrayed` loop-score variable whose slot equations reference the correct
   per-element link-score variables.

3. **A pinned mixed scalar/arrayed loop produces per-element loop scores.** A cycle
   mixing scalar and arrayed variables (e.g. arrayed stock -> scalar aggregate ->
   arrayed flow) expands to its per-element instances and is scored per element.

4. **The enumerated A2A-collapse path produces correct per-element loop scores on
   per-element-equation models.** The latent bug where
   `build_element_level_loops`' pure-dimension collapse emitted an ApplyToAll equation
   referencing one arbitrary (lexicographically-first) element's FixedIndex link score
   for every slot is fixed by the same shared machinery.

5. **C-LEARN end-to-end validation.** An `#[ignore]`d (runtime-class) integration test
   pins one of C-LEARN's climate feedback loops, compiles with LTM discovery, runs the
   VM, and asserts the pin's score is finite and non-zero in at least one scenario
   element. Run via `cargo test --release ... -- --ignored`.

6. **The C-LEARN notebook uses real pinned scores.** `notebooks/build_notebook.py`
   section 6 reads the pinned loops' per-scenario scores instead of manually composing
   link-score products, and the experience-report section reflects #653 as fixed.

### Success criteria

- All work lands via TDD: failing tests first (the three reproduction fixtures), then
  the implementation that makes them pass.
- `cargo test --workspace` stays green within the 3-minute cap; new C-LEARN coverage is
  `#[ignore]`d (debug-mode C-LEARN parse+LTM-compile alone measures ~39s, see
  "Additional Considerations").
- The pre-commit hook passes end to end.
- No silent stubbing: a pin that genuinely cannot be scored (unresolvable cycle,
  oversized expansion subgraph) surfaces a `Warning` naming the pin, never a quiet 0.

### Explicitly out of scope

- A "pinned-only" instrumentation mode (Gap 1's proposed workaround) -- obsoleted by
  the discovery-cost work; see Summary.
- Lifting the u16 slot ceiling or further reducing helper-aux volume (#654).
- Making discovery's raw-|score| loop ranking robust to near-singularity values (noted
  in the C-LEARN notebook; separate concern).
- Un-ignoring C-LEARN-scale tests in the debug-mode workspace run (requires a separate
  release-mode CI lane; tracked as its own issue).

## Acceptance Criteria

### pin-dims.AC1: Pure-A2A pins are scored per element
- **AC1.1 Success:** A pinned cycle of A2A variables over `[region]` (Bare references)
  in forced-discovery mode emits `$⁚ltm⁚loop_score⁚pin1` with `dimensions = [region]`
  and an `ApplyToAll` equation; the compiled simulation produces finite, eventually
  non-zero scores for every element slot.
- **AC1.2 Success:** `loop_partitions["pin1"]` has one entry per element slot (not
  `[None]`), resolved through element-level stocks.
- **AC1.3 Success (exhaustive dedup unchanged):** the same pin in exhaustive mode on a
  small model dedups against the enumerated A2A loop exactly as scalar pins dedup
  today (no second loop_score var; name transfers to the enumerated loop).
- **AC1.4 Edge:** a pinned A2A cycle over a multi-dimensional variable
  (`pop[Region, Age]`) carries both dimensions and one slot per element pair.

### pin-dims.AC2: Per-element-equation (FixedIndex) pins are scored per element
- **AC2.1 Success:** A pinned cycle of `Equation::Arrayed` variables whose per-element
  equations reference literal elements (the MDL-importer shape) emits one
  `$⁚ltm⁚loop_score⁚pin{n}` with an `Equation::Arrayed` over the shared dimension;
  each slot's equation references that element's FixedIndex link-score variables
  subscripted at that element.
- **AC2.2 Success:** the compiled simulation produces correct non-zero scores for
  *every* element slot, not just the lexicographically-first element.
- **AC2.3 Success:** the equation compiles -- the fragment-diagnostics pass reports no
  Warning for the pin's loop_score var.

### pin-dims.AC3: Mixed scalar/arrayed pins are scored per element
- **AC3.1 Success:** A pinned cycle mixing scalar and arrayed variables (arrayed stock
  -> per-source-element reduce -> scalar aux -> per-target-element broadcast -> arrayed
  flow) emits one `Equation::Arrayed` loop-score var over the arrayed variables' shared
  dimension, each slot referencing the per-element link-score names that exist
  (`from[e]→to`, `from→to[e]`, `"from→to"[e]`).
- **AC3.2 Success:** every element slot's score is finite and eventually non-zero.

### pin-dims.AC4: Genuinely cross-element pins keep working
- **AC4.1 Success:** A pinned migration-style cycle (`pop -> migration_pressure ->
  migration_in -> pop` where hops cross elements) that expands to a single element
  circuit produces a scalar `pin{n}` loop score whose links carry element subscripts
  (the existing cross-element loop equation machinery).
- **AC4.2 Success:** when the expansion produces multiple non-diagonal circuits, each
  gets its own scalar loop score with deterministic ids (`pin{n}` plus a stable
  suffix), all carrying the pin's user name through `model_detected_loops`.

### pin-dims.AC5: The enumerated A2A-collapse path is fixed by the same machinery
- **AC5.1 Success:** the AC2 fixture in *exhaustive* mode (no pin needed -- the
  enumerator finds the cycle) produces an enumerated A2A loop whose per-slot scores are
  correct for every element, not just the lex-first one.
- **AC5.2 Success (no regression):** Bare-A2A models keep byte-identical ApplyToAll
  loop-score equations (the compact form), pinned by existing tests.

### pin-dims.AC6: Failure modes are loud
- **AC6.1:** A pin whose element-level expansion subgraph exceeds
  `MAX_LTM_SCC_NODES` is reported invalid with a clear reason (Warning), not hung on
  or silently zeroed.
- **AC6.2:** A pin whose cycle has no element-level instantiation (no matching
  circuits) is reported invalid with a clear reason.
- **AC6.3 (no regression):** invalid-pin diagnostics for unordered/stock-free variable
  sets keep firing as today.

### pin-dims.AC7: Surfaces stay consistent
- **AC7.1:** `model_detected_loops` reports every scored pin loop (id + user name), in
  both modes, backed 1:1 by emitted loop_score vars.
- **AC7.2:** `simlin_analyze_get_relative_loop_score("pin1[elem]")` resolves an A2A
  pin's element slot through the existing `LoopElementIndex` machinery;
  `simlin_analyze_get_loop_element_count("pin1")` reports the slot count.
- **AC7.3:** pysimlin: `Sim.get_relative_loop_score("pin1[elem]")` and
  `run.loops` work for arrayed pins (new test in `test_pinned_loops.py`).

### pin-dims.AC8: C-LEARN end-to-end + notebook
- **AC8.1:** An `#[ignore]`d test pins the "Feedback cooling" climate loop on C-LEARN
  v77, compiles with LTM discovery, runs to end, and asserts the pin's score series is
  finite and non-zero for the `deterministic` scenario element.
- **AC8.2:** `notebooks/build_notebook.py` section 6 reads pinned-loop scores from the
  engine; `verify_notebook.py` still passes.

## Architecture

### Root cause

`CausalGraph::build_loop_from_cycle` (`src/simlin-engine/src/ltm/graph.rs:793`) builds
every pinned `Loop` with `dimensions: vec![]` and variable-level links. The loop-score
emitter in `model_ltm_variables` then produces a *Scalar* equation whose references
resolve through `resolve_link_score_name_for_loop`'s fallbacks: bare A2A names (a
dimension mismatch when referenced from scalar context) or one arbitrary
lexicographically-first FixedIndex name. Either way the fragment fails to compile (or
computes the wrong thing), and `assemble_module` stubs it to constant 0 with only a
fragment-diagnostics Warning.

The exhaustive enumerator does not have this problem for *Bare-shaped* A2A cycles
because `classify_cycle` -> `build_loops_from_tiered` carries `dimensions` onto the
Loop and the equation becomes `ApplyToAll`. But its slow-path A2A-collapse
(`build_element_level_loops`' pure-dimension branch) has the same latent deficiency:
it discards the per-circuit element information when collapsing, so on
per-element-equation models (where only FixedIndex link-score names exist) the
ApplyToAll equation references one arbitrary element's link score for every slot.

### The shared fix: slot-aware loop-score equations

Three pieces of shared machinery, used by both the enumerated path and the pin path:

**1. `Loop.slot_links: Vec<Vec<Link>>` (new field, `src/simlin-engine/src/ltm/types.rs`).**
For a dimensioned loop backed by per-element circuits, one element-subscripted link
cycle per dimension slot, in the same row-major slot order as
`loop_dimension_element_tuples`. Empty when the loop's links resolve uniformly (pure
Bare A2A -- the fast path) or the loop is scalar. Slots with no circuit (structurally
absent elements) hold an empty link list and score 0.

**2. Diagonal-family detection (new helper, `src/simlin-engine/src/db/ltm/loops.rs`).**
Given a group of element circuits sharing one variable-level structure, decide whether
they form a *diagonal family*: each circuit's subscripted nodes agree on a single
element tuple of the group's shared dimensions (the dims common to every arrayed
variable in the cycle), and distinct circuits map to distinct tuples. Returns the
shared dims plus the slot -> circuit mapping. Mixed circuits (scalar nodes present) can
still be diagonal; genuinely cross-element circuits (a node visiting a different
element) are not.

**3. Dimension-aware loop-score equation generation (`src/simlin-engine/src/ltm_augment.rs`).**
`generate_loop_score_variables` returns a real `datamodel::Equation` per loop:
- scalar loop -> `Scalar(product)` (unchanged);
- dimensioned loop, `slot_links` empty -> `ApplyToAll(dims, product of bare refs)`
  (unchanged -- the compact form for Bare-A2A cycles);
- dimensioned loop with `slot_links` -> `Equation::Arrayed(dims, per-slot products)`,
  where each slot's product is `generate_loop_score_equation` over that slot's
  element-subscripted links -- the existing `loop_link_score_ref` resolution already
  handles every per-element name form correctly when given subscripted links.

`model_ltm_variables` uses the returned equation directly instead of re-tagging scalar
text via `ltm_synthetic_equation`.

### Enumerated-path adoption

`build_element_level_loops`' pure-dimension branch keeps its grouping logic but no
longer discards the per-circuit links: it runs the diagonal-family detection, builds
per-slot links via the existing `build_element_subscripted_links`, and sets
`slot_links`. Bare-A2A groups (every link resolves to an emitted Bare name) keep
`slot_links` empty so their equations stay byte-identical ApplyToAll.

The mixed and cross-element branches are already correct (their per-circuit scalar
loops carry element-subscripted links) and are not changed.

### Pin-path classification

`model_pinned_loops` (`src/simlin-engine/src/db/ltm/pinned.rs`) classifies each
resolved cycle with `classify_cycle` (the same `model_edge_shapes` + dimension lookup
the tiered enumerator uses):

- **PureScalar** -> `build_loop_from_cycle` (unchanged).
- **PureSameElementA2A** -> the Loop carries `dimensions` (datamodel casing) and
  element-level stocks (`build_a2a_loop_stocks`); `slot_links` stays empty (Bare names
  exist by construction). One loop, id `pin{n}`.
- **CrossElementOrMixed** -> element-level expansion:
  1. Project `model_element_causal_edges` onto the pin's variables plus synthetic agg
     nodes (the same `keep_node` rule as the tiered enumerator's slow path).
  2. Guard: if the projected subgraph's largest SCC exceeds `MAX_LTM_SCC_NODES`, the
     pin is invalid ("expansion too large") -- AC6.1.
  3. Johnson on the subgraph; keep circuits whose agg-trimmed, subscript-stripped
     rotation equals the pin's variable-cycle rotation.
  4. Zero matching circuits -> invalid pin (AC6.2).
  5. Diagonal family -> ONE Loop, id `pin{n}`, `dimensions` = shared dims,
     `slot_links` populated -> `Equation::Arrayed` loop score.
  6. Otherwise -> one scalar Loop per circuit with element-subscripted links; ids
     `pin{n}` when there is exactly one, `pin{n}⁚{j}` (j = 1.., deterministic circuit
     order) otherwise.

`PinnedLoop` becomes `{ loops: Vec<Loop>, name: String }`; `model_detected_loops` and
the pin emitter in `model_ltm_variables` iterate the scored loops. The pin emitter also
gains the same per-link emission dispatch the enumerated path uses (subscript
stripping + agg-half routing) and registers per-slot partitions via
`partition_for_loop`.

### What stays the same

- Link-score emission (`emit_link_scores_for_edge` and friends) -- already correct for
  every shape; pins just need to reference what is emitted.
- Discovery-mode loop *discovery* (ltm_finding) -- discovered loops' scores are
  computed in Rust from link-score series, not synthetic equations.
- The FFI / ltm_post machinery -- A2A loop scores with `dimensions` already flow
  through `LoopElementIndex` / `compute_rel_loop_scores_per_element`; pins reuse it.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Shared slot-aware loop-score machinery
**Goal:** `Loop.slot_links` exists and `generate_loop_score_variables` emits
`Scalar` / `ApplyToAll` / `Arrayed` equations from it.

**Components:**
- Add `slot_links: Vec<Vec<Link>>` to `Loop` (default empty; documented invariants).
- Restructure `generate_loop_score_variables` to return per-loop
  `datamodel::Equation` values; `model_ltm_variables` consumes them directly
  (delete the `ltm_synthetic_equation` re-tagging for loop scores).
- Unit tests in `ltm_augment.rs`: scalar unchanged, ApplyToAll unchanged
  (byte-identical for a Bare-A2A loop), Arrayed emission for a loop with
  `slot_links` referencing FixedIndex / per-target-element / subscripted-A2A names.

**Done when:** new unit tests pass; all existing LTM tests pass unchanged.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Enumerated path adoption (AC5)
**Goal:** `build_element_level_loops`' pure-dimension collapse populates `slot_links`,
fixing per-element-equation models in exhaustive mode.

**Components:**
- Diagonal-family detection helper (shared dims + slot->circuit mapping).
- Pure-dimension branch: build per-slot links (`build_element_subscripted_links`),
  ordered by slot tuple; set `slot_links`; keep ApplyToAll when every link resolves to
  an emitted Bare name.
- TDD fixture: small per-element-equation model (the AC2 shape) in exhaustive mode --
  currently produces wrong scores for non-lex-first elements; asserts correct per-slot
  scores after.

**Done when:** AC5.1 fixture passes; existing element-graph / A2A tests byte-identical.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Pin classification -- pure-A2A pins (AC1)
**Goal:** Pinned PureScalar / PureSameElementA2A cycles are classified and scored.

**Components:**
- `model_pinned_loops`: classify via `classify_cycle`; PureSameElementA2A pins carry
  `dimensions` + element-level stocks. `PinnedLoop` -> `{ loops: Vec<Loop>, name }`.
- Pin emitter in `model_ltm_variables`: per-slot partitions
  (`partition_for_loop`), dimension-aware equations, dedup logic updated for the new
  struct shape.
- `model_detected_loops` updated for the struct change.
- TDD fixtures: the pure-A2A repro (forced discovery + exhaustive dedup), multi-dim
  A2A pin.

**Done when:** AC1 fixtures pass; existing scalar pin tests
(`tests/simulate_ltm_pinned.rs`) pass unchanged.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Pin element-level expansion (AC2, AC3, AC4, AC6)
**Goal:** CrossElementOrMixed pins expand on the element graph and are scored
per element (diagonal family) or per circuit (cross-element).

**Components:**
- Element-subgraph projection + Johnson + circuit filtering in `model_pinned_loops`
  (with the SCC guard and no-circuits invalid-pin diagnostics).
- Diagonal-family pins -> one Loop with `dimensions` + `slot_links`; the pin emitter's
  per-link emission dispatch gains agg-half routing (mirroring the enumerated path).
- Cross-element pins -> scalar loops with element-subscripted links; multi-circuit id
  scheme `pin{n}⁚{j}`.
- TDD fixtures: the per-element-equation repro (AC2), the mixed repro (AC3), a
  migration-style cross-element pin (AC4), oversized-expansion and no-instantiation
  invalid pins (AC6).

**Done when:** AC2/AC3/AC4/AC6 fixtures pass.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: FFI / pysimlin surface (AC7)
**Goal:** Arrayed pin scores are readable end to end through libsimlin and pysimlin.

**Components:**
- Verify (and fix if needed) `simlin_analyze_get_relative_loop_score("pin1[elem]")` and
  `simlin_analyze_get_loop_element_count("pin1")` for arrayed pins; the lone-pin
  rel-score caveat docs updated for per-slot normalization.
- pysimlin test: arrayed pin in `test_pinned_loops.py` (model with an arrayed pinned
  loop; read `pin1[elem]` scores; `run.loops` includes the pin).

**Done when:** AC7 tests pass through the full FFI stack; pysimlin test suite green.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: C-LEARN end-to-end + notebook + docs (AC8)
**Goal:** The issue's headline scenario works on the real model and is documented.

**Components:**
- `#[ignore]`d test (in `tests/simulate_ltm_pinned.rs`, gated on `file_io`): pin
  "Feedback cooling" on C-LEARN v77, compile with LTM discovery, run, assert finite
  non-zero deterministic-scenario score.
- Update `notebooks/build_notebook.py` section 6 to read pinned scores; re-verify with
  `verify_notebook.py`.
- Update `docs/design/ltm--loops-that-matter.md` (pinned-loop section) and close out
  GH #653 (comment summarizing Gap 1 obsolescence + Gap 2 fix).
- File follow-up issues: release-mode CI lane for un-ignoring C-LEARN tests; any
  residual carve-outs discovered during implementation.

**Done when:** AC8 passes; docs/index updated; issue updated.
<!-- END_PHASE_6 -->

## Additional Considerations

**Why C-LEARN tests stay `#[ignore]`d.** Measured 2026-06-01 in debug mode (the mode
the 3-minute `cargo test --workspace` cap applies to): `clearn_ltm_discovery_compiles`
takes 39.3s; `clearn_with_ltm_simulates_model_vars_identically` takes 59.6s. The recent
performance work made these fast in *release* mode (~4-6s), but a single debug-mode
C-LEARN test still consumes 22-33% of the whole workspace budget. Un-ignoring them
needs a release-mode CI lane -- tracked separately, out of scope here.

**ID scheme for multi-instance pins.** A pin that expands to multiple non-diagonal
circuits gets ids `pin{n}⁚{j}` using the reserved `⁚` separator (never present in user
content, parses as a bare id through `parse_subscripted_loop_id`). The dominant cases
(scalar, pure-A2A, diagonal families, single cross-element circuits) all keep the plain
`pin{n}` id.

**Dedup semantics in exhaustive mode are unchanged at variable granularity.** A pin
whose variable-cycle rotation matches an enumerated loop is skipped (the enumerated
loop is already correctly classified and scored); the pin's name transfers to the
enumerated loop in `model_detected_loops` exactly as today.

**No protobuf / serialization impact.** `Loop` and `PinnedLoop` are in-memory analysis
types; `LoopMetadata` (the persisted pin spec) is unchanged.

**Loud failure invariant.** Every path that cannot produce a correct score must
surface a Warning naming the pin (`model_pinned_loops` invalid list -> the existing
Warning emission in `model_ltm_variables`), never fall through to a silently-zero
equation. The fragment-diagnostics pass remains the backstop for compile failures.
