# Variable-Level LTM Loop Enumeration with Cross-Element Subgraph Expansion

Date: 2026-05-06
Tracks: GitHub issue **#482** ("ltm: enumerate variable-level loops first,
expand only cross-element subgraph"); related tech-debt entry **#25**.
Builds on: `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` (per-reference
element graph; closed as #448).

## Summary (TL;DR)

`model_element_loop_circuits` runs Johnson's circuit enumeration directly on
the element graph. For a model with V variables over an N-element dimension,
every pure same-element (A2A) variable-level cycle inflates to N element-level
cycles which `build_element_level_loops` then collapses back into one A2A
loop. Cost is multiplicative in N when it should be additive.

The 2026-04-25 per-reference refactor (closed #448) eliminated spurious NxN
edges on FixedIndex models, materially reducing pressure on the auto-flip
gate. The remaining N-fold inflation comes from legitimate same-element edges
in dense pure-A2A subgraphs and still trips `MAX_LTM_SCC_NODES = 50` sooner
than variable-level enumeration would.

The fix is a two-tier enumeration: enumerate variable-level circuits with
Johnson, classify each one's edges by `RefShape`, and short-circuit
pure-A2A and pure-scalar circuits directly into Loops. Only mixed and
pure-cross-element circuits enter the element-level enumerator -- and even
then, only on the subgraph induced by their nodes, not the full element
graph.

After this change, the auto-flip gate's domain narrows: it now applies only
to the cross-element / mixed slice of a model's structure. Pure-A2A models
of arbitrary N never hit the gate.

## Background

### Today's pipeline

1. `model_causal_edges` (variable-level, salsa-tracked) produces V variable
   nodes and the variable-level adjacency.
2. `model_element_causal_edges` (element-level, salsa-tracked) walks each
   target's Expr2 AST, classifies every reference site by `RefShape`, and
   emits per-shape element edges. After the per-reference refactor, this
   is precise: FixedIndex broadcasts produce only the truthful broadcast
   edges, not NxN.
3. `model_element_loop_circuits` runs Johnson on the element graph and
   returns indexed circuits.
4. `build_element_level_loops` (in `db_ltm.rs`) groups element circuits by
   their stripped variable-level node sequence:
   - Pure-A2A groups (every node subscripted, no repeated variables, all
     leading subscripts agree) -> one A2A `Loop` with `dimensions` set.
   - Cross-element groups -> one scalar `Loop` extracted from the unique
     stripped cycle.
   - Mixed/scalar groups -> one `Loop` per element circuit.

### The cost asymmetry

Pure-A2A cycles in the variable graph re-emerge as N redundant cycles in
the element graph -- one per element. Johnson's enumeration is sensitive
to circuit count: a pure-A2A SCC of size K (variable-level) over N
elements has the same K cycles times N copies in the element graph. Even
better, the element graph's largest SCC is N * |variable-level SCC|.

Concrete examples:

- `arrayed_population_ltm` (small): 6 variable-level edges, but 18
  element-level edges; 3 elements, A2A only. Element-SCC = 3 (bounded by
  N), so still well under MAX_LTM_SCC_NODES.
- A hypothetical pure-A2A model with 10 variables over a 100-element
  dimension and dense feedback would produce: variable-level SCC ~ 10,
  element-level SCC ~ 1000. The auto-flip gate fires at 50, so this
  model loses exhaustive analysis even though the variable-level
  structure is trivial.

WRLD3, in contrast, is fully scalar: 483 variable-level edges, 483
element edges, 166-node SCC. The size comes from cross-variable
feedback, not array structure. WRLD3 trips the gate either way.

### Edge tagging is already done -- per AST reference site

The per-reference walker in `db_analysis.rs` tags each reference site
with one of:

- `Bare`: `Expr2::Var` reference. Same-element semantics on arrayed
  sources; scalar on scalar sources.
- `FixedIndex(Vec<String>)`: every subscript is a literal element.
- `Wildcard`: at least one subscript is `*`.
- `DynamicIndex`: at least one subscript is a non-literal expression.

The variable-level cycle tagging we need is a *cycle-shape* aggregation
over the per-edge `RefShape`s: classify each variable-level edge in a
cycle by the union of shapes its references take on, then label the
cycle as one of:

- **PureScalar**: every edge has at least one Bare reference, and every
  participating variable is scalar. The cycle has no array structure;
  emit one scalar Loop directly.
- **PureSameElementA2A**: every edge has at least one Bare reference,
  every participating variable is arrayed over the *same* dimension(s),
  and no edge has a Wildcard or DynamicIndex shape. The cycle exists at
  every element; emit one A2A Loop with `dimensions` set, equivalent to
  one Bare same-element traversal.
- **CrossElementOrMixed**: at least one edge has a Wildcard, DynamicIndex,
  or FixedIndex reference, OR participating variables span different
  dimension groups, OR the cycle mixes scalar and arrayed nodes. These
  cycles need element-level enumeration to enumerate the truthful
  per-element structure.

The first two classes can short-circuit the element-level enumerator.
The third still needs element-level Johnson, but only on the subgraph
induced by the variables in that variable-level cycle (a strict subset
of the full element graph).

## Proposed architecture

### Two-tier enumeration

```
        +----------------------+
        | variable-level graph |
        +----------+-----------+
                   | Johnson
                   v
        +----------------------+
        | variable circuits    |    one per variable-level cycle
        +----------+-----------+
                   | tag each cycle by RefShape composition
                   v
        +----------------------+
        | classified circuits  |
        +----------+-----------+
                   |
       +-----------+-----------+--------------------+
       v                       v                    v
   PureScalar           PureSameElementA2A    CrossElementOrMixed
   |  emit one          |  emit one A2A       |  expand to element
   |  scalar Loop       |  Loop with dims     |  subgraph; run Johnson
   v                    v                     v
   build_element_level_loops()                element circuits
                                              are then grouped as today
                                              (per-shape Loop emission)
```

### Per-cycle edge classification

For a variable-level cycle `[v0, v1, ..., v(k-1), v0]`, walk each edge
`v(i) -> v(i+1)`. The edge's reference shapes come from the per-target
walker that already exists (`collect_reference_sites` in
`db_analysis.rs`). For each edge, compute its multiset of `RefShape`s.
Then aggregate over the cycle:

- If any edge has a Wildcard or DynamicIndex shape, classify as
  CrossElementOrMixed.
- If any edge has only FixedIndex shapes (no Bare), the cycle traverses
  a literal element subscript and is CrossElementOrMixed (the cycle
  "goes through" a specific element distinct from its neighbours).
- If any edge has a Bare shape *and* a FixedIndex shape, the edge is a
  mixed-shape (which the per-reference refactor already handles by
  emitting per-shape link scores). The cycle classifies as
  CrossElementOrMixed because its A2A interpretation is ambiguous.
- If every edge has at least one Bare shape with no other shapes, and
  every variable in the cycle is either:
  - all scalar -> PureScalar
  - all arrayed over the same dimension(s) -> PureSameElementA2A
  - mixing scalar and arrayed (e.g., scalar pass-through) -> still
    PureScalar because the bare-shape interpretation collapses the
    arrayed dimensions away when the cycle requires a scalar element to
    re-enter

The third bullet (Bare-and-arrayed mixing) is the trickiest; the
conservative answer is: if the cycle has any node whose dimension set
doesn't match every other arrayed node's dimension set exactly,
classify as CrossElementOrMixed. Pure-A2A only covers cycles where
all arrayed nodes share dims; scalar-only cycles cover the
rest.

### Edge data plumbing

Today `model_causal_edges` returns adjacency only -- no per-edge shape
information. We need a small augmentation: a parallel map keyed by
`(from, to)` carrying the multiset of shapes seen at every reference
site. Storing one `Vec<RefShape>` per edge is cheap (O(unique
references), bounded by AST size).

`model_element_causal_edges` already collects this information per
reference site via `collect_reference_sites`. We can hoist that
collection into a helper that produces both the variable-level
shape map and the element-level edges, or we can introduce a new
salsa-tracked function that returns just the shape map and depends
on the per-target AST walks.

The cleanest place to add this is a new tracked helper:

```rust
#[salsa::tracked(returns(ref))]
pub fn model_edge_shapes(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> EdgeShapesResult { ... }

pub struct EdgeShapesResult {
    /// (from_var, to_var) -> set of distinct RefShape values seen at any
    /// reference site of `from` in `to`'s AST.
    pub edge_shapes: HashMap<(String, String), BTreeSet<RefShape>>,
}
```

The map is keyed at variable level (no element subscripts). It gives
the cycle classifier all it needs to decide each cycle's class.

### Replacing model_element_loop_circuits

Today's `model_element_loop_circuits` is a simple Johnson run on the
element graph. We replace it with a *tiered* implementation that
returns a richer result:

```rust
pub struct TieredCircuitsResult {
    /// Variable-level circuits classified as PureScalar or
    /// PureSameElementA2A (the "fast path"). For each entry, the
    /// canonical variable-level cycle nodes and the inferred dimensions
    /// (empty for PureScalar; populated for PureSameElementA2A).
    pub fast_path: Vec<FastPathCircuit>,
    /// Element-level circuits (in the same shape as today's
    /// LoopCircuitsResult) that came from running element-level Johnson
    /// on the cross-element/mixed subgraph induced by the slow-path
    /// variable cycles' nodes.
    pub slow_path: LoopCircuitsResult,
}

pub struct FastPathCircuit {
    pub variables: Vec<String>,    // canonical variable names, cycle order
    pub dimensions: Vec<String>,   // empty for PureScalar
    pub kind: FastPathKind,        // PureScalar | PureSameElementA2A
}
```

`build_element_level_loops` then has two inputs: the fast-path circuits
get materialized directly into `Loop`s (we already know dimensions and
the variable-level structure); the slow-path circuits flow through the
existing element-grouping logic.

### How the slow-path subgraph is built

For the cross-element/mixed cycle classifier, we know the variable-level
nodes that participate in any cross-element cycle. Their union is the
"mixed/cross" subgraph. We restrict the element graph to those
variables' element nodes (and their incident edges), then run Johnson
on that restricted graph.

Important: the slow-path subgraph is structurally a strict subset of
the full element graph. If the slow-path variable subgraph happens to
be empty, no Johnson run on the element graph is needed at all.

For the auto-flip gate, we now consider the *slow-path* element
subgraph's largest SCC, not the full element graph's. This is the
quantitative win: pure-A2A models drop their slow-path subgraph to
nothing and never trip the gate.

## Cost model

Let:
- V = number of variables in the model
- N = elements per dimension (uniform for simplicity)
- C_var = number of variable-level circuits
- C_elem_old = number of element-level circuits enumerated under
  today's full-element Johnson run

For a pure-A2A model (every cycle is PureSameElementA2A):
- Today: C_elem_old = N * C_var (each variable cycle inflates to N
  element cycles).
- Proposed: 0 element-level circuits enumerated; C_var fast-path
  emissions, each materializing one Loop.

For a model with a mixed slice (M of V variables participate in
cross-element cycles, the rest are pure-A2A):
- Today: full-element Johnson on V * N nodes, dominated by the M*N node
  cross-element subgraph but slowed by the pure-A2A overhead.
- Proposed: pure-A2A circuits handled directly; element Johnson runs
  on the M-variable subgraph induced by the slow-path nodes (M * N
  nodes max, but typically smaller because cross-element subgraphs
  rarely use every dimension element).

Cost is **additive** in N for pure-A2A loops (we never enumerate
per-element variants of them) and **at most equal** for cross-element
slices (the slow-path subgraph is bounded by the full element graph).

## Fixture impact and measurement plan

The Phase 5 measurement postscript in
`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` records the
post-#448 baseline:

| Fixture | Pre elem-edges | Pre elem-SCC | Pre auto-flip |
|---------|---------------:|-------------:|---------------|
| cross_element_ltm | 18 | 10 | no |
| arrayed_population_ltm | 18 | 3 | no |
| hero_culture_ltm | 41 | 15 | no |
| WRLD3-03 | 483 | 166 | yes |

The measurements I want for #482 (run during Phase 2 / 3 of the
implementation):

For each fixture:
- Total variable-level circuits enumerated by Johnson
- Total element-level circuits enumerated (today's `model_element_loop_circuits`)
- Number of fast-path circuits (PureScalar + PureSameElementA2A)
- Number of slow-path subgraph element circuits (the new metric)
- Slow-path element-subgraph largest SCC (vs full element-graph SCC)
- Auto-flip status under the new gate (gate now keys on slow-path SCC)

Predictions:
- `arrayed_population_ltm`: every cycle is PureSameElementA2A. Slow-path
  subgraph empty. Element circuits = 0 (vs 6 today).
- `cross_element_ltm`: A2A reinforcing births loop is PureSameElementA2A
  (fast path); migration cycles are CrossElementOrMixed (slow path). The
  slow-path subgraph is M = ~5 variables, ~10 element nodes. Element
  circuits drop from 8 to whatever cross-element-only number is.
- `hero_culture_ltm`: scalar model. Every cycle is PureScalar.
  Slow-path subgraph empty. Element circuits = 0.
- `WRLD3-03`: scalar model. Every cycle is PureScalar. Slow-path
  subgraph empty. Element circuits = 0. Auto-flip gate no longer fires
  (variable-level analysis runs unimpeded since the gate now keys on
  slow-path SCC, which is 0 here).

The WRLD3 prediction is the headline win: a fully scalar model with a
166-node SCC currently auto-flips to discovery mode because the gate
keys on element-graph SCC. After this change, that gate's domain
narrows to the cross-element subgraph, which on WRLD3 is empty -- so
WRLD3 stays on the exhaustive path.

That said, WRLD3's variable-level SCC is itself 166 nodes, which by
itself enumerates ~1.86M circuits in Johnson and produces gigabytes of
synthetic-variable equation text. We must keep a separate gate for
*total downstream cost*, not just slow-path element-graph SCC. The
right knob is probably "max variable-level SCC size", with 50 still
the threshold but now applied to a graph that's strictly smaller for
non-arrayed cycles.

**Measurement-driven decision**: collect the variable-level SCC sizes
and total fast+slow circuit counts before deciding how to retarget
`MAX_LTM_SCC_NODES`. Likely outcome: keep the constant at 50 but
apply it to `max(slow_path_element_scc, variable_level_scc)`. WRLD3
still trips it from variable-level structure; pure-A2A models with
huge N stop tripping it.

## Auto-flip threshold

Before this change: gate keyed on full element graph's largest SCC.
After: gate keyed on the *upper bound* of variable-level SCC size and
slow-path element-subgraph SCC size.

This preserves correctness:
- Variable-level SCC dominates pure-scalar / pure-A2A models. WRLD3
  trips it for the same reason as today (166 > 50).
- Slow-path element SCC dominates legitimate cross-element pressure.
  Models with huge cross-element cliques trip it for the right reason.

The constant `MAX_LTM_SCC_NODES = 50` is retained. We update its
docstring to clarify it now applies to two SCCs and to point at this
design plan.

For users with a pure-A2A model whose variable-level SCC fits under 50
but whose element-graph SCC blows past it (the original motivation
for #482), the new gate stops misfiring.

## Stages

Each stage compiles cleanly, passes `cargo test -p simlin-engine`, and
passes `cargo clippy --workspace --all-targets -- -D warnings`. The
pre-commit hook enforces this on every commit.

### Stage 1 -- Edge-tagging utility

**Files**: `src/simlin-engine/src/db_analysis.rs`.

Add `model_edge_shapes` salsa tracked function and `EdgeShapesResult`
type. Reuses `collect_reference_sites` (already present) but aggregates
per `(from, to)` rather than per element edge.

Tests:
- Pure-A2A model: every edge maps to `{Bare}`.
- `share[r] = pop[r] / SUM(pop[*])`: edge `pop -> share` maps to
  `{Bare, Wildcard}`.
- `mig[NYC] = pop[NYC]`: edge `pop -> mig` includes `FixedIndex(["nyc"])`.
- Scalar model: every edge maps to `{Bare}` (no shape needed; the
  Bare classification still applies because a scalar variable's
  reference is structurally equivalent to a same-element pointer).

Acceptance: standalone unit tests pass. Behavior on existing fixtures
unchanged because `model_edge_shapes` isn't yet consumed.

### Stage 2 -- Cycle classifier

**Files**: `src/simlin-engine/src/db_analysis.rs`.

Add a pure helper `classify_cycle(cycle, edge_shapes, var_dims) -> CycleClass`
where `CycleClass = PureScalar | PureSameElementA2A | CrossElementOrMixed`.
Pure function -- no DB access; takes pre-computed inputs.

Tests:
- Cycle of two scalars with Bare edges -> PureScalar.
- Cycle of three same-dim arrayed vars with Bare edges -> PureSameElementA2A
  (with dimensions populated correctly).
- Cycle including a Wildcard edge -> CrossElementOrMixed.
- Cycle including a FixedIndex edge between same-shape arrays ->
  CrossElementOrMixed (the literal-index reference doesn't behave like
  Bare).
- Cycle mixing scalar and arrayed nodes -> CrossElementOrMixed unless
  every arrayed node has a Bare reference and the scalar's shape is
  trivially Bare; in that conservative pass, classify as PureScalar
  for the cycle to be safely emitted as one scalar Loop. (See the
  conservative-bullet discussion above; the test will pin the exact
  rule.)

Acceptance: classifier unit tests pass.

### Stage 3 -- Tiered circuit enumerator

**Files**: `src/simlin-engine/src/db_analysis.rs`.

Add `model_loop_circuits_tiered` salsa tracked function:
1. Run Johnson on the variable graph (= existing `model_loop_circuits`).
2. For each variable-level circuit, classify via `classify_cycle`.
3. Partition circuits into fast-path and slow-path lists.
4. For slow-path circuits, build the induced element-graph subgraph and
   run Johnson on it.
5. Return `TieredCircuitsResult { fast_path, slow_path }`.

Tests:
- Pure-A2A model: every variable circuit lands in `fast_path` with
  populated dimensions; `slow_path` is empty.
- Pure-scalar model (no arrays): every circuit lands in `fast_path`
  with empty dimensions; `slow_path` is empty.
- cross_element_ltm fixture: A2A births loop in fast_path; migration
  cycles in slow_path; slow_path element-circuit count matches
  hand-computed expectation.
- Mixed scalar+arrayed (e.g., total_population = SUM(pop[*]) in a
  larger cycle): cycle classifies as CrossElementOrMixed and lands in
  slow_path.

Acceptance: tiered enumerator tests pass.

### Stage 4 -- Wire tiered enumerator into LTM

**Files**: `src/simlin-engine/src/db_ltm.rs` (`build_element_level_loops`),
optionally `src/simlin-engine/src/db_analysis.rs` (replace
`model_element_loop_circuits` with a wrapper around
`model_loop_circuits_tiered`).

Strategy:
1. `model_element_loop_circuits` keeps its current return type for
   backwards compatibility, but its body now runs
   `model_loop_circuits_tiered` and unifies fast-path + slow-path into
   one element-circuit list. (Fast-path circuits are materialized as
   N copies in the element graph form, just as Johnson would have
   produced today.)
2. `build_element_level_loops` is updated to accept the
   `TieredCircuitsResult` directly via a new signature, and does the
   right thing per partition: fast-path -> direct Loop emission;
   slow-path -> existing element-grouping logic.

This preserves loop ID stability: the order in which Loops are emitted
must match today's order so `assign_loop_ids` produces the same
`r1, r2, b1, ...` IDs on existing fixtures.

The cleanest preservation strategy: we sort fast-path and slow-path
groups by the same canonical key the existing grouping uses (joined
stripped variable-level node sequence), which is what
`build_element_level_loops` already does. As long as the merged
group order matches the today-sorted one, IDs are stable.

I think the simplest and safest path is **Strategy A**: keep
`model_element_loop_circuits` as-is and add a parallel
`model_loop_circuits_tiered`. Wire `build_element_level_loops` to
prefer the tiered representation when available, with a feature-flag
fall-back. Once tests are green, retire the full-element Johnson run
in a later cleanup stage.

Tests:
- All existing `simulate_ltm` tests pass without modification.
- Loop IDs unchanged on cross_element_ltm and arrayed_population_ltm
  fixtures.
- A new round-trip test asserts that `build_element_level_loops` from
  the tiered representation produces the same `Vec<Loop>` (modulo
  ordering canonicalization) as the today's representation.

Acceptance: existing fixtures unchanged; `simulate_ltm` test suite
passes.

### Stage 5 -- Auto-flip gate update

**Files**: `src/simlin-engine/src/db_ltm.rs` (`model_ltm_variables`),
`src/simlin-engine/src/ltm.rs` (docstring update on `MAX_LTM_SCC_NODES`).

Update the auto-flip computation to:

```rust
let max_scc_size = if is_discovery_user {
    0
} else {
    let var_edges = model_causal_edges(db, model, project);
    let var_scc = causal_graph_from_edges(var_edges).largest_scc_size();

    let element_edges = model_element_causal_edges(db, model, project);
    let elem_scc = causal_graph_from_element_edges(element_edges)
        .largest_scc_size();

    var_scc.max(elem_scc)
};
```

Wait -- this is *exactly today's behavior* on element graphs that
contain everything. The substantive change is to gate on
`element_subgraph_for_slow_path` rather than the full element graph.
But computing the slow-path subgraph requires running the cycle
classifier first, which depends on Johnson on the variable graph. We
end up doing tier-1 Johnson before the gate fires.

**Refined plan**: tier-1 Johnson on the variable graph is the gate.
The gate fires if:
- The variable-level SCC exceeds 50, OR
- The slow-path *subgraph* SCC exceeds 50.

We need to keep an early gate that doesn't require running Johnson at
all (Johnson can be expensive on dense graphs even on the variable
graph). The variable-level SCC computation is cheap (Tarjan, O(V+E)),
so that's safe to use as an early gate.

For the element-level slow-path subgraph: we don't actually know the
slow path until after we classify. But we can compute an *over-
approximation* cheaply: any variable that has a non-Bare-only reference
shape is potentially in a slow-path cycle. The set of such variables
times their dimension elements gives an upper bound on the slow-path
node count. Tarjan on the element subgraph induced by that
over-approximation gives a usable upper bound for the gate.

Conservative compromise for first cut: keep the gate exactly as today
(full element graph SCC) but document that it's an upper bound and
that the *actual* enumeration cost is now bounded by the slow-path
subgraph. This is correct but loses the headline WRLD3 win for
exhaustive mode. We can revisit the gate after measurement -- if WRLD3
runs cleanly under the new tiered enumerator (because its slow path
is empty even though its full SCC is 166), we lower the WRLD3 gate
to whatever the slow-path SCC is.

For this PR's scope: leave the gate alone, document the change, and
file a follow-up issue to retarget the gate after measurement.

### Stage 6 -- New fixture demonstrating the structural win

**Files**: `src/simlin-engine/tests/simulate_ltm.rs` (new test), or
a new `src/simlin-engine/src/db_tiered_circuits_tests.rs`.

Build a synthetic test model:
- 1 dimension `Region` with ~15 elements (small enough for fast tests)
- ~6 variables, all arrayed over `Region`, with dense feedback (each
  variable references every other) -- pure A2A semantics
- A test-only override (e.g., a `pub const TEST_MAX_LTM_SCC_NODES_OVERRIDE`
  visible to tests) that lowers the gate enough to demonstrate the win

Why test-only override: per `docs/dev/rust.md` test-time-budget
guidelines, we must not build fixtures sized to trip the production
gate. A 6-variable, 15-element, dense pure-A2A model has element-graph
SCC = 6 * 15 = 90, so the gate fires at 50. We override the gate to
something smaller (say 30) so the same 6 * 15 fixture still fires the
gate before the change but doesn't fire after the change.

Test assertion:
- Before: under the override, the model auto-flips to discovery mode
  (verified via the diagnostic accumulator picking up the auto-flip
  warning).
- After: under the same override, the model stays on exhaustive mode
  (no auto-flip warning; loop scores emitted).

This is the PR's primary structural-win demonstration.

### Stage 7 -- Documentation

**Files**: `src/simlin-engine/CLAUDE.md`, `docs/design/ltm--loops-that-matter.md`,
`docs/tech-debt.md`.

- Update the engine CLAUDE.md to mention the tiered enumerator.
- Update the LTM design doc with a new "Tiered Loop Enumeration"
  section.
- Mark tech-debt #25 as RESOLVED (or update the assessment to note the
  structural fix landed but the auto-flip gate retargeting is a
  follow-up).
- Index the new design plan in `docs/README.md` if applicable.

### Stage 8 -- Optional cleanup

**Files**: as needed.

If the tiered path proves stable, retire the parallel
`model_element_loop_circuits` (collapse it into
`model_loop_circuits_tiered`'s element-subgraph stage). Defer to a
follow-up PR.

## Risks

### R1: Loop ID stability

`assign_loop_ids` assigns deterministic IDs (`r1`, `b1`, ...) based on
the ordering of detected loops. If the tiered enumerator changes that
ordering, downstream tests that pin loop IDs (e.g.,
`a2a_reinforcing_loop` in `test_cross_element_ltm_exhaustive` looks
for `\u{205A}r1`) will break.

Mitigation: build the merged Loop list in the same canonical order
`build_element_level_loops` uses today (sorted by joined stripped
variable-level node sequence). Add a test that pins the ID assignments
on cross_element_ltm and arrayed_population_ltm.

### R2: Salsa cache invalidation

The new `model_edge_shapes` and `model_loop_circuits_tiered` are
salsa-tracked. Their dependencies (variable parse results, dim
context) are existing tracked inputs. The cache invalidates only when
the underlying AST or dim shape changes -- exactly what we want.

The risk is in `build_element_level_loops` if its new signature
introduces a non-tracked dependency. Mitigation: keep
`build_element_level_loops` as a non-tracked helper called from
`model_ltm_variables` (which is tracked). The helper's inputs are all
tracked outputs.

### R3: A2A dimension on LtmSyntheticVar

Today the `dimensions` field on `LtmSyntheticVar` (and `Loop`) drives
A2A expansion in the link-score and loop-score generators. After the
tiered enumeration, fast-path A2A loops produce their dimensions from
the variable-level cycle's nodes' dimensions (which match if the
cycle classified as PureSameElementA2A). The dim list is the same as
the dimensions today's element-grouping code derives from
`representative[0]`'s subscript.

Mitigation: pin a unit test that asserts the fast-path A2A `Loop`'s
`dimensions` match exactly what `build_element_level_loops` would
have produced from the element-grouped equivalent.

### R4: Conservative classification breaks things

The cycle classifier is intentionally conservative: when in doubt,
classify as CrossElementOrMixed and let the element-level enumerator
handle it. This gives correctness at the cost of leaving some
performance on the table. The mixed/cross cases are then handled
exactly as today.

Risk: a cycle that *should* classify as PureSameElementA2A but
incorrectly classifies as CrossElementOrMixed produces correct loops
but pays the old cost. This is a soft regression (no correctness
issue), so it's safe to ship and tighten later.

### R5: Edge shape map size

`EdgeShapesResult.edge_shapes` keys on `(from, to)` and values are
small `BTreeSet<RefShape>` (max ~4 entries per edge). Total size is
bounded by `|edges| * 4`. For WRLD3 (483 edges), that's ~2KB. Safe.

## Acceptance Criteria

The slug for this work is `ltm-482-var-loops-first`. All AC identifiers
below use that scope.

### ltm-482-var-loops-first.AC1: Edge-shape extraction

- **AC1.1**: `model_edge_shapes` produces a map keyed by every
  variable-level edge, with the multiset of distinct `RefShape` values
  observed at any reference site.
- **AC1.2**: For a pure-A2A model (`pop[r] -> births[r]`), every edge
  has shape set `{Bare}`.
- **AC1.3**: For `share[r] = pop[r] / SUM(pop[*])`, the `pop -> share`
  edge has shape set `{Bare, Wildcard}`.
- **AC1.4**: For a fixed-index reference `mig[NYC] = pop[NYC]`, the
  `pop -> mig` edge contains a `FixedIndex(["nyc"])` shape.

### ltm-482-var-loops-first.AC2: Cycle classification

- **AC2.1**: Pure-scalar cycles classify as `PureScalar`.
- **AC2.2**: Cycles whose every edge has only `Bare` references and
  whose every node has the same arrayed dimension classify as
  `PureSameElementA2A` with the correct dimensions list.
- **AC2.3**: Cycles with any `Wildcard`, `DynamicIndex`, or
  `FixedIndex` edge classify as `CrossElementOrMixed`.

### ltm-482-var-loops-first.AC3: Tiered enumeration

- **AC3.1**: For pure-A2A fixtures (`arrayed_population_ltm`), the
  tiered enumerator's slow-path element-circuits list is empty.
- **AC3.2**: For scalar-only fixtures (`hero_culture_ltm`), the
  tiered enumerator's slow-path is empty.
- **AC3.3**: For mixed fixtures (`cross_element_ltm`), the tiered
  enumerator's slow-path contains exactly the cross-element cycles
  (not the A2A reinforcing births cycle).

### ltm-482-var-loops-first.AC4: Loop output stability

- **AC4.1**: All existing tests in `tests/simulate_ltm.rs` pass without
  modification.
- **AC4.2**: Loop IDs in `arrayed_population_ltm` and
  `cross_element_ltm` fixtures are unchanged before/after the
  refactor.
- **AC4.3**: The set of `LtmSyntheticVar` names produced by
  `model_ltm_variables` is unchanged before/after on existing
  fixtures.

### ltm-482-var-loops-first.AC5: Structural-win demonstration

- **AC5.1**: A new test fixture (synthetic dense pure-A2A model with
  ~6 variables and ~15 elements) under a test-only lowered gate
  threshold demonstrates: pre-change auto-flip fires; post-change it
  does not.

### ltm-482-var-loops-first.AC6: Documentation updated

- **AC6.1**: `src/simlin-engine/CLAUDE.md` references the tiered
  enumerator.
- **AC6.2**: This design plan is committed to
  `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`.

## Out of Scope

- Tightening the auto-flip gate to key only on slow-path SCC
  (deferred to follow-up; needs measurement to validate).
- Retiring `model_element_loop_circuits` once tiered is stable
  (deferred to follow-up cleanup).
- Polarity-confidence metric changes (#485, separate work).
- LOOPSCORE/PATHSCORE builtins (#484, separate work).

## Success Criteria

- The new `model_loop_circuits_tiered` salsa function ships with unit
  tests covering pure-A2A, pure-scalar, mixed, and cross-element
  cycle classification.
- `build_element_level_loops` consumes the tiered representation and
  produces the same `Vec<Loop>` on existing fixtures.
- A new structural-win test demonstrates that under a test-only
  lowered gate, a dense pure-A2A model auto-flips before the change
  and stays exhaustive after.
- All `tests/simulate_ltm.rs` tests pass.
- Pre-commit hook passes on every commit.
- `cargo test --workspace` finishes within the 3-minute cap.

## Measurement Postscript

Captured 2026-05-06 on the post-Stage-6 branch tip via the
`measurement_postscript_*` integration tests in
`src/simlin-engine/tests/simulate_ltm.rs`.

| Fixture | var SCC | elem SCC | var circuits | elem circuits (legacy) | fast path | slow path | slow-path SCC |
|---------|--------:|---------:|-------------:|------------------------:|----------:|----------:|--------------:|
| cross_element_ltm | 5 | 10 | 3 | 8 | 1 | 6 | 8 |
| arrayed_population_ltm | 3 | 3 | 2 | 6 | 2 | 0 | 0 |

Notes per fixture:

- **cross_element_ltm**: 5 stocks/auxes/flows in one variable-level
  SCC (population, migration_pressure, migration_in, migration_out,
  total_population's wildcard reducer back-edge). 3 variable-level
  cycles: the population<->births A2A cycle classifies as
  PureSameElementA2A (fast path), and the two FixedIndex-driven
  migration cycles classify as CrossElementOrMixed (slow path). The
  slow-path subgraph excludes births[*] (which is in the fast-path
  cycle but not in any cross-element cycle), so the slow-path Johnson
  enumerates 6 element-level circuits versus 8 in the legacy full-element
  Johnson run. Auto-flip status: no in either run (both elem_scc=10 and
  var_scc=5 well under 50).
- **arrayed_population_ltm**: pure-A2A model with 2 variable-level
  cycles (births reinforcing, deaths balancing) over 3 regions.
  Both cycles classify as PureSameElementA2A; fast path emits 2 cycles
  and the slow-path subgraph is empty. Legacy enumeration produced 6
  element-level circuits (2 cycles * 3 regions); the tiered enumerator
  produces 0 element-level circuits and 2 fast-path cycles.
  Auto-flip status: no in either run.
- **WRLD3-03** (not measured here; from prior postscripts): scalar
  model with var_scc = elem_scc = 166, well above the gate threshold.
  Auto-flip fires in both runs. The new variable-level gate fires
  before any Johnson runs at all (legacy gate also fired before
  Johnson). No structural change in WRLD3 behavior.
- **hero_culture_ltm** (not measured here; scalar model with elem_scc
  = 15 from prior postscript): every cycle classifies as PureScalar;
  fast_path equals var_circuits, slow_path is empty. Auto-flip status
  unchanged.

### Threshold decision

`MAX_LTM_SCC_NODES = 50` is retained, applied to two SCCs in series:

1. **Variable-level SCC** (early gate, before any Johnson runs).
   Cheap Tarjan on the variable graph. WRLD3 trips here exactly as
   before the refactor.
2. **Slow-path subgraph SCC** (late gate, computed inside
   `model_loop_circuits_tiered`). Cheap Tarjan on the cross-element /
   mixed slice. Models with huge cross-element subgraphs auto-flip
   here without paying for slow-path Johnson.

Pure-A2A and pure-scalar models contribute 0 to the slow-path SCC,
so the late gate only fires for legitimate cross-element pressure.
The legacy "full element-graph SCC" gate is removed: it was an
upper bound on both variable-level and cross-element pressure, and
the per-reference refactor (#448) had already broken pure-A2A models
into N independent small SCCs, so the legacy gate was effectively
gating on `max(variable SCC, cross-element SCC)` -- which is what
the new gate does explicitly.
