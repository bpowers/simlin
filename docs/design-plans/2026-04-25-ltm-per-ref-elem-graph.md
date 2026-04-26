# LTM Per-Reference Element Graph and Partial Equations

Date: 2026-04-25
Tracks tech-debt items: **#20** (FixedIndex N-squared edges) and **#26**
(A2A partial equation wrong on mixed same-element / cross-element refs).
Related issues: GH **#448**, and indirectly **#25**, **#35**.

## Summary (TL;DR)

The element-level causal graph in `db_analysis.rs` collapses every variable
reference into a single `ElementDependencyKind` per `(from, to)` edge. This
priority-based collapsing fails on two structurally similar patterns:

1. **Fixed-index references** (`x[NYC]`) get classified as `CrossElement`
   and expanded to NxN edges, when truth is N broadcast edges from one
   source element. (#20)
2. **Mixed references** (`a / a[NYC]` or `pop / SUM(pop[*])`) lose information
   — one classification per `(from, to)` pair cannot represent that the same
   source variable appears with multiple distinct access shapes in the same
   equation. The link-score partial equation then wraps every reference to
   the source uniformly in `PREVIOUS()`, which is wrong for at least one of
   the access shapes regardless of which choice we make. (#26)

Both bugs share a root cause: the engine compresses per-AST-occurrence
semantics into a single per-edge classification. The principled fix is to
walk the AST once per target variable and emit element edges and partial
equations *per reference site*. This eliminates the conflation at the source
and removes downstream patches built around its mitigations.

## Background

### Today's pipeline

1. `model_causal_edges` (salsa, `db_analysis.rs`) builds variable-level
   edges from each variable's flat dependency set.
2. `model_element_causal_edges` (salsa, `db_analysis.rs`) expands those
   variable-level edges into element-level edges. For each `(from, to)`
   edge, it:
   a. Calls `classify_element_dependency` once, which walks the target's
      AST and returns a single `ElementDependencyKind` (`Scalar`,
      `SameElement`, or `CrossElement`) using the priority
      `CrossElement > SameElement > Scalar`.
   b. Calls `expand_edge_to_elements` with that classification to write
      the element edges.
3. `model_ltm_variables` (`db_ltm.rs`) generates link-score equations.
   For each `(from, to)` edge it parses the target equation text and
   wraps every reference to the source in `PREVIOUS()`, except for the
   target source itself. The wrapping is uniform across all references.

### The conflation

`classify_in_expr` already sees that `Subscript(source, [literal_element])`
and bare `Var(source, ArrayBounds)` are different access shapes — but it
folds the result of the walk into a single highest-priority kind for the
edge. Because `CrossElement > SameElement`, the bare-Var pattern's
information is silently discarded whenever a fixed-index reference also
appears.

### Concrete examples in our test corpus

`test/cross_element_ltm/cross_element.stmx` contains:

```
migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01
migration_pressure[Boston] = (population[Boston] - population[NYC]) * 0.01
migration_in[NYC] = MAX(migration_pressure[Boston] * -1, 0)
migration_in[Boston] = MAX(migration_pressure[NYC] * -1, 0)
total_population = SUM(population[*])
```

`migration_pressure -> ?` and `migration_in -> ?` are subscript-defined
arrayed equations. `population -> migration_pressure` and
`migration_pressure -> migration_in` are exactly the fixed-index broadcast
pattern that gets spuriously expanded to all-pairs today.

### Why the current mitigation isn't a fix

The runtime mitigation is "the spurious link scores are 0 anyway." That
is true for the *score values* but not for:

- **Cycle partitioning.** Spurious edges merge stocks that are
  structurally independent into the same SCC, biasing partition sums in
  `compute_rel_loop_scores`.
- **Loop enumeration scale.** NxN density on dimensioned subgraphs
  inflates SCC sizes, triggering the `MAX_LTM_SCC_NODES = 50` auto-flip
  gate (#25) on models that exhaustive enumeration could otherwise
  handle.
- **Loop count.** Johnson's algorithm enumerates spurious circuits for
  every false edge.
- **Partial-equation correctness (#26).** The wrap-everything strategy
  is provably wrong on `share[R] = population / SUM(population[*])`:
  evaluating the partial against `from = population` should leave the
  bare-Var `population` live and wrap only the `population[*]` refs
  inside the SUM in `PREVIOUS`. Today both refs get wrapped together,
  so the partial equals the full expression and link-score magnitude
  is pinned at 1.

## Approach

**Single AST walk per target variable** that emits *both* element edges
and the per-reference metadata needed to build correct partial equations.

### Key data structure

For each target variable, classify each *reference site* in its AST. A
reference is one occurrence of an `Expr2::Var` or `Expr2::Subscript` node
whose identifier matches a source variable. Each reference carries an
**access shape**:

```
enum RefShape {
    Bare,                      // Expr2::Var (used in scalar or A2A bare context)
    Wildcard,                  // Subscript with at least one IndexExpr2::Wildcard
    FixedIndex(Vec<ElemRef>),  // Subscript with all literal element refs
    DynamicIndex,              // Subscript with @N / Range / arbitrary Expr index
                               // (conservative: treat like Wildcard)
}
```

`ElemRef` resolves a literal index to its dimension and element name.
Multidimensional fixed indices are vectors of `ElemRef`s, one per
dimension axis.

Each reference site is keyed by `(source_ident, RefShape)`. A target's AST
analysis produces a map `Map<source_ident, Vec<RefShape>>` (or, for
efficiency, `Map<(source, RefShape), Vec<AstPath>>`). Reference sites with
identical `(source, RefShape)` are pooled because the link-score and
edge-emission rules are determined by the shape alone.

### Edge emission per reference site

| Source dims | Target dims | RefShape | Edges emitted |
|------------|-------------|----------|---------------|
| scalar | scalar | Bare | `from -> to` |
| scalar | arrayed | Bare | `from -> to[d]` for each target element d |
| arrayed | scalar | Bare | `from[d] -> to` for each source element d |
| arrayed | arrayed (same dims) | Bare | `from[d] -> to[d]` for each shared element d |
| arrayed | * | Wildcard / DynamicIndex | full cross-product (today's CrossElement) |
| arrayed | scalar | FixedIndex(elem) | `from[elem] -> to` (one edge) |
| arrayed | arrayed | FixedIndex(elem) | `from[elem] -> to[d]` for each target element d |
| (multi-dim mixed) | * | partial fixed | element-by-element resolution |

Edge sets from multiple reference sites in the same target are merged
(unioned). This gives the truthful element graph.

### Partial-equation building per link score

For a link score `(source, RefShape) -> target`, build the partial
equation by walking the target's AST and for each reference to source:

- If the reference's shape **matches** `RefShape`, leave it live.
- If the reference's shape **differs**, wrap it in `PREVIOUS()`.
- Every other source variable's references are wrapped in `PREVIOUS()`
  as today.

Examples:

| Target equation | RefShape under analysis | Partial |
|-----------------|------------------------|---------|
| `pop / SUM(pop[*])` | source=pop, shape=Bare | `pop / PREVIOUS(SUM(pop[*]))` |
| `pop / SUM(pop[*])` | source=pop, shape=Wildcard | `PREVIOUS(pop) / SUM(pop[*])` |
| `(pop[NYC] - pop[Boston]) * 0.01` | source=pop, shape=FixedIndex(NYC) | `(pop[NYC] - PREVIOUS(pop[Boston])) * 0.01` |
| `(pop[NYC] - pop[Boston]) * 0.01` | source=pop, shape=FixedIndex(Boston) | `(PREVIOUS(pop[NYC]) - pop[Boston]) * 0.01` |

The link-score numerator-and-denominator formula is unchanged; only the
partial equation that fills the numerator changes.

### Naming the per-shape link scores

Today: `$⁚ltm⁚link_score⁚{from}→{to}` — one variable per `(from, to)`
edge, where arrayed edges occupy N slots via dimensions.

Proposed: keep the existing scheme for Bare, Wildcard, and DynamicIndex
shapes (single variable per shape, A2A expansion handles per-element).
For FixedIndex(elem) introduce one scalar variable per literal
source-element subscript:

`$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` (same on-the-wire format
already used by `try_cross_dimensional_link_scores` for arrayed-to-scalar
edges, so the discovery parser doesn't need format changes).

For arrayed-target FixedIndex links, the link score is itself
A2A-dimensioned over the target's dimensions and emits a single multi-slot
variable, just like Bare A2A scores today. Discovery's `parse_link_offsets`
already understands per-element subscript notation.

A target variable that uses a source under multiple shapes (e.g.,
`share = pop / SUM(pop[*])`) gets two link-score variables for the
`(pop, share)` direction: one Bare, one Wildcard. The loop-score
multiplier picks the right one based on which AST occurrence the loop
edge corresponds to.

### Loop classification consequences

`build_element_level_loops` groups element circuits into A2A loops vs.
mixed/cross-element loops by inspecting subscripts. The new edge set is
sparser, so circuit shapes change:

- A2A loops continue to look identical (diagonal edges only).
- Today's cross-element loops induced by spurious all-pairs edges
  disappear. Tech-debt #35's "A2A loop_partitions = None" bug is
  unaffected (its trigger condition is independent), but the regression
  fixture documenting it may need adjustment.
- New cross-element loops induced by **legitimate** fixed-index
  feedback (the cross_element_ltm fixture has these) get correctly
  enumerated as scalar mixed loops with edge labels keyed on the
  specific element subscripts.

### What does NOT change

- `model_causal_edges` (variable-level edges) is unchanged. Other code
  (variable-level loop circuits, layout metadata, JSON SDAI relationships)
  continues to consume it as-is.
- Discovery mode's `parse_link_offsets`, `SearchGraph`,
  `find_strongest_loops`, and `rank_and_filter` all operate on element
  edges as already named; only the *set of element edges produced* changes.
- The synthetic-variable naming convention and the post-simulation
  relative loop-score computation in `ltm_post.rs` are unchanged.
- VM, interpreter, simulation engine: all unchanged. The new link-score
  variables are regular auxiliaries.

## Phases

### Phase 1 — Tests pin the desired contract (TDD red phase)

Write tests that fail under current behavior and will pass after the fix.

- **Unit tests in `db_analysis.rs`:** for representative AST patterns,
  assert exact element-edge sets:
  - `relative_pop[R] = population / population[NYC]` →
    `{pop[d] -> rel_pop[d] for each d, pop[nyc] -> rel_pop[d] for each d}`
    (2N − 1 unique edges, NOT NxN)
  - `share[R] = population / SUM(population[*])` →
    `{pop[d] -> share[d] for each d (Bare), pop[d] -> share[e] for all (d,e) (Wildcard)}`
    (still N² because of the wildcard reducer; this case is correctly
    captured today and must remain correct)
  - `migration_pressure[NYC] = (pop[NYC] - pop[Boston]) * 0.01` →
    fixed indices broadcast to all `migration_pressure` elements
  - Partial-collapse: `out[D1] = SUM(in[D1, *])` (preserve)
- **Property test:** for each generated arrayed model, the variable-level
  projection of element edges (strip subscripts, dedup) equals the edges
  in `model_causal_edges`. This catches accidental edge omissions in the
  refactor.
- **Partial-equation tests:** unit-test the new per-shape partial equation
  builder against the table in this design.
- **Integration regression:** run the existing `simulate_ltm` suite,
  particularly `test_cross_element_ltm_exhaustive`, and pin current
  golden values; if values change after the fix, document why per-test.

### Phase 2 — Implement the AST-walking element graph

Replace `classify_element_dependency` + `expand_edge_to_elements` with a
single AST walker. Concrete steps:

1. Add an internal `ReferenceSite` data type capturing source ident,
   `RefShape`, and (for `FixedIndex`) the resolved element subscripts.
2. Replace the inner loop of `model_element_causal_edges` with a per-target
   AST walk that collects `ReferenceSite`s and emits edges according to
   the table above.
3. Keep `model_causal_edges` and the variable-level pipeline untouched.
4. Delete `ElementDependencyKind` and the now-unused expansion helpers
   (or downgrade them to internal helpers if still useful for stocks).
5. Update `db_element_graph_tests.rs` to cover the new contract.

Acceptance: Phase 1 tests pass; existing tests pass without golden-data
changes (or with documented changes per test); the test_cross_element_ltm
suite passes with edges that match the truth table.

### Phase 3 — Per-reference partial equations and per-shape link scores

Refactor `link_score_equation_text` and `build_partial_equation` so that:

1. The link score for `(from, to, RefShape)` builds its partial equation
   by walking the target AST, leaving `from` references that match
   `RefShape` live, and wrapping all other references (of `from` and
   any other dependency) in `PREVIOUS()`.
2. `model_ltm_variables` enumerates `(from, to, RefShape)` tuples — one
   per shape distinct in the target's references to `from`.
3. The naming scheme for FixedIndex uses
   `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` with multidim subscripts as
   already established.
4. The loop-score equation references the correct per-shape link score
   for each loop edge. `build_element_level_loops` emits edge metadata
   identifying the shape so loop-score generation can resolve it.
5. The discovery parser (`parse_link_offsets`) handles the new
   subscripted-from names. (Likely already does, since arrayed-to-scalar
   uses the same naming.)

Acceptance: link scores for the cross_element_ltm fixture match
hand-calculated expected values for at least one timestep; the
`pop / SUM(pop[*])` case from #26 produces non-trivial partition-aware
scores instead of magnitude-1 placeholders.

### Phase 4 — Loop classification and enrichment

Update `build_element_level_loops` to consume the new sparser edge set:

1. Verify A2A loop detection still works (diagonal-only edges in
   pure-A2A subgraphs).
2. Verify legitimate cross-element loops are detected and classified.
3. Remove dead code paths created by the disappearance of spurious
   cross-element circuits.
4. Surface RefShape on `Loop::links` for downstream consumers if needed.

Acceptance: tests in `tests/simulate_ltm.rs` pass; the "cross_element"
fixture's loop list matches manually-validated expected loops; SCC
sizes shrink on at least one fixture (measure with a small benchmark).

### Phase 5 — Auto-flip threshold sanity check

The `MAX_LTM_SCC_NODES = 50` gate (#25) was tuned against the spurious
edge set. Re-measure SCC sizes on representative arrayed fixtures
(WRLD3, cross_element_ltm, hero_culture_ltm, arrayed_population_ltm)
and decide whether the gate should be retuned. Likely no threshold
change is needed — this phase is just a measurement and a comment
update if appropriate.

Acceptance: a short measurement note added to this design plan; the
auto-flip gate remains correct, possibly with a comment explaining why
the threshold is conservative w.r.t. the new edge counts.

### Phase 6 — Documentation and tech-debt closure

1. Update `docs/design/ltm--loops-that-matter.md` "Element-Level Causal
   Graph" section: the per-edge classification table is replaced with a
   per-reference table. The narrative is updated to reflect AST-walking.
2. Update `src/simlin-engine/CLAUDE.md` references to
   `ElementDependencyKind` (it no longer exists or is internal).
3. Mark items #20 and #26 in `docs/tech-debt.md` as RESOLVED with commit
   pointers.
4. Note in `docs/tech-debt.md` #25 that the auto-flip gate's empirical
   pressure has eased (with measurement reference).

Acceptance: docs reflect the new design; tech-debt index is current.

### Phase 7 — Final cleanup

1. Remove any temporary scaffolding or feature flags introduced during
   Phase 2.
2. Delete now-dead code paths.
3. Final pre-commit-hook clean run.

Acceptance: clean commit; pre-commit passes; `cargo test --workspace`
passes within the 3-minute cap.

## Acceptance Criteria

The slug for this work is `ltm-per-ref-elem-graph`. All AC identifiers
below use that scope.

### ltm-per-ref-elem-graph.AC1: Element-edge structure is per-AST-reference

- **ltm-per-ref-elem-graph.AC1.1 Fixed-index broadcast not over-expanded:**
  For `relative_pop[R] = population / population[NYC]` over a dimension
  R of size N, the element-graph must contain exactly the diagonal
  same-element edges (`population[d] -> relative_pop[d]` for each d in
  R) plus the broadcast-from-NYC edges (`population[NYC] -> relative_pop[d]`
  for each d in R, deduplicated). Total unique edges: 2N − 1, not N².
- **ltm-per-ref-elem-graph.AC1.2 Wildcard reducer remains all-pairs:**
  For `share[R] = population / SUM(population[*])`, the wildcard
  reducer continues to emit all-pairs edges from every source element
  to every target element (`population[d] -> share[e]` for all d, e),
  *in addition to* the diagonal SameElement edges. The bare-Var
  numerator's diagonal edges must not be lost.
- **ltm-per-ref-elem-graph.AC1.3 Cross-element fixture edge set:**
  For `test/cross_element_ltm/cross_element.stmx`, the element graph
  must contain the truthful broadcast edges for every fixed-index
  reference (`population[NYC] -> migration_pressure[d]`,
  `migration_pressure[Boston] -> migration_in[NYC]`, etc.) and must NOT
  contain the spurious all-pairs edges that today's code produces.
- **ltm-per-ref-elem-graph.AC1.4 Variable-level projection invariant:**
  For every project, the set of variable-level edges produced by
  stripping subscripts and deduplicating from
  `model_element_causal_edges` equals the set produced by
  `model_causal_edges`. Verified with property tests over randomly
  generated arrayed models.
- **ltm-per-ref-elem-graph.AC1.5 Multidim partial-fixed conservative:**
  For multidimensional sources where some indices are literal and
  others are wildcards (e.g., `source[NYC, *]`), the conservative
  initial behavior is to treat as Wildcard shape. Documented with a
  TODO; not a regression vs today.

### ltm-per-ref-elem-graph.AC2: Per-shape partial equations are correct

- **ltm-per-ref-elem-graph.AC2.1 Bare-shape partial holds wildcard at PREVIOUS:**
  For `share[R] = population / SUM(population[*])` and the link score
  keyed by `(population, share, Bare)`, the partial equation must
  leave the bare `population` reference live and wrap the
  `population[*]` inside the SUM in `PREVIOUS()`. The resulting
  link-score magnitude is partition-aware and not pinned at 1.
- **ltm-per-ref-elem-graph.AC2.2 Wildcard-shape partial holds bare at PREVIOUS:**
  For the same equation, the link score keyed by
  `(population, share, Wildcard)` must wrap the bare `population` in
  `PREVIOUS()` and leave the wildcard reducer live.
- **ltm-per-ref-elem-graph.AC2.3 FixedIndex per-element partials:**
  For `migration_pressure[NYC] = (pop[NYC] - pop[Boston]) * 0.01`, the
  link score keyed by `(pop, migration_pressure, FixedIndex(NYC))`
  must yield partial `(pop[NYC] - PREVIOUS(pop[Boston])) * 0.01`. The
  link score keyed by `(pop, migration_pressure, FixedIndex(Boston))`
  must yield partial `(PREVIOUS(pop[NYC]) - pop[Boston]) * 0.01`.
- **ltm-per-ref-elem-graph.AC2.4 Other-source refs still wrapped:**
  Every reference to a variable other than the link's `from` must
  continue to be wrapped in `PREVIOUS()` regardless of `RefShape`,
  preserving the ceteris-paribus invariant.

### ltm-per-ref-elem-graph.AC3: Link score variables track shapes

- **ltm-per-ref-elem-graph.AC3.1 Per-shape link score emission:**
  When a target equation references a source under multiple distinct
  `RefShape`s, `model_ltm_variables` must emit one `LtmSyntheticVar`
  per `(from, to, RefShape)` tuple, with the appropriate naming
  convention.
- **ltm-per-ref-elem-graph.AC3.2 FixedIndex naming convention:**
  FixedIndex link scores use the existing per-element naming
  `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` (already used by
  `try_cross_dimensional_link_scores`). The discovery parser handles
  these names through its existing `from_str.contains('[')` branch.
- **ltm-per-ref-elem-graph.AC3.3 Wildcard and DynamicIndex carry stable shape suffixes:**
  Wildcard-shape link scores use a `⁚wildcard` suffix on the `to` side:
  `$⁚ltm⁚link_score⁚{from}→{to}⁚wildcard`. DynamicIndex uses
  `⁚dynamic`. The suffix is **unconditional** — it appears regardless
  of whether Bare also exists for the same `(from, to)` pair — so each
  shape's name is a stable function of `(from, to, shape)`. The discovery
  parser strips the suffix during `parse_link_offsets`. This is a
  deliberate name-format change versus the pre-refactor convention
  (where Wildcard-only models named the link score `{from}→{to}` with
  no suffix); the discovery parser is updated correspondingly. No
  on-disk artifact carries these names persistently — they exist only
  within a single simulation run's results.

### ltm-per-ref-elem-graph.AC4: Loop detection consumes the new edge set

- **ltm-per-ref-elem-graph.AC4.1 A2A loops still detected:**
  Pure-A2A loops (e.g., `population[d] -> births[d] -> population[d]`)
  continue to be detected and classified as A2A loops with shared IDs
  in `build_element_level_loops`.
- **ltm-per-ref-elem-graph.AC4.2 Legitimate cross-element loops detected:**
  The cross-element fixture's intended cross-element loops (going
  through `migration_pressure[*]` and `migration_in[*]` with literal
  index references) are detected and classified as scalar mixed loops.
  The spurious cross-element loops induced by today's all-pairs
  expansion disappear from the output.
- **ltm-per-ref-elem-graph.AC4.3 Existing simulate_ltm tests pass:**
  All tests in `src/simlin-engine/tests/simulate_ltm.rs` pass without
  modification. Any deliberate loop-count or score-magnitude changes
  in fixtures are explicitly documented per-test with reasoning.

### ltm-per-ref-elem-graph.AC5: Performance does not regress

- **ltm-per-ref-elem-graph.AC5.1 SCC sizes shrink or stay equal:**
  On the fixtures `test/cross_element_ltm`, `test/arrayed_population_ltm`,
  `test/hero_culture_ltm`, and `test/metasd/WRLD3-03/wrld3-03.mdl`, the
  largest element-graph SCC after the fix is less-than-or-equal-to the
  size before, measured by a small benchmark. (This phase is
  measurement-only; no threshold change is required as a gate.)
- **ltm-per-ref-elem-graph.AC5.2 Pre-commit budget honored:**
  The pre-commit hook completes successfully and `cargo test --workspace`
  remains under the 3-minute wall-clock cap.

### ltm-per-ref-elem-graph.AC6: Documentation reflects the new design

- **ltm-per-ref-elem-graph.AC6.1 Design doc updated:**
  `docs/design/ltm--loops-that-matter.md` "Element-Level Causal Graph"
  section is rewritten to describe the per-reference walker. The
  classification table is replaced with the per-reference table from
  this design plan.
- **ltm-per-ref-elem-graph.AC6.2 Engine CLAUDE.md updated:**
  References to `ElementDependencyKind` in `src/simlin-engine/CLAUDE.md`
  are removed or updated to reflect the new internal naming.
- **ltm-per-ref-elem-graph.AC6.3 Tech-debt items closed:**
  `docs/tech-debt.md` items **#20** and **#26** are marked RESOLVED
  with the resolving commit hash. Item **#25** is updated to reflect
  the SCC-pressure measurement.

## Success Criteria

- `test_cross_element_ltm_exhaustive` and `test_cross_element_ltm_discovery`
  pass with edge-set assertions added in Phase 1.
- Property test (variable-level projection equals `model_causal_edges`)
  passes on at least 100 random arrayed models.
- Partial-equation correctness on `share = pop / SUM(pop[*])` is
  manually verified: the Bare-shape link score is non-trivial and
  partition-aware.
- Existing `simulate_ltm` integration tests pass. Golden-data changes,
  if any, are documented per-test with reasoning.
- Pre-commit hook passes (Rust fmt/clippy/test, TS lint/typecheck/test,
  WASM build, Python).
- `cargo test --workspace` completes within the 3-minute cap.
- Tech-debt items #20 and #26 marked RESOLVED in `docs/tech-debt.md`.
- Design doc `docs/design/ltm--loops-that-matter.md` updated.

## Risks and Open Questions

### R1: Existing golden data may shift

Several integration tests have hand-computed or reference-software-derived
expected values (e.g. `simulates_population_ltm` against
`test/logistic_growth_ltm/ltm_results.tsv`). These are scalar/A2A models
without fixed-index references, so values *should* be unchanged — but
property of the refactor must verify this. Plan: run the full
`simulate_ltm` suite with no golden-data updates first; investigate any
diffs before updating goldens.

### R2: The `cross_element_ltm` fixture has loop-count assertions

Tech-debt #34's resolution note in `tech-debt.md` mentions
`test_cross_element_ltm_exhaustive`'s assertions were relaxed to "at
least one slot non-zero" because the broadcast bug hid equilibrium
elements. After the fix, those slots may legitimately be non-zero again,
allowing tighter assertions. Plan: tighten where reasonable, document
where not.

### R3: Discovery mode link-offset parsing

`parse_link_offsets` parses link-score variable names from result
offsets. It already handles subscripted-from names for arrayed-to-scalar
edges (`{from}[{elem}]→{to}`). Need to verify it also handles
arrayed-to-arrayed FixedIndex names (`{from}[{elem}]→{to}` where target
is itself dimensioned and the link score is A2A). Plan: trace through
the discovery parser early in Phase 3.

### R4: Multi-dimensional fixed indices

`source[NYC, *]` is partial — the conservative initial implementation
treats anything with a remaining wildcard as Wildcard shape. This is
strictly an over-approximation (so safe), but loses the broadcast-from-NYC
information. Plan: leave as conservative initial behavior; file a
follow-up tech-debt item for the multi-dim partial-fixed pattern if
real models exercise it.

### R5: Index expressions that look fixed but aren't

`source[i+1]` where `i` is a position iterator is element-relative, not
fixed. Initial implementation: only treat literal element identifiers
and integer literals as `FixedIndex`. Anything more complex (including
arithmetic on positions) falls back to `DynamicIndex` shape and uses
the conservative cross-product expansion. Document the conservative
choice with a TODO; refining is a separate effort if real models
exercise the pattern.

## Measurement Plan

Before Phase 2, capture SCC sizes and edge counts on:

- `test/cross_element_ltm/cross_element.stmx`
- `test/arrayed_population_ltm/*` (whatever fixtures exist)
- `test/hero_culture_ltm/*`
- `test/metasd/WRLD3-03/wrld3-03.mdl` (for the ceiling)

For each: number of element edges, largest SCC size, circuit count
(if it completes within auto-flip), measured wall-clock for
`model_element_causal_edges` and `model_element_loop_circuits`. Capture
a "before" baseline; re-run "after" Phase 2 + Phase 3 land. The numbers
go into a brief postscript section of this document.

This is informational, not gating — the success criteria above are the
gates.

## Out of Scope

- Tech-debt **#21** (polarity blind to array reducers): related but
  independent. Static polarity analysis lives in `analyze_link_polarity`
  in `ltm.rs`, not in the element-graph builder.
- Tech-debt **#27** (STDDEV/RANK fallback scores): the per-element
  link-score for STDDEV/RANK is generated by
  `generate_element_to_scalar_equation`, which lives in
  `ltm_augment.rs`. Distinct change surface from this work.
- Tech-debt **#35** (A2A `partition = None`): independent bug in
  `partition_for_loop` keying. Should be fixed separately; the test
  fixture from this work may exercise it but the fix is local to
  partition resolution.

## Measurement Postscript

Captured 2026-04-25 on commit `d0ad3924` (pre-Phase-2 baseline) and
commit `eba8fd5f` (post-Phase-4 branch tip). Numbers are produced by
`cargo run --release --example ltm_full_bench -- <fixture>`; the bench
was extended in Phase 5 to report element-level largest-SCC size and
to load `.sd.json` fixtures.

For the "before" measurements the bench changes (commits `54b0cb9d`
and `eba8fd5f`) were cherry-picked onto a detached `d0ad3924` so that
both runs report the same set of stage notes. The cherry-picks touch
only `examples/ltm_full_bench.rs`, leaving the production LTM
pipeline at the pre-Phase-2 baseline.

| Fixture | Pre var-edges | Pre elem-edges | Pre elem-SCC | Post elem-edges | Post elem-SCC | Auto-flip pre / post |
|---------|--------------:|---------------:|-------------:|----------------:|--------------:|---------------------|
| cross_element_ltm | 8 | 20 | 10 | 18 | 10 | no / no |
| arrayed_population_ltm | 6 | 18 | 3 | 18 | 3 | no / no |
| hero_culture_ltm | 41 | 41 | 15 | 41 | 15 | no / no |
| WRLD3-03 | 483 | 483 | 166 | 483 | 166 | yes / yes |

Notes per fixture:

- **cross_element_ltm**: element-edge count drops from 20 to 18 (-2)
  and the element-level circuit count drops from 12 to 8 because the
  per-reference walker no longer emits the spurious cross-element
  edges that FixedIndex broadcasts produced under the old
  `ElementDependencyKind::CrossElement` classification. The largest
  SCC is unchanged at 10 -- the removed edges sat inside an
  already-strongly-connected stock cluster, so they trimmed cycles
  without splitting the SCC. Auto-flip is unaffected (10 << 50 in
  both runs).
- **arrayed_population_ltm**: identical structural numbers in both
  runs. This fixture's loops are purely intra-element population
  flows (Bare references between same-shape arrayed variables); the
  per-reference walker emits the same edge set as the pre-refactor
  classification did. Auto-flip is unaffected (3 << 50).
- **hero_culture_ltm**: identical numbers in both runs. The model is
  fully scalar (27 vars, no arrayed dimensions), so the
  variable-level and element-level graphs collapse to the same shape
  and the FixedIndex path is never exercised. Auto-flip is unaffected
  (15 << 50). Included in the table now that the Phase-5 bench can
  load `.sd.json` projects.
- **WRLD3-03**: identical numbers in both runs. WRLD3 is scalar, so
  per-element expansion is again a no-op and the 166-node SCC comes
  entirely from variable-level feedback structure (population,
  capital, agriculture, persistent-pollution, non-renewable
  resources). Auto-flip fires in both runs (166 > 50), and stage 7
  reports `loop=0` for the synthetic-variable counts -- discovery
  mode bypasses loop-score generation as designed.

Threshold decision: `MAX_LTM_SCC_NODES = 50` is retained. The
post-refactor data shows that the per-reference rewrite shrinks edge
counts and circuit counts on FixedIndex models without shrinking the
underlying SCC sizes, and it leaves scalar/A2A models entirely
unchanged. WRLD3 still trips the gate for the same structural reason
it did before, and the smaller fixtures continue to fit comfortably
under the threshold with element-SCCs of 3, 10, and 15. Because no
fixture moved closer to or across the threshold, there is no data
justifying a change.

Because element-graph SCC sizes did not shrink materially on the
FixedIndex fixture (cross_element's elem-SCC stayed at 10), Phase 5's
optional `MAX_LTM_SCC_NODES` doc-comment update was skipped -- the
edge-count and circuit-count savings are real but they don't change
the auto-flip story the existing comment tells.
