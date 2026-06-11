# LTM Shape Expressiveness: One Per-Axis Access Model (Epic #488, Phase 1)

## Summary

Ten open `ltm` issues (#765, #766, #767, #764, #771, #751, #757, #525, #526, #769)
are all symptoms of the same representational gap: LTM describes *how an arrayed
variable is accessed* with several partial, locally-derived vocabularies that
cannot express common shapes, so each surface (classification, element-graph
expansion, link-score emission, loop assembly, naming) re-derives or
approximates the access independently. The agg machinery's
`AxisRead` (`src/simlin-engine/src/ltm_agg.rs:230`) is the most truthful of
these vocabularies but is (a) incomplete -- it cannot express a subset-reduced
axis (#766) or per-source slices (#767) -- and (b) confined to reducers, while
direct references squeeze through the four-variant `RefShape`
(`src/simlin-engine/src/db/analysis.rs:113`), whose `DynamicIndex` catch-all is
the source of the #525 phantom-loop pathology and the #757/#769 over-conservatism.

This design makes `AxisRead` the **single per-axis access vocabulary** for both
reducer reads and direct references, derived by one per-axis classifier and
consumed by every surface through one row/slot derivation function
(`read_slice_rows`, `src/simlin-engine/src/db/ltm/loops.rs:253`). The ten
issues fall out as consequences (one honest adjunct: #526; see the table).
This follows the recurring #761/#775 lesson -- every two-surface decision must
consume the same data -- and the epic's no-silent-wrong-numbers rule: shapes
that remain inexpressible keep the loud `unscoreable_edges` degradation from
#758, never a quiet stub.

## Definition of Done

### Primary deliverables

1. **`AxisRead::Reduced` carries an optional element subset** (#766), and
   `AggNode` carries **per-source read slices** (#767) plus a canonical
   result-axes spec, with the value-shape hoisting precondition (#771).
2. **One co-reduced-row derivation.** `try_cross_dimensional_link_scores`'
   full-cartesian re-derivation (`src/simlin-engine/src/db/ltm/link_scores.rs:300-304`
   and the grouping at `:374-380`) is replaced by `read_slice_rows` over the
   edge's variable-backed `AggNode` (#765); the #752 gate's Pinned exclusion
   (`ltm_agg.rs:1238-1242`) is deleted.
3. **Non-aligned variable-backed partial reduces mint synthetic aggs** (#764),
   reusing the existing GH #534 carve-out precedent
   (`ltm_agg.rs:557-576`) so broadcast/permuted shapes ride the
   already-correct synthetic two-half emitters and the #528 projection.
4. **A `RefShape::PerElement` family** for mixed iterated+literal subscripts
   (#525), consumed from one source of truth by classification, expansion,
   emission, AND naming; the mapped-subscript classifier gates on
   `mapped_element_correspondence` (#757); the #510 per-element construction
   generalizes to ApplyToAll targets (#769).
5. **Per-ident pin maps** in the agg-to-target emitter (#751) and a
   position-correspondence-tightened non-live-dep collapse (#526, adjunct).
6. Every previously-correct shape keeps **byte-identical emitted equations**
   (the golden-pin pattern from the #740/#761/#775 batches), and every shape
   that stays unscoreable keeps exactly one `Warning` + dropped loop scores.

### Success criteria

- TDD throughout: each task lands its RED fixtures first; named pinning tests
  flip exactly as documented per task (flip notes below).
- `cargo test --workspace` stays inside the 3-minute debug cap; all new
  fixtures are tiny (2x2 dims) and assert zero assembly warnings plus
  analytically-known scores.
- C-LEARN LTM compile stays within the #655 budget under the measurement
  protocol in "Perf guardrails".

### Explicitly out of scope

- Element-mapped (non-positional) dimension pairs: the #756 gate inside
  `mapped_element_correspondence` (`src/simlin-engine/src/dimensions.rs:694-705`)
  stays. Because every mapped acceptance in this design routes through that one
  helper, the gate is inherited everywhere by construction.
- Genuinely dynamic indices (`arr[i+1]`, `SUM(pop[idx,*])`): not statically
  describable; they keep `DynamicIndex` + the conservative cross-product, and
  dim-incompatible cases keep the #758 loud skip.
- Different-cardinality element maps (#753) and the positional-vs-element-map
  execution inconsistency (#756) themselves.
- A generated-corpus "every emitted fragment compiles" harness (see "Test
  strategy" for the call and justification).

## Architecture: the one generalization

### 1. The per-axis vocabulary (extended `AxisRead`)

```rust
// ltm_agg.rs -- the ONLY description of how one source axis is accessed.
pub enum AxisRead {
    /// One literal element of this axis is read (`pop[NYC,*]`, `pop[Region,young]`).
    Pinned(String),
    /// Iterated over the target equation's dimension space; `dim` is the
    /// TARGET dim, `source_dim` the source's declared dim on this axis
    /// (equal in the literal case, different for a positionally-mapped pair).
    Iterated { dim: String, source_dim: String },
    /// Reduced away by a reducer. `subset: None` = the full extent;
    /// `Some(elems)` = a proper-subdimension StarRange (`SUM(arr[*:Sub])`,
    /// GH #766), resolved at enumeration time via `SubdimensionRelation`.
    Reduced { subset: Option<Vec<String>> },
}
```

A **direct reference**'s access is the same vector minus `Reduced` (a
non-reducer reference never collapses an axis). The per-axis decision logic
currently inlined in `compute_read_slice` (`ltm_agg.rs:916-1016`) is extracted
into one function:

```rust
/// Classify ONE subscript index against one source axis. Returns None for
/// anything not statically describable (dynamic expr, @N, Range, declined
/// mapping). Shared verbatim by compute_read_slice (reducer args) and
/// classify_iterated_dim_shape (direct references), so the reducer path and
/// the reference path can never disagree about an axis.
fn classify_axis_access(
    idx: &IndexExpr2, axis_dim: &Dimension,
    target_iterated_dims: &[String], dim_ctx: &DimensionsContext,
) -> Option<AxisRead>
```

Its mapped-`Iterated` arm accepts a pair iff
`mapped_element_correspondence(dim, source_dim)` yields a usable
(bijective-preimage) remap via `iterated_axis_slot_elements`
(`ltm_agg.rs:277-316`) -- **both declaration directions**, replacing the
one-directional `has_mapping_to` gates at `ltm_agg.rs:979` and
`db/ltm_ir.rs:279` (#757). The correspondence helper is already
direction-agnostic and carries the #756 positional-only gate, so classification,
expansion (`expand_same_element`'s `Mapped` arm,
`db/analysis.rs:476-505`), and `link_score_dimensions`' Bare-site retarget
(`db/ltm/link_scores.rs:151-162`) all consume one predicate.

### 2. `AggNode`: per-source slices + a canonical result spec + value shape

```rust
pub struct AggSource {
    pub var: String,              // canonical model-variable name
    pub read_slice: Vec<AxisRead> // one entry per THIS source's axes
}
pub struct AggNode {
    pub name: String,
    pub equation_text: String,
    /// Canonical result axes (datamodel-cased): the union of the sources'
    /// Iterated target dims, in first-occurrence order over the canonical
    /// slice. Unchanged meaning from today's field.
    pub result_dims: Vec<String>,
    pub sources: Vec<AggSource>,  // replaces source_vars + the single read_slice
    pub is_synthetic: bool,
}
```

Invariants every consumer may rely on (enforced by the enumerator, asserted in
unit tests):

- **I1 (projection):** each source's `Iterated` axes map injectively into
  `result_dims`; a source with no `Reduced` axes whose `Iterated` axes are a
  subset of `result_dims` is a *feeder* (its value is constant per result
  slot). This is the #767 acceptance rule, replacing `combined_read_slice`'s
  identical-slices requirement (`ltm_agg.rs:1064-1077`).
- **I2 (arity):** `read_slice.len()` equals the source's declared dim count.
- **I3 (subset):** a `Reduced::subset` is a non-empty proper subset of the
  axis's elements; `None` means full extent.
- **I4 (single derivation):** the rows a source contributes per result slot are
  computed ONLY by `read_slice_rows` applied to *that source's* slice. No
  consumer re-derives rows from `from_dims` cartesian products.
- **I5 (value shape):** a node is minted only for scalar-valued reducers --
  `reducer_is_hoistable` (`ltm_agg.rs:203-208`) additionally requires
  `reducer_collapses_to_scalar` (`ltm_agg.rs:172-174`), de-hoisting RANK (#771).
  An agg node *is* "a scalar value per result slot"; RANK has no such value.

`read_slice_rows` itself changes only to honor `Reduced::subset` (enumerate the
subset's elements instead of the axis's full list). It already fixes `Pinned`
axes to their literal element -- which is exactly why making it the single
derivation fixes #765: the bug is not in the derivation but in
`try_cross_dimensional_link_scores` not using it.

### 3. `RefShape::PerElement` -- the per-axis family for direct references

```rust
pub enum RefShape {
    Bare,                  // unchanged: bare ref / all-iterated identity subscript
    FixedIndex(Vec<String>), // unchanged: all axes Pinned
    /// Mixed per-axis access: >=1 Iterated and >=1 Pinned axis (GH #525).
    /// Invariant: no Reduced entries; not all-Pinned (that canonicalizes to
    /// FixedIndex) and not all-Iterated-identity (that canonicalizes to Bare).
    PerElement { axes: Vec<AxisRead> },
    Wildcard,              // unchanged
    DynamicIndex,          // unchanged: the genuinely-unknowable catch-all
}
```

`classify_iterated_dim_shape` (`db/ltm_ir.rs:230-285`) generalizes: build the
per-axis vector with `classify_axis_access`; if every axis resolves and at
least one is `Iterated`, the shape is `Bare` (all-`Iterated`, the existing
#511 case, names/paths byte-identical) or `PerElement` (mixed). All-`Pinned`
falls through to `classify_subscript_shape`'s `FixedIndex` as today. The
canonicalization rule keeps the existing `Bare`/`FixedIndex` shapes -- and
therefore every existing link-score NAME -- untouched for already-handled
shapes; `PerElement` is minted only where today's classifier says
`DynamicIndex` and is wrong.

The four consumers, all reading the one `axes` vector:

- **Expansion** (`emit_edges_for_reference`, `db/analysis.rs:213`): a new
  `PerElement` arm calls `read_slice_rows(axes, ...)` -- with no `Reduced`
  axes the rows/slots are 1:1 -- and emits `from[row] -> to[slot ⨯ extra-target-dims]`
  (the slot projects onto `to`'s shared dims and broadcasts over unshared ones,
  the same rule `expand_same_element` applies for `Bare`). For the #525 repro
  this kills the `pop[a,young] -> row_sum[b]` phantoms at enumeration time.
- **Emission**: one scalar link score per `(row, slot)` named
  `$⁚ltm⁚link_score⁚{from}[{row}]→{to}[{slot}]` -- the **existing**
  partial-reduce name grammar from `try_cross_dimensional_link_scores`
  (`link_scores.rs:386-389`), which `loop_link_score_ref` and discovery's
  `parse_link_offsets` already resolve (element-in-name on both sides). For a
  scalar `to`, the slot side is omitted (the full-reduce form). The equation is
  the slot-pinned partial (`generate_scalar_to_element_equation`-style element
  pinning) with the live ref `{from}[{row}]` held as a `FixedIndex`-shaped
  subscript -- a real `Expr0::Subscript`, never `SUM(...)`-wrapped.
- **Naming**: no new grammar. Names are not serialized (CLAUDE.md hard rules),
  but in-flight consistency within one compile is load-bearing for loop-score
  resolution -- reusing the per-`(row, slot)` grammar means
  `resolve_link_score_name_for_loop` / `loop_link_score_ref` /
  `parse_link_offsets` need no changes.
- **Loop assembly**: `build_element_level_loops`' pure-dimension A2A collapse
  must NOT collapse circuits traversing a `PerElement` hop (the only emitted
  scores are per-`(row, slot)` scalars; an ApplyToAll loop-score equation would
  reference a nonexistent Bare name and silently stub). The routing predicate
  mirrors the existing `representative_has_partial_reduce_hop` check
  (`db/ltm/loops.rs:716-731`) but keys on the SAME IR
  (`model_edge_shapes` containing `PerElement` for the hop), so expansion and
  loop routing consume one decision -- the #752 single-gate pattern.
- **Expr0 sibling**: `classify_expr0_subscript_shape` /
  `is_live_source_iterated_dim_subscript` (`ltm_augment.rs:67-101, 253-290`)
  gain the same mixed recognition so the partial builder's live-shape match
  agrees with the IR (the documented sync requirement at
  `ltm_augment.rs:198-207`).

### 4. Per-ident pins in the agg-to-target emitter (#751)

`generate_scalar_to_element_equation`'s single
`source_pin_element: Option<&str>` (`ltm_augment.rs:1814-1847`) generalizes to
a per-ident pin map `HashMap<Ident<Canonical>, String>`.
`emit_agg_to_target_link_scores` computes, for the live agg AND for every
frozen co-agg in `reducer_subst` with non-empty `result_dims`, the projection
of the target element onto that agg's result axes -- the exact
`agg_pin_for_target` function that already exists
(`link_scores.rs:1460-1471`), applied per ident instead of once. The
other-agg freeze at `link_scores.rs:1368-1375` then produces
`PREVIOUS(B[<projected slot>])` instead of the ill-typed bare `PREVIOUS(B)`.

### 5. Non-aligned variable-backed reduces mint synthetic aggs (#764)

`walk_var_equation` already declines to treat a whole-RHS reducer as
variable-backed when the variable-backed score path cannot express it (the
GH #534 mapped carve-out, `ltm_agg.rs:557-576`), falling through to a synthetic
agg with the well-tested two-half scoring. The same fallthrough is extended to
the two #764 shapes: `result_dims` a strict subset of the variable's dims
(broadcast) or in a different order (permuted). The synthetic agg is arrayed
over `result_dims`; `emit_agg_routed_edges` (`db/analysis.rs:660`) and the
#528 projection (`agg_pin_for_target`) already handle broadcast fan-out and
slot projection, and permutation is a non-issue because slots are keyed by
`result_dims` order, not the target's declared order. Cost: one synthetic aux
duplicating the variable's value -- the same documented trade the #534
carve-out made. The aligned shape keeps the variable-backed fast path
(byte-identical), and `variable_backed_partial_reduce_agg`'s alignment
condition (`ltm_agg.rs:1243-1247`) becomes the *routing* split between the two
regimes rather than a gate with a conservative residue.

## Per-issue "falls out because" table

| Issue | Falls out because |
|---|---|
| #766 | `Reduced{subset}` makes the subrange representable in the one vocabulary; `compute_read_slice`'s StarRange arm (`ltm_agg.rs:940`, rustdoc `:876-882`, breadcrumb repointed) resolves the named subdimension via `SubdimensionRelation`; `read_slice_rows` and `emit_agg_routed_edges` iterate the subset, fixing MEAN/STDDEV divisors and dropping spurious out-of-subset edges. |
| #765 | `try_cross_dimensional_link_scores` consumes `read_slice_rows` over the edge's variable-backed `AggNode` instead of re-deriving from the `from_dims` cartesian product (`link_scores.rs:300-304`, `:374-380`); `Pinned` axes are fixed by I4's derivation, so the divisor is the true read count and unread rows get no score. The gate's Pinned exclusion is deleted. |
| #767 | `AggNode.sources` + invariant I1 admit a feeder whose slice is a projection of the canonical slice; the feeder's per-`(row, slot)` scores come from `read_slice_rows` applied to ITS slice (1:1 rows), and `emit_agg_routed_edges` routes each source by its own slice. |
| #764 | The #534 "mint a synthetic agg when the variable-backed path can't express it" precedent generalizes to broadcast/permuted `result_dims`; the synthetic emitters + #528 projection already handle both. The conservative cross-product with missing scores ceases to exist for these shapes. |
| #771 | Invariant I5: hoisting requires `reducer_collapses_to_scalar`, which already encodes the discrimination -- the gate just never consulted it. RANK stays a `Direct` reference scored by the #742 arrayed-capture path. |
| #751 | The per-ident pin map is the #528 "project the target element onto an agg's result axes" function applied to every agg ident in the substituted equation, not only the live one. |
| #757 | Classification's mapped arm consumes `mapped_element_correspondence` -- the same predicate expansion and `link_score_dimensions` already consume -- so the subscripted reverse-declared reference classifies `Bare` and gets the diagonal the bare form already gets. `compute_read_slice`'s mapped gate widens with it (same helper), un-declining reverse-declared sliced reducers as a bonus. |
| #525 | The per-axis vocabulary applied to direct references: iterated+literal mixes become `PerElement{axes}`; rows/slots come from the same `read_slice_rows`; expansion emits only the pinned diagonal (phantom loops die at enumeration); emission/naming reuse the existing per-`(row, slot)` grammar. |
| #769 | All-`Pinned` axes into a disjoint-dim arrayed target is the degenerate (zero-`Iterated`) case of the family; `try_disjoint_dim_arrayed_link_scores`' `Ast::Arrayed`-only gate (`link_scores.rs:842`) generalizes over the target's equation kind (an ApplyToAll target = an Arrayed target with one shared slot body), shrinking the #758 sweep. |
| #526 | **Does not fall out** -- it is an Expr0-side recognizer in the partial builder (`is_other_dep_iterated_dim_subscript`, `ltm_augment.rs:138-155`) with no dep dims in scope. Small adjunct, justified because it applies the same positional-correspondence *rule* as `classify_axis_access`: thread the dep's declared dim names into `IteratedDimCtx`, collapse only on exact position-and-mapping correspondence, and degrade the mismatched case to the loud `PartialEquationError` skip (#743 pattern) instead of silently freezing the wrong element. Descoping it from Phase 1 is acceptable if review judges the threading too invasive; the silent imprecision is then at least documented and unreachable by current fixtures. |

## What stays conservative (the explicit boundary)

- **Element-mapped pairs**: declined everywhere via the one
  `mapped_element_correspondence` gate (#756). Affected edges keep the
  broadcast superset (element graph) and the #758 loud skip (scores).
- **Dynamic indices** (`arr[i+1]`, `SUM(pop[idx,*])`, ranges, `@N`):
  `DynamicIndex` + conservative cross-product; dim-incompatible arrayed pairs
  keep `emit_unscoreable_conservative_edge_warning`.
- **Partial StarRange mixed with literal indices** (`SUM(matrix[D1, *:Sub])`
  classifier-side): `classify_subscript_shape` AC1.4 keeps treating only
  *all*-full-extent subscripts as `Wildcard`; the hoisted slice carries the
  subset, so the coarse classifier shape is routing-irrelevant (`ThroughAgg`
  ignores it). Documented residual, not a behavior gap.
- **De-hoisted RANK's cross-element coupling** (#771): a `RANK(pop, 1)`
  reference classifies by its syntactic shape (`Bare` -> diagonal edges), so
  loops through the rank *ordering* (element r's rank changing because element
  s moved past it) are not enumerated. This replaces today's strictly-worse
  state (cross-element loops enumerated but every score warned-zero). The
  alternative -- reclassifying value-shaped reducer args as `DynamicIndex` --
  recreates the #525 phantom pathology (cross-product edges reading diagonal
  scores) and is rejected. Residual documented in `reducer_collapses_to_scalar`
  rustdoc + a follow-up issue filed at landing time.
- **Feeders that are not projections** (e.g. `SUM(matrix[D1,*] * other[D2])`
  reading an axis outside the canonical slice): `combined_read_slice` still
  declines; the edge keeps the #743 changed-last conservative score and the
  loud co-source degradation.

## Implementation tasks

Seven independently-landable tasks, each with TDD + adversarial review like the
prior batches. Dependency order: T1 -> T2 -> {T3, T4, T5}; T6 depends on T1
(the shared `classify_axis_access`); T7 is independent.

### Task 1: per-axis substrate + agg-node truthfulness (#766, #771)
Extract `classify_axis_access`; add `Reduced::subset` (resolve StarRange
subdimensions); honor the subset in `read_slice_rows`, `emit_agg_routed_edges`,
and the agg aux's own equation; gate hoisting on `reducer_collapses_to_scalar`.
- RED: `SUM/MEAN(arr[*:Sub])` divisor + edge fixtures; RANK-on-a-loop fixture
  asserting zero warnings and positive scores.
- Flips: `rank_frozen_subtree_link_score_scores_correctly`
  (`tests/integration/ltm_array_agg.rs:4065`) -- its "exactly 1 warning naming
  the RANK agg" assertion becomes "no warnings"; repoint the
  `ltm_agg.rs:852-857` rustdoc breadcrumb from #765 to #766.
- Blast radius: byte-identity for all non-subrange, non-RANK models (existing
  suite); `element_graph_proptest` agg-hop expectations must still hold.

### Task 2: per-source `AggNode` representation (pure refactor)
`sources: Vec<AggSource>` replaces `source_vars` + single `read_slice`;
acceptance stays "all slices identical" so behavior is unchanged; consumers
(`ltm_ir`, `emit_agg_routed_edges`, both half-emitters, the #752 gate) read
per-source slices.
- RED: none (refactor); unit tests pin the new invariants I1-I3.
- Blast radius: byte-identity across the whole LTM suite -- this is the
  highest-leverage golden-pin task.

### Task 3: one co-reduced derivation (#765)
`try_cross_dimensional_link_scores` resolves the edge's variable-backed
`AggNode` and enumerates rows/slots/co-reduced sets via `read_slice_rows`
(falling back to the full cartesian only when no agg exists -- the
dynamic-index conservative family); delete the Pinned exclusion from
`variable_backed_partial_reduce_agg` and the matching loop-routing comment.
- RED: the #765 fixture (`outf[D1] = MEAN(cube[D1,x,*])` in a loop): link and
  loop scores read 0.5, no `cube[*,y,*]` score vars exist.
- Flips: `element_graph_variable_backed_pinned_mixed_reduce_stays_cross_product`
  (`src/db/element_graph_tests.rs:900`) -> read-slice diagonal.
- Risk: value churn is intended ONLY for Pinned-bearing/subset shapes; an
  explicit golden assertion pins the aligned `SUM(matrix[D1,*])` emissions
  byte-identical.

### Task 4: synthetic-agg minting for non-aligned variable-backed shapes (#764)
Extend `walk_var_equation`'s carve-out to broadcast/permuted `result_dims`;
`variable_backed_partial_reduce_agg` keeps gating the aligned fast path.
- RED: `out[D1,D3] = SUM(matrix[D1,*])` and a permuted-axes fixture, each
  closed in a loop: zero warnings, non-zero correct loop scores.
- Blast radius: aligned shape byte-identical; the new synthetic aux appears in
  layout (slot-count guard on C-LEARN, see perf).

### Task 5: feeder sub-slice acceptance (#767) -- **riskiest**
Widen `combined_read_slice` to invariant I1; route feeder element edges
per-slot; emit feeder per-`(row, slot)` scores; extend the #744 body machinery
(`pin_body_to_row` / `freeze_pinned_body`, `ltm_augment.rs:3835, :3945`) to pin
the per-row feeder reference inside the pinned body; loop-builder fast-path
routing for the feeder hop.
- RED: the #767 fixture (`growth[D1] = SUM(matrix[D1,*] * frac[D1])`) with the
  co-source closure scored non-zero.
- Flips: `un_hoisted_iterated_dim_feeder_co_source_closure_stays_loud`
  (`tests/integration/ltm_array_agg.rs:3444`) -> positive score assertions.
- Risk: the #743 implementor scoped this as multi-day and destabilizing to the
  settled #752/#534 paths -- hence it lands LAST of the agg-side tasks, after
  T2/T3 have stabilized the representation, with the full suite as its
  byte-identity gate.

### Task 6: the `PerElement` reference family (#525, #757, #769)
Classifier generalization (+ the `mapped_element_correspondence` gate swap in
both `classify_iterated_dim_shape` and `compute_read_slice`), the expansion
arm, per-`(row, slot)` emission, the Expr0 sibling, and the loop-builder
routing predicate.
- RED: the #525 repro asserting NO phantom loops and per-slot scores; a
  reverse-declared mapped subscripted fixture asserting the diagonal; the #769
  fixture (`hub[D2] = pop[a1] * 0.05` in a loop) asserting a scored loop and no
  warning.
- Flips: `gh525_two_reference_partially_iterated_row_sum_scores` (the
  flip-note block at `tests/integration/ltm_array_agg.rs:4270-4316` -- delete
  the phantom assertions, assert zero scalar `pop→row_sum` loop scores; the
  Bare-named `pop→row_sum` score is superseded by per-`(row, slot)` names);
  `element_graph_mapped_reverse_declared_subscripted_stays_cross_product`
  (`src/db/element_graph_tests.rs:1502`) -> diagonal, mirroring `:1478`.
- Risk: four surfaces + naming; mitigated by the no-new-grammar choice and the
  single-IR routing predicate.

### Task 7: per-ident pins + non-live-dep correspondence (#751, #526)
The pin-map generalization in `generate_scalar_to_element_equation` /
`emit_agg_to_target_link_scores`; thread dep dim names into `IteratedDimCtx`
and tighten `is_other_dep_iterated_dim_subscript` to positional correspondence
with a loud `PartialEquationError` fallback.
- RED: the #751 two-distinct-arrayed-reducers fixture
  (`to[D1] = SUM(m1[D1,*]) + SUM(m2[D1,*])`) compiling with zero warnings; a
  transposed-dep fixture asserting the loud skip (not a silent wrong-element
  freeze).
- Risk: low; both changes are localized to the augment/emission layer.

## Test strategy

- **Tiny fixtures, seconds-scale**: every fixture uses 2-element dimensions and
  <=8 sim steps, mirroring the existing `ltm_array_agg.rs` suite. No
  production threshold is tested by large fixtures; the cross-agg budget keeps
  its `AggLoopBudgetGuard` test override.
- **Loud-by-default assertions**: every new fixture asserts
  `assembly_warnings(...).is_empty()` plus analytically-known score values --
  the same harness the gh525/#758 tests use -- so a regression to silent
  stubbing is structurally impossible to miss.
- **Golden pins**: T2 (the representation refactor) and T3/T4 (value-bearing
  changes) gate on byte-identity of `model_ltm_variables` output for the
  already-correct shapes, enforced by the existing suite plus explicit
  equation-text assertions on sentinel fixtures (the batches' golden-pin
  pattern).
- **Proptest**: extend `element_graph_proptest`'s spec strategy
  (`src/db/element_graph_proptest.rs`) with iterated+literal mixed subscripts
  and subset reducers, with a vacuity guard for each new pattern (the GH #739
  lesson). The projection invariant and the per-reducer agg-hop expectations
  generalize unchanged.
- **The every-emitted-fragment-compiles harness** (the epic's prevention
  candidate): **follow-up, not Phase 1.** Per-generated-case LTM compilation
  blows the seconds-per-test budget (each case is a full salsa compile), and
  the phase already covers its surfaces via the per-fixture zero-warning
  assertions plus `model_ltm_fragment_diagnostics` (which makes any
  non-compiling fragment loud at runtime). File the harness as its own issue at
  landing time so it is tracked, with a release-mode/`#[ignore]` lane as the
  likely shape.

## Perf guardrails

- **Budget**: C-LEARN LTM compile (the #655 context) must not regress beyond
  the noise floor. The #738 round measured +/-5% wall-clock noise; effects
  smaller than that are adjudicated on `instructions` / `branch-misses`.
- **Protocol (named for the riskiest tasks, T5 and T6)**: warmed, interleaved
  A/B on two worktrees (never destructive git toggling), `perf stat -r 5` on
  the existing C-LEARN LTM compile benchmark (`benches/compiler.rs`), debug
  assertions off. A regression >5% wall-clock or >2% instructions blocks the
  task pending diagnosis.
- **Slot/var-count guard**: T4 and T6 add synthetic vars (one aux per
  non-aligned variable-backed reduce; per-`(row, slot)` scores per `PerElement`
  site). C-LEARN currently sits at 29,764 layout slots against the 65,536 u16
  ceiling (#654); the C-LEARN structural test grows an assertion that the
  emitted LTM var count changes by less than 1% (C-LEARN's FixedIndex-heavy
  per-element equations classify `FixedIndex`, not `PerElement`, so the
  expected delta is ~0).

## Open risks for the adversarial reviewer

1. **T5's body-pinning interaction** (#767 x #744/#762/#743): pinning the
   per-row feeder inside `pin_body_to_row` while preserving the changed-first /
   changed-last freeze conventions is the least-charted code path in this
   design; the #743 implementor explicitly flagged it as destabilizing. If it
   proves intractable inside the phase, T5 degrades gracefully: the feeder
   shape simply stays declined-and-loud (today's behavior), and the task is
   re-scoped without affecting the other six.
2. **A2A-collapse vs per-`(row, slot)` names** (T6): if the loop builder's
   routing predicate and the emission disagree for any `PerElement`-bearing
   circuit family (e.g. a mixed scalar/arrayed cycle reaching the *mixed*
   branch rather than the pure-dimension branch), the loop score references a
   nonexistent name -- loud (fragment warning + 0) but degraded. The shared-IR
   predicate is the mitigation; the RED fixtures must include a mixed-branch
   cycle through a `PerElement` hop.
3. **Intended value churn in T3**: scores for Pinned-bearing and subset shapes
   *change on purpose* (that is the fix); the risk is mis-scoping the churn.
   The byte-identity gate must therefore be expressed as "everything except the
   enumerated flip-list", and the flip-list is part of the task's review
   surface.
4. **Discovery-mode parsing of new names**: `parse_link_offsets` handles
   element-in-name passthrough today, but T6's new per-`(row, slot)` names on
   *non-reducer* edges are a new producer of that grammar; a discovery-mode
   fixture per new name form is required (the #748/#698 lesson: exhaustive and
   discovery must be exercised symmetrically).
5. **Salsa determinism**: `PerElement.axes` and `AggNode.sources` ride salsa
   `Update` types; ordering must be derived from DFS/declaration order (never
   HashMap iteration), matching the determinism contracts documented on
   `enumerate_agg_nodes` and `model_ltm_reference_sites`.
