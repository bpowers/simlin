# Loops That Matter (LTM): Implementation Design

This document describes how Simlin implements the Loops That Matter method for
feedback loop dominance analysis. For a comprehensive technical description of the
LTM method itself, see the [reference document](../reference/ltm--loops-that-matter.md).

## Architecture Overview

The implementation is split across three modules in `src/simlin-engine/src/`:

| Module | Responsibility |
|--------|---------------|
| `ltm.rs` | Causal graph construction, loop detection (Johnson's algorithm), static polarity analysis, cycle partitions |
| `ltm_augment.rs` | Synthetic variable generation: link score, loop score, and relative loop score equations |
| `ltm_finding.rs` | Strongest-path loop discovery algorithm for models too large for exhaustive enumeration |

The entry points are two methods on `Project` (in `project.rs`):

- **`with_ltm()`** -- Exhaustive mode. Detects all loops, generates synthetic
  variables for link scores, loop scores, and relative loop scores. Suitable for
  models with fewer than ~1000 loops.

- **`with_ltm_all_links()`** -- Discovery mode. Generates link score variables for
  every causal connection (not just those in loops). After simulation, the caller
  runs `discover_loops()` from `ltm_finding.rs` to identify important loops from
  the simulation results.

Both methods call `abort_if_arrayed()` first (LTM currently does not support
array/subscripted variables) and return a new `Project` augmented with synthetic
variables. The original project is consumed (moved) rather than mutated.
Augmented models are injected via `inject_ltm_vars()`, which patches both
user-model and stdlib-model datamodel representations so that `base_from()`
picks up the augmented versions instead of the stock generated code.

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

### Exhaustive Mode (`with_ltm`)

1. `CausalGraph::from_model()` builds the causal graph
2. `CausalGraph::find_loops()` uses Johnson's algorithm to enumerate all
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
   (`assign_deterministic_loop_ids`)
7. `generate_ltm_variables()` (`ltm_augment.rs`) creates synthetic variables for
   all links participating in any loop, plus loop score and relative loop score
   variables (with partition-scoped denominators)

### Discovery Mode (`with_ltm_all_links` + `discover_loops`)

1. `generate_ltm_variables_all_links()` (`ltm_augment.rs`) calls
   `CausalGraph::all_links()` to get every causal edge, then generates link score
   variables for all of them. Loop score variables are NOT generated at this stage.
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

- **Exhaustive mode**: `generate_loop_score_variables()` groups loops by partition.
  Each loop's relative score equation denominates only against loops in the same
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
variables** rather than as post-processing on raw results. Each link score, loop
score, and relative loop score becomes a regular auxiliary variable in the
augmented model.

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
  link adds one synthetic variable; each loop adds two (absolute and relative
  score). For a model with L links and N loops, this adds L + 2N variables.
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
| Internal link score | `$⁚ltm⁚ilink⁚{from}→{to}` |
| Pathway score | `$⁚ltm⁚path⁚{port}⁚{index}` |
| Composite score | `$⁚ltm⁚composite⁚{port}` |
| Loop score | `$⁚ltm⁚loop_score⁚{loop_id}` |
| Relative loop score | `$⁚ltm⁚rel_loop_score⁚{loop_id}` |

The `$` prefix prevents collisions with user-defined variables. The Unicode
separator `⁚` (U+205A) was chosen because it is a valid XID_Continue character
(so it works within identifiers) but is visually distinctive and virtually
never appears in user-authored equations. In generated equations, these variable
names are enclosed in double quotes (e.g., `"$⁚ltm⁚link_score⁚x→y"`) to ensure
correct parsing by the lexer.

The `discover_loops` function in `ltm_finding.rs` parses these names from
`results.offsets` by matching the prefix `$⁚ltm⁚link_score⁚` and splitting
the remainder on `→` (U+2192 RIGHTWARDS ARROW) to extract the `from` and `to`
variable names. The `ilink` prefix for internal module link scores ensures
discovery mode's parser does not accidentally ingest module-internal scores.

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
   if (TIME = PREVIOUS(TIME)) then 0
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
`TIME = PREVIOUS(TIME)` and `PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))`.

### Stock-to-Flow Links

`generate_stock_to_flow_equation()` in `ltm_augment.rs`.

Uses the standard instantaneous formula but recognizes that the "from" variable
is a stock. The flow's equation is modified by `build_partial_equation()` to
replace all non-stock dependencies with their `PREVIOUS()` values, isolating the
stock's contribution.

### Module Links

The implementation handles three module link cases in `generate_link_score_variables()`:

- **Variable-to-module-input** (`!from_is_module && to_is_module`): Uses composite
  link score reference when the module has internal causal pathways (determined by
  `compute_composite_ports()`). The link score variable references the module's
  internal composite via interpunct notation (e.g., `module·$⁚ltm⁚composite⁚port`).
  Falls back to the black-box transfer-function formula
  (`generate_module_link_score_equation`) for modules without causal pathways.

- **Module-output-to-variable** (`from_is_module && !to_is_module`): Uses the
  standard ceteris-paribus formula (`generate_link_score_equation`). The
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

### Composite Port Pre-computation

Before generating link score variables, `compute_composite_ports()` in
`ltm_augment.rs` scans all implicit (stdlib) models classified as `DynamicModule`,
builds their internal causal graphs, enumerates pathways from each input port to
the output, and records which ports have valid causal pathways. This
`CompositePortMap` is then used during link score generation to determine whether
a variable-to-module link should use the composite reference or fall back to the
black-box formula.

### How Composite Link Scores Work

1. **CausalGraph normalization**: When a variable references a module output via
   the interpunct notation (`module·output`), the edge is normalized to point to
   the module node itself (`normalize_module_ref`). This ensures the module
   participates correctly in loop detection.

2. **Internal instrumentation**: For each DynamicModule model,
   `generate_module_internal_ltm_variables()` in `ltm_augment.rs` generates:
   - Internal link score variables with the `$⁚ltm⁚ilink⁚` prefix for all
     causal links within the module
   - Pathway score variables (`$⁚ltm⁚path⁚{port}⁚{index}`) for each pathway,
     computed as the product of constituent internal link scores
   - Composite score variables (`$⁚ltm⁚composite⁚{port}`) that select the
     pathway with the largest absolute magnitude at each timestep

   These are added to the stdlib model's datamodel representation via
   `inject_ltm_vars()` in `project.rs`.

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

## Current Limitations

### Array Variables

Both `with_ltm()` and `with_ltm_all_links()` call `abort_if_arrayed()` and return
an error if the model contains array (subscripted) variables. Extending LTM to
arrays would require element-wise link score computation and a strategy for
reporting aggregate scores.

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

2. **No composite network fallback**: The papers describe a two-tier strategy
   where models with fewer than ~1000 loops use exhaustive enumeration on a
   composite (max-score) network. The implementation keeps the two modes entirely
   separate: `with_ltm()` for exhaustive and `with_ltm_all_links()` + `discover_loops()`
   for discovery. There is no automatic switching based on loop count.

3. **Module handling**: The papers describe composite link scores for macros
   (DELAY, SMOOTH) but do not discuss module boundaries as an implementation
   concept. The Simlin implementation extends the macro approach to modules:
   internal graphs are built recursively, pathways are enumerated, and composite
   scores are computed at each timestep. Module stock enrichment (adding
   module-internal stocks to loop stock lists) is an implementation-specific
   extension that enables correct cycle partitioning.

4. **PREVIOUS via stdlib module**: The `PREVIOUS()` function used in link score
   equations is implemented as a standard library module (`stdlib/previous.stmx`)
   using a stock-and-flow structure, not as a built-in function. This affects
   initial-timestep behavior: `TIME = PREVIOUS(TIME)` is used to detect the first
   timestep and return 0.

5. **Relative loop score formula**: The implementation uses
   `SAFEDIV(loop_score, sum_of_abs_scores, 0)` with explicit division-by-zero
   protection, while the papers present the formula without discussing this edge
   case. This means zero-activity periods produce 0 rather than NaN.

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
  IF-THEN-ELSE, loop score and relative loop score equations (including
  single-loop SAFEDIV behavior and single-balancing-loop negative scores),
  generated variable structure, end-to-end simulation with LTM

- **`ltm_finding.rs`**: SearchGraph construction and edge sorting, trivial loop,
  Figure 7 from the paper (demonstrating per-stock reset recovery), per-stock
  best_score reset, deduplication, empty graph, zero-score edges (strict
  less-than allows traversal), NaN handling, self-loops, disconnected components,
  stocks without outbound edges, link offset parsing, ID assignment,
  rank-and-filter (truncation, contribution filtering, ordering preservation,
  briefly dominant loop retention, partition-aware filtering)

### Integration Tests (`tests/simulate_ltm.rs`)

- **`simulates_population_ltm`**: Runs the logistic growth model with exhaustive
  LTM, validates relative loop scores against golden data from reference SD
  software (`test/logistic_growth_ltm/ltm_results.tsv`), runs on both interpreter
  and VM

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
