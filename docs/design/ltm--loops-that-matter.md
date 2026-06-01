# Loops That Matter (LTM): Implementation Design

This document describes how Simlin implements the Loops That Matter method for
feedback loop dominance analysis. For a comprehensive technical description of the
LTM method itself, see the [reference document](../reference/ltm--loops-that-matter.md).

## Architecture Overview

The implementation is split across three modules in `src/simlin-engine/src/`:

| Module | Responsibility |
|--------|---------------|
| `ltm.rs` | Causal graph construction, loop detection (Johnson's algorithm), static polarity analysis, cycle partitions |
| `ltm_augment.rs` | Synthetic variable generation: link score and loop score equations |
| `ltm_finding.rs` | Strongest-path loop discovery algorithm for models too large for exhaustive enumeration |
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
5. Loops are deduplicated by sorted node set (`deduplicate_loops`)
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
3. Post-simulation, `discover_loops()` (`ltm_finding.rs`) runs the strongest-path
   algorithm at each saved timestep:
   - Parses link score variable names from `results.offsets` (`parse_link_offsets`)
   - Builds a `SearchGraph` per timestep from the link score values
     (`SearchGraph::from_results`)
   - Runs the strongest-path DFS (`find_strongest_loops`)
   - Collects unique loop paths across all timesteps
4. Each discovered path is converted to a `FoundLoop` with signed loop scores
   computed at every timestep from the raw link score results
5. Loops are ranked by average absolute score, truncated to `MAX_LOOPS` (200),
   filtered by `MIN_CONTRIBUTION` (0.1%) with partition-aware thresholds, and
   assigned deterministic IDs (`rank_and_filter`)

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

The `link_score_equation_text` tracked function in `db.rs` handles three
module link cases:

- **Variable-to-module-input** (`!from_is_module && to_is_module`): Uses composite
  link score reference when the module has internal causal pathways (determined by
  `module_input_pathways_from_edges`). The link score variable references the
  module's internal composite via interpunct notation (e.g.,
  `module·$⁚ltm⁚composite⁚port`). Falls back to the black-box transfer-function
  formula (`generate_module_link_score_equation`) for modules without causal
  pathways.

- **Module-output-to-variable** (`from_is_module && !to_is_module`): Uses the
  standard ceteris-paribus formula (`generate_link_score_equation_for_link`). The
  `build_partial_equation` function is module-ref-aware: `normalize_module_ref()`
  strips interpunct suffixes so that module output references (e.g.,
  `$⁚s⁚0⁚smth1·output`) are correctly excluded from `PREVIOUS()` wrapping while
  other dependencies are held at their previous values.

- **Module-to-module** (`from_is_module && to_is_module`): Uses the black-box
  transfer-function formula (`generate_module_link_score_equation`) since modules
  have no user-visible equation for ceteris-paribus analysis.

## Module Boundary Handling

The implementation uses **composite link scores** for dynamic modules (SMOOTH,
DELAY, TREND, etc.), following Section 6 of Schoenberg & Eberlein (2020). The
composite score is the product of internal link scores along the strongest
internal pathway at each timestep.

### Module Classification

Modules are classified by `classify_module_for_ltm()` in `ltm.rs`:

- **Infrastructure** (`PREVIOUS`, `INIT`) -- used BY link score equations; never
  analyzed to avoid infinite recursion.
- **DynamicModule** -- has internal stocks (SMOOTH, DELAY, TREND, user-defined
  modules with stocks). Gets composite link scores and internal graph construction.
- **Passthrough** -- no internal stocks; treated as black box with a transfer
  score formula.

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

2. **Internal instrumentation**: For each DynamicModule model,
   `model_ltm_variables` generates:
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
- **Multiplication**: Combines polarities (positive * negative = negative). When
  one operand has unknown polarity, checks if the other is a positive or negative
  constant (`is_positive_constant` / `is_negative_constant`) or a variable with
  a constant equation (`is_positive_variable` / `is_negative_variable`)
- **Division**: Numerator preserves polarity; denominator flips. Same independence
  check as addition/subtraction.
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
  flips an otherwise-monotone curve to `Unknown`; the residual nuance is that
  it compares the y-delta `dy`, not the slope `dy/dx`, so a curve with
  non-uniform x-spacing can still misclassify (GH #536).
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

`LoopPolarity::from_runtime_scores()` in `ltm.rs` classifies polarity based
on actual simulation results. It filters out NaN and zero values, then:
- All remaining scores positive -> `Reinforcing`
- All remaining scores negative -> `Balancing`
- Mix of positive and negative -> `Undetermined`
- No valid scores -> `None` (caller falls back to structural polarity)

This catches cases where nonlinear dynamics cause polarity to change during
simulation (e.g., the yeast alcohol model from the papers). In discovery mode,
runtime polarity overrides structural polarity when available.

## Strongest-Path Algorithm

The implementation in `ltm_finding.rs` follows Appendix I of Eberlein &
Schoenberg (2020), with three scalability amendments (GH #647) that keep the
per-timestep DFS tractable on large arrayed models. The production
implementation is the integer-indexed `IndexedSearch` (topology built once,
per-step scores reloaded into reusable scratch); the `Ident`-keyed
`SearchGraph` is retained as a test-only oracle implementing the identical
algorithm.

### SearchGraph (`ltm_finding.rs`)

Built per timestep from simulation results. Edges are sorted by absolute score
descending (`from_edges`), providing the ~3x speedup described in the paper by
making the first visit to each variable likely the strongest. NaN scores are
treated as 0.

### Core DFS (`check_outbound_uses` / `IndexedSearch::dfs`)

The recursive search follows the paper's pseudocode structure, with the
pruning mechanism replaced (see "Scalability Amendments" below):

- **Cycle detection**: `visiting` set tracks nodes on the current DFS path. When
  a visited node equals TARGET, a loop is recorded.
- **Strongest-first ordering**: each node's edges are walked in descending
  |score| order, so when work bounds bind, the strongest paths are the ones
  already explored. (Accumulated path products are not tracked; loop scores
  are recomputed exactly from the link-score series after discovery.)
- **Per-stock isolation**: per-node expansion counts are reset at the start of
  each stock's search (the role the paper's per-stock `best_score` reset
  played -- Section 12.5), so one stock's search never limits loops reachable
  from another stock.
- **Deduplication**: `add_loop_if_unique` uses canonical edge-sequence
  rotations to prevent recording the same loop twice.

### Scalability Amendments (GH #647)

The paper's pseudocode degenerates on large arrayed models -- on C-LEARN v77
(element-level graph: ~4,700 nodes, ~20,500 edges) four timesteps of discovery
did not complete in 10 minutes. Three amendments make it tractable; together
they reduced full 251-step discovery to under a second:

- **Zero-score edges are excluded from the per-step graph.** A loop containing
  a zero-score (or NaN) link has loop score exactly 0 at this timestep, so it
  is not a "loop that matters" here; if it ever matters, every link is nonzero
  at the timestep where it does, and it is discoverable there. On C-LEARN ~94%
  of edges are zero at any given step, and traversing them made the DFS wander
  the whole graph.
- **Each stock's DFS is restricted to its own strongly-connected component**
  of the per-step nonzero-score subgraph (`tarjan_scc_ids`, computed once per
  step). A path that leaves the SCC can never return to the target stock, so
  the restriction loses no loops -- it only skips provably wasted exploration.
  Stocks outside any multi-node SCC (and without a self-edge) are skipped
  outright. On C-LEARN this confines each search to a <= 65-node component;
  on WRLD3 the famous 166-node static SCC fragments into <= 7-node per-step
  components.
- **The paper's `best_score` pruning is replaced by a per-node expansion cap
  scaled to component size** (`EXPANSION_BUDGET_PER_SEARCH / |SCC|`, min 1).
  The paper's pruning fails in both directions on real models: exact score
  ties (chains of single-input links score exactly 1.0) and super-unit scores
  (|score| > 1 makes path products grow) defeat it -- every non-pruned
  re-arrival re-explores the node's whole subtree, exponential in
  parallel-path structures -- and when it *does* fire it silently drops
  sibling loops whose entry path is weaker (the paper's Figure 7 failure
  mode). The expansion cap inverts the trade-off: work is bounded at
  `cap * SCC_edges` traversals per (stock, step) regardless of score
  distribution, and small components (the common case after SCC restriction)
  get effectively exhaustive enumeration -- every elementary cycle through
  the stock is found, strictly more complete than the paper's heuristic.

### Heuristic Nature

For small per-step components the search is effectively exhaustive (the
expansion cap doesn't bind), so every elementary cycle through each stock is
found -- including the paper's Figure 7 case, where the original `best_score`
pruning missed the strongest loop (`test_figure_7_paper` documents this).

For large components the expansion cap binds and the algorithm is heuristic:
loops whose every entry path is reached only after the per-node budgets are
consumed can be missed. The mitigations mirror the paper's: (a) running the
search at every timestep with different link scores (and therefore different
edge orderings) tends to discover loops missed at other timesteps, and (b)
per-stock budget resets mean different starting stocks can discover different
loops. The papers' empirical evaluation shows that missed loops are
consistently "siblings" of found loops, differing by only a few links.

### Ranking and Filtering (`rank_and_filter`)

After all timesteps are processed:

1. Sort by average absolute score descending
2. Truncate to `MAX_LOOPS` (200)
3. Filter by per-timestep relative contribution within partition: retain a loop
   if at any single timestep its absolute score is >= `MIN_CONTRIBUTION` (0.1%)
   of the partition-scoped total at that timestep. This partition-aware filtering
   prevents globally negligible loops that are dominant in their own partition
   from being incorrectly removed.
4. Assign deterministic polarity-based IDs (`r1`, `b1`, etc.)
5. Re-sort by score descending for callers

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
`row_sum[D1] = SUM(matrix[D1, *])`) or a mapped-dimension sliced reducer
(`SUM(matrix[State, *])` over `matrix[Region, D2]` with a `State→Region`
mapping -- `enumerate_agg_nodes` declines the remapped axis, tracked tech
debt), and the conservative cross-product is the right semantics for both.

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
to "every source element → scalar agg → every target element"). The only
reducers *not* hoisted are the dynamic-index carve-out (`SUM(pop[idx, *])`,
`idx` non-literal -- not statically describable, reclassified `DynamicIndex`)
and the mapped-dimension sliced reducer (above); a bare non-literal index
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
not `Eq`) dedupe to one node.

**Read slice and result dims.** Each `AggNode` carries a
`read_slice: Vec<AxisRead>` -- one `AxisRead ∈ {Pinned(elem), Iterated(dim),
Reduced}` per source axis, describing *which rows of the arrayed source the
reducer actually reads* -- and a `result_dims`, the `Iterated` axes' dims (in
order; empty for a whole-extent or pinned-slice reduce, since the result is a
scalar). `compute_read_slice` decides hoistability per axis:

- `*` / `*:Dim` ⇒ `Reduced` (the whole axis is reduced away);
- an iterated-dimension index that names the source's `i`-th dim by name ⇒
  `Iterated(d)` (the agg's result varies per element of `d`);
- a literal element name / 1-based integer ⇒ `Pinned(elem)`;
- anything else (`@N`, `Range`, a non-literal `Expr`, an iterated dim that
  only lines up via a *mapping*) ⇒ `None` -- the reducer is not statically
  describable, so it is not hoisted.

So `SUM(pop[*])` ⇒ all-`Reduced`, `result_dims = []` (a scalar agg);
`SUM(pop[NYC, *])` over `pop[Region, Age]` ⇒ `[Pinned(nyc), Reduced]`,
`result_dims = []`; `SUM(matrix[D1, *])` inside an A2A body over `D1` ⇒
`[Iterated(d1), Reduced]`, `result_dims = [D1]` (an *arrayed* agg, one slot
per `D1` element); `SUM(matrix3d[D1, NYC, *])` over an A2A-`D1` body ⇒
`[Iterated(d1), Pinned(nyc), Reduced]`. The carve-outs (tracked tech debt;
the conservative cross-product / coarse link score stays in place) are: a
reducer over a *dynamic index* (`SUM(pop[idx, *])`, `idx` non-literal -- the
IR reclassifies its reference to `DynamicIndex`); a sliced reducer whose
iterated index only matches the source row axis via a *dimension mapping*
(`SUM(matrix[State, *])` over `matrix[Region, D2]` with a `State→Region`
mapping -- `compute_read_slice` returns `None` because the `Iterated`-driven
machinery assumes the agg result axis and the source row axis are literally
the same dimension); and a multi-source reducer whose arrayed args read
incompatible slices (`combined_read_slice` returns `None` on disagreement -- a
multi-source reducer whose args *agree*, `SUM(a[*] + b[*])` over the same dim,
mints one agg carrying the combined slice and both source variables).

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
    subscript *and* the `Δsource` denominator of that link-score equation
    projects the same `[<slot>]` subscript (the bare multi-slot agg name
    doesn't compile as a scalar denominator). This is exact for the diagonal
    case (`result_dims` equal `target`'s iterated dims); the strict-prefix
    *broadcast* case (`SUM(matrix[D1, *])` inside an A2A body over `D1 × D2`,
    so the agg is over `D1` but the target is over `D1 × D2`) over-subscribes
    the agg into the cross-product -- the loop score degrades to 0 there, GH
    #528.

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
  reducer's edge, since the edges to a real variable node already exist.

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

**Cross-agg loop recovery** (#515). A cross-element feedback loop *through* an
inlined reducer visits the (subscript-free, or for an arrayed agg
`[<slot>]`-subscripted) agg node more than once, so Johnson never emits it
directly. `recover_cross_agg_loops` reconstructs it from the agg-touching
elementary "petals" (`agg → … → agg`), stitching pairwise-disjoint petal
subsets of size ≥ 2 in every distinct *cyclic ordering*: `cyclic_orderings(m)`
pins index 0 to kill rotations and skips mirror reversals (`1` for m = 2,
`(m-1)!/2` for m ≥ 3, via a hand-rolled Heap's algorithm), so each disjoint
subset of `m` petals yields `(m-1)!/2` distinct directed cycles that share a
`loop_score` (same edge multiset ⇒ same commutative product). It is bounded
by a deterministic petal priority (fewest internal nodes first, then a stable
joined-name tiebreaker -- makes truncation reproducible), a soft per-agg petal
cap (`MAX_AGG_PETALS = 8`, bounding the `2^k` subset enumeration), and a
model-wide loop-count budget (`MAX_CROSS_AGG_LOOPS = 256`, threaded as
`agg_loop_budget`, `#[cfg(test)]`-overridable via `AggLoopBudgetGuard`).
Clipping sets `LtmVariablesResult::agg_recovery_truncated` and accumulates a
`Warning` (mirroring the auto-flip-to-discovery gate), naming the truncated
aggs. `recover_agg_hop_polarities` then patches the (variable-graph-invisible,
hence `Unknown`) agg hops for monotone reducers (GH #516).

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
3. The `SearchGraph` is built from these element-level link offsets. Element-level
   stocks (expanded from `model_element_causal_edges`, which routes inlined
   reducers through their `$⁚ltm⁚agg⁚{n}` nodes) serve as DFS starting points.
4. Discovered element-level loops are grouped and classified identically to
   exhaustive mode via `build_element_level_loops`; the synthetic agg nodes are
   trimmed from each reported `FoundLoop`'s node sequence.

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
this is the only way to score a specific loop, since the heuristic search
emits no per-loop score variables at all.

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
no element-level instantiation) is reported in `PinnedLoopsResult::invalid`
and surfaced as a compilation `Warning` -- never silently scored 0.

In exhaustive mode, a scored pin loop whose variable-cycle rotation matches an
enumerated loop is skipped (the enumerated loop already carries a correct
score; the pin's name transfers onto it in `model_detected_loops`). In
discovery mode nothing is enumerated, so every scored pin loop is emitted.
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

The strongest-path search runs at every saved timestep, restricted to each
stock's per-step nonzero-score SCC with bounded per-node re-expansion (see
"Scalability Amendments"). Per-step work is bounded by roughly
`stocks_in_cycles * EXPANSION_BUDGET_PER_SEARCH * avg_degree` edge traversals
regardless of model size; C-LEARN v77 (251 steps, ~20k element-level edges)
completes the full discovery sweep in under 0.1s. The compile-time cost of
generating and compiling the link-score instrumentation -- not the discovery
DFS -- is now the dominant LTM cost on large models (GH #655 / #317).

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
- **An arrayed synthetic agg's link score over-subscripts in the broadcast
  case.** When the agg is over `D1` but the target is over `D1 × D2` (a
  strict-prefix broadcast, `SUM(matrix[D1, *])` inside an A2A body over
  `D1 × D2`), the `agg → target` link score over-subscribes the agg into the
  cross-product and the loop score degrades to 0 (GH #528). The diagonal case
  (agg dims equal the target's iterated dims) is exact.
- **Smaller magnitude/over-conservatism nits.** A transposed non-live array
  dependency's magnitude estimate in an A2A link-score partial can be
  imprecise (GH #526); `expand_same_element` takes the full cross-product
  instead of the positional-mapping diagonal for mapped dimensions (GH #527);
  and the partial-iterated arrayed subscript in an A2A link-score partial
  fails to compile because the `PREVIOUS` argument must be a `Var` (GH #525).

## Divergences from the Papers

1. **Per-timestep vs. per-dt search**: The papers describe running the
   strongest-path search at "every (or almost every) point in time," meaning each
   DT step. The implementation runs at each saved timestep (determined by
   `save_step` in sim specs), which may be coarser. This is an intentional
   simplification that trades completeness for speed.

2. **Auto-flip on large SCCs, no composite-network pre-reduction**: The papers
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
   post-simulation -- divergence 5 below -- removed that factor from
   augmentation cost.) Auto-flip emits a `CompilationDiagnostic` at
   `Warning` severity so callers can surface the fallback to users.

3. **Module handling**: The papers describe composite link scores for macros
   (DELAY, SMOOTH) but do not discuss module boundaries as an implementation
   concept. The Simlin implementation extends the macro approach to modules:
   internal graphs are built recursively, pathways are enumerated, and composite
   scores are computed at each timestep. Module stock enrichment (adding
   module-internal stocks to loop stock lists) is an implementation-specific
   extension that enables correct cycle partitioning.

4. **PREVIOUS is intrinsic**: The `PREVIOUS()` function used in link score
   equations is compiled as an intrinsic two-argument builtin. Unary syntax is
   desugared to `PREVIOUS(x, 0)`. LTM first-timestep behavior is handled
   explicitly with `TIME = INITIAL_TIME`.

5. **Relative loop score formula and timing**: The implementation computes
   `loop_score / sum_of_abs_scores` with explicit division-by-zero protection
   (yielding 0 rather than NaN), while the papers present the formula without
   discussing this edge case. It also performs this normalization in a
   post-simulation pass (`ltm_post::compute_rel_loop_scores`) rather than as
   synthesized compile-time equations, avoiding O(P^2) equation-text growth on
   models with very large same-partition loop sets (e.g. WRLD3).

6. **Flow-to-stock numerator timing**: The flow-to-stock link score numerator uses
   `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` rather than `flow - PREVIOUS(flow)`.
   In Euler integration, the flow at t-1 drove the stock change from t-1 to t, so
   `PREVIOUS(flow)` aligns the numerator and denominator to the same causal interval.
   This produces results shifted by one DT compared to reference SD software
   (Stella/iThink). The integration test (`tests/simulate_ltm.rs`) compensates by
   shifting reference timestamps forward by DT when loading golden data.

7. **Ceteris-paribus via AST transformation**: The papers describe re-evaluating
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

- **`ltm_finding.rs`**: SearchGraph construction and edge sorting, trivial loop,
  Figure 7 from the paper (all loops found via cap-bounded exhaustive search),
  per-stock search isolation, deduplication, empty graph, zero-score edges
  (excluded per step; the loop is discovered at steps where it is active), NaN
  handling, self-loops, disconnected components, stocks without outbound edges,
  SCC restriction (acyclic appendages don't change results), tied-score
  diamond-chain termination (bounded re-expansion), Tarjan SCC ids, discovery
  graph stats, link offset parsing, ID assignment, rank-and-filter (truncation,
  contribution filtering, ordering preservation, briefly dominant loop
  retention, partition-aware filtering)

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

### Integration Tests (`tests/simulate_ltm.rs`)

All integration tests use `compile_project_incremental` + VM:

- **`simulates_population_ltm`**: Runs the logistic growth model with exhaustive
  LTM, validates relative loop scores against golden data from reference SD
  software (`test/logistic_growth_ltm/ltm_results.tsv`)

- **`discovery_logistic_growth_finds_both_loops`**: Verifies discovery mode finds
  both loops in the logistic growth model

- **`discovery_cross_validates_with_exhaustive`**: Cross-validates discovery against
  exhaustive enumeration on the logistic growth model

- **`discovery_arms_race_3party`**: Tests the three-party arms race model from the
  papers (7 exhaustive loops; discovery finds all 7 with per-stock reset)

- **`discovery_decoupled_stocks`**: Tests time-varying loop activation where
  different loops become active at different timesteps

## References

- Eberlein, R. and Schoenberg, W. (2020). "Finding the loops that matter."
- Schoenberg, W., Davidsen, P., and Eberlein, R. (2020). "Understanding model
  behavior using the loops that matter method." *System Dynamics Review* 36(2).
- Schoenberg, W., Hayward, J., and Eberlein, R. (2023). "Improving loops that
  matter." *System Dynamics Review* 39(2).
