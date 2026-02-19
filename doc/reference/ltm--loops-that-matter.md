# Loops That Matter (LTM): Technical Reference

This document synthesizes three papers that collectively define the Loops That Matter
method for loop dominance analysis in system dynamics models:

1. **Schoenberg, Davidsen, and Eberlein (2020)** -- "Understanding model behavior using
   the Loops that Matter method." *System Dynamics Review* 36(2): 158--190. *Foundational
   paper introducing link scores, loop scores, and dominance analysis.*

2. **Schoenberg, Hayward, and Eberlein (2023)** -- "Improving Loops that Matter." *System
   Dynamics Review* 39(2): 140--151. *Corrects the flow-to-stock link score formula to
   eliminate sensitivity to flow aggregation.*

3. **Eberlein and Schoenberg (2020)** -- "Finding the Loops that Matter." *Conference
   paper. Describes the strongest-path loop discovery algorithm for models too large for
   exhaustive loop enumeration.*

The papers are presented here in logical order: the foundational method first, then its
correction, then the scalability solution.

---

## 1. Motivation and Problem Statement

The relationship between model structure and behavior is central to system dynamics. Given
a model, practitioners must understand *which feedback loops drive behavior at each point
in time* -- this is the **loop dominance analysis** problem.

Ford (1999) identified two needs: (i) automated analysis tools applicable to models with
many loops, and (ii) a clear and unambiguous definition of loop dominance.

Prior approaches include:

- **Eigenvalue Elasticity Analysis (EEA):** Characterizes behavior as weighted behavior
  modes via decoupled eigenvalues. Kampmann (2012) developed Independent Loop Sets (ILS);
  Oliva (2004) refined these into Shortest Independent Loop Sets (SILS). EEA requires
  model transformation into canonical form and works best on linear or near-linear models.
  It *can* analyze equilibrium states but cannot handle discrete/discontinuous models.

- **Pathway Participation Metric (PPM):** Traces causal pathways from a specific stock to
  ancestor stocks (Mojtahedzadeh et al., 2004). No model transformation required; works
  with discontinuous models. Criticized for inability to cleanly explain oscillatory
  behavior (Kampmann and Oliva, 2008): PPM-based methods produce sign changes during
  sinusoidal oscillation even though relative loop contributions remain constant.

- **Loop Impact Method:** Hayward and Boswell (2014) simplified PPM into a method
  implementable in standard SD software by adding equations. Focuses on direct impact one
  stock has on another, chaining impacts to get a loop metric. Product of impacts equals
  loop gain.

LTM addresses limitations of all three: it requires no model transformation, works on any
model (including discrete and discontinuous), produces a single per-loop metric applicable
model-wide (not per-stock), and handles oscillatory behavior cleanly.

---

## 2. Core Concepts

### 2.1 Definition of Loop Dominance

LTM defines dominance as a property of the *entire model* (or connected subcomponent), not
a single stock:

- All stocks must be connected to each other by the network of feedback loops.
- For models with disconnected stock groups, each subcomponent (**cycle partition**) has a
  separate loop dominance profile.
- Dominance is specific to a particular time period.
- A loop (or set of loops) is **dominant** if it describes **at least 50%** of the observed
  change in behavior across all stocks over the selected time period.

A standard feedback loop is a set of interconnections forming a closed path from a variable
back to itself, including at least one state variable (stock).

### 2.2 Two Metrics

LTM introduces two dimensionless metrics, both computed at each simulation timestep:

1. **Link score** -- the contribution and polarity of a single causal link
2. **Loop score** -- the contribution of a feedback loop to model behavior (product of
   constituent link scores)

Both are **completely insensitive to the number of variables and links in a loop** -- a key
advantage over methods that scale with path length.

### 2.3 Key Properties

- Calculations are done directly on original model equations during simulation.
- No model transformation, canonical form, or eigenvalue computation required.
- Uses only values computed during a regular simulation (plus one ceteris-paribus
  re-evaluation per link per timestep).
- Applicable to discrete, discontinuous, and agent-based models as long as the structure is
  a network of equations evaluated at known time points.
- Does NOT affect model validity.

---

## 3. Link Score

The link score measures the contribution and polarity of a single causal link at a point in
time. There are two cases: links between auxiliaries/flows (instantaneous) and links from
flows to stocks (integration).

### 3.1 Instantaneous Link Score (Auxiliary-to-Auxiliary, Stock-to-Flow, etc.)

For a dependent variable `z = f(x, y)` with inputs x and y, the link score for the link
x -> z is:

```
                  |Delta_x(z)|
LS(x -> z) =     |-----------|  *  sign(Delta_x(z) / Delta(x))
                  | Delta(z)  |

              = 0   if Delta(z) = 0 or Delta(x) = 0
```

Where:
- **Delta(z)** = z(t) - z(t-dt): total change in z over one timestep
- **Delta(x)** = x(t) - x(t-dt): change in x over that interval
- **Delta_x(z)** = f(x_current, y_previous) - z_previous: the **partial change** in z due
  to x alone, computed by re-evaluating f with the current value of x but the *previous*
  values of all other inputs (ceteris paribus)

**Magnitude** `|Delta_x(z) / Delta(z)|`:
- Dimensionless
- Measures the *force* that input x exerts on output z, relative to the total effect on z
- Unlike a partial derivative (which measures sensitivity), this measures how much the
  change in x *contributed* to the total change in z
- For linear equations (addition/subtraction only), values are always in [0, 1]
- For nonlinear equations with mixed polarities, can take very large values -- but this
  does not jeopardize analysis since relative values are compared

**Polarity** `sign(Delta_x(z) / Delta(x))`:
- Uses Richardson's (1995) polarity definition
- Positive: x and z change in the same direction (after isolating x's effect)
- Negative: x and z change in opposite directions

**Edge cases:** If x does not change, the link score is 0 (any loop through x is inactive).
If z does not change, all links into z have score 0.

#### Computation

After computing one timestep dt, for each non-stock variable `target`:
1. For each source variable feeding into target:
   - Re-evaluate the target equation using the **current** value of source and the
     **previous** values of all other inputs
   - This gives `tRespectSource` (the ceteris-paribus value)
   - `Delta_source(target)` = tRespectSource - previous_target
   - `Delta(source)` = current_source - previous_source
   - `Delta(target)` = current_target - previous_target
   - Apply the formula above

This roughly doubles the number of equation evaluations vs. normal simulation (one
re-evaluation per input per variable).

### 3.2 Flow-to-Stock Link Score (Corrected Formula from 2023 Paper)

The original 2020 paper defined the flow-to-stock link score as:

```
Original (FLAWED):
  LS(inflow -> S)  = |i / (i - o)| * (+1)
  LS(outflow -> S) = |o / (i - o)| * (-1)
```

where i is the inflow rate, o is the outflow rate, and (i - o) is the net flow.

#### The Problem: Aggregation Sensitivity

This formula uses the **value** of the flow, not the **change** in the flow. This makes it
sensitive to how flows are aggregated -- a purely cosmetic structural choice.

**Example demonstrating the problem:**

Consider a stock S (init=100) with separate inflow (in) and outflow (out):

| Variable | Time 1 | Time 2 |
|----------|--------|--------|
| in       | 5      | 10     |
| out      | 4      | 5      |
| S        | 101    | 106    |

Using the original formula:
- LS_magnitude(in -> S) = |10 / (10 - 5)| = 2.0
- LS_magnitude(out -> S) = |5 / (10 - 5)| = 1.0

Now restructure the same model with a single net flow auxiliary (net = in - out):
- Link from out to net (via instantaneous formula): |Delta_out(net) / Delta(net)| = |-1/4| = 0.25
- Link from net to S is 1 (single flow)
- Total: LS(out -> S) = 0.25

**Result:** Mathematically identical models produce different scores (1.0 vs 0.25).

The root cause is that flow values can be large relative to the net flow (e.g., two large
flows nearly canceling), and how large depends on the structural choice of aggregated vs.
disaggregated flows.

#### The Corrected Formula

```
Corrected (2023):
  LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
  LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
```

Where:
- **Delta(i)** = change in inflow rate: i(t) - i(t-dt)
- **Delta(o)** = change in outflow rate: o(t) - o(t-dt)
- **Delta(S_t)** = S(t) - S(t-dt) = net flow at time t (first-order change in stock)
- **Delta(S_{t-dt})** = S(t-dt) - S(t-2dt) = net flow at time t-dt
- **Delta(S_t) - Delta(S_{t-dt})** = change in net flow = second-order change in stock
  (acceleration)

**Interpretation:**
- The numerator is the first-order partial change in S with respect to the flow
- The denominator is the second-order change in S
- The formula measures: the partial change in the stock due to this flow, relative to the
  stock's acceleration

**Verification:** With the corrected formula on the disaggregated model:
- Delta(S_t) = 5, Delta(S_{t-dt}) = 1, so denominator = 5 - 1 = 4
- LS_magnitude(out -> S) = |1 / 4| = 0.25

This matches the aggregated model result. The formula is now **aggregation-invariant**.

**Implementation note:** The corrected formula shows that there is no need for a separate
calculation method for flow-to-stock links. Implementors can either: (a) use the corrected
formula directly with disaggregated flows, or (b) automatically aggregate all flows into
net flows and use link score 1 for all net-flow-to-stock links. Both produce identical
results.

### 3.3 Continuous-Time Form

Letting dt approach 0, the instantaneous link score becomes:

```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|
```

where dz/dx is the partial derivative and x_dot, z_dot are time derivatives.

The first term (dz/dx) is the **gain** between adjacent variables, as used in PPM and
Impact methods. The second term converts potential contribution (sensitivity) to realized
contribution. These gains obey the chain rule, so dz/dx is the gain regardless of how many
auxiliary variables appear between x and z.

For the flow-to-stock link:

```
LS(i -> S) = |di/dt / (d^2S/dt^2)|
```

The numerator is the rate of change of the flow; the denominator is the stock's
acceleration.

### 3.4 Link Score Between Adjacent Stocks

For two stocks S1 and S2 connected by flow f:

```
LS(S1 -> S2) = LS(S1 -> f) * LS(f -> S2)
             = (df/dS1) * |S1_dot / f_dot| * |f_dot / S2_ddot|
             = (df/dS1) * |S1_dot / S2_ddot|
```

The f_dot terms cancel in the chain -- a key algebraic property.

### 3.5 Relationship to Impact and PPM

The Impact of S1 on S2 (Hayward and Boswell, 2014) is:

```
Impact(S1 -> S2) = (df/dS1) * (S1_dot / S2_dot)
```

Relating link score to impact:

```
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * Sign(S1_dot) / Sign(S2_dot)
```

Two differences:
1. **Weighting by acceleration:** The factor |S2_dot / S2_ddot| weights the impact by the
   ratio of the target stock's velocity to its acceleration
2. **Polarity convention:** LTM measures **structural polarity** (based on model
   structure); PPM/Impact measure **behavioral polarity** (whether the link contributes to
   exponential or logarithmic behavior)

For **single-stock models** where S2 = S1, Impact equals the loop gain G1. The relative
loop score is then **identical to PPM** because all loops share the same weighting factor.

For **multi-stock models**, LTM gives different results than PPM/Impact because cross-stock
weighting factors differ. However, if link scores on a single stock are compared, results
agree with other methods (except polarity convention).

---

## 4. Loop Score

The **loop score** for loop L is the product of all link scores in the loop:

```
LoopScore(L) = LS(s1 -> t1) * LS(s2 -> t2) * ... * LS(sn -> tn)
```

where s_i -> t_i are the links in the loop and t_n = s_1 (the loop closes).

Both magnitude and sign are multiplied. An odd number of negative links produces a negative
loop score (balancing); an even number produces a positive score (reinforcing).

### 4.1 Properties

- **Dimensionless.** Can be thought of as the "force" a feedback loop applies to the
  behavior of all stocks it connects.
- **Consistent with the chain rule of differentiation** (proven in the 2020 paper's
  Appendix B). It should not matter whether two variables are connected by one complicated
  equation or three variables with two simpler equations.
- **Inactive links zero the loop:** Any loop containing a link with score 0 has loop
  score 0.
- **Isolated loop score is always +/-1:** A loop that is the only loop acting on its stocks
  always has loop score +1 (reinforcing) or -1 (balancing), regardless of gain magnitude.
  This is a key distinction from loop gain.
- **Does not predict speed of change:** Loop scores show which structure is dominant, not
  how fast change occurs.
- For multi-stock loops: LoopScore = G_n * product_i(|Si_dot / Si_ddot|), where G_n is the
  n-th order loop gain and the product runs over all stocks in the loop.

### 4.2 Chain Rule Invariance

If we decompose the equation z = f(w, x, y) into two steps (u = h(w, x), z = g(u, y)),
the product LS(x -> u) * LS(u -> z) equals LS(x -> z) computed directly.

**Caveat:** This equivalence fails if the intermediate variable u does not change (Delta_u
= 0). In that case, the link score through u becomes 0 even if both input and ultimate
output are changing. The 2020 paper notes this helpfully shows the potential feedback path
is not actually active.

### 4.3 Behavior at Extremes

- **At equilibrium** (S_dot -> 0): loop scores approach 0. Inactive loops are never
  explanatory.
- **At inflection points** (S_ddot -> 0): loop scores approach infinity. This is where
  loop dominance shifts occur. The explosion happens because the denominator in link scores
  approaches zero faster than the numerator.
- These infinities are the *opposite* of PPM-based methods, where infinities occur at
  max/min stock values and zeros represent inflection points.

### 4.4 Relative Loop Score

To compare loop contributions (and normalize away the infinities), the **relative loop
score** divides by the sum of absolute loop scores:

```
RelativeLoopScore(L) = LoopScore(L) / sum_Y(|LoopScore(Y)|)
```

where the sum runs over all loops Y in the same cycle partition.

Properties:
- Normalized to range [-1, 1]
- Sign represents structural polarity (positive = reinforcing, negative = balancing)
- Reports the fractional contribution of a loop to the change in value of all stocks at a
  point in time
- Essential because raw loop scores can become very large near dominance shift points

### 4.5 Loop Polarity

**Structural polarity** (determined from model structure):
- **Reinforcing (R):** Even number of negative links -> positive loop score
- **Balancing (B):** Odd number of negative links -> negative loop score
- **Undetermined (U):** Any link has unknown polarity (a conservative classification)

**Runtime polarity** (determined from simulation):
- **Reinforcing:** Loop score is positive throughout the simulation
- **Balancing:** Loop score is negative throughout the simulation
- **Undetermined:** Loop score changes sign during the simulation

Runtime undetermined polarity occurs in nonlinear models where link polarities depend on
variable values. Example: in the yeast alcohol model, the reinforcing births loop can
become effectively balancing when alcohol levels cause birth rates to go negative (a
formulation flaw, but it demonstrates the phenomenon).

---

## 5. Cycle Partitions

For models with a single cycle partition (every stock has a feedback path to/from every
other stock), all loops are compared against each other.

For models with multiple cycle partitions, only loops within the same partition are
compared. Loop scores between partitions are not meaningful because the stocks are
structurally independent.

LTM does **not** require loops to be independent (unlike EEA which uses ILS/SILS). All
connected loops are considered. Restricting to independent loops can filter out important
ones, as demonstrated by the three-party arms race model (Section 8.1).

---

## 6. Computational Considerations

### 6.1 When Computations Occur

- First computation after model initialization and first timestep
- Computed at each dt using Euler integration
- In principle compatible with Runge-Kutta and other integration methods
- The corrected flow-to-stock formula requires values from two previous timesteps
  (Delta(S_t) and Delta(S_{t-dt})), so link scores for flow-to-stock links are undefined
  for the first two timesteps

### 6.2 Performance

For models with 2-20 stocks and fewer than 50 feedback loops, the computational burden is
dominated by loop finding, not score computation. Analysis of all models in the 2020 paper
(including Forrester's 10-stock market growth model) takes less than 1 second total.

Equation re-evaluation multiplies computation by roughly 2x (one re-evaluation per input
per variable per timestep).

---

## 7. Application Examples

### 7.1 Bass Diffusion Model

**Structure:** Two stocks (Potential Adopters, Adopters), one flow (Adopting), parameters
for contact rate, adoption fraction, and market size. Two feedback loops:
- B1 (balancing): through probability of contact with potentials
- R1 (reinforcing): through adopter contacts

**Result:** Loop dominance shifts at the inflection point, when the stock reaches half its
maximum value. R1 dominates early (exponential growth); B1 dominates late (saturation).
The shift occurs at the inflection point where both relative scores pass through 0.5.

At the inflection point, absolute loop scores approach infinity (the two competing links
nearly cancel, driving the denominator toward zero). Relative loop scores smoothly
transition through the crossover.

### 7.2 Yeast Alcohol Model

**Structure:** Two stocks (Yeast Cells C, Alcohol A), two flows (Births B, Deaths D).
Four feedback loops: R (births), B1 (deaths), B2 (slowing births from alcohol), B3
(increasing deaths from alcohol).

**Result:** Four behavioral phases:
1. **t=0-51.5:** R dominant (exponential growth)
2. **t=52-66:** B2 dominant (slowing growth from alcohol)
3. **t=66.5-75:** B3 dominant (collapse from alcohol toxicity)
4. **t=75.5-100:** B1 dominant (natural death at low population)

Notable: R becomes effectively balancing around t=74 due to a formulation flaw (birth rate
goes negative at high alcohol levels). At t=74, no single loop is dominant.

Results agree with Ford's behavioral approach (Phaff et al., 2006), PPM, and Loop Impact,
with minor differences in the exact assignment of Phase 3.

### 7.3 Inventory Workforce Model

**Structure:** Two connected stocks (Inventory, Workers) plus one independent stock
(Expected Demand, in a separate cycle partition). Two main loops:
- B1 (major balancing): full production-hiring cycle through Inventory and Workers
- B2 (minor balancing): Workers self-adjustment through hiring/firing

**Result:** B1 dominates the oscillatory behavior in all tested parameterizations. B2's
contribution depends on "time to hire or fire." Increasing this parameter increases B2's
contribution, causing oscillations to become less damped and longer-lasting.

LTM handles the oscillatory behavior cleanly -- loop scores maintain consistent relative
contributions during oscillation. PPM-based methods produce sign changes during sinusoidal
oscillation that complicate interpretation, even though the underlying relative
contributions are constant.

---

## 8. Loop Discovery: The Strongest Path Algorithm

### 8.1 The Loop Enumeration Problem

For small models, all feedback loops can be enumerated (e.g., using Johnson's algorithm).
For large models, the number of loops grows up to the factorial of the number of stocks:
- **Urban Dynamics:** 43,722,744 loops
- **World3-03:** 330,574 loops

Exhaustive enumeration is impractical for such models. More importantly, restricting
analysis to independent loop sets misses dynamically important loops.

### 8.2 Why Independent Loop Sets Are Insufficient

The **three-party arms race model** demonstrates this. Three parties (A, B, C) each
adjust their arms level toward targets based on the others' levels:
- A targets B + 0.9C; B targets A + 1.1C; C targets 1.1A + 0.9B
- Initial: A=50, B=100, C=150

The model has 8 loops: 3 self-adjustment (balancing), 3 pairwise reinforcing, and 2
three-party reinforcing (A->B->C->A and A->C->B->A). The **shortest independent loop set**
contains only the first 6 -- it excludes the three-party loops.

However, all pairwise reinforcing loops have gain <= 1, so they alone cannot explain the
observed long-term exponential growth. The three-party loops drive long-term behavior and
account for essentially all of the loop scores after t=50, but they are not in the
independent set.

### 8.3 Why Composite Networks Fail

The authors tried building a single composite network (one score per link across all
timesteps) and discovering loops on that:

**Maximum composite (max |score| over all timesteps):**
- Composite loop scores are always >= actual scores
- Biases toward long loops (more multiplication of values >= 1)
- Numeric overflow risk (scores exceeding 1.0E300)

**Average composite (mean |score| over all timesteps):**
- Biases toward short loops (more multiplication of values <= 1)
- Better numeric properties but wrong results

Both failed: the loops ranked highest on the composite network did not correspond to the
actually most important loops over the full simulation.

### 8.4 The Adopted Strategy: Per-Timestep Discovery

Instead of a composite network, run loop discovery **at each simulation timestep** (or a
subset). This requires many more discovery passes, but each pass converges quickly because
actual link scores provide strong pruning guidance.

For models with fewer than ~1,000 total loops, exhaustive enumeration on a composite
(max-based) network is used instead, since it is guaranteed complete and fast enough.

### 8.5 Algorithm Description

The strongest-path algorithm is a modified depth-first search inspired by Dijkstra's
shortest path algorithm. Instead of minimizing additive distance, it **maximizes
multiplicative link score products** along paths. Complexity is roughly O(V^2) per
timestep.

#### Preprocessing (once per timestep)

```
For each variable in the model:
    For each outbound link from variable:
        Set link.score = |link_score| at this timestep
    Sort outbound links by score descending
    Set variable.best_score = 0
```

Sorting by score ensures the first visit to a variable is likely the strongest, enabling
earlier pruning. This provides roughly a 3x speedup.

#### Search (once per stock per timestep)

```
For each stock in the model:
    Set TARGET = stock
    Call Check_outbound_uses(stock, 1.0)
```

All feedback loops contain at least one stock, so starting from every stock ensures all
loops are reachable. The `best_score` values persist across stock iterations within a
timestep (they are NOT reset between stocks).

#### Core Recursive Function

```
Function Check_outbound_uses(variable, score):
    If variable.visiting is true:
        If variable = TARGET:
            Add_loop_if_unique(STACK, variable)
        End if
        Return
    End if

    If score < variable.best_score:
        Return                           -- pruning: weaker path
    End if

    Set variable.best_score = score
    Set variable.visiting = true
    Add variable to STACK

    For each link from variable:
        Call Check_outbound_uses(link.variable, score * |link.score|)
    End for each

    Set variable.visiting = false
    Remove variable from STACK
End function
```

**Key details:**
- `STACK` tracks the current DFS path for loop recording
- `variable.visiting` detects cycles on the current path only (not all visited nodes)
- `variable.best_score` tracks the highest cumulative score at which this variable has been
  reached. Initialized to 0 before each *timestep* (not before each stock)
- The comparison uses **strict less-than**: equal scores DO explore further
- The initial call uses score = 1.0 (multiplicative identity)
- When a cycle back to TARGET is detected, the loop is recorded if unique

### 8.6 Why This Is a Heuristic

Because the algorithm maximizes (rather than minimizes), it does **not guarantee finding
the truly strongest loop**. The specific failure mode (demonstrated in the papers with a
4-node example):

Consider nodes a, b, c, d with links:
- a->d: 100, a->b: 10
- d->b: 100, d->c: 0.1
- b->c: 10
- c->a: 10

Starting from a:
1. a->d (score 100) -> d->b (score 10000) -> b->c (score 100000) -> c->a: **loop found**
   (a->d->b->c->a, score 1,000,000)
2. Now best_score[b]=10000, best_score[c]=100000
3. a->b (score 10): 10 < best_score[b]=10000, **pruned**

The loop a->b->c->a (score 10 * 10 * 10 = 1000) is missed because b was already visited
with a much higher score via the d path. However, a->d->c->a would have score
100 * 0.1 * 10 = 100, and the found loop (score 1,000,000) is the strongest.

The mitigating factor: starting from different stocks at different timesteps, and the
consistent empirical finding that missed loops are **structurally very similar** to found
loops (differing by only a few links).

### 8.7 Completeness Evaluation

Tested against exhaustive enumeration on models small enough:

**Market Growth Model (19 loops):** All 19 found.

**Service Quality Model (104 loops):**
- 38 loops with > 0.01% contribution
- Algorithm found 76 loops total, 28 with > 0.01% contribution
- Of the top 15, only the 8th is missing
- The missing 8th loop is nearly identical to the found 4th loop -- they share the same
  path through most of the model but differ by two extra variables (desired vacancies,
  vacancies correction) in the longer one

**Economic Cycles Model (494 loops):**
- Algorithm found 261 loops
- Of the top 40, only the 22nd and 40th are missing
- Again, missing loops are structurally similar to found ones

**Pattern:** Missed loops are consistently "siblings" of found loops -- they share most of
their path but differ by a few links being slightly shorter or longer.

### 8.8 Performance on Large Models

**Urban Dynamics (43,722,744 loops):**
- Discovered 20,172 loops
- After 0.1% contribution cutoff: < 200 retained
- Computation time: 10-20 seconds (8th gen Intel Core i7)

**World3-03 (330,574 loops):**
- Discovered 2,709 loops
- After 0.1% contribution cutoff: 112 retained
- Computation time: ~4 seconds

### 8.9 Failed Approaches

The authors documented several abandoned strategies:

1. **Remaining potential on composite network:** Trace strongest links forward, use
   predicted potential to prune. Failed because strongest-outbound-link is at best modestly
   correlated with actual loop potential.

2. **Total potential score remaining:** Product of strongest link out of every variable,
   monotonically decreasing as variables are consumed. Worked with average composites but
   not maximum composites (numbers too large for cutoffs). Ultimately, the detected loops
   did not correlate with actual dynamic importance.

3. **Trimming the feedback structure:** Remove links after they appear in enough loops, or
   remove all weak links. Failed because removed links might be necessary to complete
   high-scoring loops even if their individual scores are low.

4. **Stock-to-stock network compaction:** Since all loops involve stocks, compact to
   stock-to-stock connections. Failed because (a) the number of *paths* (not variables)
   drives computation, and removing variables just creates more connections, and (b)
   eliminating parallel paths between stocks drops potentially informative loops.

---

## 9. Weaknesses and Limitations

### 9.1 Cannot Analyze Equilibrium States

When all stocks are unchanging, all loop scores are 0 by definition. EEA can provide
information under equilibrium for near-linear models.

Workarounds: introduce minute perturbations via STEP function, or analyze the transient
approach to equilibrium.

### 9.2 Focus on Endogenous Behavior

LTM focuses on feedback loops (endogenous structure). For models dominated by external
forcing functions, feedback effects may be small and the analysis less informative. The Loop
Impact method (Hayward and Boswell, 2014) may be better suited for highly forced models.

Link scores could in principle measure exogenous contributions, but this is not currently
part of the method.

### 9.3 Approximate Integration Sensitivity

The method as described uses Euler integration. Compatibility with Runge-Kutta and other
methods has not been fully explored (though in principle it should work at the level of
saved timesteps).

### 9.4 Heuristic Nature of Loop Discovery

The strongest-path algorithm does not guarantee finding the truly strongest loop, though
empirically it finds loops that are structurally very similar to the strongest. This is
acceptable for practical analysis but means the method cannot prove it has found *all*
important loops.

---

## 10. Notation Reference

| Symbol | Definition |
|--------|-----------|
| LS(x -> z) | Link score from variable x to variable z |
| Delta(z) | Total change in z: z(t) - z(t-dt) |
| Delta(x) | Change in x: x(t) - x(t-dt) |
| Delta_x(z) | Partial change in z due to x alone (ceteris paribus) |
| Delta(S_t) | Net flow at time t: S(t) - S(t-dt) |
| Delta(S_{t-dt}) | Net flow at time t-dt: S(t-dt) - S(t-2dt) |
| LoopScore(L) | Product of all link scores in loop L |
| RelativeLoopScore(L) | LoopScore(L) / sum(|LoopScore(Y)|) for all loops Y |
| dz/dx | Partial derivative of z with respect to x |
| x_dot, z_dot | Time derivatives dx/dt, dz/dt |
| S_ddot | Second time derivative d^2S/dt^2 |
| G_n | n-th order loop gain |
| Impact(S1 -> S2) | (df/dS1) * (S1_dot / S2_dot) |

---

## 11. Terminology

| Term | Definition |
|------|-----------|
| **Link score** | Dimensionless measure of contribution and polarity of a link at a point in time |
| **Loop score** | Product of all link scores in a feedback loop; measures the loop's contribution to model behavior |
| **Relative loop score** | Loop score normalized by sum of absolute loop scores; range [-1, 1] |
| **Partial change** | Change in z that would occur if only x changed (ceteris paribus) |
| **Cycle partition** | Subset of model where all stocks are connected by feedback loops |
| **Dominant loop** | Loop (or set) contributing >= 50% of change across all stocks |
| **Structural polarity** | Polarity determined from model structure (number of negative links) |
| **Behavioral polarity** | Polarity determined from curvature of behavior (used by PPM/Impact) |
| **Strongest path** | Heuristic loop discovery algorithm based on maximizing multiplicative link score products |
| **Composite feedback structure** | Network with one score per link aggregated over all timesteps (rejected for discovery) |
| **ILS** | Independent Loop Set (Kampmann, 2012) |
| **SILS** | Shortest Independent Loop Set (Oliva, 2004) |
| **EEA** | Eigenvalue Elasticity Analysis |
| **PPM** | Pathway Participation Metric |

---

## 12. Summary of Key Formulas

### Instantaneous Link Score (unchanged from 2020)

```
LS(x -> z) = |Delta_x(z) / Delta(z)| * sign(Delta_x(z) / Delta(x))
           = 0   if Delta(z) = 0 or Delta(x) = 0
```

### Flow-to-Stock Link Score (corrected 2023)

```
LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
```

### Loop Score

```
LoopScore(L) = product of LS(link_i) for all links in L
```

### Relative Loop Score

```
RelativeLoopScore(L) = LoopScore(L) / sum_Y(|LoopScore(Y)|)
```

### Continuous-Time Forms

```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|
LS(i -> S) = |di/dt / d^2S/dt^2|
LS(S1 -> S2) = (df/dS1) * |S1_dot / S2_ddot|
```

### Relationship to Impact

```
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * Sign(S1_dot) / Sign(S2_dot)
```

### Multi-Stock Loop Score

```
LoopScore = G_n * product_i(|Si_dot / Si_ddot|)
```

where G_n is the n-th order loop gain and the product runs over all stocks in the loop.

---

## References

- Eberlein, R. and Schoenberg, W. (2020). "Finding the loops that matter."
- Schoenberg, W., Davidsen, P., and Eberlein, R. (2020). "Understanding model behavior
  using the loops that matter method." *System Dynamics Review* 36(2): 158--190.
- Schoenberg, W., Hayward, J., and Eberlein, R. (2023). "Improving loops that matter."
  *System Dynamics Review* 39(2): 140--151.
- Dijkstra, E. W. (1959). "A note on two problems in connexion with graphs." *Numerische
  Mathematik* 1(1): 269--271.
- Ford, D. N. (1999). "A behavioral approach to feedback loop dominance analysis." *System
  Dynamics Review* 15(1): 3--36.
- Forrester, J. W. (1969). *Urban Dynamics.* Cambridge, Mass: MIT Press.
- Hayward, J. and Boswell, G. P. (2014). "Model behaviour and the concept of loop
  impact." *System Dynamics Review* 30(1-2): 29--57.
- Hayward, J. and Roach, P. A. (2017). "Newton's laws as an interpretive framework in
  system dynamics." *System Dynamics Review* 33(3-4): 183--218.
- Kampmann, C. E. (2012). "Feedback loop gains and system behaviour (1996)." *System
  Dynamics Review* 28(4): 370--395.
- Mojtahedzadeh, M., Andersen, D., and Richardson, G. (2004). "Using DIGEST to implement
  the pathway participation method." *System Dynamics Review* 20(1): 1--20.
- Oliva, R. (2004). "Model structure analysis through graph theory: partition heuristics
  and feedback structure decomposition." *System Dynamics Review* 20(4): 313--336.
- Richardson, G. P. (1995). "Loop polarity, loop dominance, and the concept of dominant
  polarity." *System Dynamics Review* 11(1): 67--88.
