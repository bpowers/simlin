# Eberlein & Schoenberg 2020: Finding the Loops that Matter

**Full Title:** Finding the Loops that Matter
**Authors:** Robert Eberlein and William Schoenberg
**Year:** 2020
**Context:** Third paper chronologically in the LTM series. Builds on Schoenberg et al. (2019) which introduced the LTM method and its loop score metric. This paper addresses the **loop discovery problem**: how to find the set of feedback loops on which to compute LTM scores, particularly in models too large for exhaustive enumeration.

---

## 1. Abstract and Problem Statement

The Loops that Matter (LTM) method (Schoenberg et al., 2019) provides metrics showing the contribution of feedback loops to model behavior at each point in time. To compute these metrics, the set of loops must first be identified. This paper:

1. Demonstrates that **important loops may not be independent** of one another and **cannot be determined from static structural analysis** alone.
2. Describes a **heuristic "strongest path" algorithm** for discovering the most important loops without exhaustive enumeration.
3. Shows this approach enables LTM analysis on **models of any size or complexity**.

The algorithm is heuristic -- it does not guarantee finding the truly strongest loops, but the authors argue it reliably finds loops that are strong and structurally similar to the strongest.

---

## 2. Background

### The Loop Enumeration Problem

- For small models, all feedback loops can be enumerated (e.g., using Tarjan, 1973).
- For large models, the number of loops grows potentially proportional to the **factorial of the number of stocks**, making exhaustive enumeration impractical.
- Example: The **Urban Dynamics model** (Forrester, 1969) has **43,722,744 feedback loops**.

### Prior Approaches to the Loop Set Problem

1. **Independent loop set** (Kampmann, 2012): A set of loops typically proportional in number to the number of stocks. Based on static structural analysis.
2. **Shortest independent loop set** (Oliva, 2004): A refinement that is both unique and easily discoverable.
3. **Limitations**: Both approaches are based on static equation analysis and do not consider simulation behavior. As noted by Guneralp (2006) and Huang et al. (2012), they will not necessarily find the loops most important to understanding behavior.

### The LTM Foundation

The LTM method uses a **link score** that measures the contribution of a changing input to a changing output in an equation. The **loop score** is the **product of all link scores** around the loop. This multiplicative structure is key to the discovery algorithm: as links are traversed, the cumulative loop score is known immediately, and search can be prioritized by link score magnitude.

---

## 3. Independent and Important Loops

### The Three-Party Arms Race Model (Figure 1)

A simple model with three parties (A, B, C), each with:
- A stock representing their arms level
- A flow representing their rate of change
- A target based on the other two parties' arms levels

**Setup:**
- A wants parity with B and 90% of C
- B wants parity with A and 110% of C
- C wants 110% of A and 90% of B
- Initial values: A=50, B=100, C=150

**Figure 1** shows the stock-and-flow diagram with three stocks (A's Arms, B's Arms, C's Arms), their flows (A's Changing, B's Changing, C's Changing), their targets (A's target, B's target, C's target), and the cross-connections (A for B, A for C, B for A, B for C, C for A, C for B).

### Feedback Loops in This Model

The model contains **8 feedback loops**:
1. **3 balancing stock-adjustment loops** (each stock adjusting toward its own target -- the standard balancing loop in the arms race archetype)
2. **3 pairwise reinforcing loops** (A to B's target to B to A's target, etc. -- the standard reinforcing loops in the archetype)
3. **2 reinforcing loops involving all three players**:
   - A -> B's target -> B -> C's target -> C -> A's target -> A
   - A -> C's target -> C -> B's target -> B -> A's target -> A (the reverse direction)

### Why Independent Loop Sets Are Insufficient

Following Oliva (2004) and Huang et al. (2012), the **shortest independent loop set** consists of the 3 stock-adjustment loops + 3 pairwise reinforcing loops (6 loops total). This set is "complete" in that all connections are used by at least one loop.

However, the **pairwise reinforcing loops all have gain <= 1**, meaning if only those loops are considered, behavior would necessarily trend toward balance or zero. The **A->B->C loop** (the three-party loop) is the one that determines long-term behavior, but it is NOT in the shortest independent loop set.

### Behavior (Figure 2)

**Figure 2** shows the behavior of the three stocks over 100 years:
- **B's Arms** (solid pink): Starts at 100, dips initially, then rises exponentially to ~200
- **A's Arms** (dash-dot blue): Starts at 50, rises and converges toward B, then rises exponentially
- **C's Arms** (dotted gray): Starts at 150, drops initially, converges with A and B, then all rise together in an arms race

This demonstrates initial adjustment behavior followed by long-term exponential growth driven by the three-party loop.

### Loop Scores Over Time (Figure 3)

**Figure 3** shows the relative loop scores (percent of behavior each loop is responsible for) over 100 years. Each loop is plotted as a separate line:

- **Early period (0-25 years):** The paired interactions (A and B, B and C, A and C) and self-corrections (A self, B self, C self) dominate. Scores range from about -80% to +60%. The model started far from a balanced trajectory, so adjustment loops are prominent.
- **Later period (after ~50 years):** The two long loops (A to B to C, and A to C to B) dominate, accounting for essentially all of the behavior. The short loops become negligible.

**Key insight:** Nearly all loops (including ones not in the independent set, with the paper noting a possible exception for the A-C interaction) are consequential at some point in the simulation. The important loops change over time and cannot be determined from static analysis.

---

## 4. The Composite Feedback Structure

### The Problem of Time-Varying Loop Importance

To discover all important loops, the algorithm must consider links that are active at any point during the simulation. Some links may be inactive at certain times and active at others.

### Motivating Example: Decoupled Feedback Loops (Figure 4)

A contrived two-stock model where:

```
Flow_1 = IF Stock_2 > 50 THEN Stock_2/DT ELSE Stock_1/DT
Flow_2 = IF Stock_1 > 10 AND Stock_1 < 20 THEN Stock_1/DT ELSE Stock_2/DT
```

Both stocks start at 1. The model has:
- Two **minor loops** (Stock_1 -> Flow_1 -> Stock_1 and Stock_2 -> Flow_2 -> Stock_2)
- One **major loop** (Stock_1 -> Flow_2 -> Stock_2 -> Flow_1 -> Stock_1)

**Behavior (Figure 5):** The two minor loops run independently until Stock_1 reaches 10. Then Stock_1 drives Flow_2 (but Flow_1 is still determined by Stock_1). When Stock_2 reaches 50, it drives both flows, creating the major loop. Only one feedback loop is active at any given time.

### Link Scores Over Time (Figure 6)

**Figure 6** plots the link scores for all four variable-to-flow connections over time:

- **Flow-to-stock connections** are always 1 (each stock has a single flow).
- **Stock_1 to Flow_2**: Score is 1 at time 5, 0 otherwise.
- **Stock_2 to Flow_1**: Score is 0 until time 7, then 1 afterward.
- All link scores start at 0 (an LTM convention -- nothing has changed at time 0).

This demonstrates that every link is active at some point but not at others. To find all loops, all links that are ever non-zero must be included.

### Composite Link Score Approaches (Both Rejected)

The authors tried two approaches to create a single "composite" network from which to discover loops:

**Approach 1: Maximum link score magnitude over all times**
- For this model, gives 1 for every link.
- **Problem:** Composite loop scores always >= actual loop scores. For big models, longer loops get bigger composite scores (bias toward long loops). Numeric overflow: scores can exceed 1.0E300.

**Approach 2: Average link score magnitude over all times**
- For this model: flows average to 1; Stock_1->Flow_1 averages 0.5; Stock_2->Flow_2 averages 0.9; Stock_1->Flow_2 averages 0.1; Stock_2->Flow_1 averages 0.5.
- Minor loop 1 score: 0.5; Minor loop 2 score: 0.9; Major loop score: 0.05.
- **Problem:** Longer loops get systematically lower composite scores (bias toward short loops). Nice numeric properties but biased.

**Outcome:** Both approaches failed. The detected loops ranked as most important on the composite network did not correspond to the most important loops when examining actual scores over the full simulation. The authors **abandoned composite network approaches** for prioritized discovery.

### The Adopted Solution: Per-Timestep Discovery

Instead of building a composite network, the algorithm performs loop discovery **at every (or almost every) simulation timestep**. This requires many more discovery passes, but each pass converges much faster because the strongest path algorithm on the actual (non-composite) network converges quickly.

### Composite Network for Small Models

The composite feedback structure is still used for an **initial identification pass** that will find loops exhaustively if the total count is below ~1,000. For this pass, the maximum of all link scores is used. Many models have fewer than 1,000 loops, and for these the full enumeration suffices without the strongest path algorithm.

---

## 5. The Strongest Path Algorithm

### Analogy to Dijkstra's Algorithm

The algorithm is inspired by Dijkstra's shortest path algorithm (1959):
- In Dijkstra: if going from a to b, and route through c costs 10km, any route through c costing more than 10km can be pruned.
- In loop discovery: instead of minimizing distance (additive), we **maximize link score products (multiplicative)**. When reaching variable c, if a previous path had a bigger score, we don't explore further from c on the current path.

### Why This Is a Heuristic (Not Exact)

The maximization (rather than minimization) means the approach does **not guarantee finding the optimal solution**. The specific failure mode:

**Figure 7** demonstrates the problem with a 4-node graph (a, b, c, d):
- The direct branch from `a` to `b` is weaker initially than going from `a` to `d`.
- The search therefore prunes the direct `a -> b` path after reaching `b` via a higher-scoring intermediate path.
- As stated in the paper, this can cause the algorithm to miss loop `a -> b -> c -> a` (score 1000) and instead find `a -> d -> c -> a` (score 100).

This is the core reason the strongest-path search is a heuristic rather than an exact optimizer.

**Mitigating factor:** Starting the search from different stocks (b or c in this example) might discover the missed loop. However, it is theoretically possible (though difficult) to construct graphs where starting from any node fails to find the strongest loop.

### The Algorithm (Detailed Steps)

Repeated for each computational interval (or a subset for performance tuning):

1. **Compute link scores** for every connection in the model (some may be 0).
2. **Sort outbound links** for every variable by link score magnitude (descending), so the strongest link is first.
3. **For every stock** in the model (all loops involve at least one stock), start the search:
   - a. Go through each outbound link in order; multiply the current score by that link's score; test the destination variable:
     - i. If the destination is the **starting stock**: record the loop and score. Check for uniqueness; if already found, ignore.
     - ii. If the destination is **currently being visited** (on the current path but not the starting stock): return (this loop will be found when starting from another stock).
     - iii. If the destination was **previously visited with a higher score**: return (pruning).
     - iv. If the destination has **not been visited, or was visited with a lower score**: update the score and recurse (execute step a from this variable).

### Computational Complexity

- Similar to Dijkstra's algorithm: **roughly O(V^2)** where V is the number of variables.
- Sorting edges by score improves efficiency -- the first visit to a variable is more likely to have the highest score, enabling earlier pruning.
- Most computational burden is in **uniqueness checking** and **loop information construction** for later processing.

### Pseudocode (Appendix I -- Verbatim)

```
STACK - a vector of variables
TARGET - the stock currently under investigation

Function Check_outbound_uses(variable, score)
    If variable.visiting is true
        If variable = TARGET
            Add_loop_if_unique(STACK, variable)
        End if
        Return
    End if
    If score is less than variable.best_score
        Return
    End if
    Set variable.best_score = score
    Set variable.visiting = true
    Add variable to STACK
    For each link from variable
        Call Check_outbound_uses(link.variable, score * link.score)
    End for each
    Set variable.visiting = false
    Remove variable from STACK
End function

For each time in the run
    For each variable in the model
        For each link from variable
            Set link.score from available data on link score (outside scope)
        End For each
        Set variable.best_score = 0
    End For each
    For each stock in the model
        Set TARGET = stock
        Check_outbound_uses(stock, 1.0)
    End for
End for
```

**Key details of the pseudocode:**
- `STACK` tracks the current path for loop recording and cycle detection.
- `variable.visiting` is a boolean flag for detecting cycles on the current DFS path (set true on entry, false on exit).
- `variable.best_score` tracks the highest score with which this variable has been visited across all paths from the current TARGET. Initialized to 0 before each stock's search.
- The initial call uses score = 1.0 (multiplicative identity).
- The outer loop iterates over **every timestep** in the simulation.
- At each timestep, `best_score` is reset to 0 for all variables before searching from each stock.
- Loop uniqueness must be checked since the same loop can be discovered from different starting stocks.

---

## 6. Completeness Analysis

The authors compared the strongest path algorithm against exhaustive enumeration on models small enough for full loop enumeration (< 100,000 loops).

### Market Growth Model (Forrester, 1968 / Morecroft, 1983)

- **Total loops:** 19
- **Discovered by algorithm:** 19 (all found)
- Perfect result for this small model.

### Service Quality Model (Oliva & Sterman, 2001)

- **Total loops:** 104 in the main set (plus 4 additional sets of smoothed quality loops -- each smooth is a single negative feedback loop)
- **Loops with > 0.01% contribution:** 38 out of 104
- **Discovered by algorithm:** 76 loops total, 28 with > 0.01% contribution
- **Missing loops:** Of the top 15 loops (by average contribution), the 8th is missing from the algorithm's results.

**Table 1: Detailed comparison of 4th and 8th loops**

The 4th loop (found by both methods) and the 8th loop (missed by strongest path) are nearly identical:

| 4th Loop (found) | 8th Loop (missed) |
|---|---|
| experience rate | experience rate |
| Experienced Personnel | Experienced Personnel |
| total labor | total labor |
| on office service capacity | on office service capacity |
| service capacity | service capacity |
| work pressure | work pressure |
| work intensity | work intensity |
| potential order fulfillment | potential order fulfillment |
| order fulfillment | order fulfillment |
| Service Backlog | Service Backlog |
| desired service capacity | desired service capacity |
| Change Desired Labor | Change Desired Labor |
| Desired Labor | Desired Labor |
| labor correction | labor correction |
| desired hiring | desired hiring |
| **desired vacancies** | *(skipped)* |
| **vacancies correction** | *(skipped)* |
| indicated labor order rate | indicated labor order rate |
| labor order rate | labor order rate |
| Vacancies | Vacancies |
| hiring rate | hiring rate |
| Rookies | Rookies |

The loops are identical through "desired hiring", then the 4th loop goes through two extra variables (desired vacancies, vacancies correction) before rejoining at "indicated labor order rate". This is a **very typical miss pattern**: the algorithm finds one of two sibling loops that share most of their path but differ by a few links (one being slightly shorter or longer than the other).

### Economic Cycles Model (Mass, 1975)

- **Total loops:** 494 (plus additional minor loop sets)
- **Discovered by algorithm:** 261
- **Missing from top 40:** Only the 22nd and 40th are missing.
- The missing loops again share much in common with nearby loops, though the relationship is not as simple as the service quality example.

### Pattern of Misses

The consistent finding across models: **missed loops are structurally very similar to found loops** -- they are "siblings" that share most of their path but differ by a few links being slightly shorter or longer. This gives confidence that even when the exact strongest loop is missed, a structurally similar loop is found.

---

## 7. Performance

### Urban Dynamics Model (Forrester, 1969)

- **Total loops (exhaustive):** 43,722,744
- **Loops discovered by algorithm:** 20,172
- **After 0.1% contribution cutoff:** < 200 loops retained for display
- **Computation time:** 10-20 seconds on 8th generation Intel Core i7
- Too many loops to compare against full set, but experiments with different algorithm tunings showed the same pattern: when tuned for speed (finding fewer loops), missed loops were similar-but-longer/shorter siblings of found loops.

### World3-03 Model (Meadows et al., 2004)

- **Total loops (exhaustive):** 330,574
- **Loops discovered by algorithm:** 2,709
- **After 0.1% contribution cutoff:** 112 loops
- **Computation time:** ~4 seconds
- Both reinforcing and balancing loops identified as important at different simulation phases.

---

## 8. Paths Not Taken (Failed Approaches)

The authors describe several abandoned approaches, sharing failures to benefit future researchers.

### What Worked from the Start

**Sorting search nodes by strongest connection** -- this was correct from the beginning and typically speeds up search by a factor of 3x. Never formally tested because it was so intuitive, but accidental tests confirmed the speedup.

### Failed Approach 1: Remaining Potential on Composite Network

- Trace strongest links out of each node until hitting a terminal or detecting a loop.
- Use the biggest score along that forward path as the node's "potential" to contribute to a loop.
- Abandon search when current score * remaining potential falls below threshold (based on already-discovered loops).
- **Why it failed:** Picking only the strongest outbound link is at best modestly correlated with real potential. The forward path might include already-visited variables.

### Failed Approach 2: Total Potential Score Remaining

- Compute potential as the **product of the strongest link out of every variable**.
- As variables are traversed, decrease potential by removing the current node; increase current score by the actual path chosen.
- The product of current score and potential is **monotonically decreasing**.
- Set a threshold (based on already-found loops) to terminate unpromising paths.
- **Partial success:** Worked well for average-based composite scores.
- **Why it ultimately failed:** The most powerful loops identified this way did not correlate with the most powerful loops when looking at total contribution to dynamics over the full simulation. With maximum-based composite scores, numbers were too large for reasonable cutoff thresholds.

### Failed Approach 3: Trimming the Feedback Structure (Removing Links)

Two sub-approaches:
1. **Remove links after they appear in enough loops** (assuming strongest loops found first).
2. **Remove all weak links** to reduce connectivity.
- **Why it failed:** Removed links might be necessary to complete a high-scoring loop even though the link itself scores low individually. While the sibling-loop pattern (similar loops with a few more/fewer links) suggests this might not be fatal, the authors could not convince themselves it was safe, nor could they guarantee performance across different model sizes.

### Failed Approach 4: Stock-to-Stock Network Compaction

- Since all loops involve stocks, compact the model to use only stock-to-stock connections.
- **Theoretical appeal:** Smaller network, potentially faster search. Stock-to-stock connections could be selected for maximum strength.
- **Why it failed:**
  1. Speed improvements did not materialize. The number of paths (not variables) drives computation, and removing non-stock variables just creates more connections between remaining variables.
  2. Eliminating parallel paths between stocks **drops potential feedback loops** that might be more interpretable or informative than the ones retained.

---

## 9. Conclusions

- The paper demonstrates the importance of drawing from **all loops** (not just independent sets) when determining which loops matter.
- The strongest path algorithm is a **practical heuristic** that gives good results for large models.
- Though it does not guarantee finding the truly strongest loops, observations show that missed loops are structurally similar to found ones.
- The algorithm provides a good balance of outcome quality and computational burden.
- The technique's ultimate value depends on its utility for providing understanding to model builders and consumers.

---

## 10. Key Algorithmic Insights Summary

1. **Link scores are multiplicative** along a loop path -- this is what enables Dijkstra-like pruning.
2. **Sort outbound links by score** (descending) before traversal -- 3x speedup.
3. **Search from every stock** at **every timestep** -- composite networks fail because loop importance varies over time.
4. **Prune by best_score**: if a variable was already visited with a higher cumulative score, don't explore further (heuristic, not exact).
5. **Detect cycles** with a visiting flag (like DFS coloring) -- if we hit a node on the current path that isn't the target, skip it (another starting stock will find that loop).
6. **Uniqueness checking** required because the same loop can be found from different starting stocks.
7. **Threshold at 0.1% contribution** for display -- most discovered loops are negligible.
8. For small models (< 1,000 loops), **exhaustive enumeration** on the composite network is used instead.

---

## 11. Figures Summary

| Figure | Description |
|--------|-------------|
| **Figure 1** | Stock-and-flow diagram of the three-party arms race model (A, B, C) with stocks, flows, targets, and cross-connections. |
| **Figure 2** | Time series of A's Arms, B's Arms, C's Arms over 100 years. Shows initial adjustment then exponential growth driven by the three-party loop. B starts at 100 and rises, A starts at 50 and converges upward, C starts at 150 and drops before rising. |
| **Figure 3** | Relative loop scores (%) for all 8 loops over 100 years. Short-term: self-correction and pairwise loops dominate (scores range -80% to +60%). Long-term: the two three-party loops dominate (converging to ~+/-50%). |
| **Figure 4** | Stock-and-flow diagram of a simple two-stock model with conditional IF-THEN-ELSE equations creating decoupled feedback loops that activate at different times. |
| **Figure 5** | Behavior of the two-stock model over 10 months. Shows Stock Flow 1, Stock Flow 2, and "From 1 to 2 loop" activity. Triangular/piecewise behavior as different loops activate and deactivate. |
| **Figure 6** | Link scores for all 4 variable-to-flow links over 10 months. Flow-to-stock always 1. Stock_1->Flow_2 is 1 at time 5 only. Stock_2->Flow_1 is 0 until time 7 then 1. Demonstrates time-varying link activity. |
| **Figure 7** | A 4-node directed graph (a, b, c, d) with labeled link scores demonstrating the failure case of directly applying Dijkstra's algorithm to loop finding. The algorithm finds a->d->c->a (score 100) but misses a->b->c->a (score 1000). |

---

## 12. Bibliography

- Dijkstra, E. W. (1959). A note on two problems in connexion with graphs. *Numerische Mathematik*, 1(1), 269-271.
- Forrester, J. W. (1968). Market Growth as Influenced by Capital Investment. *Industrial Management Rev. (MIT)*, 9(2), 83-105.
- Forrester, J. W. (1969). *Urban dynamics*. Cambridge, Mass: M.I.T. Press.
- Guneralp, B. (2006). Towards coherent loop dominance analysis: progress in eigenvalue elasticity analysis. *System Dynamics Review*, 22(3), 263-289.
- Huang, J., Howley, E., and Duggan, J. (2012). Observations on the shortest independent loop set algorithm. *System Dynamics Review*, 28(3), 276-280.
- Kampmann CE. (2012). Feedback loop gains and system behaviour (1996). *System Dynamics Review* 28(4): 370-395.
- Mass, NJ. (1975). *Economic Cycles: An Analysis of Underlying Causes*, Cambridge, Massachusetts.
- Meadows, DH., Randers, J., & Meadows, DL. (2004). *The limits to growth: The 30-year update*.
- Morecroft, JDW. (1983). System Dynamics: Portraying Bounded Rationality. *Omega*, 11(2), 131-142.
- Oliva, R. (2004). Model structure analysis through graph theory: partition heuristics and feedback structure decomposition. *System Dynamics Review*, 20(4): 313-336.
- Oliva R, Sterman JD. (2001). Cutting corners and working overtime: quality erosion in the service industry. *Management Science* 47(7): 894-914.
- Schoenberg, W., Davidsen, P., & Eberlein, R. (2019). Understanding model behavior using loops that matter. *arXiv preprint arXiv:1908.11434*.
- Tarjan, R. (1973). Enumeration of the Elementary Circuits of a Directed Graph. *SIAM J. Comput.*, 2(3), 211-216.
