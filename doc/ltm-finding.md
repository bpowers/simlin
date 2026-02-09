# Finding the Loops That Matter: Implementation Plan

## Assumptions and Clarifications Needed

### Assumptions Made

1. **Algorithm scope**: We implement the "strongest path" loop discovery algorithm from Eberlein & Schoenberg (2020), a heuristic that finds important loops without exhaustive enumeration. It complements (not replaces) the existing exhaustive Johnson's algorithm in `ltm.rs`.

2. **Two-phase approach**: The paper describes a two-tier strategy:
   - First, build a "composite feedback structure" using `max(|link_score|)` over all timesteps for each link, then enumerate loops exhaustively on that composite graph.
   - If fewer than ~1000 loops are found, use those directly (no need for per-timestep search).
   - If >= 1000 loops, fall back to per-timestep strongest-path search.

3. **Per-timestep discovery**: The paper's final algorithm runs loop discovery at each computational interval (or a subset). We run the strongest-path search at each **saved** timestep as a post-processing step on simulation results. This is a simplification: the paper says "every (or almost every) point in time," but saved timesteps are sufficient and much simpler to implement. This deviation should be noted.

4. **Link scores are already computed**: Our existing `ltm_augment.rs` generates synthetic variables that compute link scores during simulation. The strongest-path algorithm consumes these. No changes to link score computation formulas are needed; however, we need to generate link score variables for ALL causal links (not just those in known loops) to support discovery mode.

5. **Loop uniqueness**: Two loops are considered identical if they contain the same set of nodes (consistent with the existing `deduplicate_loops()` in `CausalGraph`, which uses node sets). The paper: "Need to check loop for uniqueness. If we already have it, then ignore it."

6. **Absolute values for search, signed values for scores**: During the DFS traversal, accumulated path scores use `|link_score|` (absolute values) for pruning -- we want the largest magnitude. After a loop is found, the actual loop score is computed as the signed product of link scores. The paper's Appendix I uses `score * link.score` (without explicit absolute value), but the pruning comparison `score < variable.best_score` is a magnitude comparison with initial score 1.0, so using absolute values for the search is the correct interpretation.

### Clarifications Needed / Open Questions

1. **Subset of timesteps**: The paper mentions "a subset of those based on performance tuning." Our default: run at every saved timestep. If this proves too slow for large models, we can add a stride parameter later.

2. **Threshold for switching**: The paper uses 1000 loops. We use this value as a constant. If it needs tuning, it's a one-line change.

3. **NaN handling**: At t=0, link scores are NaN (PREVIOUS values don't exist). Flow-to-stock scores are NaN for two timesteps. We treat NaN as 0 for discovery purposes and skip timestep 0 entirely.

4. **Figure 7 edge list**: The paper's Figure 7 edge labels may be ambiguous in the PDF rendering. We need the exact edge list to create a correct unit test. Based on the paper's text describing path scores: a->d gives accumulated 100, d->c gives accumulated 10 (so d->c weight = 0.1), c->? leads to b with accumulated 100 (unclear). **We should verify the exact graph against Vensim/Stella or by contacting the authors.** For now, we use the most likely reading: a->b:10, a->d:100, b->c:10, c->a:10, d->c:0.1, d->b:100, and trace the algorithm carefully in the test.

---

## Background

### Problem

The existing LTM implementation uses Johnson's algorithm to exhaustively enumerate all feedback loops. This works for small models but does not scale: the number of loops grows factorially with the number of stocks. The Urban Dynamics model has 43,722,744 feedback loops.

### Solution from the Paper

Eberlein & Schoenberg (2020) describe a "strongest path" heuristic:

1. **Independent loop sets are insufficient**: The shortest independent loop set (Kampmann, Oliva) is based on static structure and misses dynamically important loops. The three-party arms race demonstrates this: the two long loops (A->B->C->A and A->C->B->A) drive long-term behavior but are NOT in the shortest independent set.

2. **Composite feedback structure fails**: Building a single composite score per link (max or average over time) and searching that doesn't reliably find the most important loops. Max biases toward long loops with numeric overflow risk; average biases toward short loops.

3. **Per-timestep search works**: Running the strongest-path search at each timestep using actual link scores finds important loops. Each search is fast because it is guided by link score magnitudes.

### The Strongest Path Algorithm

The algorithm has two parts: preprocessing (once per timestep) and a recursive DFS (once per stock per timestep).

**Preprocessing (once per timestep):**
```
For each variable in the model:
    For each link from variable:
        Set link.score from the link score at this timestep
    Sort outbound links by |link.score| descending
    Set variable.best_score = 0
```

**Search (once per stock):**
```
For each stock in the model:
    Set TARGET = stock
    Call Check_outbound_uses(stock, 1.0)
```

Note: `best_score` values are reset once per timestep in the preprocessing step and **persist across all stock iterations** within that timestep. This is critical -- the paper's Appendix I shows the reset happening before the stock loop, not inside it.

**The `Check_outbound_uses(variable, score)` function** (from Appendix I):
```
If variable.visiting is true:
    If variable = TARGET:
        Add_loop_if_unique(STACK, variable)
    End if
    Return
End if
If score < variable.best_score:
    Return
End if
Set variable.best_score = score
Set variable.visiting = true
Add variable to STACK
For each link from variable:
    Call Check_outbound_uses(link.variable, score * |link.score|)
End for each
Set variable.visiting = false
Remove variable from STACK
```

**Key properties:**
- Complexity is roughly O(V^2) per timestep (similar to Dijkstra's shortest path)
- Sorting edges by score makes the first visit to a variable likely the strongest
- The `best_score` check prunes weaker paths early
- Starting from every stock ensures all loops are reachable (all SD feedback loops involve a stock)
- The `visiting` flag tracks nodes on the current DFS path only (not all visited nodes)
- The comparison `score < variable.best_score` uses **strict less-than**: equal scores DO explore further
- This is a heuristic -- it is NOT guaranteed to find the absolute strongest loop (see Figure 7 example below)

---

## High-Level Goals

1. **Implement the strongest-path loop discovery algorithm** as described in Appendix I of Eberlein & Schoenberg (2020).
2. **Integrate with existing LTM infrastructure**: Use computed link scores from simulation results.
3. **Validate correctness** against hand-computed examples and against exhaustive enumeration on small models.
4. **Target 90+% code coverage** with unit tests, integration tests, and golden data validation.

---

## Concrete Implementation Steps

### Phase 1: Core Algorithm and Unit Tests

**File: `src/simlin-engine/src/ltm_finding.rs` (new file)**

This file contains the entire strongest-path algorithm. The public API is a single function and one result type.

#### Data Structures

```rust
use crate::common::{Canonical, Ident, Result};
use crate::ltm::Loop;
use crate::results::Results;
use crate::project::Project;

// --- Constants (from the paper) ---

/// Maximum loops to retain after discovery (paper uses 200)
const MAX_LOOPS: usize = 200;

/// Minimum average relative contribution to keep a loop (paper uses 0.1%)
const MIN_CONTRIBUTION: f64 = 0.001;

// --- Internal types ---

/// An outbound edge in the search graph: target variable and |link_score|.
struct ScoredEdge {
    to: Ident<Canonical>,
    score: f64, // absolute value of link score at this timestep
}

/// The search graph for one timestep: adjacency list with edges sorted by |score| desc.
struct SearchGraph {
    /// variable -> outbound edges, sorted by |score| descending
    adj: HashMap<Ident<Canonical>, Vec<ScoredEdge>>,
    /// stock variables (search starts from each stock)
    stocks: Vec<Ident<Canonical>>,
}

// --- Public types ---

/// A loop found by the strongest-path algorithm, with its scores over time.
pub struct FoundLoop {
    /// The loop structure (reuses existing Loop type from ltm.rs)
    pub loop_info: Loop,
    /// Loop score at each timestep: (time, signed_score)
    /// The signed score is the product of the signed link scores.
    pub scores: Vec<(f64, f64)>,
    /// Average |score| over the simulation (for ranking/filtering)
    pub avg_abs_score: f64,
}
```

**Design notes:**
- `ScoredEdge` stores only `to` and `score` -- the `from` is implicit in the adjacency list key.
- `SearchGraph` is private to this module. The only public API is the `discover_loops` function.
- `FoundLoop` reuses the existing `Loop` type from `ltm.rs` for the structural information, adding runtime score data.

#### Public API

```rust
/// Run the strongest-path loop discovery on simulation results.
///
/// Reads link score values from `results` (computed during simulation via
/// LTM synthetic variables), then runs the strongest-path DFS at each saved
/// timestep to discover important loops.
///
/// The project must have been augmented with `with_ltm(all_links=true)` before
/// simulation so that link score variables exist for all causal links.
pub fn discover_loops(
    results: &Results,
    project: &Project,
) -> Result<Vec<FoundLoop>>
```

#### Algorithm Implementation

The core search is implemented as methods on `SearchGraph`:

```rust
impl SearchGraph {
    /// Build from a list of (from, to, score) triples.
    fn from_edges(
        edges: Vec<(Ident<Canonical>, Ident<Canonical>, f64)>,
        stocks: Vec<Ident<Canonical>>,
    ) -> Self {
        // Build adjacency list, sort each edge list by |score| descending
    }

    /// Build from simulation results at a specific timestep.
    ///
    /// Scans results.offsets for variables matching the LTM link score prefix
    /// "$⁚ltm⁚link_score⁚{from}⁚{to}" (⁚ = U+205A), reads values
    /// at the given step, and builds the adjacency list.
    ///
    /// NaN values (at initial timesteps) are treated as 0.
    ///
    /// Results layout: results.data is a flat [f64] array. results.step_size
    /// is the number of variables per timestep. results.step_count is the
    /// number of timesteps. Value at step t for variable with offset o is at
    /// results.data[t * results.step_size + o].
    /// results.iter() yields chunks of step_size elements, one per timestep.
    fn from_results(results: &Results, step: usize, project: &Project) -> Result<Self>

    /// Run the strongest-path search, returning discovered loop paths.
    /// Each returned path is a Vec<Ident<Canonical>> of variables forming
    /// the loop (first element = last element = a stock).
    fn find_strongest_loops(&self) -> Vec<Vec<Ident<Canonical>>>
}
```

The `find_strongest_loops` method implements the paper's algorithm:

1. Initialize `best_score: HashMap<Ident<Canonical>, f64>` with 0 for all variables.
2. For each stock, set `TARGET = stock` and call `check_outbound_uses`.
3. `best_score` persists across stock iterations (NOT reset per-stock).
4. Collect discovered paths, deduplicate by node set.

#### Converting discovered paths to `Loop` objects

After the DFS finds paths, they must be converted to the existing `Loop` type from `ltm.rs`. This requires:
1. Converting the path to a sequence of `Link` objects (with polarity)
2. Identifying which variables in the path are stocks
3. Computing structural polarity

The `CausalGraph` already has helper methods for this: `circuit_to_links()`, `find_stocks_in_loop()`, `calculate_polarity()`. The `discover_loops` function should build a `CausalGraph` from the project and use these methods. Alternatively, the path-to-Loop conversion can be done directly using the project's variable information.

#### Computing signed loop scores

After discovering a loop's structure, compute its actual signed score at each timestep by reading the individual link score variables from `results` and multiplying them (preserving sign). This is distinct from the unsigned search score used during the DFS.

#### TDD test cases for Phase 1

All tests should be in `#[cfg(test)] mod tests` within `ltm_finding.rs`.

1. **SearchGraph construction**: Build from known edges, verify sorting and structure.

2. **Trivial loop**: One stock, one flow, one loop. Verify exactly one loop found.

3. **Figure 7 from the paper** (critical test):

   Edges (from paper's diagram): a->b:10, a->d:100, b->c:10, c->a:10, d->c:0.1, d->b:100.
   All nodes are stocks for this test.

   Trace through the algorithm step by step:
   - **Preprocessing**: Sort each node's outbound edges by |score| desc.
     - a: [(d, 100), (b, 10)]
     - b: [(c, 10)]
     - c: [(a, 10)]
     - d: [(b, 100), (c, 0.1)]
   - **TARGET = a** (first stock):
     - `check(a, 1.0)`: best_score[a]=1.0, visiting={a}, path=[a]
       - Edge a->d (score 100): `check(d, 100)`
         - best_score[d]=100, visiting={a,d}, path=[a,d]
           - Edge d->b (score 100): `check(b, 100*100=10000)`
             - best_score[b]=10000, visiting={a,d,b}, path=[a,d,b]
               - Edge b->c (score 10): `check(c, 10000*10=100000)`
                 - best_score[c]=100000, visiting={a,d,b,c}, path=[a,d,b,c]
                   - Edge c->a (score 10): `check(a, 100000*10=1000000)`
                     - a is visiting AND a=TARGET: **FOUND loop [a,d,b,c]** (score 1M)
                 - unvisit c
             - unvisit b
           - Edge d->c (score 0.1): `check(c, 100*0.1=10)`
             - 10 < best_score[c]=100000: **PRUNED**
         - unvisit d
       - Edge a->b (score 10): `check(b, 10)`
         - 10 < best_score[b]=10000: **PRUNED**
     - unvisit a
   - **TARGET = b**: `check(b, 1.0)`
     - 1.0 < best_score[b]=10000: **PRUNED** (best_score persists!)
   - **TARGET = c**: `check(c, 1.0)`
     - 1.0 < best_score[c]=100000: **PRUNED**
   - **TARGET = d**: `check(d, 1.0)`
     - 1.0 < best_score[d]=100: **PRUNED**

   **Result**: Only one loop found: [a, d, b, c] with search score 1,000,000.
   The loops a->b->c->a (search score 1000) and a->d->c->a (search score 100) are NOT found because the a->d->b path dominates and sets high best_scores that prune other paths.

   This demonstrates the heuristic nature: the algorithm finds ONE strong loop but misses others. The paper acknowledges this: "It is however, possible (though messy) to create diagrams where starting anywhere would fail to find the strongest loop."

   **Note**: With different models/timesteps, the algorithm runs again with different link scores. Over many timesteps, loops that are missed at one time may be found at another.

4. **Multi-stock with best_score persistence**: Verify that best_score values set while searching from stock A correctly prune paths when searching from stock B.

5. **Loop deduplication**: Same loop found from different starting stocks (if best_score allows) appears only once.

6. **Empty graph**: No edges -> no loops found.

7. **Zero-score links**: Links with score 0 should not contribute to loops (score * 0 = 0, which is not > best_score of 0 since we use strict less-than... actually 0 is NOT < 0, so it WOULD proceed). Test this edge case: a link with score 0 should still be traversed but won't improve best_score beyond 0.

8. **NaN handling**: NaN scores treated as 0.

### Phase 2: Integration with Results and End-to-End Pipeline

**Files: `src/simlin-engine/src/ltm_finding.rs`, `src/simlin-engine/src/ltm_augment.rs`, `src/simlin-engine/src/ltm.rs`, `src/simlin-engine/src/project.rs`**

This phase wires the algorithm into the existing infrastructure.

#### 2a: Generate link scores for ALL causal links

Currently, `generate_ltm_variables()` in `ltm_augment.rs` only generates link score variables for links that participate in detected loops. For discovery mode, we need link scores for ALL causal links in the model.

**Changes to `ltm.rs`:**
- Add a public method `CausalGraph::all_links(&self) -> Vec<Link>` that iterates over `self.edges` and returns a `Link` for each edge, computing polarity via the existing `get_link_polarity()` method. This gives us every causal connection in the model.

**Changes to `ltm_augment.rs`:**
- Modify `generate_ltm_variables()` to accept an `all_links: bool` parameter.
- When `all_links` is true: skip loop detection, call `CausalGraph::all_links()` to get every causal link, generate link score variables for all of them, and do NOT generate loop score or relative loop score variables (those will be computed post-simulation by `discover_loops`).
- When `all_links` is false: existing behavior (generate for loop links only).

**Changes to `project.rs`:**
- Modify `with_ltm()` to accept an `all_links: bool` parameter (or add `with_ltm_all_links()` as a separate method -- the simpler option since it avoids changing the existing API).

**Variable name format**: Link score variables use the naming convention `$⁚ltm⁚link_score⁚{from}⁚{to}` where `⁚` is Unicode codepoint U+205A (TWO DOT PUNCTUATION). Copy this character exactly from the existing code in `ltm_augment.rs`. The `discover_loops` function parses these names from `results.offsets` by matching the prefix `"$⁚ltm⁚link_score⁚"` and splitting the remainder on `'⁚'` to extract the `from` and `to` variable names.

**Results layout**: `Results` stores data in `Box<[f64]>` with layout `[var0_t0, var1_t0, ..., varN_t0, var0_t1, ...]`. Each variable has an offset in `results.offsets: HashMap<Ident<Canonical>, usize>`. To read link score for variable with offset `o` at step `t`: `results.data[t * results.step_size + o]`. The `results.step_size` field gives the number of values per timestep.

#### 2b: End-to-end orchestration in `discover_loops`

The `discover_loops` function:

1. Scans `results.offsets` for link score variables, building a mapping of `(from, to) -> offset`.
2. For each saved timestep `t` (skipping t=0 where scores are NaN):
   a. Reads link scores from results at step `t`.
   b. Builds a `SearchGraph` with edges sorted by |score| descending.
   c. Runs `find_strongest_loops()` to get discovered paths.
   d. Deduplicates against previously found loops (by node set).
3. Converts all unique discovered paths to `Loop` objects using project variable information.
4. For each unique loop, computes the signed loop score at every timestep by reading and multiplying the constituent link scores from results.
5. Computes average |score| for each loop.
6. Sorts by average |score| descending.
7. Truncates to `MAX_LOOPS` (200) and filters out loops with avg contribution < `MIN_CONTRIBUTION` (0.1%).

#### 2c: Edge cases to handle

- **Implicit models**: Skip them (stdlib models like PREVIOUS). Check `model.implicit`.
- **Array variables**: `abort_if_arrayed()` must still be enforced. Discovery mode inherits this restriction.
- **Module links**: The existing `generate_link_score_variables` handles module-to-var, var-to-module, and module-to-module links. Discovery mode uses the same code path.
- **NaN at early timesteps**: Flow-to-stock link scores are NaN for the first TWO timesteps (need `PREVIOUS(PREVIOUS(...))`). Skip these timesteps or treat NaN as 0.

#### TDD test cases for Phase 2

1. **`CausalGraph::all_links()` unit test**: Build a `CausalGraph` from a model with known structure, verify it returns all edges with correct polarity.

2. **`with_ltm_all_links()` test**: Create a model, call `with_ltm_all_links()`, verify link score variables exist for ALL causal links (including those not in any loop, like `carrying_capacity -> fraction_of_carrying_capacity_used` in the logistic growth model).

3. **`SearchGraph::from_results()` test**: Run simulation on logistic growth model with all-links mode, build SearchGraph at a mid-simulation timestep, verify edge scores match expected values.

4. **Full pipeline - logistic growth**: `with_ltm_all_links()` -> simulate -> `discover_loops()` -> verify the 2 loops (B1 and U1) are found with relative scores matching `test/logistic_growth_ltm/ltm_results.tsv` within tolerance.

5. **Full pipeline - cross-validation**: For the logistic growth model, run both exhaustive mode (`with_ltm()` existing) and discovery mode. Verify discovery finds the same loops.

### Phase 3: Test Models and Validation

Create XMILE test models from the paper and validate.

#### 3a: Three-party arms race model

**File: `test/arms_race_3party/arms_race.stmx`**

Model from Figure 1 of the paper:
- 3 stocks: A's Arms (init=50), B's Arms (init=100), C's Arms (init=150)
- 3 flows: A's Changing, B's Changing, C's Changing
- Each flow = (target - current_arms) / adjustment_time
- A's target = B's Arms + 0.9 * C's Arms ("A wants parity with B and 90% of C")
- B's target = A's Arms + 1.1 * C's Arms ("B wants parity with A and 110% of C")
- C's target = 1.1 * A's Arms + 0.9 * B's Arms ("C wants 110% of A and 90% of B")
- Adjustment times: A=10, B=10, C=10
- Sim specs: 0 to 100, dt=1

Expected loops (8 total):
- 3 self-adjustment (balancing): A->A, B->B, C->C
- 3 pairwise reinforcing: A<->B, B<->C, A<->C
- 2 three-way reinforcing: A->B->C->A and A->C->B->A

From Figure 3 in the paper: the two long loops dominate after ~t=50, and the pairwise interactions and self-corrections are important at the beginning.

**Golden data**: We do NOT have golden data from the paper for this model (Figure 3 shows a graph but no tabular data). We generate golden data by:
1. Running exhaustive LTM (the model has only 8 loops, well under 1000)
2. Verifying the 8 loops are structurally correct
3. Running discovery mode and confirming it finds all 8 loops
4. Using the exhaustive results as golden data for future regression testing

#### 3b: Decoupled stocks model

**File: `test/decoupled_stocks/decoupled.stmx`**

Model from Figure 4 of the paper:
- 2 stocks: Stock_1 (init=1), Stock_2 (init=1)
- 2 inflows:
  - Flow_1 = IF Stock_2 > 50 THEN Stock_2/DT ELSE Stock_1/DT
  - Flow_2 = IF Stock_1 > 10 AND Stock_1 < 20 THEN Stock_1/DT ELSE Stock_2/DT
- Sim specs: 0 to 10, dt=1

Expected loops (4 potential):
- Stock_1 self-loop via Flow_1 (active when Stock_2 <= 50)
- Stock_2 self-loop via Flow_2 (active when Stock_1 <= 10 or Stock_1 >= 20)
- Cross-loop: Stock_1 -> Flow_2 -> Stock_2 -> Flow_1 -> Stock_1 (active when Stock_1 is 10-20 AND Stock_2 > 50)
- Cross-loop: Stock_2 -> Flow_1 -> Stock_1 -> Flow_2 -> Stock_2 (same conditions, opposite direction)

The paper notes these loops activate at different times, demonstrating why per-timestep discovery is necessary.

**Golden data**: Same approach as arms race -- generate via exhaustive mode and use as regression baseline.

#### 3c: Cross-validation test

For both test models, the integration test should:
1. Run exhaustive LTM (`with_ltm()`)
2. Run discovery LTM (`with_ltm_all_links()` -> simulate -> `discover_loops()`)
3. Verify discovery finds all loops that have > 0.1% average contribution in the exhaustive results

---

## File Summary

| File | Action | Description |
|------|--------|-------------|
| `src/simlin-engine/src/ltm_finding.rs` | **New** | Core strongest-path algorithm and `discover_loops()` function |
| `src/simlin-engine/src/ltm.rs` | **Modify** | Add `CausalGraph::all_links()` method |
| `src/simlin-engine/src/ltm_augment.rs` | **Modify** | Add `all_links: bool` param to `generate_ltm_variables()` |
| `src/simlin-engine/src/project.rs` | **Modify** | Add `with_ltm_all_links()` method |
| `src/simlin-engine/src/lib.rs` | **Modify** | Add `pub mod ltm_finding;` |
| `src/simlin-engine/tests/simulate_ltm.rs` | **Modify** | Add discovery-mode integration tests |
| `test/arms_race_3party/` | **New** | Three-party arms race test model |
| `test/decoupled_stocks/` | **New** | Two-stock decoupled model |

---

## Testing Strategy

### Unit Tests (in `ltm_finding.rs`, `#[cfg(test)] mod tests`)

1. **SearchGraph construction**: Build from known edges, verify sorting
2. **Trivial loop**: One stock, one flow -> one loop found
3. **Figure 7**: Hard-coded graph with step-by-step trace (see Phase 1 test case 3)
4. **best_score persistence**: Verify scores persist across stock iterations
5. **Loop deduplication**: Same loop from different starting points -> one result
6. **Empty graph**: No edges -> no loops
7. **Zero-score edges**: Verify behavior with 0-valued links
8. **NaN handling**: NaN treated as 0

### Unit Tests (in `ltm.rs`)

9. **`CausalGraph::all_links()`**: Verify all edges returned with correct polarity

### Unit Tests (in `ltm_augment.rs`)

10. **`generate_ltm_variables(all_links=true)`**: Verify link scores generated for all causal links

### Integration Tests (in `tests/simulate_ltm.rs`)

11. **Logistic growth - discovery mode**: Full pipeline against golden data
12. **Logistic growth - cross-validation**: Exhaustive vs discovery produce same loops
13. **Arms race**: Three-party model, verify 8 loops found
14. **Decoupled stocks**: Verify time-varying loop discovery

### Coverage Target

- `ltm_finding.rs`: 95+% line coverage
- New/changed code in `ltm.rs`, `ltm_augment.rs`, `project.rs`: 90+% on changed lines

---

## Implementation Order for Agent Team

Three phases, with parallelism possible after Phase 1:

1. **Phase 1** (blocking): Core algorithm in `ltm_finding.rs` + unit tests (test cases 1-8). This is the foundation and must be complete before other phases.

2. **Phase 2** (depends on Phase 1): Integration with Results, changes to `ltm.rs`/`ltm_augment.rs`/`project.rs`, end-to-end pipeline, integration tests (test cases 9-12).

3. **Phase 3** (depends on Phase 2): Create test models (arms race, decoupled stocks), generate golden data, add validation tests (test cases 13-14).

**Phase 2 can be split across agents** since the changes to `ltm.rs`, `ltm_augment.rs`, and `project.rs` are independent of each other and have clear interfaces. However, the `discover_loops` function in `ltm_finding.rs` depends on all of them, so final integration must wait.

### Key Implementation Details for Agents

#### Naming conventions
- Follow `#[cfg_attr(feature = "debug-derive", derive(Debug))]` for new structs
- Use `Ident<Canonical>` for all variable names, create via `crate::common::canonicalize()`
- LTM synthetic variables use prefix `$⁚ltm⁚` (the `⁚` is Unicode codepoint U+205A, TWO DOT PUNCTUATION -- copy it exactly from the existing code in `ltm_augment.rs`)
- Error handling: use `crate::common::Result<T>`
- Loop IDs: lowercase, e.g., `r1`, `b1`, `u1` (see `assign_deterministic_loop_ids()`)

#### Existing infrastructure to reuse
- `TestProject::new()` builder from `test_common.rs` for unit tests
- `simulate_ltm_path()` from `tests/simulate_ltm.rs` for integration tests
- `CausalGraph::circuit_to_links()` and `calculate_polarity()` for path-to-Loop conversion
- `Results.offsets` for variable name -> offset mapping
- `Results.iter()` returns chunks of `step_size` elements, one chunk per timestep

#### Critical invariants
- `best_score` must NOT be reset between stock iterations within a timestep
- NaN values in results must be handled (treat as 0, or skip the timestep)
- The `visiting` set tracks only nodes on the current DFS path, not all previously visited nodes
- Strict less-than (`<`) for the pruning comparison, not less-than-or-equal
