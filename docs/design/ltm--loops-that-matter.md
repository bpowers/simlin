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
`db_ltm.rs`, invoked as part of `compile_project_incremental`. LTM compilation
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

1. `model_causal_edges` (salsa tracked, `db_analysis.rs`) builds the causal graph
2. `model_loop_circuits` uses Johnson's algorithm to enumerate all
   elementary circuits via DFS (`find_circuits_from` / `dfs_circuits`)
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
| Pathway score | `$⁚ltm⁚path⁚{port}⁚{index}` |
| Composite score | `$⁚ltm⁚composite⁚{port}` |
| Loop score | `$⁚ltm⁚loop_score⁚{loop_id}` |

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
- **Lookup tables**: Analyzes monotonicity of graphical functions
  (`analyze_graphical_function_polarity`) -- checks consecutive y-values to
  determine if the table is monotonically increasing (Positive), decreasing
  (Negative), or neither (Unknown). Combines with the argument's polarity.
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
Schoenberg (2020) closely.

### SearchGraph (`ltm_finding.rs`)

Built per timestep from simulation results. Edges are sorted by absolute score
descending (`from_edges`), providing the ~3x speedup described in the paper by
making the first visit to each variable likely the strongest. NaN scores are
treated as 0.

### Core DFS (`check_outbound_uses`)

The recursive search follows the paper's pseudocode:

- **Cycle detection**: `visiting` set tracks nodes on the current DFS path. When
  a visited node equals TARGET, a loop is recorded.
- **Pruning**: `best_score` tracks the highest accumulated score at which each
  variable has been reached. Strict less-than comparison (`score < best_score`)
  prunes weaker paths; equal scores are explored.
- **Per-stock reset**: `best_score` is reset to zero for all variables at the
  start of each stock's search, following the paper's pseudocode (Section 12.5).
  This prevents one stock's search from pruning reachable loops when searching
  from a different stock.
- **Deduplication**: `add_loop_if_unique` uses sorted node sets to prevent
  recording the same loop twice.

### Heuristic Nature

The algorithm does not guarantee finding the truly strongest loop. The specific
failure mode (demonstrated in the paper's Figure 7 and tested in
`test_figure_7_paper`) is that visiting a node via a strong path sets a high
`best_score` that prunes exploration via weaker paths that might lead to
different (but still valid) loops.

The mitigation is twofold: (a) running the search at every timestep with different
link scores tends to discover loops missed at other timesteps, and (b) resetting
`best_score` per stock means different starting stocks can discover different
loops. The papers' empirical evaluation shows that missed loops are consistently
"siblings" of found loops, differing by only a few links.

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

### Element-Level Causal Graph

`model_element_causal_edges` (salsa tracked, `db_analysis.rs`) builds the
element-level graph by walking each target variable's `Expr2` AST and
emitting one or more element edges per reference site. A reference site is
one occurrence of an `Expr2::Var` or `Expr2::Subscript` node naming a
source variable; `collect_reference_sites` finds them and
`emit_edges_for_reference` writes the edges.

Each reference is classified by access shape:

| Source dims | Target dims | RefShape         | Edges emitted                                |
|-------------|-------------|------------------|----------------------------------------------|
| scalar      | scalar      | Bare             | `from -> to`                                 |
| scalar      | arrayed     | Bare             | `from -> to[d]` for each target element d    |
| arrayed     | scalar      | Bare             | `from[d] -> to` for each source element d    |
| arrayed     | arrayed     | Bare             | `from[d] -> to[d]` per shared element        |
| arrayed     | any         | Wildcard         | `from[d] -> to[e]` full cross-product        |
| arrayed     | scalar      | FixedIndex(elem) | `from[elem] -> to` (one edge)                |
| arrayed     | arrayed     | FixedIndex(elem) | `from[elem] -> to[d]` for each target element d |
| arrayed     | any         | DynamicIndex     | conservative full cross-product              |

`Bare` covers both bare `Expr2::Var` references (scalar dep or A2A
same-element). `FixedIndex` carries the resolved element subscripts from a
literal-index `Subscript` node. `Wildcard` covers reducer patterns
(`SUM(x[*])`); `DynamicIndex` covers any subscript with non-literal indices
(`@N`, `Range`, `StarRange`, or arbitrary `Expr`).

Edges from multiple reference sites in the same target are unioned. For
`relative_pop[R] = population / population[NYC]`, the bare numerator emits
diagonal edges `population[d] -> relative_pop[d]` and the fixed-index
denominator emits broadcast edges `population[NYC] -> relative_pop[d]` --
2N - 1 unique edges, not N^2. For `share[R] = pop / SUM(pop[*])`, the bare
numerator and the wildcard reducer each emit their own edge sets; the
result is the union (N diagonals plus N^2 cross-pairs, deduplicated).

Structural flow-to-stock edges (an inflow or outflow's variable name does
not appear in the stock's equation, which holds only the initial value)
are emitted as same-element diagonals without AST consultation.

Multidimensional subscripts where some indices are literal and others are
wildcards (e.g., `source[NYC, *]`) are conservatively classified as
`Wildcard`. A future refinement could honor partial-fixed semantics; the
overhead is bounded today because such patterns are uncommon in real
models.

Stock names are similarly expanded: `population` with dimension `Region`
becomes `population[NYC]`, `population[Boston]`, etc. When no variables in
a model are arrayed, the element graph is identical to the variable graph
(zero overhead).

This per-reference design replaces the earlier `ElementDependencyKind`
classifier that collapsed every reference between a `(from, to)` pair to a
single kind. That collapse over-expanded fixed-index references to N^2
edges (resolving tech-debt #20) and forced the link-score partial equation
to wrap every reference uniformly in `PREVIOUS()`, breaking targets that
mixed bare and reducer references (resolving tech-debt #26). The
post-refactor measurements in
`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` show that the
element-graph SCC sizes that previously drove tech-debt #25's auto-flip
pressure on FixedIndex models are no longer inflated by spurious edges,
though `MAX_LTM_SCC_NODES = 50` was retained because WRLD3-class models
trip the gate from variable-level cycle structure rather than
element-graph artifacts.

### Link Score Classification

Three categories of element-level link scores:

**A2A same-dimension** and **scalar-to-arrayed**: The standard ceteris-paribus
link score equation is generated once with dimensions on the `LtmSyntheticVar`.
The simulation engine evaluates it per element automatically via A2A expansion.
These appear in results as a single variable occupying N slots (one per element).

**Arrayed-to-scalar (cross-dimensional)**: When an arrayed source feeds a scalar
target through a reducing function, the link represents how each source element
contributes to the scalar output. `classify_reducer` in `ltm_augment.rs` walks
the target's AST to find the reducing builtin and classify it:

| Reducer kind | Functions | Equation strategy |
|-------------|-----------|-------------------|
| Linear | SUM, MEAN | Algebraic shortcut: partial = `PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))` (divided by N for MEAN) |
| Nonlinear | MIN, MAX, STDDEV, RANK | Explicit expansion: reconstruct the reducer with all elements except the current one wrapped in `PREVIOUS()` |
| Constant | SIZE | Output depends only on dimension cardinality; link score is always 0 |

`generate_element_to_scalar_equation` produces N separate scalar link score
variables (one per source element), each with its own equation isolating that
element's contribution.

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
  participating in such cycles. Johnson runs on this restricted subgraph,
  and the results flow through the same per-circuit grouping logic the
  legacy `build_element_level_loops` uses.

Slow-path element-level circuits are grouped by their variable-level node
sequence (strip subscripts, join) to distinguish A2A loops from mixed loops:

**A2A loops**: All circuits in a group have the same variable-level structure
and every node carries a subscript. These are collapsed into a single `Loop`
with a shared ID (e.g., `r1`) and `dimensions` populated from the underlying
variables. Loop score equations are generated with those dimensions, producing
N result slots (one per element) with per-element dominance profiles. Relative
loop scores are derived post-simulation per-element by `compute_rel_loop_scores`
consumers (e.g. `libsimlin::analysis`), normalizing each element's loop score
against the per-element partition sum.

**Mixed loops**: Circuits containing scalar nodes or with inconsistent
variable-level structures. Each circuit becomes its own scalar `Loop` with a
unique ID. This handles cross-element feedback paths (e.g.,
`population[NYC] -> migration -> population[Boston] -> migration -> population[NYC]`).

### Discovery Mode

When `ltm_discovery_mode = true`, element-level discovery proceeds as:

1. `model_ltm_variables` generates link score variables for all edges. A2A link
   scores occupy N slots; cross-dimensional scores are N separate scalar
   variables.
2. Post-simulation, `discover_loops_with_graph` receives the `LtmSyntheticVar`
   list and datamodel dimensions. `parse_link_offsets` expands A2A link score
   slots into per-element edges: for each A2A link score at offset O with
   dimension of size N, it emits N `LinkOffset` entries at offsets O, O+1, ...,
   O+N-1 with element-subscripted from/to names.
3. The `SearchGraph` is built from these element-level link offsets. Element-level
   stocks (expanded from `model_element_causal_edges`) serve as DFS starting
   points.
4. Discovered element-level loops are grouped and classified identically to
   exhaustive mode via `build_element_level_loops`.

## Current Limitations

### Euler Integration Only

The corrected flow-to-stock formula uses discrete differences that assume Euler
integration. The papers note compatibility with Runge-Kutta "in principle" but
this has not been explored in the implementation.

### Performance on Very Large Models

The strongest-path algorithm runs at every saved timestep, each with O(V^2)
complexity. For very large models (1000+ variables), this could become slow. The
paper reports 10-20 seconds for Urban Dynamics (43M loops) on 2018 hardware, but
the per-saved-timestep approach is a simplification of the paper's "every
computational interval" strategy.

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
   `model_ltm_variables` in `src/simlin-engine/src/db_ltm.rs` does automatically
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
  Figure 7 from the paper (demonstrating per-stock reset recovery), per-stock
  best_score reset, deduplication, empty graph, zero-score edges (strict
  less-than allows traversal), NaN handling, self-loops, disconnected components,
  stocks without outbound edges, link offset parsing, ID assignment,
  rank-and-filter (truncation, contribution filtering, ordering preservation,
  briefly dominant loop retention, partition-aware filtering)

### Salsa Pipeline Tests

- **`db_ltm_tests.rs`**: LTM equation text generation via salsa tracked
  functions, link score caching behavior

- **`db_ltm_unified_tests.rs`**: `model_ltm_variables` for simple models,
  stdlib modules (SMOOTH), passthrough modules, and discovery mode

- **`db_ltm_module_tests.rs`**: Module-specific LTM tests: SMOOTH models
  compile with LTM, composite scores are generated for stdlib modules,
  user-defined modules with feedback receive LTM treatment

- **`db_tests.rs`** (LTM subset): Salsa LTM caching, discovery vs exhaustive
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
