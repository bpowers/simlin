# Loops That Matter (LTM): Implementation Design

This document describes how Simlin implements the Loops That Matter method for
feedback loop dominance analysis. For a comprehensive technical description of the
LTM method itself, see the [reference document](../reference/ltm--loops-that-matter.md).

## Architecture Overview

The implementation is split across these modules in `src/simlin-engine/src/`:

| Module | Responsibility |
|--------|---------------|
| `ltm.rs` | Causal graph construction, loop detection (Johnson's algorithm), static polarity analysis, cycle partitions |
| `ltm_augment.rs` | Synthetic variable generation: link score and loop score equations |
| `ltm_finding.rs` | Post-simulation loop discovery for models too large for exhaustive enumeration: scoring, retention, ranking, and the cap |
| `ltm_finding_enum.rs` | Discovery's exact candidate generator: union-graph elementary-circuit enumeration and its retention pass |
| `ltm_finding_fallback.rs` | Discovery's shortest-path candidate generator, used when the enumeration cannot finish within its budgets or the caller's deadline |
| `ltm_post.rs` | Post-simulation computation: normalizes loop scores into relative loop scores using the cycle-partition mapping produced during LTM compilation |

The production entry point is the `model_ltm_variables` tracked function in
`db/ltm.rs`, invoked as part of `compile_project_incremental`. LTM compilation
is controlled by two flags on `SourceProject`:

- **`ltm_enabled`** -- When true, LTM synthetic variables are generated for every
  model (root and sub-models) during incremental compilation.

- **`ltm_discovery_mode`** -- Controls which edges get link scores. When false
  (exhaustive mode), link scores are generated only for edges participating in
  detected loops, plus one `loop_score` variable per loop. When true (discovery
  mode), link scores are generated for all causal edges.  Relative loop scores
  are derived post-simulation in both modes via
  [`crate::ltm_post::compute_rel_loop_scores`] from the raw `loop_score`
  timeseries and the cycle-partition mapping cached on
  `LtmVariablesResult::loop_partitions`.

Every model -- root, stdlib, and user-defined -- receives identical LTM treatment
via `model_ltm_variables`. The function auto-detects sub-model behavior by
checking for input ports with causal pathways to the output, and generates
pathway and composite scores for such models. Array/subscripted variables are
supported via element-level graph expansion (see "Array Support" below).

## Key Data Structures

### CausalGraph (`ltm.rs`)

```rust
pub struct CausalGraph {
    edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    stocks: HashSet<Ident<Canonical>>,
    variables: HashMap<Ident<Canonical>, Variable>,
    module_graphs: HashMap<Ident<Canonical>, Box<CausalGraph>>,
}
```

The adjacency-list representation of a model's causal structure. Built from a
`ModelStage1` by `CausalGraph::from_model()`, which:

- Creates edges from each variable's equation dependencies to the variable itself
- Handles stocks specially: edges come from inflows and outflows, not from the
  stock's initial-value equation
- For modules classified as `DynamicModule`, recursively builds sub-graphs
  (`module_graphs`) to enable cross-module loop detection and module stock
  enrichment
- Normalizes module output references (e.g. `module·output`) to point to the
  module node itself via `normalize_module_ref()`

### Link (`ltm.rs`)

A single causal connection between two variables, with a statically-analyzed
polarity (`Positive`, `Negative`, or `Unknown`).

### Loop (`ltm.rs`)

A feedback loop: a list of `Link`s forming a closed path, the stocks it contains
(including module-internal stocks), a polarity classification, and a deterministic
ID (e.g., `r1`, `b2`, `u1`).

### CyclePartitions (`ltm.rs`)

Groups of stocks connected by feedback paths (strongly connected components in
the stock-to-stock reachability graph). Each partition gets its own set of
relative loop scores. Computed by `CausalGraph::compute_cycle_partitions()` using
BFS reachability followed by Tarjan's SCC algorithm.

### FoundLoop (`ltm_finding.rs`)

Produced by discovery mode. Wraps a `Loop` with its signed score timeseries
and average absolute score for ranking.

## Two Modes of Operation

### Exhaustive Mode (`ltm_discovery_mode = false`)

1. `model_causal_edges` (salsa tracked, `db/analysis.rs`) builds the causal graph
2. `model_loop_circuits_tiered` runs Johnson's algorithm on the variable
   graph and partitions cycles by `RefShape` composition:
   - Pure-scalar / pure-A2A cycles emit a single `Loop` directly (fast path).
   - Cross-element / mixed cycles drive an element-level Johnson run on
     the slow-path subgraph (the element graph restricted to the
     variables in those cycles).
3. Module nodes appear as regular vertices in the parent graph; loops through
   modules are found naturally by the same algorithm
4. After circuit detection, `enrich_with_module_stocks()` post-processes each
   loop: for any module node in the circuit, it identifies the relevant input
   port and uses `enumerate_module_pathways()` to find internal pathways, then
   collects internal stocks along those pathways (namespaced with the module
   instance name, e.g. `smooth·smoothed`)
5. Loops are deduplicated by **canonical edge-sequence rotation** (issue
   #308): rotations of the same directed cycle collapse, while two distinct
   directed cycles over the same node set (e.g. the arms-race three-party
   pair `A -> B -> C -> A` vs `A -> C -> B -> A`) are retained as separate
   loops
6. Deterministic IDs are assigned by sorting loops by their content key
   (`assign_loop_ids`)
7. `model_ltm_variables` generates synthetic variables for all links
   participating in any loop, plus one `loop_score` variable per loop.
   Relative loop scores are not materialized as synthetic variables; they are
   computed post-simulation from the raw loop score timeseries using the
   cycle-partition mapping cached on `LtmVariablesResult::loop_partitions`

### Discovery Mode (`ltm_discovery_mode = true` + `discover_loops`)

1. `model_ltm_variables` with `ltm_discovery_mode = true` generates link score
   variables for all causal edges (not just those in loops). Loop score variables
   are NOT generated at this stage.
2. The augmented project is simulated normally (interpreter or VM).
3. Post-simulation, `discover_loops()` (`ltm_finding.rs`) generates candidate
   cycles from the recorded series (see "Post-Simulation Loop Discovery"):
   - Parses link score variable names from `results.offsets` (`parse_link_offsets`)
     into the element-level edge set, and builds the integer-indexed topology
     once (`IndexedSearch::build`)
   - Builds the union-of-active-edges graph with its per-edge activity bitsets
     and contiguous score rows (`ActivityGraph::build`)
   - Enumerates every ever-simultaneously-active elementary cycle of that graph
     (`enumerate_active_circuits`) -- the exact candidate universe -- or, when
     the enumeration budgets or the caller's deadline trip, samples cycles with
     the shortest-path fallback (`fallback::sweep`)
4. Each candidate path is converted to a `FoundLoop` with signed loop scores
   computed at every timestep from the raw link score results. A loop edge
   `x → m` into a multi-output module is recomputed against the pathway ending
   at the exit port the loop traverses (the per-exit-port recompute, GH #698 --
   see "Passthrough composites and per-exit-port loop scoring"), so discovery
   and exhaustive agree on the loop's polarity
5. Loops are filtered by `MIN_CONTRIBUTION` (0.1%) against their partition's
   whole-universe mass, ranked competitive-first by mean partition-relative
   importance (loops trivially alone in their cycle partition -- relative score
   +/-1 by construction -- sort after all competing loops), capped at
   `MAX_LOOPS` (200) by a coverage-aware selection that guarantees each step's
   dominant loop a slot, assigned deterministic IDs, and annotated with
   result-scoped cycle-partition metadata (`rank_and_filter`; see "Ranking and
   Filtering")

## Cycle Partitions

The implementation computes cycle partitions (groups of stocks connected by
feedback loops) to ensure relative loop scores are only compared within the
same structural group. This follows Section 8 of the reference: for models with
disconnected stock groups, each subcomponent has a separate loop dominance profile.

### How Partitions Are Computed

1. `build_stock_reachability()` performs BFS from each stock through the full
   causal graph (continuing past intermediate stocks) to determine which other
   stocks are reachable
2. `tarjan_scc()` runs Tarjan's strongly connected components algorithm on the
   stock-to-stock reachability graph, with deterministic node ordering
3. The resulting SCCs become partitions; each stock maps to exactly one partition

### How Partitions Are Used

- **Exhaustive mode**: `generate_loop_score_variables()` records each loop's
  partition on the emitted `loop_score` `LtmSyntheticVar`. Post-simulation,
  `compute_rel_loop_scores()` (`ltm_post.rs`) groups loops by partition and
  normalizes each loop score against the sum of absolute scores within its own
  partition, ensuring structurally independent stock groups don't dilute each
  other's scores.
- **Discovery mode**: `rank_and_filter()` computes per-partition, per-timestep
  score totals. A loop is retained if at any single timestep its absolute score
  is >= `MIN_CONTRIBUTION` of its partition's total. This prevents globally tiny
  but partition-dominant loops from being filtered out.

### Module-Internal Stocks and Partitions

Module-internal stocks (e.g. `smooth·smoothed`) are namespaced with the module
instance name and included in loop stock lists via `enrich_with_module_stocks()`.
These do not appear in the partition map (partitions are computed on the parent
graph), but `CyclePartitions::partition_for_loop()` handles this gracefully: it
finds the partition from any parent-level stock in the loop, with a debug
assertion that all parent-level stocks agree.

## Synthetic Variable Approach

The central design decision is to implement LTM scores as **synthetic simulation
variables** rather than as post-processing on raw results. Each link score and
loop score becomes a regular auxiliary variable in the augmented model. Relative
loop scores are the single exception: they are computed in Rust post-simulation
(`ltm_post::compute_rel_loop_scores`) from the raw `loop_score` timeseries to
avoid quadratic growth in equation text for partitions that contain many loops
(a single partition with P loops would otherwise synthesize P equations, each
summing over all P denominators -- O(P^2) text).

### Why Synthetic Variables

- **Reuses existing infrastructure**: The simulation engine (both the AST
  interpreter and bytecode VM) already handles variable evaluation, dependency
  ordering, and result collection. No separate LTM computation pass is needed.
- **Consistency**: LTM scores are computed using the same equation evaluation
  machinery as the model itself. The ceteris-paribus re-evaluation (holding all
  inputs except one at their previous values) is expressed directly in the
  equation language via `PREVIOUS()`.
- **Transparency**: Users can inspect the generated equations to understand exactly
  what is being computed. The equations are regular SD equations, not opaque
  calculations.
- **VM compatibility**: Both the interpreter (`Simulation`) and compiled VM (`Vm`)
  can run LTM-augmented models without any code changes to the execution engines.

### Trade-offs

- **Model size**: The augmented model has significantly more variables. Each causal
  link adds one synthetic variable; each loop adds one absolute `loop_score`
  variable. For a model with L links and N loops, this adds L + N variables.
  Relative loop scores are not synthesized; they are computed post-simulation.
- **Simulation cost**: Link score equations re-evaluate the target variable's
  equation with ceteris-paribus substitutions, roughly doubling the per-variable
  evaluation cost. This matches the ~2x overhead described in the papers.
- **Equation complexity**: The generated equations are long and contain nested
  `PREVIOUS()`, `SAFEDIV()`, and conditional expressions. They are not intended
  for human authoring.

## Naming Convention for Synthetic Variables

All LTM synthetic variables use a `$` prefix and U+205A (TWO DOT PUNCTUATION, `⁚`)
as a separator:

| Variable | Pattern |
|----------|---------|
| Link score | `$⁚ltm⁚link_score⁚{from}→{to}` |
| Link score (per source element) | `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` |
| Link score (per target element) | `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` |
| Aggregate node | `$⁚ltm⁚agg⁚{n}` |
| Pathway score | `$⁚ltm⁚path⁚{port}⁚{index}` |
| Composite score | `$⁚ltm⁚composite⁚{port}` |
| Loop score | `$⁚ltm⁚loop_score⁚{loop_id}` |

The per-element link-score names ride the element on the `from` side (an
arrayed-source → scalar-target reducer edge, one scalar variable per source
element) or the `to` side (a scalar-source → arrayed-target edge, one scalar
variable per target element). See "Aggregate Nodes" and "Link Score
Classification" below.

`$⁚ltm⁚agg⁚{n}` is a synthetic auxiliary that stands in for a maximal inlined
array-reducer subexpression: an aux whose equation is the canonical reducer
subexpr (`SUM(pop[*])`, `MEAN(...)`), conceptually inserted between the
reducer's array-element sources and the consumers that referenced it inline.
Whole-RHS-scalar reducers are *not* synthesized -- the variable whose entire
dt-equation is the reducer (`total_population = SUM(population[*])`) *is* the
aggregate node. See "Aggregate Nodes" below.

Relative loop scores are not emitted as synthetic variables. They are computed
post-simulation by `ltm_post::compute_rel_loop_scores` from `loop_score` result
offsets and the cycle-partition mapping cached on `LtmVariablesResult`.

The `$` prefix prevents collisions with user-defined variables. The Unicode
separator `⁚` (U+205A) was chosen because it is a valid XID_Continue character
(so it works within identifiers) but is visually distinctive and virtually
never appears in user-authored equations. In generated equations, these variable
names are enclosed in double quotes (e.g., `"$⁚ltm⁚link_score⁚x→y"`) to ensure
correct parsing by the lexer.

The `discover_loops` function in `ltm_finding.rs` parses these names from
`results.offsets` by matching the prefix `$⁚ltm⁚link_score⁚` and splitting
the remainder on `→` (U+2192 RIGHTWARDS ARROW) to extract the `from` and `to`
variable names. Sub-model link scores use the same `$⁚ltm⁚link_score⁚` prefix
but are namespaced by interpunct resolution (`module·$⁚ltm⁚link_score⁚...`),
so the discovery parser's prefix match on the root model's flat result offsets
naturally excludes them.

## Link Score Equations

Three categories of link score equations are generated, corresponding to the
three link types in the LTM method.

### Auxiliary-to-Auxiliary (Instantaneous Links)

`generate_auxiliary_to_auxiliary_equation()` in `ltm_augment.rs`.

For a link from `x` to `z` where `z = f(x, y, ...)`:

1. Get the equation text of `z`, preferring the post-compilation AST (via
   `expr2_to_string`) over the original `eqn` field. This ensures that
   identifiers in the equation match those in the dependency set (important for
   modules: the `eqn` field holds the original text like `SMTH1(x, 5)` while
   the AST holds the expanded form like `$⁚s⁚0⁚smth1·output`).
2. Compute the dependency set from the AST via `identifier_set()`.
3. Build the ceteris-paribus partial equation using `build_partial_equation()`,
   which parses the equation into an `Expr0` AST, recursively walks the tree
   wrapping variable references in `PREVIOUS()` for all dependencies except `x`
   (`wrap_deps_in_previous`), and prints the result back to equation text. This
   AST-based approach avoids the pitfalls of text-based replacement (e.g.,
   replacing `x` inside `x_rate`, or corrupting function names like `MAX`).
4. The link score is:
   ```
   if (TIME = INITIAL_TIME) then 0
   else if ((z - PREVIOUS(z)) = 0) OR ((x - PREVIOUS(x)) = 0) then 0
   else ABS(SAFEDIV((partial_eq - PREVIOUS(z)), (z - PREVIOUS(z)), 0))
      * SIGN(SAFEDIV((partial_eq - PREVIOUS(z)), (x - PREVIOUS(x)), 0))
   ```

### Flow-to-Stock Links

`generate_flow_to_stock_equation()` in `ltm_augment.rs`.

Implements the corrected 2023 formula (Schoenberg et al., Eq. 3). The numerator
uses `PREVIOUS()` to align timing: at time t, `PREVIOUS(flow)` is the flow value
at t-1 that drove the stock change from t-1 to t.

```
numerator = PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))
denominator = (stock - PREVIOUS(stock)) - (PREVIOUS(stock) - PREVIOUS(PREVIOUS(stock)))
link_score = sign * ABS(SAFEDIV(numerator, denominator, 0))
```

The denominator is the second-order change in the stock (its "acceleration").
The ratio is wrapped in `ABS()` because flow-to-stock polarity is structural:
inflows always contribute positively (+1), outflows negatively (-1). The sign
is applied outside the absolute value. This equation returns 0 for the first
two timesteps (insufficient history for second-order differences), guarded by
`TIME = INITIAL_TIME` and `PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME`.

### Stock-to-Flow Links

`generate_stock_to_flow_equation()` in `ltm_augment.rs`.

Uses the standard instantaneous formula but recognizes that the "from" variable
is a stock. The flow's equation is modified by `build_partial_equation()` to
replace all non-stock dependencies with their `PREVIOUS()` values, isolating the
stock's contribution.

### Module Links

`module_link_score_equation` in `db.rs` is the single source of truth for a
module-involved link's equation, shared verbatim by the `(from, to)`-keyed
`link_score_equation_text` and the per-shape `link_score_equation_text_shaped`
(a module link's equation does not depend on the reference `RefShape`, so the
two twins delegate to the same helper and can never drift). It handles three
cases, each preferring a faithful link score and only falling back to the
signed unit transfer when nothing better exists:

- **Variable-to-module-input** (`!from_is_module && to_is_module`): When the
  target module's sub-model emits a composite link score for the port the edge
  feeds (a DynamicModule with an input→output pathway), the link score IS that
  composite, referenced via interpunct notation `module·$⁚ltm⁚composite⁚port`.
  This is the module's internal transfer -- exactly the macro treatment
  (ref §6). When the sub-model exposes no composite (a passthrough), the link
  score is the **signed unit transfer** (below) against the module's *output*
  ref `module·port` -- a readable scalar, never the bare module name.

- **Module-output-to-variable** (`from_is_module && !to_is_module`): The
  dependent's equation references the module output via `module·port`, so a
  real ceteris-paribus partial is available -- the link score is that exact
  partial (`generate_link_score_equation_for_link` on the located output ref).
  `build_partial_equation` is module-ref-aware: `normalize_module_ref()`
  strips interpunct suffixes so module output references (e.g.
  `$⁚s⁚0⁚smth1·output`) are excluded from `PREVIOUS()` wrapping while other
  dependencies are held at their previous values. Falls back to the signed
  unit transfer only if the output reference cannot be located in the target
  AST. (Before GH #675 this arm used the gain `Δto/Δ(module·output)`, which is
  why a single-input downstream that should score ±1 instead scored the gain.)

- **Module-to-module** (`from_is_module && to_is_module`): `from`'s output is
  wired into `to`'s input port. The edge source in the parent graph is the
  normalized module node `from`, but `to`'s `ModuleInput::src` is the
  module-qualified `from·output`, so the match is by
  `normalize_module_ref(src) == from`, not raw equality. When `to`'s sub-model
  exposes a composite for that port, the link score IS `to`'s composite for
  the port (the macro treatment again -- the wiring from `from`'s output to
  `to`'s input port is an identity, so the loop product equals the
  fully-expanded model's). Otherwise it is the signed unit transfer between
  the two modules' output refs.

**Composites resolve in both modes (GH #548 / #675).** Since GH #548,
`build_submodel_metadata` lays out a sub-model's LTM synthetic vars (composites
included) in the parent's flattened offset map whenever `ltm_enabled`, which
holds in *both* exhaustive and discovery mode. An empirical probe confirmed a
SMOOTH composite resolving to a nonzero value in a discovery run. Discovery
mode therefore uses the *same* composite reference exhaustive mode does -- the
pre-#675 discovery-only gain variant (`Δ(module·output)/Δfrom`, justified by a
since-stale "cross-module refs don't resolve in discovery" assumption) is gone.
`module_composite_ports` reads the sub-model's actual `model_ltm_variables`
output to decide whether a composite exists for a port, rather than guessing
from the module's stock count.

**Signed unit-transfer fallback (GH #675).** The residual genuine black-box
case -- no composite (a passthrough exposes none) and no ceteris-paribus
partial (the endpoint is a module with no parent-visible equation) -- uses
`black_box_unit_transfer_equation`: `0` at `INITIAL_TIME`, `0` when either
endpoint is unchanged, else `SIGN(Δto)·SIGN(Δfrom)`. This is a *link score*,
not the gain `dz/dx` (ref §3.3): for a single-input black box `z = F(x)` all
of `Δz` is attributable to `x`, so `|Δ_x(z)/Δ(z)| = 1` and only the sign
remains -- the unit transfer is exact. For a stateful/multi-input box it is
the perfect-mixing-spirit approximation (ref §6): polarity exact, magnitude
approximated as `1`. Crucially it preserves the **isolated-loop ±1 invariant**
(Appendix B): an isolated feedback loop routed through a passthrough-module
chain (including a module→module link) has raw loop score exactly ±1 regardless
of the module gains, because a link score normalizes the gain away whereas the
old `Δto/Δfrom` gain made the loop score scale with the product of the gains.
References are always readable scalars (`module·port`), never the bare module
name -- a bare module name is not a scalar-readable variable, so the prior
formula's bare-name references stubbed the fragment to a constant 0 (which
zeroed every loop through such a module).

## Module Boundary Handling

The implementation uses **composite link scores** for dynamic modules (SMOOTH,
DELAY, TREND, etc.), following Section 6 of Schoenberg & Eberlein (2020). The
composite score is the product of internal link scores along the strongest
internal pathway at each timestep.

### Module Classification

Modules fall into three LTM roles:

- **Infrastructure** (`PREVIOUS`, `INIT`) -- used BY link score equations; never
  analyzed to avoid infinite recursion.
- **DynamicModule** -- has internal stocks (SMOOTH, DELAY, TREND, user-defined
  modules with stocks). Gets composite link scores and internal graph construction.
- **Passthrough** -- no internal stocks. A passthrough whose internals form an
  aux chain from input to output still emits pathway/composite vars (its chain
  is a pure expression LTM scores exactly), so a link into its input port
  references the composite just like a dynamic module. The signed unit-transfer
  fallback (see Module Links) now fires only for a *pathway-less* module -- one
  whose output does not depend on its input at all.

The `CausalGraph` builders that carry module data (`CausalGraph::from_model`
and the shared `db::analysis::model_variables_and_module_graphs` used by
`causal_graph_with_modules` and the element-level
`causal_graph_from_element_edges_with_modules`) build a recursive internal
sub-graph for **every** referenced sub-model -- DynamicModule and passthrough
alike -- since the discovery-mode per-exit-port pathway recompute (GH #698)
needs the passthrough's sub-graph too. (Before GH #698 only DynamicModules got a
sub-graph, gated by a now-removed `classify_module_for_ltm` stock check; a
pathless module's sub-graph enumerates no pathways, so building it is harmless.)
The bare `causal_graph_from_element_edges` constructor still leaves `variables`
/ `module_graphs` empty -- it is used where module data is not needed; the
production discovery path (`analyze_model`) uses the `_with_modules` enriching
variant.

#### Passthrough composites and per-exit-port loop scoring (PR #684)

`model_ltm_variables` no longer suppresses LTM vars for a stockless sub-model
that has parent-visible input→output pathways: the stock-free early return now
fires only when the model is genuinely STATELESS -- neither parent-level
stocks, nor input-port pathways, nor (since GH #748) any transitively
stock-carrying module instance (`modules_carry_state`). A parent-stock-free
*root* whose only state lives inside modules (a SMOOTH/DELAY instance or a
user sub-model with an INTEG) therefore runs the pass and scores its loops; a
truly stateless root -- with no parent reading `module·var`, hence no output
ports, and no module-internal state -- still emits nothing. A passthrough
sub-model emits the same `$⁚ltm⁚path⁚{port}⁚{idx}` / `$⁚ltm⁚composite⁚{port}`
vars a dynamic module does.

The composite alone is, however, the WRONG score for a loop through a
*multi-output* module: the composite max-abs-selects across ALL of the module's
pathways. Consider a passthrough exposing `pos = input·0.02` and
`neg = -input`, with the feedback loop reading only `m·pos` and a side variable
reading `m·neg`. A single-dependency link score is just `(Δz)/|Δz| = SIGN(Δz)`,
so the `0.02` coefficient cancels and BOTH pathways have magnitude exactly 1 in
the degenerate normalized sense. The composite's `if ABS(path0) >= ABS(path1)`
selection is therefore comparing 1 against 1: it falls through to the
`>=` first-index TIE-BREAK, which picks `path0` -- and `path0` is `neg`, whose
sign opposes `pos`, flipping the loop's polarity. Composites cannot fix this
because the candidates always tie at magnitude 1, so max-abs can never recover
the loop's actual port: it just returns whichever pathway is enumerated first.

So in **exhaustive** mode the loop-score equation overrides each `x → m` module
link with a **per-exit-port pathway selection**. The exit port is read off the
NEXT loop link `m → y` (the unique `m·port` `y` reads, or `y`'s matching
`ModuleInput.src` when `y` is itself a module); the entry port is `m`'s
`ModuleInput` whose normalized `src` is `x`. Both are *unique-match* lookups:
the port is ambiguous, and the override is skipped, if (a) `x` feeds two input
ports of `m` (`x → m.a` AND `x → m.b` collapse to one `x → m` edge -- PR #705
r3353459409); (b) a non-module reader `y` reads two distinct `m·port`s; or (c)
`y` is itself a module reading two distinct output ports of `m` on different
inputs (`m·early → y.p` AND `m·late → y.q` collapse to one `m → y` edge -- PR
#705 r3353597299). Two of `y`'s inputs naming the SAME `m·port` are NOT
ambiguous (a unique distinct port). On any ambiguity the base composite link
score (a documented first-matched-port approximation) stands rather than the
recompute arbitrarily picking the first matching port. The discovery recompute
`recompute_module_input_edge_series` applies the identical ambiguous
entry/exit fallback. The parent recomputes the
sub-model's pathway map with the SAME salsa-cached inputs the sub-model's own
emission uses (`model_causal_edges` + the sorted `find_model_output_ports`), so
the recomputed pathway indices match the emitted `$⁚ltm⁚path⁚{entry}⁚{idx}`
vars index-for-index. The override is an alias synthetic var
`$⁚ltm⁚link_score⁚{x}→{m}⁚via⁚{exit}` whose equation is:

- the single matching pathway ref `m·$⁚ltm⁚path⁚{entry}⁚{idx}` when exactly one
  pathway ends at the exit port;
- a max-abs selection over the matching refs when several do (the accumulator
  helpers are named with a `⁚viaacc⁚` infix so they sort -- and therefore
  evaluate -- before the alias within the `link_score` category);
- `"0"` when no pathway connects entry to exit (truthful: no causal transfer --
  but pathway-budget truncation, GH #649, can also produce this, signalled by
  the existing `pathways_truncated` Warning).

The alias references SUB-model pathway vars (`m·…`), which the parent evaluates
when it runs module `m` -- before the parent's appended `link_score` fragments,
exactly as the existing composite reference resolves at the current step
(verified by an end-to-end simulation assertion, not by reasoning alone). The
override is threaded into the loop-score equation builder as a side-table keyed
by `(loop_id, link_index)`; `Loop.links` is NOT rewritten (loop IDs derive from
the link sequence and the FFI's id→score correspondence depends on it).

**Discovery-mode per-exit-port recompute (GH #698)**: discovery mode emits NO
loop-score vars (loops are scored and ranked post-simulation from the recorded
link-score series), so there is no loop-score *equation* to override. Candidate
generation still reads the composite when it decides whether a module-input
edge is active at a step (a composite that is 0 at every step does imply every
per-port score is 0, so the zero case loses no loop; the NaN case is a stated
boundary -- see "Honest boundaries"). But the **post-simulation score
recompute** -- which converts each candidate path into a `FoundLoop` by
multiplying the per-step signed link scores -- applies the SAME
per-exit-port selection the exhaustive override applies: for a loop edge
`x → m` whose next edge `m → y` identifies the exit port (the `m·port` `y`
reads, or `y`'s matching `ModuleInput.src` when `y` is itself a module), it
recomputes that edge's series by max-abs-selecting over the sub-model's
`m·$⁚ltm⁚path⁚{entry}⁚{idx}` pathway scores that *end at the exit port*,
instead of reading the composite's offset. The pathway indices are recovered
from the module's recursively-built sub-graph
(`enumerate_pathways_to_outputs_with_truncation`) over the **same project-wide
sorted output-port set the sub-model emitted against**. That set is NOT
re-derived parent-scoped inside the recompute (a parent-scoped scan of the
analyzed model alone would shift every pathway index whenever ANOTHER project
model -- or a nested instantiation -- reads an additional output port sorting
before the loop's port, and then the recompute would read the wrong pathway,
re-introducing the wrong-signed edge; GH #698 / PR #705 r3353097150). Instead
`discover_loops_with_graph` takes a `SubModelOutputPorts` map keyed by
sub-model canonical name; `analyze_model` builds it from the SAME emission
decision (`db::ltm::sub_model_output_ports`, including the `stdlib⁚`-prefixed
short-circuit to exactly `["output"]`) via the public
`analysis::build_sub_model_output_ports`, so the recompute and emission are
identity-by-construction. The db-less `discover_loops(&Results, &Project)`
convenience path reconstructs the same set with the same project-wide-union +
stdlib-output semantics (`project_sub_model_output_ports`). This is
`recompute_module_input_edge_series` in `ltm_finding.rs`; it falls back to the
base composite offset whenever the entry or exit port is indeterminate (e.g. an
ambiguous multi-output reader), the sub-model is absent from the map, or no
pathway connects entry to exit, so single-output modules (SMOOTH, DELAY, …) and
pathless modules are unaffected.

Recovering the pathway indices required the discovery `CausalGraph` to carry
module sub-graphs + the variable map. **The production analysis path builds the
discovery graph from element-level edges** (`analyze_model` →
`model_element_causal_edges` → `causal_graph_from_element_edges`), and that bare
constructor leaves `variables` / `module_graphs` EMPTY. So `analyze_model` now
uses `causal_graph_from_element_edges_with_modules`, which enriches the
element-level graph with the same `(variables, module_graphs)` pair
`causal_graph_with_modules` builds (the shared `model_variables_and_module_graphs`
in `db/analysis.rs`). Both that helper and `CausalGraph::from_model` (the
`discover_loops` convenience path) build a sub-graph for **every** referenced
sub-model, not only stockful `DynamicModule`s: a stockless passthrough emits
pathway vars (PR #684) but otherwise has no sub-graph to consult; a pathless
module's sub-graph enumerates no pathways and is harmless. (This is why the
prior `classify_module_for_ltm` stock gate on sub-graph construction is gone.)
The element-level module nodes are keyed by the bare module instance name --
the same key `module_graphs` and the module `Variable` use -- so the recompute's
lookups resolve. `reconstruct_model_variables` reconstructs a module instance
through `reconstruct_implicit_variable` (not the generic parse+lower path, which
resolves inputs against an empty `scope.models` and so drops them), so the
recompute can read a module's entry/exit ports off its preserved `ModuleInput`s
-- the same fix `reconstruct_single_variable` already applied for the exhaustive
override.

Because the discovery graph is element-level, an arrayed loop's non-module
nodes carry element subscripts (`s[nyc] → m → growth[nyc]`).
`recompute_module_input_edge_series` therefore `strip_subscript`s `link.from`
(entry match vs the bare `ModuleInput.src`), `link.to`/`next.from` (module node
+ sequentiality guard), and `next.to` (exit-reader lookup in the bare-keyed
variable map) before each name comparison -- mirroring the exhaustive twin,
which strips `link.from`/`link.to`/`next.from`/`next.to` (PR #705 r3353758167).
The pathway vars stay namespaced by the bare module instance (`m·$⁚ltm⁚path…`),
so they are looked up with the stripped module name. This defense is LIVE, not latent, since
GH #716 closed. A scalar module output feeding an arrayed reader
(`growth[Region] = m·pos`) used to emit a single scalar constant-0 link score
that dropped the loop in discovery; such an edge is now scored per target
element by `db::ltm::link_scores::try_implicit_scalar_to_arrayed_link_scores`,
which also owns the per-element module INSTANCES that a per-element expansion
mints (`$⁚growth⁚0⁚smth1⁚north`), whose partials were previously `scalarize`d
onto element 0's arm. The unit-level
`recompute_strips_element_subscripts_before_port_match` exercises the matching
code directly, and `analyze_model_arrayed_module_loop_is_discovered_per_element`
pins the end-to-end result: one loop per element of the reader, none crossing
elements.

Before this fix, discovery read the composite's offset for the `x → m` edge;
because single-dependency pathways all normalize to magnitude exactly 1, the
composite's max-abs tie-break picked an arbitrary output port, and a loop
through a multi-output module whose ports have opposing signs got the arbitrary
port's sign -- empirically inverting the loop's polarity (+1.0 in exhaustive vs
-1.0 in discovery on a `pos = input*0.02` / `neg = -input` repro). The
cross-mode regression guard is
`discovery_multi_output_loop_polarity_matches_exhaustive` in
`tests/integration/simulate_ltm.rs`.

**Deterministic output-port ordering (GH #680)**: `find_model_output_ports`
sorts its result. The merge-order of the `HashSet` it previously returned was
process-nondeterministic; both the sub-model's pathway-index assignment and the
parent's recomputation must agree on that order, so the sort is a prerequisite
for the index-for-index identity above (and closes #680).

**Residual gap**: pinned loops (`LOOPSCORE`) pass an empty override map, so a pin
whose cycle traverses a multi-output module still scores its input→module link
against the arbitrary-port base fallback. Pins through single-output modules
(the common case) are unaffected.

### Unified Module LTM Treatment

Every model (root, stdlib, user-defined) receives identical LTM treatment via
the `model_ltm_variables` tracked function. The function auto-detects sub-model
behavior by checking for input ports with causal pathways to the output
(`module_input_pathways_from_edges`). For models with valid input-to-output
pathways, pathway and composite score variables are generated. The composite
score is the "LTM interface" of a module -- the parent model's link score for
`input -> module` references `module·$⁚ltm⁚composite⁚port`.

### How Composite Link Scores Work

1. **CausalGraph normalization**: When a variable references a module output via
   the interpunct notation (`module·output`), the edge is normalized to point to
   the module node itself (`normalize_module_ref`). This ensures the module
   participates correctly in loop detection.

2. **Internal instrumentation**: For each model with input→output pathways
   (DynamicModule or passthrough), `model_ltm_variables` generates:
   - Internal link score variables with the `$⁚ltm⁚link_score⁚` prefix for all
     causal links within the module
   - Pathway score variables (`$⁚ltm⁚path⁚{port}⁚{index}`) for each pathway,
     computed as the product of constituent internal link scores
   - Composite score variables (`$⁚ltm⁚composite⁚{port}`) that select the
     pathway with the largest absolute magnitude at each timestep

   These are compiled and included as part of the incremental compilation
   pipeline via the salsa tracked function graph.

3. **Pathway enumeration**: `enumerate_module_pathways()` in `ltm.rs` finds all
   simple paths from each input port to the output variable within the module's
   internal causal graph. Input ports are identified as nodes with no incoming
   edges within the module. For smth1, the sole pathway is `input -> flow -> output`.

4. **Composite selection**: `generate_max_abs_chain()` produces a deterministic
   nested selection equation. For a single pathway, this is just the pathway
   score. For multiple pathways, it generates a chain:
   `if ABS(p1) >= ABS(p2) then p1 else p2`.

5. **Parent model reference**: The parent model's link score for
   `input_src -> module_instance` references the module's composite via
   interpunct notation: `"module·$⁚ltm⁚composite⁚port"`. The compiler resolves
   this through the standard `module·var` mechanism in `context.rs`.

### Loop Suppression and Module Stock Enrichment

Internal module-only loops (e.g., smth1's `output -> flow -> output`) are not
reported in the parent model's loop list. Johnson's algorithm traverses module
nodes as opaque vertices in the parent graph and does not descend into module
internals, so these internal-only loops are naturally excluded.

Loops that pass through modules (e.g., `stock -> module -> aux -> stock`) ARE
found by Johnson's algorithm because module instances appear as regular nodes in
the parent causal graph with incoming edges (from input sources) and outgoing
edges (to downstream variables that reference the module output).

After circuit detection, `enrich_with_module_stocks()` post-processes each loop:
for any module node in the circuit, it identifies the predecessor in the circuit
(the variable feeding into the module), determines which input port the
predecessor maps to, uses `enumerate_module_pathways()` to find internal pathways
from that port to the output, and collects internal stocks along those pathways.
These stocks are namespaced with the module instance name using the interpunct
separator (e.g., `smooth·smoothed`) and added to the loop's stock list. This
ensures correct cycle partitioning when module internals contain stocks that
participate in the feedback structure. If the input port cannot be determined or
has no matching pathway, the enrichment falls back to including all stocks in the
module's internal graph.

## Polarity Analysis

### Static Polarity

`analyze_link_polarity()` in `ltm.rs` determines link polarity from the compiled
AST (`Ast<Expr2>`) at compile time. The recursive analysis
(`analyze_expr_polarity_with_context`) handles:

- **Variable references**: Returns the current polarity context if the variable
  matches `from_var` (accounting for module ref normalization), `Unknown` otherwise
- **Addition**: Preserves polarity; if one operand is independent of `from_var`
  (checked via `expr_references_var`), uses the other operand's polarity
- **Subtraction**: Left operand preserves polarity; right operand flips. Same
  independence check as addition.
- **Multiplication**: When one operand is independent of `from_var`, its VALUE
  sign decides (`cofactor_value_sign`): a PROVABLE sign -- a numeric literal
  seen through unary negations (`literal_sign`; the lexer takes no leading
  sign, so a parsed `-5` is `Op1(Negative, Const(5))`), or a variable whose
  whole equation is one (`provable_value_sign`) -- preserves or flips the
  other operand's polarity exactly. A bare named quantity (`Var` /
  `Subscript`) without a provable sign is positive by the SD labeling
  convention, so `net_growth = population * fractional_growth` labels
  `population -> net_growth` Positive -- the reading every CLD gives it, and
  the same convention the Division arm has always applied. A COMPOUND
  co-factor (`k - x`, `1 - pop/K`) stays `Unknown`: its value sign is
  derived, not conventional, and that is exactly the class whose sign flips
  mid-run. When BOTH operands depend on `from_var`, the
  product rule `d(f*g)/dx = f'g + fg'` mixes operand VALUES into the sign,
  so plain sign composition is unsound (it labeled logistic growth
  `pop*(1 - pop/K)` a definite Negative while the true partial flips at
  K/2). The rule: agreeing derivative signs AND both operands
  positive-by-convention (bare variable references, positive constants, or
  positive-constant variables -- NOT compound expressions like `1 - pop/K`)
  propagate the shared polarity (covers `pop*pop/capacity`); everything else
  is `Unknown`.
- **Division**: When the independent operand's value sign is provable, it is
  used exactly (`-5/y` is Positive -- `d(n/y)/dy = -n/y^2` -- and `x/-5` is
  Negative; the pre-fix rules flipped/passed unconditionally and got both
  wrong). For a NON-constant independent operand the conventional SD
  positive-value assumption applies (numerator passes polarity through,
  denominator flips), documented as a labeling convention rather than a
  proof -- `share = pop/total` reads as `total -> share` Negative on every SD
  diagram even though `pop > 0` is unprovable. Both-sides-dependent division
  mirrors multiplication: opposing derivative signs with
  positive-by-convention operands propagate, else `Unknown`.
- **Unary negation and NOT**: Flip polarity
- **IF-THEN-ELSE**: Returns the common polarity if both branches agree, `Unknown`
  otherwise
- **Lookup tables** (`LOOKUP` / `BuiltinFn::Lookup` and the `LookupForward` /
  `LookupBackward` extrapolation variants): Analyzes monotonicity of graphical
  functions (`analyze_graphical_function_polarity`) -- checks consecutive
  y-values to decide if the table is monotonically increasing (Positive),
  decreasing (Negative), or neither (Unknown), then combines with the
  argument's polarity. The strict-monotonicity test uses a y-range-relative
  epsilon, `max(EPSILON, range_rel * (y_max - y_min))` (#492), so a near-flat
  arm with imported numeric noise (`...12.0001, 12.0000, 12.0002...`) no longer
  flips an otherwise-monotone curve to `Unknown`. Comparing the y-delta `dy`
  rather than the slope `dy/dx` is correct for the sign question: a
  piecewise-linear interpolation between y-points that increase consecutively
  is monotone regardless of x-spacing (slope magnitude varies, its sign does
  not), so non-uniform x-spacing cannot misclassify a valid table -- the only
  exposure is a table whose x-points are themselves non-monotone, which is
  malformed input (GH #536 is narrower than its title suggests).
- **Per-element graphical functions** (#502): when an *arrayed* source feeds an
  *arrayed* per-element graphical-function target -- each element of the target
  has its own lookup `Table` (the per-element `tables` list on `Variable::Var`) --
  the per-element table polarities are folded into one link polarity, and the link
  is `Positive` / `Negative` only if every element agrees. The multi-dimensional
  case (a per-element GF over more than one dimension) stays conservatively
  `Unknown`.
- **Non-decreasing builtins**: `EXP`, `LN`, `LOG10`, `SQRT`, `ARCTAN`, `INT` --
  propagate the inner expression's polarity unchanged
- **Max/Min (two-arg)**: Non-decreasing in each argument; if one operand returns
  Unknown, checks whether it actually references `from_var` to distinguish
  independent expressions from truly non-monotonic ones
- **Flow-to-stock**: Inflows are `Positive`, outflows are `Negative` (fixed
  structural polarity)
- **Arrayed equations**: Checks all elements; returns `Unknown` if any two
  elements disagree

If any link in a loop has `Unknown` polarity, the loop's structural polarity is
classified as `Undetermined` (`calculate_polarity`).

### Runtime Polarity

`LoopPolarity::from_runtime_scores()` in `ltm/types.rs` classifies polarity
based on actual simulation results. It filters out NaN and zero values, then:
- All remaining scores positive -> `Reinforcing`
- All remaining scores negative -> `Balancing`
- Mixed signs, one polarity dominant with confidence >=
  `POLARITY_CONFIDENCE_THRESHOLD` (0.99) -> `MostlyReinforcing` /
  `MostlyBalancing` ("Rux"/"Bux")
- Mixed signs below the threshold -> `Undetermined`
- No valid scores -> `None` (caller falls back to structural polarity)

This catches cases where nonlinear dynamics cause polarity to change during
simulation (e.g., the yeast alcohol model from the papers).

#### Which surfaces reclassify, and which do not (GH #679)

`model_detected_loops` is a *pre-simulation* salsa query, so it can only report
*structural* polarity. Pervasively for module-heavy models the static polarity
of a `variable -> module` / `module -> variable` black-box link is `Unknown`,
so a loop through a module boundary is labelled `Undetermined` (confidence 0.0)
even when its simulated loop score is single-signed at every active step.
Runtime reclassification is therefore a *post-simulation* concern, and the
surfaces handle it differently:

- **Discovery (`analyze_model` / MCP / `simlin_analyze_discover_loops`)**: the
  `FoundLoop` path in `ltm_finding.rs` derives each loop's polarity directly
  from `from_runtime_scores` over the loop's own per-step score series
  (falling back to the trimmed-chain structural polarity for an all-zero/NaN
  series). Fully reclassified.
- **pysimlin `Run.loops`**: sources polarity / confidence / partition straight
  from the engine primitive (bound as `Sim.get_loops_runtime` ->
  `reclassify_loops_from_results`, GH #679/#685, the all-slots Rust source of
  truth) and attaches the per-step relative-score series on top. There is no
  Python-side reclassification: the classification rules live in exactly one
  place, the Rust engine (`ltm/types.rs`).
- **libsimlin / WASM / TS `simlin_analyze_get_loops`**: **intentionally
  structural-only**. The FFI takes only a `SimlinModel` (no simulation
  `Results` in hand), folds `MostlyReinforcing`/`MostlyBalancing` to
  `Reinforcing`/`Balancing`, and drops `polarity_confidence` at the C ABI
  boundary. Surfacing runtime polarity here requires the FFI plumbing tracked
  under GH #495; it is **not** delivered by this change. A consumer reading
  loop polarity from `get_loops` must expect the structural label, not the
  runtime one.

`db::analysis::reclassify_loops_from_results(loops, results, loop_partitions)`
is the **canonical in-engine reclassification primitive** -- it reads each
loop's `$⁚ltm⁚loop_score⁚{id}` slot(s) from a `Results` and applies
`from_runtime_scores` to overwrite `polarity`/`polarity_confidence`. As of this
writing it has **no production caller**: it exists so a future sim-bearing Rust
consumer (e.g. when GH #495's FFI lands) has one correct place to call rather
than re-deriving the loop-score read. It is exercised by engine tests.

The **loop id never changes** under reclassification. Loop detection and the
deterministic `r{n}`/`b{n}`/`u{n}` id assignment happen at compile time before
any simulation, and the FFI id->score correspondence plus salsa caching depend
on the id being stable: a loop detected as `u1` keeps the id `u1` even when its
runtime polarity is `Reinforcing`. Only the polarity *field* reflects the
runtime classification. A loop whose score is never active (every slot/step
zero or non-finite) keeps its structural polarity -- there is no runtime
evidence to override it.

**A2A semantics across the sites.** The Rust `reclassify_loops_from_results`
helper concatenates *all* element slots of an A2A loop into one sample set (so a
loop that is reinforcing in one element and balancing in another classifies
`Undetermined`). pysimlin `Run.loops` is built on this primitive, so it reports
exactly this all-slots classification. Discovery uses one scalar score series
per `FoundLoop` (its links are element-level, so a discovered loop is always
scalar). The exhaustive (sim-bearing) and discovery surfaces thus
agree on scalar loops and differ only in how an A2A loop's element slots are
reduced -- the exhaustive path now uses the all-slots reading rather than slot 0.

## Post-Simulation Loop Discovery

Discovery mode finds the loops that matter *after* the simulation, from the
recorded link-score series. The implementation is `ltm_finding.rs` plus two
`#[path]`-mounted siblings (`ltm_finding_enum.rs`, `ltm_finding_fallback.rs`,
split out for the per-file line cap only). The design plan
`docs/design-plans/2026-08-17-ltm-discovery-exact.md` holds the measurement
ledger this section summarizes.

The pipeline has three stages, and only the first is lossy:

```
parse_link_offsets -> IndexedSearch::build      node ids, per-edge result slots
      |
      |  1. CANDIDATE GENERATION
      +----> ActivityGraph::build -> enumerate_active_circuits   exact: the universe
      |      or fallback::sweep                                  a sample
      |
      |  2. EXACT SCORING
      +----> each candidate's per-step score = the product of its links'
      |      recorded score series -- never an accumulated search estimate
      |
      |  3. RETAIN, RANK, CAP
      `----> retain_circuits / rank_and_filter / select_reported -> FoundLoop list
```

The generator decides only WHICH cycles are proposed, never what they are
worth, so switching generators cannot change a reported loop's scores. Which
generator ran is reported as `DiscoveryResult::enumeration_complete`.

### The union graph and activity bitsets

Because discovery runs after the simulation, the set of edges that ever carried
signal is observable. `ActivityGraph::build` scans the results slab once and
keeps every element-level causal edge whose recorded |link score| is
active at one or more saved steps in `1..step_count` -- the **union graph** -- storing per
edge:

- a word-packed **activity bitset** over saved steps, and
- a contiguous copy of its signed per-step score series, so every later scoring
  pass reads sequentially instead of striding the slab by `step_size`.

`is_active` is defined once (`ltm_finding.rs`) and read by both generators, so
they cannot disagree about which cycles exist: a value is active when it is
finite and nonzero, or infinite (a divergent link is real signal); only NaN and
an exact zero are inactive. Step 0 is excluded from union membership and masked
out of every activity test: every link-score equation's `TIME = INITIAL_TIME`
guard arm is emitted as the literal constant `0`, so a cycle "active" only there
is not a scorable loop. Self-edges are dropped at build time -- an elementary
cycle never repeats a node, so a self-edge can neither be nor extend one, and a
single variable referencing itself is not feedback in the SD sense (the same
`circuit.len() > 1` contract exhaustive mode states).

A loop's score is the product of its link scores, so it is nonzero at step `t`
only if every one of its edges is active at `t`. The AND of a path's activity
bitsets is therefore exactly the set of steps at which the path can score, and
an empty AND is a proof that no extension of that path can ever score either.
That single fact is what makes exact enumeration affordable and what bounds the
scoring work afterwards (`ActivityGraph::active_window` restricts a circuit's
scoring to the `[first, last]` step span of its AND -- a 5x reduction on
World3).

### Exact enumeration (`ltm_finding_enum.rs`)

`enumerate_active_circuits` emits every elementary cycle of the union graph
whose activity AND is nonempty -- exactly the **universe** of loops that can
ever have a nonzero score, at saved-step resolution. It is a min-root
Tiernan-style search: for each root in ascending node id, walk simple paths,
maintaining the running AND, and emit a cycle when a path closes back on the
root with a nonempty AND. Each cycle is emitted once, rooted at its minimum
node id.

- **Per-root induced-subgraph SCC.** For root `r` only the nodes in `r`'s
  strongly connected component *within the subgraph induced by nodes `>= r`*
  (Johnson's `A_k`) are explorable. This is exact -- every cycle whose minimum
  node is `r` lies entirely inside that component -- and it is what removes the
  dead-end wandering that made two thirds of World3's descents fruitless.
  Membership is stamped with a per-root generation counter, so a root costs only
  the nodes it actually reaches.
- **On-path blocking only, no Johnson unblocking.** The activity-bitset pruning
  is path-dependent (whether a node is worth revisiting depends on the AND
  carried into it), which breaks Johnson's blocked-set invariant. The induced
  SCC recovers most of what unblocking would have bought.
- **Edge-row emission.** A circuit is a sequence of `u32` edge rows (closing
  edge included), stored compressed-row style in one flat array. A row indexes
  both an activity bitset and a contiguous score series, so retention scores a
  circuit with no `(from, to)` lookup and no per-circuit allocation. Node paths
  are derived (`circuit_nodes`) only where a consumer needs one -- cross-agg
  stitching and materialization.
- **No per-visit allocation.** The running AND is written straight onto a stack
  and truncated on prune, at any bitset width.

Enumeration runs under four bounds: `MAX_DISCOVERY_ENUM_CIRCUITS` (1,000,000
circuits), `MAX_DISCOVERY_ENUM_VISITS` (100M edge visits -- the circuit count
alone does not bound work, since a graph can force long paths that rarely
close), `MAX_DISCOVERY_ENUM_EDGE_ROWS` (20M rows, i.e. 80 MB, the memory bound
-- cost scales with circuits times mean circuit length, and mean length is a
property of the graph rather than of the budget), and the caller's deadline,
checked at the first edge visit and every `DEADLINE_CHECK_INTERVAL` (8192)
visits after it. Any trip returns `complete: false`, and the caller **discards
the partial circuit list** rather than merging it: a partial enumeration is
biased by node-id root order and its per-partition totals are not the universe's,
so it can supply neither candidates nor denominators honestly. The fallback is
the principled sample instead.

### Retention against the universe (`retain_circuits`)

A circuit is retained iff at some saved step its |score| is at least
`MIN_CONTRIBUTION` (0.1%) of its cycle partition's total |score| mass at that
step -- `rank_and_filter`'s rule, applied with full-universe denominators. The
final totals are not known until the pass is over, so the decision is made in
two parts:

1. **Pass** (every circuit): score it over its active window, add its mass into
   its partition's running total, and record
   `max_t |s(t)| / running_total(t)`. The running total only grows, so that
   ratio is an upper bound on the circuit's true peak share, and a circuit
   falling short of it is dropped without ever being scored again.
2. **Confirm** (only circuits whose bound clears the threshold): recompute the
   exact ratio against the final totals.

Two classes skip both tests. A circuit in a `NormGroup::Solo` group (no stock
resolves to a parent-level partition) is its own denominator, so "ever active"
is the whole test and the enumerator has already proved it. A module-traversing
circuit is kept unconditionally and banks **no** raw mass, because what it
reports is the per-exit-port override series rather than the raw product: the
raw product multiplies in the module COMPOSITE, which max-abs-selects across all
of the module's output ports, so the two series can differ by any factor. Its
reported mass joins the denominators after materialization instead.

The pass outputs the survivors, the per-partition per-step totals, and the
per-partition circuit COUNT over the whole universe (retention non-survivors
included). NaN handling comes from the finished product rather than from the
links, which is what keeps an `Inf * 0` step -- NaN with no NaN link anywhere --
out of the totals and unable to satisfy retention.

**The exactness boundary.** The confirm step is exact against the totals this
pass computes, which are not quite the totals the report is normalized against:
after materialization, `ltm_finding.rs` adds each module-traversing loop's
reported override mass and subtracts the mass of every duplicate representative
the reported-cycle dedup discards. Only the subtraction can move a non-module
circuit's outcome, and it can only LOWER a denominator -- so a circuit dropped
here for falling short against the pre-correction total could, against the
corrected one, have cleared the threshold, and having never been materialized it
is never reconsidered. The error is bounded by the dropped duplicates' share of
the partition's final mass, which is zero except in a partition holding a
hoisted-reducer duplicate pathway.

### Materialization and the two total corrections

Cross-agg stitching (GH #696, via `stitch_cross_agg_node_paths` -- the one
helper both generators' node paths go through) collects its petals from the
FULL enumerated set rather than from the retention survivors -- a petal can fail retention while the
stitched combination passes -- and its stitched sequences join the candidate set
(deduped against the survivors by canonical rotation). Survivors plus stitched sequences are then
materialized into `FoundLoop`s: links from the causal graph, the per-exit-port
module override series (GH #698, memoized per `(module-input source, module
instance, exit-port reader)`), the synthetic-agg trim, the exact per-step score
product, and runtime polarity.

Two distinct circuits can trim to the same *reported* loop (a direct reference
and its hoisted-reducer twin differ only in the synthetic agg node the report
hides). The dedup keeps the strongest representative, matching the composite
link-score rule (ref 6.3). The universe totals are then corrected so that each
distinct reported cycle contributes mass exactly once and by the series it
actually reports: a module-traversing loop's materialized override mass is
ADDED, and a dropped duplicate's raw mass is SUBTRACTED (along with its slot in
the partition's loop count). Every other circuit -- retention non-survivors
included -- keeps its raw enumerated product in the totals untouched.

### The shortest-path fallback (`ltm_finding_fallback.rs`)

When the enumeration cannot complete -- its budgets trip, or the caller's
wall-clock budget expires (GH #647) -- candidates come from a shortest-path
sweep instead. Its cost is `steps * seeds * E log V` with no cliff, so it is
bounded before the work starts and interruptible between searches; and what it
drops is characterizable rather than an artifact of traversal order, which is
the standing requirement on anything that stands in for the exact enumeration.

Per saved step `t in 1..step_count`: build that step's active adjacency and its
reverse, weight every edge, compute the step's SCCs (a cycle lives inside one
component, so each search is restricted to its seed's), then per seed run a
forward Dijkstra from the seed and -- under the default closure policy -- a
reverse Dijkstra into it. Both searches order routes on `(weight, hops)`.

The strategy is a `FallbackConfig` on four axes. Each default was settled by
`examples/ltm_fallback_eval` measuring recall against the exact enumeration on
World3 and C-LEARN, not by argument; the sweep tables are in the design plan's
"Measured" section.

- **Weight** (`FallbackWeight::{ClampedLogAbs, RelativeLinkScore, HopCount,
  ShiftedLogAbs}`, default `ClampedLogAbs`). Every arm must be non-negative,
  because Dijkstra's optimality argument needs it and a super-unit link (gain
  above 1 -- World3 carries 37-91 of them per step) is a NEGATIVE edge in raw
  `-ln` space where no feasible Johnson potentials exist (a negative cycle there
  is just a loop with gain > 1). `ClampedLogAbs` (`w = max(0, -ln|s|)`) is
  therefore an UPPER bound on the true cost: it discards a super-unit link's
  gain rather than expressing it, leaving a zero-weight plateau that the hop
  tie-break resolves. `RelativeLinkScore` (`w = -ln(|s| / sum of |s| over the
  target's active in-edges)`, ref 13.3) is non-negative without clamping.
  `HopCount` (`w = 1`) is the score-blind control the others have to beat.
  `ShiftedLogAbs` (`w = ln(step max finite |s|) - ln|s|`) keeps the gain the
  clamp discards; it was measured and REJECTED -- the per-hop `ln(max)` term
  dwarfs the product term on these models and the arm degenerates toward a hop
  count -- and stays selectable as a documented negative result.
- **Seeds** (`FallbackSeeds::{Stocks, StocksAndStocklessSccs, AllSccNodes}`,
  default `StocksAndStocklessSccs`). Every SD feedback loop contains a stock, but
  the runtime graph also carries cycles whose state hides in a module level or a
  `PREVIOUS` lag between two auxes; one extra seed per stockless non-trivial SCC
  reaches those. Seeding the whole cyclic core recovers a little more and costs
  more than the exact enumeration it stands in for, so it stays selectable and
  unused.
- **Closures** (`FallbackClosures::{SeedInEdges, EveryEdge}`, default
  `EveryEdge`). `SeedInEdges` closes only the seed's own in-edges, giving the
  minimum-weight elementary cycle through the seed -- cheap, and narrow, since
  one shortest-path tree holds one route per node and parallel routes collapse.
  `EveryEdge` closes every edge `u -> w` inside the seed's component whose
  source the forward tree reached and whose target the reverse tree reached,
  giving `path(seed..u) + (u -> w) + path(w..seed)`: the minimum-weight cycle
  through both the seed AND that edge, the strength-weighted analogue of edge
  coverage. It is the lever that earns its cost. A closure whose two tree halves
  share a node is not elementary and is SKIPPED rather than spliced -- a spliced
  walk is no longer the minimum-weight cycle through its edge, so it would not
  be the thing this policy claims to emit.
- **Tie-break** (`FallbackTieBreak::{Hops, NodeId}`, default `Hops`). Under the
  clamp's zero-weight plateau many routes tie exactly, and something has to
  decide: fewer hops is a statement about the model, lower node id is the
  measurement control. Measured recall-neutral on both models, so `Hops` is kept
  for the more meaningful tie at no measured cost.

Emitted cycles are deduped by a rotation-independent fingerprint over the
cycle's directed edge SET (an elementary cycle is determined by that set), with
bucket hits resolved by an exact rotation comparison -- so opposite-direction
cycles over one node set stay distinct loops (GH #308) and the duplicates, which
after the first few steps are nearly every candidate, cost no allocation. The
candidate volume is bounded at `MAX_FALLBACK_PATHS` (200,000, checked at every
dedup insert); a trip stops the sweep and reports `truncated`, the same signal a
deadline expiry gives, since both mean the sweep did not get to sample
everything it would have.

**What the sweep drops, stated:** cycles through no seed at all (which the seed
policy widens), and, for a given (seed, edge, step), every cycle but the
cheapest. So the recall ceiling is an OPTIMALITY restriction -- which cycle wins
the competition for a given seed and edge -- not a question of how much of the
graph got visited. On the `ClampedLogAbs` plateau many cycles tie at that
minimum and the sweep emits one per pair (the tie-break's choice) rather than
the whole tied set, which is the unmeasured lever a k-best-under-ties extension
would pull.

**Deadline sites:** the clock is read at exactly three bounded places -- once at
the top of each step, once before each seed's searches, and once per fixed pop
interval inside a search, so one seed whose component is most of the graph
cannot overrun the budget on its own. An unbudgeted sweep reads the clock
nowhere.

### The budget split

A caller's wall-clock `budget` is split by `ENUM_BUDGET_FRACTION` (0.5): the
enumeration path (`ActivityGraph::build`, `enumerate_active_circuits`,
`retain_circuits`) must finish within half of it, and the fallback then runs
against the caller's own expiry. The split exists because the two generators are
sequential and only the second yields partial results -- an undivided budget is
one the fallback never sees, spent entirely inside an enumeration that is then
discarded for being incomplete. Every phase of both checks the deadline at a
bounded interval; an unbudgeted call never reads the clock at all.

The budget bounds candidate GENERATION, not the call. Materializing candidates
into `FoundLoop`s and `rank_and_filter` both run to completion afterwards, so a
budgeted run can exceed its budget by that tail (about a quarter of World3's
discovery time) and still report `truncated == false`. Compilation and
simulation are outside it too.

### Ranking and Filtering (`rank_and_filter`)

Over the materialized loops:

1. **Normalization groups.** A loop normalizes against its cycle partition, or
   -- when its stocks resolve to no parent-level partition (a pure
   module-internal or `PREVIOUS`-lagged loop) -- against its own
   `NormGroup::Solo` group (GH #750). Unrelated unpartitioned loops must not
   share a denominator or count as each other's competition.
2. **Denominators.** On the enumeration path the per-partition per-step totals
   are the universe's (`UniverseStats::totals`, corrected as above): a retention
   non-survivor's mass is still in the denominator, matching exhaustive mode,
   where the enumerated set IS the universe. On the fallback path there is no
   universe to measure against, so the discovered set supplies its own totals.
   `NaN` summands are excluded and `Inf` kept, mirroring
   `ltm_post::denom_summand`.
3. **Retention filter**, peak semantics: keep a loop if at ANY single step its
   |score| is >= `MIN_CONTRIBUTION` (0.1%) of its group's total there. This runs
   BEFORE any cap (GH #310), so a loop dominant in a small partition but
   globally low-magnitude is no longer lost to a truncate-before-filter.
   `DiscoveryResult::retained_loops` reports how many survived, read before the
   cap.
4. **Competing-vs-solo classification.** On the enumeration path a partition is
   competing iff its UNIVERSE circuit count is >= 2, however many loops survived
   retention or the cap -- sound precisely because every enumerated circuit is
   ever-active by construction, so a co-member cannot be a phantom. On the
   fallback path the discovered set is the only population there is.
5. **Rank competitive-first** by mean |relative loop score| over each loop's
   active steps (the literature's loop-inclusion measure, ref 13.3, GH #543),
   with raw `avg_abs_score` breaking exact ties and a content key breaking
   those. Loops trivially ALONE in their group come after ALL competing loops:
   a solo loop's relative score is exactly +/-1 at every active step *by
   construction*, so its perfect mean carries zero discriminative information,
   and on real models dozens of isolated two-variable decay loops would
   otherwise pin the top of the ranking. Never-active (`NaN`) loops sort last.
   The mean is taken over active steps only ("delayed averaging", ref 13.3), so
   a briefly-dominant loop is not penalized for the steps it sleeps through --
   which is the loop the retention filter exists to keep.
6. **Coverage-aware cap** (`select_reported`). Under `MAX_LOOPS` (200) pressure
   membership is not a plain truncation. Every loop that is, at some step, the
   |relative score| maximum within a COMPETING group is an **anchor** and keeps
   its slot (`anchor_ranks`, `k = 1`), unconditionally -- so a
   dominance-over-time reading never names the wrong loop for a step. `k` may
   then rise to cover runners-up, bounded by `MAX_ANCHOR_K` (3) and taken only
   while the enlarged anchor set stays at or under `ANCHOR_SHARE_OF_CAP` (one
   half) of the cap, so the coverage guarantee can deepen but can never crowd
   the ordinary ranking below half of a capped report. Remaining slots are
   filled in ranking order. In the pathological case where the `k = 1` anchors
   alone exceed the cap, the cap applies to the anchors, in ranking order: a
   loop that dominates no step is a worse answer to "what drove this step" than
   one that dominates a different step. Solo loops never anchor and are dropped
   first. Presentation order is unchanged by any of this -- only membership.
7. **IDs and partition metadata.** Deterministic polarity-based ids (`r1`,
   `b1`, `u1`) are assigned in a content-key visitation order, so they do not
   depend on discovery order. Each reported loop then carries a result-scoped
   dense `FoundLoop::partition` index into `DiscoveryResult::partitions`
   (per partition: element-level stock names and returned-loop count), in
   first-appearance order over the final list. Those indices are result-scoped:
   the underlying SCC numbering renumbers when stocks are added or renamed, so a
   consumer needing durable identity keys on the stock-name set. The metadata
   flows through `analysis::ModelAnalysis::partitions`, the FFI
   `SimlinDiscoveredPartition`, and pysimlin's `Analysis.partitions` /
   `Loop.partition`, so callers can group loops by feedback subsystem --
   importance is only comparable within a partition.

### What a caller can tell about completeness

`DiscoveryResult` reports five things, each answering a different question:

| field | question |
|---|---|
| `enumeration_complete` | Did the exact generator run AND finish? `false` means the candidates are the fallback's sample. A budget trip and a deadline expiry are deliberately not distinguished: the report is equally a sample either way. |
| `universe_loops` | How many circuits the enumerated universe held. `Some` exactly when `enumeration_complete` (stitched cross-agg loops are combinations of circuits rather than circuits, so they are not counted). |
| `fallback_candidates` | How many distinct cycles the sweep proposed, after dedup and before retention. `Some` exactly when the fallback ran -- the mirror of `universe_loops`. |
| `retained_loops` | How many loops passed retention BEFORE the `MAX_LOOPS` cap: the only way to tell a capped prefix from the whole retained set. |
| `truncated` | Whether the fallback sweep stopped early (deadline or candidate budget). Its loops cover only the steps it processed; the per-step series of the loops it DID find are complete, since each loop is rescored over every step once its path is known. |
| `agg_recovery_truncated` | Whether cross-agg petal stitching hit its loop-count budget, so some reducer loops are absent (GH #696) -- structurally incomplete, distinct from the wall-clock signal. |

Five of the six reach `analysis::ModelAnalysis`, and from there libsimlin,
pysimlin, and the MCP read/edit outputs (`enumerationComplete` is always on the
MCP wire, since its interesting value is `false` and an elided field would leave
a client unable to tell an exact analysis from a sampled one).
`fallback_candidates` stays engine-internal: it exists so the two generators'
candidate VOLUMES are directly comparable in `examples/ltm_discovery_bench` and
`examples/ltm_fallback_eval`, which is a measurement question rather than a
caller's.

### Measured shape

Both figures below are release builds of `examples/ltm_discovery_bench` on the
two models that drive this design; the design plan's "Measured" section is the
ledger with the per-phase breakdowns and the fallback sweep tables.

- **C-LEARN v77** (911 variables, ~26k LTM variables, 251 saved steps, 116
  stocks): a union graph of ~3,000 edges holding **162** ever-simultaneously-
  active cycles. Discovery completes in ~0.04 s, of which the phases *before*
  candidate generation -- `parse_link_offsets` and the topology build -- are the
  dominant cost; enumeration, retention, materialization and ranking together
  are under 3 ms. 153 loops are reported and the cap does not bind.
- **World3-03** (401 saved steps, 15 stocks): a union graph of 258 edges over
  one 135-node SCC holding **150,827** ever-simultaneously-active cycles.
  Discovery completes in ~0.4 s, dominated by enumeration, retention, and
  `FoundLoop` materialization; 2,979 circuits survive retention and the cap
  reports 200 of them.

The gap between the two is this design's thesis as a measurement: a
shortest-path sample recovers most of what matters on a sparse runtime graph
(C-LEARN: every step's dominant loop, and nearly the whole exact ranking) and
almost none of the exact ranking on a dense one (World3). That is what
`enumeration_complete == false` exists to say.

An independent pure-Python re-implementation of the enumerator, the scoring, the
retention filter, and the ranking -- written from the design plan rather than
translated from the Rust -- reproduces both models exactly, universe count,
survivor count, reported list, and score series bit for bit
(`notebooks/build_ltm_discovery_audit.py`, regenerable, gitignored output).

### Honest boundaries

Completeness is a claim about the *recorded* series at *saved-step* resolution,
and three things sit outside it:

- **Sub-save-step activity is invisible.** Discovery samples at `save_step`
  rather than at every dt (GH #309, divergence 1 below), so a loop active only
  between save points is never sampled. A "baton-passing" loop whose links are
  each active over time but never all simultaneously active at a *saved* step is
  likewise invisible to both generators, even though exhaustive mode reports it
  (GH #699; `discovery_decoupled_stocks` demonstrates it on
  `test/decoupled_stocks`). This is shared with the published per-step method.
  Read "loses nothing" as "loses no loop that is ever simultaneously active at a
  sampled step".
- **A module-input edge's activity is read from the module composite.** The
  composite max-abs-folds over the module's pathways as
  `if ABS(a) >= ABS(b) then a else b`, and every comparison against NaN is
  false, so a NaN pathway in the `b` position wins and the edge reads NaN --
  inactive -- even when a finite pathway exists. A loop through such an edge is
  then absent from the union graph, although the per-exit-port series it would
  have reported is finite. The zero case is safe in the other direction: a
  composite that is 0 at every step does imply every per-port score is 0.
- **Stockless cycles are kept.** A 2+-node cycle carrying no stock -- state
  hidden in a module level, or a `PREVIOUS` lag between two auxes -- is real
  feedback and is reported, in its own `NormGroup::Solo` group ranked after
  every competing loop. It never anchors under the cap, because a Solo loop's
  relative score is +/-1 by construction and anchoring it would guarantee a slot
  to the one class of loop that carries no comparative information. Neither
  model in the corpus currently produces one (0 of C-LEARN's 153 and 0 of
  World3's 200 reported loops have an empty stock list, under either generator,
  as `examples/ltm_discovery_bench` prints), so the rule costs those two models
  nothing and exists for the models that do; the fallback's default seed policy
  (`StocksAndStocklessSccs`) is what keeps such a cycle reachable when the
  enumeration cannot run, since no stock-seeded search can reach it.

## Array Support

LTM extends to arrayed (subscripted) variables by operating on an element-level
causal graph. Variable-level edges are expanded to element-level edges, loops
are detected at element granularity, and link/loop scores are generated per
element.

### The reference-site classification IR

`model_ltm_reference_sites` (salsa tracked, `db/ltm_ir.rs`) is the single
place a causal edge's access shape *and* aggregate-node routing are decided.
It walks each variable's `Expr2` AST exactly once, consults
`enumerate_agg_nodes` (the sole "is this subexpression a hoistable maximal
reducer" decider), and buckets every `Expr2::Var` / `Expr2::Subscript`
reference by its `(from, to)` causal edge into a `Vec<ClassifiedSite>`. Each
`ClassifiedSite` carries:

- `shape: RefShape` -- `Bare`, `FixedIndex(elems)`, `Wildcard`, or
  `DynamicIndex` (the AST-walker helpers `classify_subscript_shape` /
  `resolve_literal_index` / `classify_iterated_dim_shape` live in
  `db/ltm_ir.rs`);
- `target_element: Option<String>` -- set when the reference is inside an
  `Ast::Arrayed` per-element expression, pinning the target node set to that
  one element tuple;
- `routing: SiteRouting` -- `Direct` or `ThroughAgg { agg }`. A reference is
  `ThroughAgg` iff it is syntactically inside a hoisted reducer *and* a
  synthetic agg of `to` reads `from` (the `route_through_agg =
  !routed_aggs.is_empty() && in_reducer` decision and the
  `aggs_in_var(to).filter(is_synthetic && reads-from)` filter exist here and
  nowhere else).

`model_element_causal_edges`, `model_edge_shapes`, and `model_ltm_variables`
are pure readers of this IR -- none re-walks the AST for shape/routing, none
restates the agg-routing filter.

### Element-Level Causal Graph

`model_element_causal_edges` (salsa tracked, `db/analysis.rs`) builds the
element-level graph by reading the IR's classified sites for each
variable-level edge and emitting one or more element edges per site. A
`Direct` site uses its `shape` / `target_element` via
`emit_edges_for_reference`; a `ThroughAgg` site routes only the rows the
reducer's `read_slice` reads through the synthetic agg via
`emit_agg_routed_edges` (see "Aggregate Nodes"). The shape/routing truth
table for `Direct` sites:

| Source dims | Target dims | RefShape | Edges emitted |
|-------------|-------------|----------|---------------|
| scalar | scalar | Bare | `from -> to` |
| scalar | arrayed | Bare | `from -> to[d]` for each target element d |
| arrayed | scalar | Bare | `from[d] -> to` for each source element d (reduction) |
| arrayed | arrayed (same dims) | Bare | `from[d] -> to[d]` per shared element (diagonal) |
| arrayed | arrayed (partial collapse) | Bare | `from[d1,d2] -> to[d1]` (delegates to `expand_same_element`) |
| arrayed | scalar | FixedIndex(elems) | `from[elems] -> to` (one edge) |
| arrayed | arrayed | FixedIndex(elems) | `from[elems] -> to[d]` for each target element d |
| arrayed | any | Wildcard / DynamicIndex | conservative full cross-product (N×M) |

`Wildcard` covers a subscript with at least one `Wildcard` index, or all
indices `Wildcard` / `StarRange` (the reducer-style whole-extent access);
`DynamicIndex` covers any other non-literal index (`@N`, `Range`, an
arbitrary `Expr`, a *partial* `StarRange` mixed with literals) -- *and* the
not-hoistable dynamic-index reducer carve-out `SUM(pop[idx, *])`, which the
IR reclassifies from `Wildcard` to `DynamicIndex` so a `Direct` site that
*could* have been a hoisted reducer never falls through to the conservative
cross-product. So a `Direct` `Wildcard` site is now only a *whole-RHS*
variable-backed reducer's argument (`total = SUM(population[*])`,
`row_sum[D1] = SUM(matrix[D1, *])`), and the conservative cross-product is
the right semantics for it.

**Iterated-dimension subscripts** (#511). An explicit subscript whose
indices are *exactly* the target equation's iterated (apply-to-all)
dimensions, in the position matching the source's declared dimension order --
`row_sum[Region]` inside `growth[Region, Age] = ... + row_sum[Region] * c`,
where each index `d_i` either names the source's `i`-th dim or a dimension
that *maps* to it (the AC3.5 mapped case) -- classifies as `Bare`, not
`DynamicIndex`. Such a reference reads the *same* `Region` element of
`row_sum` per iterated tuple, so `emit_edges_for_reference` projects it via
`expand_same_element` (`row_sum[d1] -> growth[d1, d2]` for each `d2`), not
the N×M cross-product. (A *sliced reducer argument* with the same shape --
`SUM(matrix[D1, *])` inside an A2A body over `D1` -- is a different path: it
is hoisted into an arrayed agg by `enumerate_agg_nodes`, so its reference is
`ThroughAgg` and its `Wildcard` shape is ignored. The iterated-dim `Bare`
branch is for a *whole-equation*-iterated subscript like `x[State]` inside
`target[State] = x[State] * c`.)

**Aggregate-node reroute.** A reference inside a *maximal inlined reducer
subexpression* is not expanded as an all-pairs cross-product. The IR records
it as `ThroughAgg`, and `model_element_causal_edges` routes only the rows the
reducer's `read_slice` reads through the synthetic agg node:
`source[<read slice>] → $⁚ltm⁚agg⁚{n}[<iterated>]` then `$⁚ltm⁚agg⁚{n}[<iterated>] → target[e]`,
so the per-reducer cost is O(N + M) edges (a whole-extent reduce degenerates
to "every source element → scalar agg → every target element"). A
positionally-MAPPED sliced reducer (`SUM(matrix[State, *])` over
`matrix[Region, D2]` with a positional `State→Region` mapping, GH #534) is
hoisted too: the `Iterated` axis carries the (target, source) dimension
pair, the agg is arrayed over the TARGET dim (`State`), and each source row
is remapped to the slot of its positionally-corresponding target element
(`iterated_axis_slot_elements` -- the preimage of
`positional_correspondence`, which is the right rule here because
`matrix[State, *]` names the dimension the equation ITERATES and execution
folds that to an ordinal; an explicit element map is therefore honoured as a
DECLARED correspondence but not READ, GH #997). The only reducers *not*
hoisted are the dynamic-index carve-out (`SUM(pop[idx, *])`, `idx`
non-literal -- not statically describable, reclassified `DynamicIndex`), a
pair with no declared correspondence at all, and a `MappedRead` axis
(`SUM(matrix[Region, *])` naming a NON-iterated dimension, GH #997: its
executed rule admits a many-to-one correspondence that the one-slot-per-row
remap cannot express, so `compute_read_slice` declines it) -- all of which
keep the conservative cross-product; a bare non-literal index
(`arr[i+1]`) is a dynamic reference, not a reducer, so it stays conservative.
Variable-backed aggs (`total_population = SUM(population[*])`) are already
real nodes -- their edges come from the normal arrayed→scalar /
scalar→arrayed reference walker -- so they are not rerouted.

Edges from multiple reference sites in the same target are unioned. For
`relative_pop[R] = population / population[NYC]`, the bare numerator emits
diagonal edges `population[d] -> relative_pop[d]` and the fixed-index
denominator emits broadcast edges `population[NYC] -> relative_pop[d]` --
2N - 1 unique edges, not N^2. For `share[R] = pop / SUM(pop[*])`, the bare
numerator emits the N diagonals `pop[d] -> share[d]` and the hoisted
`SUM(pop[*])` reducer emits the N `pop[d] -> $⁚ltm⁚agg⁚0` edges plus the N
`$⁚ltm⁚agg⁚0 -> share[d]` edges -- 3N edges, not N + N² (and as the source
dimension grows relative to the target's, or as more consumers share the
reducer, the gap widens: an 8-region `share` model goes from 80 element edges
to 40). A sliced reducer narrows further still: `target[Region] = SUM(pop[NYC, *])`
over `pop[Region, Age]` routes only the `Age`-many NYC rows through the agg
(`pop[nyc, adult] → agg`, `pop[nyc, child] → agg`, `agg → target[r]` for each
r), not every `pop` element.

Structural flow-to-stock edges (an inflow or outflow's variable name does
not appear in the stock's equation, which holds only the initial value) are
emitted as same-element diagonals without consulting the IR. An edge with no
IR entry (a module edge, an unreconstructable target, a synthesized dep with
no AST reference) falls back to a same-element diagonal `Bare` emission so
the variable-level projection invariant still holds.

Stock names are similarly expanded: `population` with dimension `Region`
becomes `population[NYC]`, `population[Boston]`, etc. When no variables in a
model are arrayed, the element graph is identical to the variable graph (zero
overhead).

This per-reference design replaces the earlier `ElementDependencyKind`
classifier that collapsed every reference between a `(from, to)` pair to a
single kind. That collapse over-expanded fixed-index references to N^2 edges
(resolving tech-debt #20) and forced the link-score partial equation to wrap
every reference uniformly in `PREVIOUS()`, breaking targets that mixed bare
and reducer references (resolving tech-debt #26). Reducer references went
through a brief intermediate stage -- a per-shape
`$⁚ltm⁚link_score⁚{from}→{to}⁚wildcard` / `…⁚dynamic` variant -- which the
aggregate-node treatment then made obsolete and retired: the lumped reducer
link score is decomposed into the chain `source[d] → $⁚ltm⁚agg⁚{n} → target`,
each link of which has a real per-element score (see "Aggregate Nodes"). The
post-refactor measurements in
`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` show that the
element-graph SCC sizes that previously drove tech-debt #25's auto-flip
pressure on FixedIndex models are no longer inflated by spurious edges,
though `MAX_LTM_SCC_NODES = 50` was retained because WRLD3-class models trip
the gate from variable-level cycle structure rather than element-graph
artifacts.

### Aggregate Nodes

An *aggregate node* is the conceptual stand-in for an inlined array-reducer
subexpression, mirroring how the LTM papers handle macros like `DELAY3` and
`SMOOTH`: the aggregation has hidden internal structure, so causality is
routed *through* it rather than scored as one lumped link.

`enumerate_agg_nodes` (salsa-tracked, `ltm_agg.rs`) walks every variable's
`Expr2` AST left-to-right depth-first and identifies each maximal reducer
subexpression. The recognized set -- `SUM`, `MEAN` (single-arg), `MIN` /
`MAX` (single-arg), `STDDEV`, `RANK`, `SIZE` -- and its `Linear` / `Nonlinear`
/ `Constant` classification live in one table, `reducer_kind` /
`ReducerKind` in `ltm_agg.rs`; every other reducer-recognition site in the
LTM machinery (the Expr0-walk-time `is_array_reducer_name`, `classify_reducer`,
the static-polarity `agg_reducer_is_monotone`) is a thin reader of it, so the
"is this a reducer" / "what kind" answers can't drift apart. AST-identical
subexpressions (keyed by canonical printed equation text, since `Expr2` is
not `Hash` and so cannot key a map directly) dedupe to one node.

**Read slice and result dims.** Each `AggNode` carries a
`read_slice: Vec<AxisRead>` -- one `AxisRead ∈ {Pinned(elem), Iterated(dim),
Reduced}` per source axis, describing *which rows of the arrayed source the
reducer actually reads* -- and a `result_dims`, the `Iterated` axes' dims (in
order; empty for a whole-extent or pinned-slice reduce, since the result is a
scalar). `compute_read_slice` decides hoistability per axis:

- `*` / `*:Dim` ⇒ `Reduced` (the whole axis is reduced away);
- an iterated-dimension index that names the source's `i`-th dim by name, or
  a positionally-MAPPED iterated dim (`State` over a `Region` source axis
  with a positional `State→Region` mapping, GH #534) ⇒
  `Iterated{dim, source_dim}` (the agg's result varies per element of the
  TARGET dim `dim`; `dim == source_dim` for the literal case);
- a literal element name / 1-based integer ⇒ `Pinned(elem)`;
- anything else (`@N`, `Range`, a non-literal `Expr`, an iterated dim whose
  mapping is element-mapped, reverse-declared, or non-positional) ⇒ `None`
  -- the reducer is not statically describable, so it is not hoisted.

So `SUM(pop[*])` ⇒ all-`Reduced`, `result_dims = []` (a scalar agg);
`SUM(pop[NYC, *])` over `pop[Region, Age]` ⇒ `[Pinned(nyc), Reduced]`,
`result_dims = []`; `SUM(matrix[D1, *])` inside an A2A body over `D1` ⇒
`[Iterated(d1,d1), Reduced]`, `result_dims = [D1]` (an *arrayed* agg, one
slot per `D1` element); `SUM(matrix3d[D1, NYC, *])` over an A2A-`D1` body ⇒
`[Iterated(d1,d1), Pinned(nyc), Reduced]`; `SUM(matrix[State, *])` over
`matrix[Region, D2]` inside an A2A body over `State` (positional
`State→Region` mapping) ⇒ `[Iterated{state, region}, Reduced]`,
`result_dims = [State]` -- the agg is arrayed over the TARGET's iterated
dim, and the emitters remap each source row to the slot of its
positionally-corresponding target element (`iterated_axis_slot_elements`,
the preimage inversion of `positional_correspondence`, the rule the ITERATED
spelling gets). The carve-outs (tracked tech debt;
the conservative cross-product / coarse link score stays in place) are: a
reducer over a *dynamic index* (`SUM(pop[idx, *])`, `idx` non-literal -- the
IR reclassifies its reference to `DynamicIndex`); a mapped sliced reducer
the correspondence declines -- a pair with no declared correspondence, or a
`MappedRead` axis whose executed rule the slot remap cannot invert
(GH #997) -- or a mapping declared
only in the reverse direction (on the source's dimension; GH #757 tracks
that direction's classification); and a multi-source reducer whose arrayed
args read incompatible slices (`combined_read_slice` returns `None` on
disagreement -- a multi-source reducer whose args *agree*, `SUM(a[*] +
b[*])` over the same dim, mints one agg carrying the combined slice and
both source variables).

Two kinds of agg:

- **Synthetic** (`is_synthetic == true`): the reducer is a *sub-expression* of
  a larger equation (`share[r] = pop[r] / SUM(pop[*])`). A `$⁚ltm⁚agg⁚{n}`
  auxiliary is minted whose dt-equation is exactly the reducer (arrayed over
  `result_dims` when those are non-empty). `model_ltm_variables` emits the aux
  plus two link-score families:
  - `source[<read row>] → $⁚ltm⁚agg⁚{n}` -- one scalar
    `$⁚ltm⁚link_score⁚{from}[<row>]→{agg}` (or `…→{agg}[<slot>]` when the agg
    is arrayed) per *read* row -- only the rows the slice reads. The agg's
    *own* equation is the reducer, so the `Linear` / `Nonlinear` / `Constant`
    classification applies directly (varying that row moves the agg by exactly
    its own co-reduced delta regardless of what else the reducer combines).
  - `$⁚ltm⁚agg⁚{n} → target` -- the partial of `target`'s equation with `agg`
    held live, with every hoisted reducer subexpression in `target` first
    textually substituted by its agg name (so `agg` appears where `SUM(...)`
    was, and any other hoisted reducer becomes `PREVIOUS(agg_j)`). For an
    arrayed `target` this is one scalar `$⁚ltm⁚link_score⁚{agg}→{to}[{e}]` per
    target element; for a scalar `target`, a single `$⁚ltm⁚link_score⁚{agg}→{to}`.
    When the agg is itself arrayed, the agg side carries an `[<slot>]`
    subscript -- the target element's *projection* onto the agg's
    `result_dims` axes -- on the link-score name, on the `Δsource`
    denominator (the bare multi-slot agg name doesn't compile as a scalar
    denominator), *and* on the agg's pinned references in the equation body
    (GH #528). For the diagonal case (`result_dims` equal `target`'s iterated
    dims) the projection is the full target tuple; for the strict-prefix
    *broadcast* case (`SUM(matrix[D1, *])` inside an A2A body over `D1 × D2`,
    so the agg is over `D1` but the target is over `D1 × D2`) it drops the
    broadcast axes -- pinning by the full tuple instead would over-subscribe
    the 1-D agg, fail fragment compilation, and stub the score (and every
    loop through the agg) to 0.

  A loop running through the inlined reducer therefore traverses
  `… → from[<row>] → $⁚ltm⁚agg⁚{n}[<slot>] → to[e] → …`, and the loop-score
  equation composes the two halves by the chain rule -- recovering each source
  row's fractional contribution to the aggregate's velocity, exactly the
  factor that matters when elements have very different magnitudes. **Model
  equations are not rewritten**; the simulation evaluates the inline reducer,
  and the agg aux evaluates to the same value. A *scalar* feeder of a (possibly
  arrayed) hoisted reducer -- `scale` in `growth[D1] = SUM(matrix[D1, *] * scale)`
  -- is handled by `emit_agg_routed_edges`: `from_dims.is_empty()` ⇒ emit
  `from → agg[<each result-dim combo>]` (or the bare `from → agg` when the agg
  is scalar) and a bare element-graph node for `from`, not the malformed
  `from[]` node the row-layout machinery would mint (GH #533 for the both-scalar
  fast-path edge case).

- **Variable-backed** (`is_synthetic == false`): the reducer is the *entire*
  dt-equation of a scalar or apply-to-all variable (`total_population = SUM(pop[*])`,
  `row_sum[D1] = SUM(matrix[D1, *])`). That variable *is* the aggregate node;
  no synthetic is minted, and its edges to/from come from the normal
  arrayed→scalar / scalar→arrayed reference walker -- the element-graph reroute
  leaves the conservative cross-product in place for the variable-backed
  reducer's edge, since the edges to a real variable node already exist. One
  exception (GH #534): a whole-RHS reducer with a MAPPED iterated axis
  (`out[State] = SUM(matrix[State, *])` over a positionally-mapped pair)
  mints a *synthetic* agg instead -- the variable-backed link-score path
  (`try_cross_dimensional_link_scores`' partial-reduce arm) matches result
  axes against source axes by name, so a remapped pair falls off it onto the
  per-shape `Wildcard` partial, whose PREVIOUS-wrapping mangles the iterated
  index into the non-compiling `matrix[PREVIOUS(state), *]` (a
  silently-stubbed constant-0 score). Routing through a synthetic agg gives
  the whole-RHS case the same remapped two-half scoring as an inline mapped
  reducer.

**Loop reporting trims agg nodes.** `$⁚ltm⁚agg⁚{n}` nodes don't appear in the
user-facing loop list -- like the internal stocks of `DELAY3`/`SMOOTH` in the
papers, they're machinery, not a variable the modeler authored. The discovery
and exhaustive paths report each `FoundLoop` / `Loop` with the synthetic agg
nodes trimmed out of the node sequence (the loop-score equation, however, is
the product of the *un-trimmed* link-score chain, so the agg's two halves are
both factored in). This resolves GH #503: a cross-element loop through a
reducer is no longer normalized by the wrong (diagonal A2A) link score; the
denominator is naturally Δ(aggregate).

### Link Score Classification

Categories of element-level link scores:

**A2A same-dimension** and **scalar-to-arrayed (per element)**: For an A2A
edge, the standard ceteris-paribus equation is generated once with dimensions
on the `LtmSyntheticVar`; the simulation engine evaluates it per element via
A2A expansion (one variable, N slots). For a scalar-source → arrayed-target
edge, one *scalar* `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` is emitted per target
element (the element rides on the `to` side); a single Bare-A2A variable would
be undiscoverable because the discovery parser would invent a `{from}[{elem}]`
node that doesn't match the scalar source's bare node.

**Arrayed-to-scalar (cross-dimensional / whole-RHS reducer)**: When an arrayed
source feeds a scalar (or partially-collapsed) target through a reducing
function that is the target's *entire* equation -- the variable-backed
aggregate-node case -- each source element gets its own scalar link score.
`classify_reducer` (a thin reader of `ltm_agg::reducer_kind`) walks the
target's AST to find the reducing builtin and classify it; `is_bare` tracks
whether the reducer is the whole RHS or nested inside arithmetic (a nested
reducer falls back to the delta-ratio, since the algebraic shortcut ignores
the surrounding arithmetic):

| Reducer kind | Functions | Equation strategy (`is_bare`) |
|-------------|-----------|-------------------|
| Linear | SUM, MEAN | Algebraic shortcut: partial = `PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))` (divided by N for MEAN) |
| Nonlinear | MIN, MAX | Nested binary calls: reconstruct the reducer with every element except the current one wrapped in `PREVIOUS()` (`MIN(s[d], MIN(PREVIOUS(s[e]), ...))`) |
| Nonlinear | STDDEV | Analytic ceteris-paribus partial (#483): the unrolled population-variance `sqrt` formula -- `sqrt((Σ_i (s'_i - m)^2) / N)` with `s'_i = s[d]` when `i == d` else `PREVIOUS(s[i])`, `m = (Σ_i s'_i) / N` string-inlined -- matching the engine's STDDEV (divisor `N`, not `N-1`; `vm.rs::Opcode::ArrayStddev`). Single-element variance is identically 0. |
| Nonlinear | RANK | Documented delta-ratio stand-in: the partial is `target` directly, so the surrounding link-score formula degenerates to `|Δtarget/Δtarget|`. RANK is an order statistic -- non-differentiable, array-argument-only, and unreachable as a real scalar/A2A reducer RHS (RANK returns an array -- a dimension error) -- so the delta-ratio is the conservative answer, pinned by `test_generate_rank_keeps_delta_ratio` so the choice is explicit, not a silent fallback. |
| Constant | SIZE | Output depends only on dimension cardinality; link score is always 0 |

`generate_element_to_scalar_equation` produces N separate scalar link score
variables (one per source element), each with its own equation isolating that
element's contribution. (Arrayed-result reducers -- `agg[D1] = SUM(matrix[D1,*])`
-- are supported too: each `(target_element, source_slice)` pair gets a scalar
partial-reduce link score `$⁚ltm⁚link_score⁚{from}[{d1,d2}]→{to}[{d1}]`.)

**Inlined reducer (synthetic aggregate node)**: When the reducer is a
*sub-expression* of a larger equation, the link from the array elements to the
consumer is *not* one lumped score. The reducer is hoisted into `$⁚ltm⁚agg⁚{n}`
and the link is the chain `source[<read row>] → $⁚ltm⁚agg⁚{n} → target` -- the
`source → agg` half uses the same `classify_reducer` machinery over the row's
co-reduced slice (the agg's equation *is* the reducer), and the `agg → target`
half is a plain Bare partial of `target`'s equation with the reducer subexpr
AST-substituted by the agg name. See "Aggregate Nodes" above.

**FixedIndex (per source element)**: A literal-index reference `from[NYC]`
inside `target` gets its own scalar `$⁚ltm⁚link_score⁚{from}[{nyc}]→{to}` (one
per literal element referenced, expanding to the target's dims if the target is
arrayed); the partial holds `from[nyc]` live and wraps the rest in `PREVIOUS`.

**Disjoint-dimension arrayed → arrayed (per source element)** (#510): When an
arrayed *per-element-equation* target (`Ast::Arrayed`) references an arrayed
source by literal element subscripts of a dimension *disjoint* from the
target's -- `target[D1, D2]` whose `<element subscript>` equations reference
`source[m]`, `m ∈ D3`, D3 sharing no dimension with D1/D2 --
`try_disjoint_dim_arrayed_link_scores` (called from `emit_link_scores_for_edge`
before the per-shape fallback) reuses the reference-site IR for `(from, to)`
(each site's shape is `FixedIndex(elems)` for `source[m]`) and emits one
`$⁚ltm⁚link_score⁚{from}[{m}]→{to}` per distinct referenced source element --
an `Equation::Arrayed` over `to`'s dims that holds `source[m]` live in the
slots that reference it and freezes it at `PREVIOUS` elsewhere. (The pre-#510
path silently collapsed the per-element `Equation::Arrayed` to the first
slot's text, since `link_score_dimensions` returned `[]` for the disjoint
edge.) If the target references the source via a *non-literal* index (a
`DynamicIndex` site) the edge is not statically scoreable:
`emit_unscoreable_disjoint_edge_warning` accumulates a `CompilationDiagnostic`
`Warning` naming the edge, *no* link-score variable is emitted, and the caller
does not fall through to the per-shape fallback (which would build the
misleading scalarized stand-in).

### Loop Scores

Tiered loop enumeration (`model_loop_circuits_tiered`) classifies each
variable-level cycle into one of three categories before deciding whether
element-level enumeration is needed:

- **PureScalar / PureSameElementA2A**: every traversed edge has only `Bare`
  references and every variable in the cycle is either uniformly scalar or
  uniformly arrayed over the same dimension list. The cycle materializes
  directly into a single `Loop` (with `dimensions` populated for the A2A
  case) without entering the element-level enumerator. This is the fast
  path; cost is O(K) per cycle of size K rather than O(K * N) on N
  elements.
- **CrossElementOrMixed**: any edge has a `Wildcard`, `FixedIndex`, or
  `DynamicIndex` reference, or the cycle mixes scalar and arrayed nodes,
  or the arrayed nodes don't share a dimension list. These cycles drive
  the slow-path subgraph: the element graph restricted to the variables
  participating in such cycles, *with synthetic `$⁚ltm⁚agg⁚{n}` nodes
  kept* (a cross-element loop through a hoisted reducer genuinely traverses
  the agg, so dropping it would hide the loop). Johnson runs on this
  restricted subgraph, and the results flow through the same per-circuit
  grouping logic the legacy `build_element_level_loops` uses.

Slow-path element-level circuits are grouped by their variable-level node
sequence (strip subscripts, join) to distinguish A2A loops from mixed loops:

**A2A loops**: All circuits in a group have the same variable-level structure
and every node carries a subscript. These are collapsed into a single `Loop`
with a shared ID (e.g., `r1`), `dimensions` populated from the underlying
variables, and `stocks` populated at *element* granularity (#487) -- the A2A
loop's stock set is the element-subscripted stocks it actually traverses, not
the variable-level stocks. Loop score equations are generated with those
dimensions, producing N result slots (one per element) with per-element
dominance profiles. The loop-id → cycle-partition mapping is cached as
`LtmVariablesResult::loop_partitions: HashMap<String, Vec<Option<usize>>>` --
*per slot* of an A2A loop, since two elements of the same A2A loop can land in
different cycle partitions (the slot's stocks differ). Relative loop scores are
derived post-simulation by `compute_rel_loop_scores` consumers (e.g.
`libsimlin::analysis`), normalizing each `(partition, slot)` loop score against
the sum of absolute scores in that partition at that slot -- so an independent
A2A loop's normalization no longer cross-pollutes a sibling A2A loop that
happens to share a loop ID but lives in a different partition.

**Cross-element / mixed loops**: Circuits containing scalar nodes or with
inconsistent variable-level structures. Each circuit becomes its own scalar
`Loop` with a unique ID. A loop that genuinely visits distinct elements
(`pop[nyc] → mp[boston] → mi[nyc] → pop[nyc]`) keeps the element subscripts on
its `Link.from` / `Link.to` strings, and `classify_cycle` /
`build_element_level_loops` produce a loop-score equation that references the
*subscripted* link scores along the actual path
(`"$⁚ltm⁚link_score⁚{from}→{to}"[e]` for a per-element slot of an A2A link
score, or the per-element scalar `$⁚ltm⁚link_score⁚{from}[{e}]→{to}` /
`$⁚ltm⁚link_score⁚{from}→{to}[{e}]` form) -- not the diagonal A2A scores the
loop doesn't visit. A loop running through an inlined reducer traverses the
synthetic agg node (`… → from[<row>] → $⁚ltm⁚agg⁚{n}[<slot>] → to[e] → …`);
the agg is trimmed from the *reported* node sequence but its two link-score
halves are factored into the loop score (see "Aggregate Nodes").

**Cross-agg loop recovery** (#515 exhaustive, #696 discovery). A cross-element
feedback loop *through* an inlined reducer visits the (subscript-free, or for an
arrayed agg `[<slot>]`-subscripted) agg node more than once, so neither Johnson
(exhaustive) nor either discovery candidate generator emits it directly -- all
of them produce only elementary circuits. The recovery is shared: the combinatorial
core `stitch_cross_agg_petals` reconstructs the loop from the agg-touching
elementary "petals" (`agg → … → agg`), stitching each pairwise-disjoint petal
subset of size ≥ 2 into ONE canonical loop -- the chosen petals concatenated
in priority order (GH #676). One loop per subset is exact for dominance
analysis: every cyclic ordering of a fixed subset traverses the same edge
multiset (each petal contributes the same `agg→head`/internal/`tail→agg`
edges regardless of its position in the concatenation), and the loop score is
a commutative product over that multiset, so all orderings share one
`loop_score`; emitting more orderings would only burn the loop budget on
dominance-indistinguishable duplicates. It is bounded
by a deterministic petal priority (fewest internal nodes first, then a stable
joined-name tiebreaker -- makes truncation reproducible), a soft per-agg petal
cap (`MAX_AGG_PETALS = 8`, bounding the `2^k` subset enumeration), and a
model-wide loop-count budget (`MAX_CROSS_AGG_LOOPS = 256`, threaded as
`agg_loop_budget` / `cross_agg_loop_budget()`, `#[cfg(test)]`-overridable via
`AggLoopBudgetGuard`). The two modes differ only in how they feed the core and
build the result: exhaustive's `recover_cross_agg_loops` extracts petals from
Johnson's circuit strings (via `collect_agg_petals`) and turns each stitched
sequence into a `Loop`, setting `LtmVariablesResult::agg_recovery_truncated` on
clipping and accumulating a `Warning` (mirroring the auto-flip-to-discovery
gate), naming the truncated aggs; discovery's `discover_loops_with_graph`
extracts petals from the discovered element paths, appends the stitched
sequences back into `all_paths` (so they flow through the identical FoundLoop /
score / trim / rank pipeline), and sets `DiscoveryResult::agg_recovery_truncated`
(post-simulation, so a flag rather than a salsa `Warning`).
`recover_agg_hop_polarities` then patches the (variable-graph-invisible, hence
`Unknown`) agg hops for monotone reducers in the exhaustive path (GH #516);
discovery derives polarity from the runtime score series directly.

### Discovery Mode

When `ltm_discovery_mode = true`, element-level discovery proceeds as:

1. `model_ltm_variables` generates link score variables for all edges: A2A link
   scores occupy N slots; an arrayed-source → scalar-target reducer is N
   per-source-element scalar `$⁚ltm⁚link_score⁚{from}[{d}]→{to}` variables; a
   scalar-source → arrayed-target edge is N per-target-element scalar
   `$⁚ltm⁚link_score⁚{from}→{to}[{e}]` variables; an inlined reducer is the
   `$⁚ltm⁚agg⁚{n}` aux plus its two link-score families.
2. Post-simulation, `discover_loops_with_graph` receives the `LtmSyntheticVar`
   list and datamodel dimensions. `parse_link_offsets` expands A2A link score
   slots into per-element edges: for each A2A link score at offset O with
   dimension of size N, it emits N `LinkOffset` entries at offsets O, O+1, ...,
   O+N-1 with element-subscripted from/to names. Per-element scalar link scores
   (element on the `from` *or* the `to` side) and agg-hop link scores
   (`$⁚ltm⁚agg⁚{n}` on either end) ride through `parse_link_offsets`'s
   `[`-in-name single-passthrough branch unchanged -- the element / agg name is
   already in the variable name. A Bare/FixedIndex collision on the same
   expanded element key is broken Bare-first.
3. The candidate-generation topology (`IndexedSearch`, and the union graph
   built over it) comes from these element-level link offsets. Element-level
   stocks (expanded from `model_element_causal_edges`, which routes inlined
   reducers through their `$⁚ltm⁚agg⁚{n}` nodes) are the fallback's seed set,
   and their cycle partitions are what every loop normalizes against.
4. Each candidate path becomes its own `FoundLoop`; the synthetic agg nodes
   are trimmed from its node sequence by `trim_synthetic_aggs_from_loop_links`.
   Both generators emit only *elementary* element-graph circuits, so a
   cross-element loop through an inlined reducer -- which visits the agg node
   more than once and is therefore non-elementary -- is structurally
   unreachable to either. Discovery
   recovers these by stitching, exactly as exhaustive mode does (GH #696):
   after candidate generation, `discover_loops_with_graph` treats each
   single-agg candidate path as a *petal* and feeds them through
   `stitch_cross_agg_node_paths` -- the one helper BOTH generators' node paths
   go through, so a reducer model's recovered loops differ between them only
   by the petals each found -- to the SHARED
   combinatorial core
   `stitch_cross_agg_petals` (`src/db/ltm/loops.rs`) -- the same petal priority,
   pairwise-disjoint-internal-node rule, `MAX_AGG_PETALS` cap,
   one-canonical-loop-per-subset emission, and `cross_agg_loop_budget()` that
   `recover_cross_agg_loops` uses, so discovery recovers exactly the loops
   exhaustive does. The stitched
   element-level node sequences are appended to `all_paths` (deduped by
   canonical rotation against the elementary ones) and flow through the
   identical FoundLoop construction / score-product / trim / rank pipeline; a
   stitched loop's edge multiset is the union of its petals' disjoint edges, so
   its per-step loop score is the product of the petals' link scores --
   identical to how any discovered loop is scored. When the loop-count budget
   clips recovery, `DiscoveryResult::agg_recovery_truncated` is set (the
   discovery-mode analogue of `LtmVariablesResult::agg_recovery_truncated`),
   surfaced through `analysis::ModelAnalysis::agg_recovery_truncated`. Because
   the recovery is post-simulation there is no salsa diagnostic accumulator to
   emit a `Warning` into, so the flag is the signal. (The shared core is narrow
   in the same way exhaustive's is: a petal is a circuit touching exactly one
   agg once, so a single candidate path that already visits two distinct aggs
   is a complete loop, not a petal -- the generators emit it directly when it
   is elementary.)

### Per-Slot Loop Score Equations

A dimensioned loop's score variable carries one of two equation shapes,
decided by `ltm_augment::generate_loop_score_variables`:

- **`Equation::ApplyToAll`** when every link of the cycle resolves to an
  emitted Bare A2A link-score name (`{from}→{to}`). Each element slot of the
  loop score reads its own slot of each link score diagonally -- the compact
  form, used for apply-to-all (Bare-reference) models.
- **`Equation::Arrayed`** (one equation per dimension element) when the
  cycle's link scores only exist as per-element names -- FixedIndex
  (`{from}[{e}]→{to}`) or per-target-element (`{from}→{to}[{e}]`) forms, the
  shape per-element-equation (MDL-imported) models produce. Each slot's
  equation is the link product of that element's own circuit, built from
  `Loop::slot_links` (the per-slot element-subscripted link cycles captured by
  `build_element_level_loops`' pure-dimension collapse). Slots with no backing
  circuit score a constant 0.

Before the per-slot form existed (GH #653), the A2A-collapse emitted an
ApplyToAll equation referencing one arbitrary (lexicographically-first)
element's FixedIndex link score for every slot: that element's slot was
correct, and every other slot read a frozen ceteris-paribus partial and scored
0.

## Pinned Loops (LOOPSCORE)

A modeler pins a loop by naming its variable set (the `SetLoopName` patch
primitive, persisted as `LoopMetadata`; see LTM ref section 10). The engine
then ALWAYS emits that loop's `loop_score` -- in both modes. In discovery mode
this is the only way to score a specific loop, since discovery emits no
per-loop score variables at all -- and it is how a modeller keeps a named loop
comparable across runs and parameter sweeps regardless of what candidate
generation reported.

`db/ltm/pinned.rs::model_pinned_loops` resolves each pin:

1. Order the variable set into a closed cycle against the causal graph
   (`order_variable_cycle`); validate it contains a stock.
2. Dimension-classify the cycle with the same `classify_cycle` machinery the
   tiered enumerator uses:
   - **PureScalar**: one scalar `Loop`.
   - **PureSameElementA2A**: one `Loop` carrying the cycle's dimensions and
     element-level stocks -- its loop score is an arrayed (ApplyToAll)
     variable, one slot per element.
   - **CrossElementOrMixed** (literal-element references, mixed scalar/arrayed
     variables, reducer shapes): the cycle is expanded on the element graph
     (`expand_pin_on_element_graph`): project `model_element_causal_edges`
     onto the pin's variables plus synthetic agg nodes, guard the subgraph SCC
     against `MAX_LTM_SCC_NODES`, run Johnson, keep the circuits whose
     agg-trimmed variable set equals the pin's, and group them with
     `build_element_level_loops`. A diagonal family collapses into one arrayed
     `Loop` with `slot_links` (per-slot Arrayed score); genuinely
     cross-element instances become element-subscripted scalar `Loop`s.
3. Assign pin-derived ids: `pin{n}` for single-loop pins, `pin{n}⁚{j}` for
   multi-instance ones. These never collide with the enumerator's
   `r{n}`/`b{n}`/`u{n}` namespace.

A pin that fails any step (unordered set, no stock, oversized expansion SCC,
expansion past the `MAX_LTM_CIRCUITS` budget, no element-level instantiation)
is reported in `PinnedLoopsResult::invalid` and surfaced as a compilation
`Warning` -- never silently scored 0.

**Sibling-cycle limitation.** A pin names a variable *set*, and
`order_variable_cycle` resolves it to the lexicographically-first Hamiltonian
cycle over that set. A set that admits two distinct directed cycles -- the
three-party arms-race pair `A -> B -> C -> A` vs `A -> C -> B -> A`, both
over the same variables -- can therefore only pin one direction (the
lex-first); the other is not expressible through the pin API and is silently
not the one scored. The enumerator finds and scores both directions in
exhaustive mode (canonical-rotation dedup keeps them distinct), so this
gap bites only in discovery mode or when the user expects the *other*
direction's score. Tracked as a known limitation; resolving it requires
extending the pin primitive to carry cycle order. (A pin whose only stock
is module-internal validates since GH #673: the has-stock validation counts
stocks inside traversed modules via the same `enrich_with_module_stocks`
enrichment the enumerator applies -- and since GH #748 the LTM pass itself
runs on such module-only roots at all.)

In exhaustive mode, a scored pin loop whose variable-cycle rotation matches an
enumerated loop is skipped (the enumerated loop already carries a correct
score; the pin's name transfers onto it in `model_detected_loops`). In
discovery mode no loops are enumerated at COMPILE time (the loop universe is
enumerated post-simulation, after the pins are already compiled), so every
scored pin loop is emitted.
Per-slot cycle partitions are registered through the same
`partition_for_loop` resolution enumerated loops use, so post-simulation
relative-score normalization (`ltm_post`) and the FFI's subscripted access
(`simlin_analyze_get_relative_loop_score("pin1[elem]")`,
`simlin_analyze_get_loop_element_count`) work identically for pins and
enumerated loops.

## Current Limitations

### Euler Integration Only

The corrected flow-to-stock formula uses discrete differences that assume Euler
integration. The papers note compatibility with Runge-Kutta "in principle" but
this has not been explored in the implementation.

### Performance on Very Large Models

Discovery's cost is driven by the size of its candidate universe, which is a
property of the runtime graph rather than of the variable count: C-LEARN v77
(251 saved steps, ~21k element-level edges, ~26k LTM variables) holds 162
ever-simultaneously-active cycles and completes discovery in ~0.04 s, of which
the phases before candidate generation -- `parse_link_offsets` and the topology
build -- are the larger part; World3-03 (401 steps, 428 edges) holds 150,827 and
takes ~0.4 s, dominated by enumeration, retention, and `FoundLoop`
materialization. A denser runtime graph than either is what the enumeration
budgets and the caller's wall-clock budget exist for: past them, candidates come
from the shortest-path fallback and `enumeration_complete` reports it.

The compile-time cost of generating and compiling the link-score
instrumentation -- not discovery -- remains the dominant LTM cost on large
models (GH #655 / #317): C-LEARN's LTM compile is ~2.3 s and its LTM simulation
~2.6 s against discovery's 0.04 s.

### Residual array carve-outs

The arrays-hardening cluster closed the conservative-slice carve-out (#514),
the rel-loop-score cross-pollution (#487), the iterated-dimension limitation
(#511), and the disjoint-dim degenerate link score (#510), but a few narrow
cases remain deliberate carve-outs:

- **Dynamic-index reducers stay unhoisted.** A reducer indexed by a
  non-literal/computed index -- `SUM(pop[idx, *])` with a dynamic `idx`,
  `arr[i+1]` -- is not statically describable, so it is not hoisted into an
  aggregate node; its reference stays on the conservative `DynamicIndex`
  cross-product path (a coarse `from[d] → to[e]` for every pair). Related: a
  scalar feeder of a hoisted reducer whose target is also scalar bypasses
  `ThroughAgg` routing on the both-scalar fast path (GH #533), and mapped-
  dimension sliced reducers (`SUM(matrix[State, *])` over `matrix[Region, D2]`
  with a `State→Region` mapping) decline hoisting because the `Iterated`-driven
  machinery assumes the agg result axis and the source row axis are literally
  the same dimension (GH #534).
- **RANK keeps the delta-ratio approximation.** RANK is an order statistic --
  non-differentiable and unreachable as a real scalar/A2A reducer RHS (it
  returns an array) -- so its link score is the delta-ratio stand-in, pinned by
  `test_generate_rank_keeps_delta_ratio` so the choice is explicit. (STDDEV, in
  contrast, now gets an analytic ceteris-paribus partial, #483.)
- **Cross-agg loop recovery is budgeted.** For a reducer in a feedback loop
  over a very large dimension, the recovered cross-element loop list can be
  incomplete: `recover_cross_agg_loops` clips at `MAX_AGG_PETALS` petals per
  agg and `MAX_CROSS_AGG_LOOPS` loops model-wide, sets
  `agg_recovery_truncated`, and emits a `Warning`.
- **Multi-dim per-element graphical-function polarity is conservative.** A
  per-element graphical function over a single dimension gets per-element static
  polarity (#502); over more than one dimension it stays `Unknown`. The
  monotonicity check itself compares the y-delta `dy`, not the slope `dy/dx`,
  so a non-uniform x-spacing can still misclassify (GH #536).
- **Smaller magnitude/over-conservatism nits.** A transposed non-live array
  dependency's magnitude estimate in an A2A link-score partial can be
  imprecise (GH #526); `expand_same_element` takes the full cross-product
  instead of the positional-mapping diagonal for mapped dimensions (GH #527);
  and the partial-iterated arrayed subscript in an A2A link-score partial
  fails to compile because the `PREVIOUS` argument must be a `Var` (GH #525).

## Divergences from the Papers

1. **Per-timestep vs. per-dt sampling**: The papers describe searching at
   "every (or almost every) point in time," meaning each DT step. The
   implementation reads the recorded series at each saved timestep (determined
   by `save_step` in sim specs), which may be coarser. This is an intentional
   simplification that trades completeness for speed (GH #309).

2. **Enumeration over the union graph, not a per-step search**: the papers
   search for the strongest paths at each point in time and accumulate the
   loops found. Discovery instead builds ONE union-of-active-edges graph over
   all saved steps, with a per-edge activity bitset, and enumerates every
   elementary cycle whose edges are simultaneously active at some step. That is
   a superset relationship, not a different heuristic: the enumerated set is
   provably every cycle a per-step search could have found at any step, so
   discovery is exact where the papers' method samples. The shortest-path
   fallback that stands in when the enumeration cannot finish is also not the
   papers' `best_score` DFS -- it is a per-(seed, step) Dijkstra whose dropped
   set is characterizable (for a given seed, edge and step it keeps the
   minimum-weight cycle and drops the rest), which a per-node work bound is
   not.

3. **Auto-flip on large SCCs, no composite-network pre-reduction**: The papers
   describe a two-tier strategy in which models with fewer than ~1000 loops use
   exhaustive enumeration on a composite (max-score) network. The implementation
   does not build that composite pre-reduction: `ltm_enabled` runs exhaustive
   enumeration and `ltm_discovery_mode` runs `discover_loops()`. However,
   `model_ltm_variables` in `src/simlin-engine/src/db/ltm.rs` does automatically
   switch from exhaustive to discovery in two phases. The early gate fires on
   the variable-level causal graph's largest SCC (cheap Tarjan, no Johnson
   yet). The late gate fires on the slow-path element-level subgraph's largest
   SCC, computed inside `model_loop_circuits_tiered` after variable-level
   cycles are classified. Both gates use `MAX_LTM_SCC_NODES` (currently 50,
   defined in `src/simlin-engine/src/ltm.rs`). Above either size, Johnson
   circuit enumeration blows past reasonable memory and time budgets on its
   own; see `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md` and
   `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`
   for the measurements. (The legacy per-loop relative-score equation synthesis
   compounded this with an O(P^2) text blowup; moving the normalization
   post-simulation -- divergence 6 below -- removed that factor from
   augmentation cost.) Auto-flip emits a `CompilationDiagnostic` at
   `Warning` severity so callers can surface the fallback to users.

   The node-count gates alone do not bound enumeration cost -- elementary-
   circuit count is super-exponential in SCC *density*, not node count (a
   near-complete 14-node digraph holds ~119M circuits and OOMs uncapped
   Johnson while passing both 50-node gates) -- so every production Johnson
   run additionally carries a circuit budget, `MAX_LTM_CIRCUITS` (100,000,
   defined alongside `MAX_LTM_SCC_NODES`). Exhausting it marks the
   `LoopCircuitsResult`/`TieredCircuitsResult` as `truncated`, which the
   shared `model_ltm_mode` gate treats exactly like an oversized SCC: the
   model flips to discovery with its own `Warning`. A pinned loop whose
   element-level expansion exceeds the budget becomes an invalid pin
   (reported, never silently scored).

4. **Module handling**: The papers describe composite link scores for macros
   (DELAY, SMOOTH) but do not discuss module boundaries as an implementation
   concept. The Simlin implementation extends the macro approach to modules:
   internal graphs are built recursively, pathways are enumerated, and composite
   scores are computed at each timestep. Module stock enrichment (adding
   module-internal stocks to loop stock lists) is an implementation-specific
   extension that enables correct cycle partitioning.

5. **PREVIOUS is intrinsic**: The `PREVIOUS()` function used in link score
   equations is compiled as an intrinsic two-argument builtin. Unary syntax is
   desugared to `PREVIOUS(x, 0)`. LTM first-timestep behavior is handled
   explicitly with `TIME = INITIAL_TIME`.

6. **Relative loop score formula and timing**: The implementation computes
   `loop_score / sum_of_abs_scores` with explicit division-by-zero protection
   (yielding 0 rather than NaN), while the papers present the formula without
   discussing this edge case. It also performs this normalization in a
   post-simulation pass (`ltm_post::compute_rel_loop_scores`) rather than as
   synthesized compile-time equations, avoiding O(P^2) equation-text growth on
   models with very large same-partition loop sets (e.g. WRLD3).

7. **Flow-to-stock numerator timing and `time_step` scaling**: The flow-to-stock
   link score numerator uses `time_step * (PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow)))`
   rather than the published bare `flow - PREVIOUS(flow)`. Two deliberate changes:
   - *1-DT shift*: in Euler integration, the flow at t-1 drove the stock change
     from t-1 to t, so `PREVIOUS(flow)` aligns the numerator and denominator to
     the same causal interval. This produces results shifted by one DT compared
     to reference SD software (Stella/iThink). The integration test
     (`tests/integration/simulate_ltm.rs`) compensates by shifting reference
     timestamps forward by DT when loading golden data. This convention is also
     documented for end users in the reference doc's Section 3.2 (the "numerator
     timing convention" note under
     [Flow-to-Stock Link Score](../reference/ltm--loops-that-matter.md)).
   - *`time_step` factor*: the denominator (second-order stock change) is
     `dt * (netflow(t-dt) - netflow(t-2dt))` in Euler, carrying one `dt` that the
     raw flow delta in the numerator lacks; without the factor every
     flow-to-stock link score is `1/dt` too large and the error compounds once
     per stock in a loop. The published Eq. 3 omits the factor because every
     worked example in the papers uses dt=1. Verified empirically: at dt=0.25
     an isolated loop scores +1.0 with the factor (4.0 without), and at dt=1
     the paper's Table 3 values (1.25 / -0.25) reproduce exactly.

8. **Ceteris-paribus via AST transformation**: The papers describe re-evaluating
   equations with current values of one input and previous values of all others.
   The implementation achieves this by parsing the equation into an AST,
   recursively transforming it to wrap non-excluded dependencies in `PREVIOUS()`,
   and printing the result back to equation text. This is done once at
   augmentation time (not per-timestep), producing a static equation that the
   simulation engine evaluates normally.

## Test Coverage

### Unit Tests

- **`ltm.rs`**: Loop detection on known models (reinforcing, balancing, no-loop,
  module, multi-module), polarity analysis (AST expressions including addition,
  subtraction, multiplication, division, unary negation, IF-THEN-ELSE, graphical
  functions, arrayed equations, Max/Min builtins), flow-to-stock polarity, runtime
  polarity classification, deduplication, deterministic ID assignment, module
  dependencies, empty variable ASTs, path formatting

- **`ltm_augment.rs`**: Equation generation for all link types
  (auxiliary-to-auxiliary, flow-to-stock, stock-to-flow, module links),
  AST-based partial equation building (`build_partial_equation`) with tests
  for builtin function preservation, simple substitution, no-deps-to-wrap, and
  IF-THEN-ELSE, loop score equations, generated variable structure

- **`ltm_post.rs`**: Post-simulation relative loop score computation --
  partition grouping, SAFEDIV-0 semantics on empty-denominator timesteps,
  property-based equivalence with the reference compile-time formula on
  synthetic loop-score matrices

- **`ltm_finding_tests.rs`** (the `#[cfg(test)]` sibling of `ltm_finding.rs`),
  by family:
  - *Enumerator*: a brute-force oracle over randomized synthetic graphs
    (every ever-simultaneously-active cycle, compared set-for-set), each cycle
    emitted exactly once, opposite-direction cycles kept distinct, a cycle
    active at only one step, a `PREVIOUS` self-latch reported as no loop
    (AC1.1), a staggered-activity cycle reported by neither generator
    (GH #699), and one arm per enumeration budget reporting incomplete.
  - *Retention*: hand-computed products and totals, the confirm step rescuing
    a circuit whose running bound overstated its share, module and Solo
    circuits kept, the `Inf * 0` product excluded from both totals and
    retention (AC4.1), universe circuit counts per partition, and the
    step-0 activity window.
  - *Fallback parity*: both generators agree on a simple model, on a diamond
    (both arms recovered), on a module loop, and on a stockless two-node cycle
    that ranks last (AC1.3); an enumeration budget trip falls back and still
    reports the same loops.
  - *Deadlines*: one arm per phase (`ActivityGraph::build`,
    `enumerate_active_circuits`, `retain_circuits`) for an already-expired
    deadline, a mid-search expiry, and the never-reads-the-clock claim when
    unbudgeted; plus the budget split itself.
  - *Ranking and selection*: retention before the cap (GH #310), competitive-
    first order, solo demotion and the magnitude tie-break, determinism under
    permutation, universe-based competing classification (both arms, AC5.2),
    and the coverage-aware cap (each step's dominant loop kept, `k`
    escalation within the anchor share, anchors over the cap, ties, empty
    steps, per-group maxima; AC5.1).
  - *Counts and metadata*: universe/retained/fallback-candidate reporting,
    module override mass in the denominator (AC4.2), the trimmed-duplicate
    mass subtraction (AC4.3), cross-agg stitching end to end and its budget,
    link offset parsing (every A2A/FixedIndex/per-element shape), ID
    assignment, Tarjan SCC ids, and `discovery_graph_stats`.

- **`ltm_finding_fallback_tests.rs`**: Dijkstra exactness against a brute-force
  minimum-weight cycle on random graphs; one arm per weight formulation
  (including the infinite-score arms and the step shift); each tie-break arm
  and the proof that the tie-break changes order but not membership; the
  every-edge closures (minimum-weight through their edge, superset of the
  seed's in-edge closures, both arms of a diamond); each seed policy and what
  a stockless cycle needs to be reachable; self-edge handling; rotation dedup
  across seeds and steps; the deadline sites (already expired, between steps,
  between seeds, inside a search, unbudgeted); the candidate budget; and
  content-pure output.

### Salsa Pipeline Tests

- **`db/ltm_tests.rs`**: LTM equation text generation via salsa tracked
  functions, link score caching behavior

- **`db/ltm_unified_tests.rs`**: `model_ltm_variables` for simple models,
  stdlib modules (SMOOTH), passthrough modules, and discovery mode

- **`db/ltm_module_tests.rs`**: Module-specific LTM tests: SMOOTH models
  compile with LTM, composite scores are generated for stdlib modules,
  user-defined modules with feedback receive LTM treatment

- **`db/tests.rs`** (LTM subset): Salsa LTM caching, discovery vs exhaustive
  variable counts, incremental invalidation, layout slot allocation with LTM

### Integration Tests (`tests/integration/simulate_ltm.rs`)

All integration tests use `compile_project_incremental` + VM:

- **`simulates_population_ltm`**: Runs the logistic growth model with exhaustive
  LTM, validates relative loop scores against golden data from reference SD
  software (`test/logistic_growth_ltm/ltm_results.tsv`)

- **`discovery_logistic_growth_finds_both_loops`**: Verifies discovery mode finds
  both loops in the logistic growth model

- **`discovery_cross_validates_with_exhaustive`**: Cross-validates discovery against
  exhaustive enumeration on the logistic growth model

- **`discovery_arms_race_3party`**: Tests the three-party arms race model from
  the papers (8 exhaustive loops -- both traversal directions of the three-way
  cycle count, issue #308 -- and discovery finds all 8)

- **`discovery_decoupled_stocks`**: Tests time-varying loop activation where
  different loops become active at different timesteps

## References

- Eberlein, R. and Schoenberg, W. (2020). "Finding the loops that matter."
- Schoenberg, W., Davidsen, P., and Eberlein, R. (2020). "Understanding model
  behavior using the loops that matter method." *System Dynamics Review* 36(2).
- Schoenberg, W., Hayward, J., and Eberlein, R. (2023). "Improving loops that
  matter." *System Dynamics Review* 39(2).
