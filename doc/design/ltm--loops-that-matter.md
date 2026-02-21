# Loops That Matter (LTM): Implementation Design

This document describes how Simlin implements the Loops That Matter method for
feedback loop dominance analysis. For a comprehensive technical description of the
LTM method itself, see the [reference document](../reference/ltm--loops-that-matter.md).

## Architecture Overview

The implementation is split across three modules in `src/simlin-engine/src/`:

| Module | Responsibility |
|--------|---------------|
| `ltm.rs` | Causal graph construction, loop detection (Johnson's algorithm), static polarity analysis |
| `ltm_augment.rs` | Synthetic variable generation: link score, loop score, and relative loop score equations |
| `ltm_finding.rs` | Strongest-path loop discovery algorithm for models too large for exhaustive enumeration |

The entry points are two methods on `Project` (`src/simlin-engine/src/project.rs:37-94`):

- **`with_ltm()`** -- Exhaustive mode. Detects all loops, generates synthetic
  variables for link scores, loop scores, and relative loop scores. Suitable for
  models with fewer than ~1000 loops.

- **`with_ltm_all_links()`** -- Discovery mode. Generates link score variables for
  every causal connection (not just those in loops). After simulation, the caller
  runs `discover_loops()` from `ltm_finding.rs` to identify important loops from
  the simulation results.

Both methods return a new `Project` augmented with synthetic variables. The
original project is consumed (moved) rather than mutated.

## Key Data Structures

### CausalGraph (`ltm.rs:151-160`)

```rust
pub struct CausalGraph {
    edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    stocks: HashSet<Ident<Canonical>>,
    variables: HashMap<Ident<Canonical>, Variable>,
    module_graphs: HashMap<Ident<Canonical>, Box<CausalGraph>>,
}
```

The adjacency-list representation of a model's causal structure. Built from a
`ModelStage1` by `CausalGraph::from_model()` (`ltm.rs:164`), which:

- Creates edges from each variable's equation dependencies to the variable itself
- Handles stocks specially: edges come from inflows and outflows, not from the
  stock's initial-value equation
- Recursively builds sub-graphs for module instances (`module_graphs`), enabling
  cross-module loop detection

### Link (`ltm.rs:27-31`)

A single causal connection between two variables, with a statically-analyzed
polarity (Positive, Negative, or Unknown).

### Loop (`ltm.rs:36-41`)

A feedback loop: a list of `Link`s forming a closed path, the stocks it contains,
a polarity classification, and a deterministic ID (e.g., `r1`, `b2`, `u1`).

### FoundLoop (`ltm_finding.rs:70-78`)

Produced by discovery mode. Wraps a `Loop` with its signed score timeseries
and average absolute score for ranking.

## Two Modes of Operation

### Exhaustive Mode (`with_ltm`)

1. `CausalGraph::from_model()` builds the causal graph
2. `CausalGraph::find_loops()` uses Johnson's algorithm (`ltm.rs:235`) to enumerate
   all elementary circuits via DFS (`dfs_circuits`, `ltm.rs:289`)
3. Cross-module loops are detected separately (`find_cross_module_loops`, `ltm.rs:322`)
4. Loops are deduplicated by node set (`deduplicate_loops`, `ltm.rs:704`)
5. Deterministic IDs are assigned by sorting loops by their content key
   (`assign_deterministic_loop_ids`, `ltm.rs:663`)
6. `generate_ltm_variables()` (`ltm_augment.rs:86`) creates synthetic variables for
   all links participating in any loop, plus loop score and relative loop score
   variables

### Discovery Mode (`with_ltm_all_links` + `discover_loops`)

1. `generate_ltm_variables_all_links()` (`ltm_augment.rs:97`) calls
   `CausalGraph::all_links()` (`ltm.rs:587`) to get every causal edge, then
   generates link score variables for all of them. Loop score variables are NOT
   generated at this stage.
2. The augmented project is simulated normally (interpreter or VM).
3. Post-simulation, `discover_loops()` (`ltm_finding.rs:321`) runs the
   strongest-path algorithm at each saved timestep:
   - Parses link score variable names from `results.offsets` (`parse_link_offsets`,
     `ltm_finding.rs:262`)
   - Builds a `SearchGraph` per timestep from the link score values
     (`SearchGraph::from_results`, `ltm_finding.rs:112`)
   - Runs the strongest-path DFS (`find_strongest_loops`, `ltm_finding.rs:136`)
   - Collects unique loop paths across all timesteps
4. Each discovered path is converted to a `FoundLoop` with signed loop scores
   computed at every timestep from the raw link score results
5. Loops are ranked by average absolute score, truncated to `MAX_LOOPS` (200),
   filtered by `MIN_CONTRIBUTION` (0.1%), and assigned deterministic IDs
   (`rank_and_filter`, `ltm_finding.rs:469`)

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
the remainder on `→` to extract the `from` and `to` variable names
(`parse_link_offsets`, `ltm_finding.rs`).

## Link Score Equations

Three categories of link score equations are generated, corresponding to the
three link types in the LTM method.

### Auxiliary-to-Auxiliary (Instantaneous Links)

`generate_auxiliary_to_auxiliary_equation()` (`ltm_augment.rs:349`)

For a link from `x` to `z` where `z = f(x, y, ...)`:

1. Take the equation text of `z`
2. Replace every dependency except `x` with `PREVIOUS(dep)` using whole-word
   substitution (`replace_whole_word`, `ltm_augment.rs:27`)
3. This creates the ceteris-paribus partial equation
4. The link score is: `|partial_eq - PREVIOUS(z)| / |z - PREVIOUS(z)| * sign(...)`

The whole-word replacement uses Unicode XID rules (`is_word_char`,
`ltm_augment.rs:79`) to avoid replacing substrings of longer identifiers
(e.g., replacing `x` in `x_rate` would be incorrect).

### Flow-to-Stock Links

`generate_flow_to_stock_equation()` (`ltm_augment.rs:415`)

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
is applied outside the absolute value. This equation returns NaN for the first
two timesteps (insufficient history for second-order differences).

### Stock-to-Flow Links

`generate_stock_to_flow_equation()` (`ltm_augment.rs:444`)

Uses the standard instantaneous formula but recognizes that the "from" variable
is a stock. The flow's equation is modified by replacing all non-stock
dependencies with their `PREVIOUS()` values, isolating the stock's contribution.

### Module Links (Black Box)

`generate_module_link_score_equation()` (`ltm_augment.rs:200`)

Module connections use a simplified transfer-function formula. The implementation
checks three cases: module-output-to-variable, variable-to-module-input, and
module-to-module. All three use the same basic formula measuring total change
in the target relative to total change in the source.

## Module Boundary Handling

The implementation uses **composite link scores** for dynamic modules (SMOOTH,
DELAY, TREND, etc.), following Section 6 of Schoenberg & Eberlein (2020). The
composite score is the product of internal link scores along the strongest
internal pathway at each timestep.

### Module Classification

Modules are classified by `classify_module_for_ltm()` (`ltm.rs`):

- **Infrastructure** (PREVIOUS, INIT) -- used BY link score equations; never
  analyzed to avoid infinite recursion.
- **DynamicModule** -- has internal stocks (SMOOTH, DELAY, TREND, user-defined
  modules with stocks). Gets composite link scores.
- **Passthrough** -- no internal stocks; treated as black box with a transfer
  score formula.

### How Composite Link Scores Work

1. **CausalGraph normalization**: When a variable references a module output via
   the interpunct notation (`module·output`), the edge is normalized to point to
   the module node itself (`normalize_module_ref` in `ltm.rs`). This ensures the
   module participates correctly in loop detection.

2. **Internal instrumentation**: For each DynamicModule model, `ltm_augment.rs`
   generates internal link score variables with the `$⁚ltm⁚ilink⁚` prefix.
   These are added to the stdlib model's datamodel representation via
   `inject_ltm_vars()` in `project.rs`, which adds augmented stdlib models to
   the datamodel. `base_from()` detects these overrides by name and skips
   loading the stock version from generated code.

3. **Pathway enumeration**: `enumerate_module_pathways()` in `ltm.rs` finds all
   simple paths from each input port to the output variable within the module's
   internal causal graph. For smth1, the sole pathway is `input -> flow -> output`.

4. **Pathway scores**: For each pathway, a variable like `$⁚ltm⁚path⁚input⁚0`
   is generated whose equation is the product of constituent internal link scores.

5. **Composite selection**: For each input port, a composite variable
   `$⁚ltm⁚composite⁚{port}` selects the pathway with the largest absolute
   magnitude at each timestep. For a single pathway (most common), this is
   just the pathway score. For multiple pathways, a nested `if ABS(p1) >= ABS(p2)
   then p1 else p2` chain is generated.

6. **Parent model reference**: The parent model's link score for
   `input_src -> module_instance` references the module's composite via
   interpunct notation: `"module·$⁚ltm⁚composite⁚port"`. The compiler resolves
   this through the standard `module·var` mechanism in `context.rs`.

### Variable Naming Conventions

- **Parent link scores**: `$⁚ltm⁚link_score⁚{from}→{to}` (arrow separator
  avoids ambiguity with module idents containing `⁚`)
- **Internal link scores**: `$⁚ltm⁚ilink⁚{from}→{to}` (the `i` prefix
  ensures discovery mode's `parse_link_offsets()` does not ingest them)
- **Pathway scores**: `$⁚ltm⁚path⁚{port}⁚{index}`
- **Composite scores**: `$⁚ltm⁚composite⁚{port}`

### Loop Suppression

Internal module-only loops (e.g., smth1's `output -> flow -> output`) are not
reported in the parent model's loop list. The parent DFS traverses module nodes
as opaque vertices and does not descend into module internals. Cross-module loops
(where a loop passes through a module connecting to external variables) are
detected by `find_cross_module_loops()` and reported with module nodes in the path.

## Polarity Analysis

### Static Polarity

`analyze_link_polarity()` (`ltm.rs:734`) determines link polarity from the AST
at compile time. The analysis handles:

- **Addition/subtraction**: Addition preserves polarity; subtraction flips the
  right operand (`ltm.rs:835-852`)
- **Multiplication**: Combines polarities (positive * negative = negative);
  checks if one operand is a constant with known sign (`ltm.rs:853-909`)
- **Division**: Numerator preserves, denominator flips (`ltm.rs:910-922`)
- **Unary negation and NOT**: Flip polarity (`ltm.rs:925-933`)
- **IF-THEN-ELSE**: Returns the common polarity if both branches agree, Unknown
  otherwise (`ltm.rs:934-954`)
- **Lookup tables**: Analyzes monotonicity of graphical functions
  (`analyze_graphical_function_polarity`, `ltm.rs:1010-1052`)
- **Flow-to-stock**: Inflows are Positive, outflows are Negative
  (`ltm.rs:614-623`)

If any link in a loop has Unknown polarity, the loop's structural polarity is
classified as Undetermined (`calculate_polarity`, `ltm.rs:637`).

### Runtime Polarity

`LoopPolarity::from_runtime_scores()` (`ltm.rs:96`) classifies polarity based
on actual simulation results: all-positive means Reinforcing, all-negative means
Balancing, mixed means Undetermined. This catches cases where nonlinear dynamics
cause polarity to change during simulation (e.g., the yeast alcohol model from
the papers).

## Strongest-Path Algorithm

The implementation in `ltm_finding.rs` follows Appendix I of Eberlein &
Schoenberg (2020) closely.

### SearchGraph (`ltm_finding.rs:59-64`)

Built per timestep from simulation results. Edges are sorted by absolute score
descending (`from_edges`, `ltm_finding.rs:82`), providing the ~3x speedup
described in the paper by making the first visit to each variable likely the
strongest.

### Core DFS (`check_outbound_uses`, `ltm_finding.rs:185-237`)

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
- **Deduplication**: `add_loop_if_unique` (`ltm_finding.rs:240`) uses sorted node
  sets to prevent recording the same loop twice.

### Heuristic Nature

The algorithm does not guarantee finding the truly strongest loop. The specific
failure mode (demonstrated in the paper's Figure 7 and tested in
`test_figure_7_paper` at `ltm_finding.rs:624`) is that visiting a node via a
strong path sets a high `best_score` that prunes exploration via weaker paths
that might lead to different (but still valid) loops.

The mitigation is twofold: (a) running the search at every timestep with different
link scores tends to discover loops missed at other timesteps, and (b) resetting
`best_score` per stock means different starting stocks can discover different
loops. The papers' empirical evaluation shows that missed loops are consistently
"siblings" of found loops, differing by only a few links.

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
the per-timestep saved-step approach is a simplification of the paper's "every
computational interval" strategy.

### Module Internal Loops

The black-box approach means internal feedback loops within modules are not
analyzed by LTM. If a module contains internal feedback that is relevant to
the parent model's behavior, it will be represented only by the aggregate
transfer score.

## Divergences from the Papers

1. **Per-timestep vs. per-dt search**: The papers describe running the
   strongest-path search at "every (or almost every) point in time," meaning each
   DT step. The implementation runs at each saved timestep (determined by
   `save_step` in sim specs), which may be coarser. This is noted in
   `doc/ltm-finding.md` as an intentional simplification.

2. **No composite network fallback**: The papers describe a two-tier strategy
   where models with fewer than ~1000 loops use exhaustive enumeration on a
   composite (max-score) network. The implementation keeps the two modes entirely
   separate: `with_ltm()` for exhaustive and `with_ltm_all_links()` + `discover_loops()`
   for discovery. There is no automatic switching based on loop count.

3. **Module handling**: The papers do not discuss module boundaries. The black-box
   approach is an extension specific to this implementation.

4. **PREVIOUS via stdlib module**: The `PREVIOUS()` function used in link score
   equations is implemented as a standard library module (`stdlib/previous.stmx`)
   using a stock-and-flow structure, not as a built-in function. This affects
   initial-timestep behavior: `TIME = PREVIOUS(TIME)` is used to detect the first
   timestep and return NaN.

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

## Test Coverage

### Unit Tests

- **`ltm.rs`**: Loop detection on known models (reinforcing, balancing, no-loop,
  module, multi-module), polarity analysis (AST expressions, graphical functions,
  flow-to-stock), runtime polarity classification, deduplication, deterministic
  ID assignment, real model tests (fishbanks, logistic growth)

- **`ltm_augment.rs`**: Equation generation for all link types
  (auxiliary-to-auxiliary, flow-to-stock, outflow-to-stock, stock-to-flow,
  module links), whole-word replacement, loop score and relative loop score
  equations, dollar-sign variable parsing

- **`ltm_finding.rs`**: SearchGraph construction and sorting, trivial loop, Figure
  7 from the paper, per-stock best_score reset, deduplication, empty graph,
  zero-score edges, NaN handling, self-loops, disconnected components, link offset
  parsing, ID assignment, rank-and-filter (truncation, contribution filtering,
  ordering)

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
