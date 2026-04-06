# LTM Array Support Design

## Summary

This design extends Simlin's Loops That Matter (LTM) feedback loop dominance analysis to work with arrayed (subscripted) variables -- a class of variables the LTM pipeline currently skips entirely. The core idea is that an arrayed variable like `population[Region]` with three regions is not one variable for feedback analysis purposes, but three independent participants in causal loops, each with its own link scores, loop scores, and relative loop scores.

The approach works in three layers. First, the existing LTM compilation pipeline is extended so that synthetic LTM variables can be arrayed using the engine's existing Apply-to-All (A2A) mechanism -- the same ceteris paribus formula evaluates once per array element, with the compiler's existing array expansion handling the per-element dispatch. Second, a new element-level causal graph expands variable-level edges into element-level edges (e.g., `population[NYC] -> births[NYC]`), enabling Johnson's algorithm and the strongest-path discovery algorithm to find element-specific feedback loops, including cross-dimensional paths like `population[NYC] -> migration -> population[Boston]`. Third, cross-dimensional edges (where an arrayed variable feeds into a scalar through SUM, MIN, etc.) get per-element link scores through algebraic shortcuts or explicit element expansion, depending on whether the reducing function is linear. The design reuses existing infrastructure heavily -- A2A expansion, salsa incremental compilation, the bytecode VM -- and adds zero overhead for models without arrays, since the element-level graph collapses to the variable-level graph when no dimensions are present.

## Definition of Done

Extend Simlin's LTM (Loops That Matter) implementation to support arrayed/subscripted variables by treating every array element as an independent participant in feedback loop analysis. Each element of an arrayed variable gets its own link scores, loop scores, and relative loop scores. The causal graph is expanded to element-level nodes so that element-specific feedback paths (including cross-dimensional ones like `population[NYC] -> migration -> population[Boston]`) are detected and scored independently. The discovery algorithm ("Finding the Loops that Matter") operates on this expanded graph, with its contribution threshold naturally filtering noise. Both exhaustive and discovery modes work. The design is complete and covers all edge types: arrayed-to-arrayed (same dim), arrayed-to-scalar, scalar-to-arrayed, and cross-dimensional.

**Success criteria:**
- LTM produces non-zero, per-element link and loop scores for models with arrayed stocks in feedback loops
- Element-specific feedback paths through cross-dimensional edges are detected
- Discovery mode finds important element-level loops and ranks them by contribution
- Existing scalar LTM behavior is unchanged for models without arrays

**Key exclusion:** Implementation of the full design may be phased, but the design itself covers the complete element-level case.

## Acceptance Criteria

### ltm-arrays.AC1: A2A LTM pipeline support
- **ltm-arrays.AC1.1 Success:** An `LtmSyntheticVar` with non-empty `dimensions` compiles to A2A bytecode occupying `product(dim_lengths)` slots in the layout
- **ltm-arrays.AC1.2 Success:** `PREVIOUS()` within A2A LTM equations reads per-element previous values (not the same slot for all elements)
- **ltm-arrays.AC1.3 Success:** An `LtmSyntheticVar` with empty `dimensions` compiles identically to current behavior (scalar, 1 slot)
- **ltm-arrays.AC1.4 Edge:** All existing scalar LTM tests pass unchanged after the pipeline changes

### ltm-arrays.AC2: Element-level causal graph
- **ltm-arrays.AC2.1 Success:** A2A same-dimension edge `x[D] -> z[D]` expands to `x[d] -> z[d]` for each d in D
- **ltm-arrays.AC2.2 Success:** Arrayed-to-scalar edge `x[D] -> z` (via SUM/MEAN/etc.) expands to `x[d] -> z` for each d in D
- **ltm-arrays.AC2.3 Success:** Scalar-to-arrayed edge `x -> z[D]` expands to `x -> z[d]` for each d in D
- **ltm-arrays.AC2.4 Success:** Cross-element edge (wildcard subscript in A2A equation) expands to all-to-one edges
- **ltm-arrays.AC2.5 Success:** Multi-dimensional array `x[D1,D2] -> z[D1]` expands with partial collapse
- **ltm-arrays.AC2.6 Success:** Arrayed stocks are identified as element-level stock nodes
- **ltm-arrays.AC2.7 Edge:** Model with no arrays produces an element graph identical to the variable graph (zero overhead)

### ltm-arrays.AC3: Element-level loop detection
- **ltm-arrays.AC3.1 Success:** For A2A model with N elements, N structurally-identical element-level loops are found
- **ltm-arrays.AC3.2 Success:** Cross-element loop (e.g., `population[NYC] -> migration -> population[Boston] -> migration -> population[NYC]`) is detected as a distinct loop
- **ltm-arrays.AC3.3 Success:** Element-level cycle partitions correctly group element-level stocks connected by feedback
- **ltm-arrays.AC3.4 Success:** Element-level stocks NOT connected by cross-element feedback are in separate partitions

### ltm-arrays.AC4: A2A link scores (same-dimension and scalar-to-arrayed)
- **ltm-arrays.AC4.1 Success:** A2A aux-to-aux link score produces non-zero per-element values for an arrayed feedback model
- **ltm-arrays.AC4.2 Success:** A2A flow-to-stock link score produces per-element values matching the corrected 2023 formula
- **ltm-arrays.AC4.3 Success:** A2A stock-to-flow link score produces per-element values (bug fix: `generate_stock_to_flow_equation` handles `Equation::ApplyToAll`)
- **ltm-arrays.AC4.4 Success:** Scalar-to-arrayed link score varies by element (different target element values produce different scores)
- **ltm-arrays.AC4.5 Success:** Each element's link score is computed independently using only that element's values

### ltm-arrays.AC5: Cross-dimensional link scores (arrayed-to-scalar)
- **ltm-arrays.AC5.1 Success:** `SUM(x[*])` edge produces N scalar link scores, one per element, using the algebraic shortcut
- **ltm-arrays.AC5.2 Success:** `MEAN(x[*])` edge produces N scalar link scores using the algebraic shortcut
- **ltm-arrays.AC5.3 Success:** `MIN(x[*])` edge produces N scalar link scores using explicit element expansion
- **ltm-arrays.AC5.4 Success:** `MAX(x[*])` edge produces N scalar link scores using explicit element expansion
- **ltm-arrays.AC5.5 Success:** `STDDEV(x[*])` edge produces N scalar link scores using explicit element expansion
- **ltm-arrays.AC5.6 Success:** `RANK(x, n)` edge produces N scalar link scores using explicit element expansion
- **ltm-arrays.AC5.7 Success:** `SIZE(x[*])` edge is skipped (constant, link score always 0)
- **ltm-arrays.AC5.8 Success:** Algebraic shortcut for SUM produces identical results to explicit element expansion (cross-validation)

### ltm-arrays.AC6: Loop scores and relative scores
- **ltm-arrays.AC6.1 Success:** Pure-dimension A2A loop score is the element-wise product of A2A link scores
- **ltm-arrays.AC6.2 Success:** Mixed loop (scalar + arrayed links) produces scalar loop score per element-level loop
- **ltm-arrays.AC6.3 Success:** Relative loop scores normalize within element-level cycle partitions
- **ltm-arrays.AC6.4 Success:** For a simple A2A model, each element's relative loop scores sum to ~100% independently
- **ltm-arrays.AC6.5 Success:** A2A loops share one loop ID across elements; mixed loops get individual IDs

### ltm-arrays.AC7: Discovery mode on element-level graph
- **ltm-arrays.AC7.1 Success:** Discovery mode on arrayed model finds element-specific loops
- **ltm-arrays.AC7.2 Success:** 0.1% contribution threshold filters unimportant element-level loops
- **ltm-arrays.AC7.3 Success:** Discovery cross-validates with exhaustive mode on a small arrayed model (same loops found)
- **ltm-arrays.AC7.4 Success:** `parse_link_offsets` correctly handles A2A link score names spanning N contiguous result slots
- **ltm-arrays.AC7.5 Success:** SearchGraph construction maps element-level edges to correct link score values

### ltm-arrays.AC8: Test models and backwards compatibility
- **ltm-arrays.AC8.1 Success:** Integration test with A2A arrayed feedback model passes in both exhaustive and discovery modes
- **ltm-arrays.AC8.2 Success:** Integration test with cross-element feedback model passes in both modes
- **ltm-arrays.AC8.3 Success:** All existing non-array LTM tests in `simulate_ltm.rs` pass unchanged
- **ltm-arrays.AC8.4 Success:** All existing non-array LTM tests in `db_ltm_tests.rs`, `db_ltm_unified_tests.rs`, `db_ltm_module_tests.rs` pass unchanged
- **ltm-arrays.AC8.5 Success:** Documentation accurately describes element-level LTM architecture

## Glossary

- **LTM (Loops That Matter)**: A feedback loop dominance analysis method that scores causal links and loops in a system dynamics model to determine which feedback loops drive behavior at each point in a simulation. Described in Eberlein and Schoenberg (2020).
- **Arrayed variable / subscripted variable**: A variable defined over one or more dimensions (e.g., `population[Region]`), representing multiple values indexed by dimension elements. Called "subscripted" in Vensim terminology and "arrayed" in XMILE/Simlin terminology.
- **A2A (Apply to All)**: An equation mode where a single equation template is evaluated independently for each element of a dimension. The compiler expands the template into per-element bytecode via `expand_a2a_with_hoisting`. Represented as `Equation::ApplyToAll(dims, eqn)` in the AST.
- **Element-level causal graph**: The expanded graph where each array element becomes its own node (e.g., `population[NYC]`, `population[Boston]`), enabling feedback loop detection at element granularity rather than variable granularity.
- **Link score**: A synthetic variable measuring how strongly one variable influences another at each timestep, computed via ceteris paribus re-evaluation (varying one input while holding all others at their previous values via `PREVIOUS()`).
- **Loop score**: The product of link scores around a feedback loop at each timestep, representing the loop's overall strength.
- **Relative loop score**: A loop score normalized against all other loops in the same cycle partition, expressing each loop's share of total feedback activity.
- **Cycle partition**: A group of stocks connected by feedback paths, computed as a strongly connected component (SCC) in the stock-to-stock reachability graph. Relative loop scores are only compared within the same partition.
- **Ceteris paribus**: "All else being equal" -- the technique of re-evaluating an equation while varying one input and holding all others at their previous timestep values. The foundation of link score computation.
- **Johnson's algorithm**: An algorithm for enumerating all elementary circuits (simple cycles) in a directed graph. Used in exhaustive mode to find all feedback loops.
- **Discovery mode**: LTM mode for large models where exhaustive enumeration is impractical. Generates link scores for all edges, simulates, then finds important loops post-simulation via a strongest-path depth-first search.
- **Exhaustive mode**: LTM mode that uses Johnson's algorithm to enumerate all feedback loops before simulation, then generates link/loop/relative scores only for edges participating in detected loops.
- **Synthetic variable**: A variable injected into the model by the LTM pipeline that does not appear in the user's original definition. Link scores, loop scores, and relative loop scores are all synthetic variables evaluated by the same simulation engine as regular model variables.
- **Cross-dimensional edge**: A causal edge where an arrayed source feeds into a target of a different dimension or a scalar, typically through array-reducing functions like SUM or MIN. Requires special treatment because one target value depends on all source elements.
- **Array-reducing builtin / reducer**: A builtin function that collapses an array dimension into a scalar value (SUM, MEAN, MIN, MAX, STDDEV, SIZE, RANK). The design classifies these as "linear" (SUM, MEAN) or "nonlinear" (all others) for link score generation.
- **Salsa**: An incremental computation framework used by the Simlin engine. Tracked functions are memoized and automatically recomputed only when their declared inputs change, enabling fine-grained caching of compilation results.
- **Tracked function**: A salsa function whose return value is cached and incrementally maintained. Examples: `model_causal_edges`, `model_ltm_variables`, `compute_layout`.
- **`PREVIOUS()`**: A builtin intrinsic that returns a variable's value from the prior timestep. Compiled to the `LoadPrev` VM opcode. Central to ceteris paribus link score equations.
- **`SAFEDIV(a, b, fallback)`**: A builtin that returns `a/b` when `b` is nonzero, or `fallback` otherwise. Used in link score equations to avoid division-by-zero producing NaN.
- **Composite score**: A synthetic variable summarizing how strongly a module's input propagates through its internal structure to affect its output. Acts as the "LTM interface" of a module for parent-model link score references.
- **`LtmSyntheticVar`**: The Rust struct representing a synthetic LTM variable during compilation. This design adds a `dimensions` field to it, enabling A2A (arrayed) LTM variables.
- **Interpunct notation**: The `module_instance·variable_name` syntax (using Unicode middle dot) for referencing a variable inside a module instance from the parent model. Distinguished from subscript bracket notation for array elements.
- **Tarjan's SCC algorithm**: An algorithm for finding strongly connected components in a directed graph, used to compute cycle partitions from the stock-to-stock reachability graph.
- **`Expr2` AST**: The third stage of progressive AST lowering in the compilation pipeline, after module expansion and dimension resolution. Link score generation inspects `Expr2` nodes to determine whether a source variable appears with a wildcard subscript (cross-element dependency) or a dimension reference (same-element dependency).
- **Wildcard subscript (`x[*]`)**: Notation indicating that all elements of an array are consumed, typically inside an array-reducing builtin like `SUM(x[*])`. Signals a cross-element (all-to-one) dependency in the element-level causal graph.
- **EXCEPT equations**: Arrayed variables with per-element equation overrides (Vensim EXCEPT semantics), where individual elements can have different equations from the default template.
- **Partial collapse**: Edge expansion for multi-dimensional arrays where some but not all dimensions are consumed. For example, `x[D1,D2] -> z[D1]` collapses D2 while preserving D1, producing edges `x[d1,d2] -> z[d1]` for each combination.
- **`ScopeStage0`**: The initial symbol-table scope constructed during LTM equation compilation. The design extends it to carry dimension context so the lowering pipeline can resolve dimension names and trigger A2A expansion for arrayed LTM variables.
- **SearchGraph**: A per-timestep weighted directed graph built from link score simulation results, used by discovery mode's strongest-path DFS to find the most influential feedback loops.

## Architecture

### Core Insight: Every Array Element Is an Independent Variable

An arrayed variable `population[Region]` with 3 regions is not one variable with 3 values -- from a feedback perspective, it is 3 independent variables that happen to share an equation template. Each element participates in its own causal pathways, has its own polarity, and gets its own link scores.

The design treats every array element independently by:
1. Expanding the causal graph to element-level nodes for loop detection
2. Generating per-element link scores via A2A synthetic variables (for same-dimension edges) or scalar synthetic variables (for cross-dimensional edges)
3. Running the discovery algorithm on the element-level graph, with contribution thresholds naturally filtering noise

### Element-Level Causal Graph

A new salsa tracked function `model_element_causal_edges` expands the variable-level `CausalEdgesResult` into an element-level graph. Nodes use subscript notation: `population[NYC]`, `births[NYC]`, `total_pop` (scalar nodes keep their names). This avoids collision with module interpunct notation (`module·var`).

Edge expansion rules, applied per variable-level edge `x -> z`:

| x dimensions | z dimensions | Element-level expansion |
|-------------|-------------|------------------------|
| scalar | scalar | `x -> z` (unchanged) |
| scalar | arrayed[D] | `x -> z[d]` for each d in D |
| arrayed[D] | scalar | `x[d] -> z` for each d in D |
| arrayed[D] | arrayed[D] (same-element A2A) | `x[d] -> z[d]` for each d in D |
| arrayed[D] | arrayed[D] (cross-element via SUM/etc.) | `x[d] -> z` for each d in D |
| arrayed[D1] | arrayed[D2] (different dims) | Equation analysis determines element mapping |
| arrayed[D1,D2] | arrayed[D1] (partial collapse) | `x[d1,d2] -> z[d1]` for each (d1,d2) |

**Detecting same-element vs cross-element dependency**: The expansion function examines the target variable's Expr2 AST. When the source variable appears with a wildcard subscript (`x[*]` inside a SUM, MEAN, etc.), the dependency is all-to-one. When it appears with a dimension reference (`x[DimA]` in A2A context, meaning same-element), the dependency is one-to-one.

### Link Score Variable Types

Link scores are generated per variable-level edge, classified by the source/target dimension relationship:

**Same-dimension A2A edges** (e.g., `population[D] -> births[D]`): Generate ONE A2A link score variable `$⁚ltm⁚link_score⁚population->births` with `dimensions: [D]`. The equation is the standard ceteris paribus formula with bare variable names. The compiler's A2A expansion resolves each reference to the current element automatically.

**Arrayed-to-scalar edges** (e.g., `population[D] -> total_pop`): Generate N scalar link score variables `$⁚ltm⁚link_score⁚population[NYC]->total_pop`, etc. Each equation holds all OTHER elements at their previous values while varying only the current element. Two sub-approaches depending on the reducing function:

- *Linear reducers (SUM, MEAN)*: Algebraic shortcut. For SUM: `partial[D] = PREVIOUS(target) + (source[D] - PREVIOUS(source[D]))`. For MEAN: divide by `SIZE(source[*])`. One A2A helper variable per edge.
- *Nonlinear reducers (MIN, MAX, STDDEV, RANK)*: Explicit element expansion. Each scalar equation lists all dimension elements with selective PREVIOUS wrapping. For `z = MIN(population[*])` and element NYC: `partial = MIN(population[NYC], PREVIOUS(population[Boston]), PREVIOUS(population[LA]))`.
- *SIZE*: Always constant (depends on dimension cardinality, not values). Link score is always 0. Skipped.

**Scalar-to-arrayed edges** (e.g., `growth_factor -> births[D]`): Generate ONE A2A link score variable with `dimensions: [D]`. The standard ceteris paribus formula works: the scalar source has the same PREVIOUS value for all elements, but the target varies per element.

**Flow-to-stock and stock-to-flow edges**: Same dimension classification. A2A when both share the same dimension. The existing `generate_flow_to_stock_equation` and `generate_stock_to_flow_equation` work unchanged for A2A -- the compiler handles per-element evaluation. The stock-to-flow generator needs a bug fix to handle `Equation::ApplyToAll` (currently falls through to `"0"` for non-`Scalar` equations).

### Loop Scores and Relative Scores

**Loop detection**: Johnson's algorithm (exhaustive mode) runs on the element-level causal graph. Each discovered loop is a sequence of element-level nodes.

**Pure-dimension loops** (all arrayed nodes share dimension D, no scalar intermediaries): The loop score is A2A over D. The equation is the element-wise product of A2A link scores. One loop ID shared across all elements (e.g., `b1[D]`).

**Mixed loops** (involves scalar nodes or cross-dim edges): Each element-level loop instance gets a scalar loop score. The equation references specific A2A link score elements via subscript and scalar link scores by name. Individual loop IDs per element-level path.

**Cycle partitions**: Computed from the element-level stock-to-stock reachability graph. Element-level stocks like `population[NYC]` and `population[Boston]` may be in the same partition (connected through migration) or different partitions (if no cross-element feedback exists).

**Relative loop scores**: Normalized within element-level cycle partitions. For A2A loop scores, normalization is per-element. For scalar element-level loops, each gets its own relative score.

### Discovery Mode

Discovery mode generates link scores for ALL element-level edges (not just those in detected loops), simulates, then runs the strongest-path search post-simulation on the element-level graph.

**SearchGraph construction**: The `discover_loops()` function parses link score variable names from `results.offsets`. For A2A link scores, the name maps to N contiguous slots; each element provides the edge weight for that element-level edge. For scalar cross-dim link scores, each name maps to one slot for one element-level edge.

**Search execution**: The strongest-path DFS runs on the element-level graph, starting from element-level stocks. The algorithm is unchanged; it operates on a larger graph with more starting stocks.

**Ranking and filtering**: `rank_and_filter` applies to element-level loops. The 0.1% contribution threshold naturally filters element-level loops that don't matter, surfacing only those driving behavior.

### A2A Support in the LTM Compilation Pipeline

Four localized changes enable A2A LTM synthetic variables:

1. **`LtmSyntheticVar`** (`src/simlin-engine/src/db.rs`): Add `dimensions: Vec<DimensionName>` field. Empty = scalar (backwards-compatible).

2. **`parse_ltm_equation`** (`src/simlin-engine/src/db_ltm.rs`): When `dimensions` is non-empty, produce `Equation::ApplyToAll(dims, eqn)` instead of `Equation::Scalar(eqn)`.

3. **`compute_layout` Section 3** (`src/simlin-engine/src/db.rs`): Instead of hardcoding `size: 1`, look up dimension sizes from the project's dimension context. For scalar LTM vars (empty dims), `size = 1` (unchanged). For A2A vars, `size = product of dimension lengths`.

4. **`compile_ltm_equation_fragment`** (`src/simlin-engine/src/db_ltm.rs`): The mini-layout assigns `size` based on actual dimension sizes instead of hardcoding 1. The `ScopeStage0` receives dimension context so the lowering pipeline can resolve dimension names and trigger A2A expansion.

The equation TEXT is unchanged. For A2A link scores, the same ceteris paribus formula applies per element via the compiler's existing `expand_a2a_with_hoisting`. No new A2A expansion code is needed.

### Salsa Pipeline Integration

New salsa tracked functions integrate into the existing incremental compilation pipeline:

| Function | Depends on | Returns |
|----------|-----------|---------|
| `model_element_causal_edges` | `model_causal_edges`, `project.dimensions`, variable ASTs | `ElementCausalEdgesResult` |
| `model_element_loop_circuits` | `model_element_causal_edges` | `ElementLoopCircuitsResult` |
| `model_element_cycle_partitions` | `model_element_causal_edges` | `ElementCyclePartitionsResult` |

Modified tracked functions:

| Function | Change |
|----------|--------|
| `model_ltm_variables` | Depends on element-level graph; generates A2A + scalar link scores |
| `link_score_equation_text` | Handles element-level link IDs; produces element-specific equations for cross-dim |
| `compute_layout` | Section 3 uses dimension sizes from `LtmSyntheticVar.dimensions` |
| `assemble_module` | Pass 3 handles A2A LTM variables via pipeline changes |

**Incrementality properties**:
- Adding a dimension element: element graph recomputes, new LTM variables, layout and compilation cascade
- Changing an equation but not structure: salsa backdating on `model_causal_edges`; element graph may be unchanged; only equation fragments recompile
- Toggling `ltm_enabled`: all LTM-dependent queries invalidate (existing behavior)
- Models without arrays: zero overhead (element graph = variable graph, all LTM vars scalar)

## Existing Patterns

The design follows several established patterns in the codebase:

**Salsa tracked function decomposition** (`src/simlin-engine/src/db.rs`, `src/simlin-engine/src/db_analysis.rs`): The per-model tracked function pattern (`model_causal_edges`, `model_loop_circuits`, `model_cycle_partitions`) is well-established. The new `model_element_causal_edges` follows the same pattern: a tracked function that depends on upstream queries and returns a cached result.

**LTM variable generation** (`src/simlin-engine/src/db_ltm.rs`): The unified `model_ltm_variables` function already generates link scores, loop scores, and composite scores for any model. The extension adds dimension awareness to `LtmSyntheticVar` and handles element-level edge classification within the same function.

**A2A equation compilation** (`src/simlin-engine/src/compiler/mod.rs`): The compiler already expands `Ast::ApplyToAll(dims, expr)` into per-element bytecode via `expand_a2a_with_hoisting`. The LTM pipeline change reuses this by producing `Equation::ApplyToAll` instead of `Equation::Scalar`, triggering the same expansion path.

**Module composite scoring** (`docs/design-plans/2026-03-29-ltm-module-scoring.md`): The recently-completed module scoring design established the pattern of uniform LTM treatment for all models. This design extends that principle: every model receives element-level LTM treatment when it has arrayed variables.

**Divergence from existing patterns**: The current LTM compilation path (`compile_ltm_equation_fragment`) builds a mini-layout that hardcodes `size: 1` for LTM variables and constructs a minimal `ScopeStage0` without dimension context. The A2A extension requires enriching both. This is a deliberate extension of the existing pattern, not a replacement.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: A2A LTM Pipeline Support

**Goal:** Enable LTM synthetic variables to be arrayed (A2A) without changing what variables are generated. This is the foundation for all subsequent phases.

**Components:**
- `LtmSyntheticVar` in `src/simlin-engine/src/db.rs` -- add `dimensions: Vec<DimensionName>` field
- `parse_ltm_equation` in `src/simlin-engine/src/db_ltm.rs` -- produce `Equation::ApplyToAll` when dimensions are present
- `compute_layout` Section 3 in `src/simlin-engine/src/db.rs` -- use dimension sizes for LTM variable slot allocation
- `compile_ltm_equation_fragment` in `src/simlin-engine/src/db_ltm.rs` -- pass dimension context to `ScopeStage0`, use actual size in mini-layout

**Dependencies:** None (first phase)

**Done when:** A manually-constructed `LtmSyntheticVar` with non-empty `dimensions` compiles to A2A bytecode and occupies the correct number of slots in the layout. Existing scalar LTM tests pass unchanged (empty dimensions = scalar, backwards-compatible). Tests verify: A2A LTM variable compiles, evaluates per-element, PREVIOUS works per-element within A2A LTM equations.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Element-Level Causal Graph

**Goal:** Expand the variable-level causal graph to element-level nodes, enabling element-specific loop detection.

**Components:**
- `ElementCausalEdgesResult` struct and `model_element_causal_edges` salsa tracked function in `src/simlin-engine/src/db_analysis.rs` -- expands variable-level edges to element-level using dimension context and AST analysis
- Edge expansion logic: same-element (A2A dim ref), all-to-one (wildcard/SUM), scalar identity
- AST inspection for wildcard vs dimension-ref dependency detection
- Element-level stock identification

**Dependencies:** Phase 1 (pipeline support for A2A LTM vars)

**Done when:** For a model with arrayed variables, `model_element_causal_edges` produces an element-level graph with correct edge expansion for all edge types (scalar-scalar, scalar-arrayed, arrayed-scalar, arrayed-arrayed same-dim, arrayed-arrayed cross-element). Tests verify each expansion rule from the Architecture section's table. Models without arrays produce an element graph identical to the variable graph.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Element-Level Loop Detection

**Goal:** Detect feedback loops on the element-level causal graph, including element-specific loops through cross-dimensional edges.

**Components:**
- `model_element_loop_circuits` salsa tracked function in `src/simlin-engine/src/db_analysis.rs` -- runs Johnson's algorithm on the element-level graph
- `model_element_cycle_partitions` salsa tracked function -- computes element-level stock-to-stock reachability and Tarjan's SCC
- Loop deduplication and ID assignment adapted for element-level loops

**Dependencies:** Phase 2 (element-level causal graph)

**Done when:** Johnson's algorithm finds element-level loops. For a simple A2A model with N elements, N structurally-identical loops are found (one per element). For a cross-element model (e.g., migration), element-specific loops like `population[NYC] -> migration -> population[Boston] -> migration -> population[NYC]` are detected. Cycle partitions correctly group element-level stocks connected by feedback.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: A2A Link Score Generation

**Goal:** Generate A2A link score variables for same-dimension edges and scalar-to-arrayed edges.

**Components:**
- Updated `model_ltm_variables` in `src/simlin-engine/src/db_ltm.rs` -- uses element-level graph to classify edges; generates A2A link scores with appropriate dimensions for same-dim and scalar-to-arrayed edges
- Updated `link_score_equation_text` in `src/simlin-engine/src/db_ltm.rs` -- handles A2A link score generation
- Bug fix in `generate_stock_to_flow_equation` in `src/simlin-engine/src/ltm_augment.rs` -- handle `Equation::ApplyToAll` (currently falls through to `"0"`)
- Test model: arrayed stock with A2A feedback loop (e.g., `population[Region] -> births[Region] -> population[Region]`)

**Dependencies:** Phase 1 (A2A pipeline), Phase 2 (element-level graph for edge classification)

**Done when:** A model with A2A arrayed feedback produces non-zero per-element link scores. Each element's link score is independently computed using its own variable values. A2A link score variables compile via the normal A2A expansion path. Tests cover: aux-to-aux, flow-to-stock, stock-to-flow, and scalar-to-arrayed edges.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Cross-Dimensional Link Scores (Arrayed-to-Scalar)

**Goal:** Generate per-element link scores for arrayed-to-scalar edges, covering all array-reducing builtins.

**Components:**
- New `generate_element_to_scalar_equation` in `src/simlin-engine/src/ltm_augment.rs` -- produces element-specific ceteris paribus equations
- Linear reducer shortcut (SUM, MEAN): algebraic formula using `PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))` pattern
- Nonlinear reducer expansion (MIN, MAX, STDDEV, RANK): explicit element enumeration with selective PREVIOUS wrapping
- SIZE detection: skip (constant, link score always 0)
- Scalar link score variables named `$⁚ltm⁚link_score⁚source[element]->target`
- Updated `model_ltm_variables` to generate these scalar link scores for arrayed-to-scalar edges

**Dependencies:** Phase 2 (element-level graph identifies arrayed-to-scalar edges), Phase 4 (A2A link score infrastructure)

**Done when:** A model with `total = SUM(population[*])` in a feedback loop produces N per-element link scores measuring each element's contribution to the scalar variable. Tests cover all array-reducing builtins: SUM (algebraic), MEAN (algebraic), MIN (expansion), MAX (expansion), STDDEV (expansion), RANK (expansion), SIZE (skipped). Tests verify the algebraic shortcut matches explicit expansion for SUM and MEAN.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Loop Scores and Relative Scores

**Goal:** Generate element-level loop scores and relative loop scores from the element-level link scores.

**Components:**
- Updated `generate_loop_score_variables` in `src/simlin-engine/src/ltm_augment.rs` -- handles A2A loop scores (product of A2A link scores) and scalar element-level loop scores (referencing specific elements of A2A link scores)
- Updated relative loop score generation with element-level cycle partition normalization
- Loop ID assignment: A2A loops share one ID; element-specific mixed loops get individual IDs
- `model_ltm_variables` orchestrates loop score variable generation from element-level loop circuits and link score classification

**Dependencies:** Phase 3 (element-level loop circuits and partitions), Phase 4 (A2A link scores), Phase 5 (cross-dim link scores)

**Done when:** Element-level loop scores are computed for both pure-dimension loops (A2A) and mixed loops (scalar). Relative loop scores normalize correctly within element-level partitions. For a simple A2A model, each element's relative loop score sums to ~100% independently. For a cross-element model, element-specific loops have independent scores. Integration test validates against a model with known per-element loop dominance behavior.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Discovery Mode on Element-Level Graph

**Goal:** Discovery mode operates on the element-level graph, finding important element-specific loops post-simulation.

**Components:**
- Updated `parse_link_offsets` in `src/simlin-engine/src/ltm_finding.rs` -- handles A2A link score names (mapping to N contiguous result slots) and scalar cross-dim link score names
- Updated `SearchGraph::from_results` -- builds element-level search graph from element-level link score values
- Updated `find_strongest_loops` -- DFS starts from element-level stocks
- Updated `rank_and_filter` -- element-level cycle partitions for partition-aware filtering
- `model_ltm_variables` with `ltm_discovery_mode = true` generates link scores for ALL element-level edges

**Dependencies:** Phase 5 (all link score types available), Phase 6 (element-level partitions)

**Done when:** Discovery mode on a model with arrayed feedback finds element-specific loops and ranks them by contribution. The 0.1% threshold filters unimportant element-level loops. Cross-validates: discovery mode on a small model finds the same loops as exhaustive mode. Integration test with arrayed version of the logistic growth or arms race model.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Test Models and Documentation

**Goal:** Create dedicated test models, validate end-to-end, and update documentation.

**Components:**
- New test model: arrayed population with A2A feedback (e.g., `population[Region]` with per-region births/deaths)
- New test model: cross-element feedback (e.g., migration between regions, or simplified aging chain with policy feedback)
- Integration tests in `src/simlin-engine/tests/simulate_ltm.rs` -- end-to-end validation of element-level LTM for both exhaustive and discovery modes
- Updated `docs/design/ltm--loops-that-matter.md` -- document array support, element-level graph expansion, edge classification rules
- Updated `src/simlin-engine/CLAUDE.md` -- document new tracked functions and A2A LTM support

**Dependencies:** Phase 7 (all functionality complete)

**Done when:** All integration tests pass for both test models in both exhaustive and discovery modes. Documentation accurately describes the element-level LTM architecture. Existing non-array LTM tests continue to pass unchanged.
<!-- END_PHASE_8 -->

## Additional Considerations

**Performance for large dimensions**: The element-level graph has V*E nodes (V variables, E max elements). For large dimensions (E=100+), Johnson's algorithm may be slow and discovery mode's DFS may need optimization. The 0.1% contribution threshold in `rank_and_filter` mitigates this by pruning unimportant loops early. Further optimization (sampling timesteps, pruning low-score element-level stocks from the search) can be added if profiling reveals bottlenecks.

**Multi-dimensional arrays**: Variables with 2+ dimensions (e.g., `migration[from, to]`) create element-level nodes for each combination. The edge expansion logic handles partial collapse (e.g., `migration[from, *] -> population[from]`) by the same AST inspection mechanism used for 1D arrays.

**Interaction with LTM module scoring**: The module composite scoring design (completed) operates at the variable level. Element-level LTM applies to the parent model's arrayed variables; module internals (SMOOTH, DELAY) remain at the variable level within their sub-models. A module instance like `smooth_instance` is one node in the element-level graph, even if the parent model's variables feeding it are arrayed. Module composite scores are referenced by the parent's element-level link scores via the existing interpunct notation.

**EXCEPT equations**: Arrayed variables with per-element equation overrides (Vensim EXCEPT semantics) are handled by the element-level graph expansion. Each element gets its own node, and `link_score_equation_text` uses that element's specific equation (from the `Ast::Arrayed` HashMap) rather than the default.
