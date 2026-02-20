# Detailed Summary: "Understanding Model Behavior Using the Loops that Matter Method"

**Authors:** William Schoenberg, Pal Davidsen, Robert Eberlein
**Journal:** System Dynamics Review, Vol. 36, No. 2, pp. 158-190 (April/June 2020)
**DOI:** 10.1002/sdr.1658

---

## 1. Abstract and Problem Statement

The paper presents the **Loops that Matter (LTM)** method, a new numeric method for **loop dominance analysis** in system dynamics models. The method introduces a metric called the **loop score** to determine the contribution of each feedback loop to model behavior at each instant in time.

The central problem: the relationship between structure and behavior is fundamental to system dynamics. Practitioners need to (1) create model structure, (2) understand how structure produces observed behavior, and (3) figure out how to improve structure. Step 2 is the focus -- and existing tools rely on either practitioner intuition or complex algorithmic approaches (eigenvalue analysis, pathway participation metrics).

Ford (1999) stated the field needs: (i) automated analysis tools applicable to models with many loops, and (ii) a clear and unambiguous understanding of loop dominance.

### Definition of Loop Dominance

The paper defines loop dominance as a concept applying to the **entirety of a model**, not just a single stock. Requirements:
- All stocks must be connected to each other by the network of feedback loops.
- For models with disconnected stock groups, each subcomponent (cycle partition) has a separate loop dominance profile.
- Dominance is specific to a particular time period.
- A loop (or set of loops) is **dominant** if it describes **at least 50%** of the observed change in behavior across all stocks over the selected time period.

---

## 2. Literature Review

### 2.1 Eigenvalue Elasticity Analysis (EEA)

- Forrester (1982) first documented that eigenvalue elasticities could explain relative contributions of loops in linear systems.
- EEA characterizes behavior as a weighted combination of behavior modes, each characterized by decoupled eigenvalues.
- EEA examines both link and loop significance by identifying the relationship (elasticity) between parameters making up loop gains and the eigenvalues characterizing behavior.
- Kampmann (2012) developed the **Independent Loop Set (ILS)** -- a singular set of independent loops producing the full model behavior.
- Oliva (2004) extended this with the **Shortest Independent Loop Set (SILS)** using only geodetic loops -- the de facto standard for EEA.

### 2.2 Pathway Participation Metric (PPM)

- PPM does not use eigenvalues but focuses on links between variables (Mojtahedzadeh et al., 2004).
- Starting point: behavior of a single variable (typically a stock).
- Behavior is partitioned based on periods where slope and convexity are maintained (first and second time derivatives don't change sign).
- Seven behavior patterns enumerated by Mojtahedzadeh et al. (2004).
- Determines dominance by tracing causal pathways between stock under study and ancestor stocks.
- PPM is more specific than EEA: only explains impact on specific stocks of interest.
- Does not identify dominance of behavior modes.
- Advantages: no manipulation of model structure required; works with discontinuous models.
- Criticisms (Kampmann and Oliva, 2009): inability to clearly explain oscillatory behavior; may fail when two pathways have similar importance.

### 2.3 Loop Impact Method

- Hayward and Boswell (2014) simplified PPM into the **Loop Impact method**.
- Can be implemented in standard SD software by adding equations -- no engine modification needed.
- Focuses on direct impact one stock has on another (not pathways).
- Chains pathways together to measure the Loop Impact metric.
- Sato (2016) modified it for engineering force concepts.
- Hayward and Roach (2017) developed a Newtonian physics framework.

---

## 3. The Loops that Matter Method

### 3.1 Key Characteristics

LTM is categorized (per Duggan and Oliva, 2013) as algorithmically performing a "formal assessment of dominant structure and behavior" for models of any size, complexity, or dimensionality.

Key properties:
- Like PPM: calculations done directly on original model equations, walking causal pathways between stocks through intermediate variables.
- Unlike EEA: no model transformation; no canonical form (eigenvalues/eigenvectors).
- Uses only values computed during a regular simulation.
- Applicable to discrete, discontinuous, and agent-based models as long as the structure is a network of equations evaluated at known time points.
- Does NOT manipulate equations or use variable values not computed during regular simulation.
- Does NOT affect model validity.

LTM introduces **two metrics**:
1. **Link score** -- measures contribution and polarity of a link between an independent and dependent variable
2. **Loop score** -- measures contribution of a feedback loop to model behavior, indicative of feedback polarity

Both are calculated at each time interval during simulation. Loop scores are **completely insensitive** to the number of variables and links in a loop.

Standard loop definition: a set of interconnections forming a closed path from a variable back to itself, including at least one state variable (stock).

---

## 4. Link Score Definition

### 4.1 Links Without Integration (Auxiliary-to-Auxiliary, etc.)

For a dependent variable `z = f(x, y)` with two inputs `x` and `y`, the link score for the link `x -> z` is:

```
                | delta_xz |
LS(x -> z)  =  | -------- | * sign(delta_xz / delta_x)       if delta_z != 0 AND delta_x != 0
                | delta_z  |

             =  0                                              if delta_z = 0 OR delta_x = 0
```

**Equation 1** (discrete form, computed each dt)

Where:
- `delta_z` = change in z from previous time to current time = z(t) - z(t-dt)
- `delta_x` = change in x over that interval = x(t) - x(t-dt)
- `delta_xz` = **partial change in z with respect to x** -- the amount z *would have changed* if x changed by the amount it did, but y had *not* changed (ceteris paribus). Computed as: f(x_current, y_previous) - z_previous.

The link score has two components:

**Magnitude:** `|delta_xz / delta_z|`
- Dimensionless.
- Describes the *force* that input x exerts on output z, relative to the total effect on z.
- Unlike a partial derivative (sensitivity), this describes how much the change in x *contributed* to the total change in z.
- For linear equations (addition/subtraction only), values are always in [0, 1].
- For nonlinear equations with mixed polarities, can take very large values -- but this doesn't jeopardize analysis since relative values are compared.

**Polarity:** `sign(delta_xz / delta_x)`
- Same formulation as Richardson (1995) polarity definition.
- Uses partial difference notation for consistency with magnitude.
- The delta_xz value is reused for both magnitude and polarity computation.

**Exception cases:** If x does not change, link score is 0 (loop through x is inactive). If z does not change, all links into/out of z have score 0.

### 4.2 Links With Integration (Flow-to-Stock)

For a stock `s = integral(i - o)` with inflow `i` and outflow `o`:

```
Inflow:   LS(i -> s) = |i / (i - o)| * (+1)
Outflow:  LS(o -> s) = |o / (i - o)| * (-1)
```

**Equation 2**

Correspondence to Equation 1:
- Flow value (`i` or `o`) corresponds to `delta_xz` (partial change) -- the amount the stock would change if no other flows were active (times dt).
- Denominator `(i - o)` corresponds to `delta_z` -- the total change in the stock (times dt).
- Polarity is fixed: +1 for inflows, -1 for outflows.

If net flow `(i - o) = 0`: link score is defined as 0. This is safe because any loop through a stock whose value doesn't change will have score 0 (due to the outgoing link from the stock having score 0).

**Important behavior:** If inflow and outflow are nearly balanced (small stock change), link scores from flows become large but close in value. This happens because the denominator approaches 0 faster than the numerator.

**Key distinction from PPM and Loop Impact:** Those methods treat integration links the same as algebraic links, measuring change in stock value relative to change in flow value (second derivative). LTM directly uses flow value relative to change in stock value (first derivative). This allows LTM to define a single number for the importance of a whole loop, rather than being limited to effects on a single stock.

### 4.3 Link Score Computation Examples

**Example 1 (Table 1): Linear equation `z = 2x + y`**

| Variable | Time 1 | Time 2 | delta | Partial change | Link score magnitude |
|----------|--------|--------|-------|----------------|---------------------|
| x        | 5      | 7      | 2     | delta_xz = 4   | 4/5 = 0.8           |
| y        | 4      | 5      | 1     | delta_yz = 1   | 1/5 = 0.2           |
| z = 2x+y | 14     | 19     | 5     | --             | --                   |

Computation for delta_xz: f(x_current=7, y_previous=4) - z_previous = 2*7+4 - 14 = 18 - 14 = 4.

Four-fifths of the change in z is caused by the change in x.

**Example 2 (Table 2): Nonlinear equation `z = (w + x) / y`**

| Variable | Time 1 | Time 2 | delta | Partial change | LS magnitude | Polarity | Link score |
|----------|--------|--------|-------|----------------|-------------|----------|------------|
| w        | 7      | 10     | 3     | delta_wz = 1   | 5           | +1       | 5          |
| x        | 2      | 4      | 2     | delta_xz = 0.67| 3.33        | +1       | 3.33       |
| y        | 3      | 5      | 2     | delta_yz = -1.2| 6           | -1       | -6         |
| z=(w+x)/y| 3      | 2.8    | -0.2  | --             | --          | --       | --         |

This example demonstrates why absolute value and sign are separated: delta_z is negative while delta_xz for x is positive. Without absolute value in magnitude, incorrect polarities would result.

---

## 5. Loop Score Definition

The **loop score** for loop L_x is the product of all link scores in the loop:

```
Loop Score(L_x) = LS(s1 -> t1) * LS(s2 -> t2) * ... * LS(sn -> tn)
```

**Equation 3**

Where:
- `s_i -> t_i` are the links in the loop
- `t_n = s_1` (the loop closes)
- Both magnitude and sign are multiplied
- Odd number of negative links -> negative loop; even number -> positive loop

Properties:
- Dimensionless.
- Can be thought of as the "force" a feedback loop applies to behavior of all stocks it connects.
- Multiplication is consistent with the **chain rule of differentiation** (proven in Appendix B).
- Any loop containing an inactive link (score 0) gets loop score 0.
- An **isolated loop** (only loop acting on its stocks) always has loop score +1 or -1 regardless of gain. This is a key distinction from loop gain.
- Loop scores do NOT predict speed of change; they only show which structure is dominant.
- Distinct from Loop Impact (Hayward and Boswell, 2014) where product of impacts equals loop gain.

### 5.1 Relative Loop Score

To compare loop contributions, the **relative loop score** normalizes by the sum of absolute loop scores:

```
Relative Loop Score(L_X) = Loop Score(L_X) / sum_Y(|Loop Score(L_Y)|)
```

**Equation 4**

Where the sum is over all loops Y in the same cycle partition.

Properties:
- Normalized to range [-1, 1].
- Sign still represents feedback polarity.
- Reports polarity and fractional contribution of a loop to the change in value of all stocks at a point in time.
- Essential because raw loop scores can become very large (especially near equilibria where denominators approach zero).

### 5.2 Cycle Partitions

- For models with a single cycle partition (every stock has a path to/from every other stock): compare all loops.
- For multiple cycle partitions: only compare loop scores within loops affecting the same subset of stocks.
- LTM does **not** require loops to be independent (unlike EEA which uses ILS/SILS).
- All connected loops are considered, independent or not in the topological sense.
- Restricting loops can filter out important ones (Guneralp, 2006; Huang et al., 2012).

---

## 6. Computational Considerations

### 6.1 When Computations Occur

- First computation after model initialization and first time step.
- Computed at each `dt` (time step) using Euler integration.
- Could in principle work at longer/shorter sampling intervals and with Runge-Kutta integration.

### 6.2 Algorithm Pseudocode (Figure 1)

The paper provides pseudocode for calculating all link scores after computing one dt of the model:

```
for each variable (target) in model:
    if target is a stock:
        apply Equation 2 for each flow (source) into the stock
    else:
        for each source variable feeding into target:
            1. Compute tRespectSource = recalculate target using
               current value of source and PREVIOUS values of all other inputs
            2. deltaTRespectS = tRespectSource - previous_target_value   (partial change)
            3. deltaSource = current_source - previous_source            (change in source)
            4. deltaTarget = current_target - previous_target            (change in target)
            5. Compute sign (avoid divide by zero)
            6. Compute link score using already-calculated values
```

Key implementation detail: equations must be re-evaluated once for each independent variable input (ceteris paribus computation). This roughly doubles the number of equation evaluations vs. normal simulation.

### 6.3 Performance

- For models with 2-20 stocks and <50 feedback loops: largest burden is identifying the full set of feedback loops, not computing scores.
- Analysis of all models in the paper (including Forrester's 10-stock market growth model) takes **less than 1 second** total, including XMILE parsing, loop finding, cycle partitioning, and all metrics.
- Equation re-evaluation multiplies computations by ~2x or more (similar to linearization requirements).
- Implemented by modifying the open-source sd.js engine (Powers, 2019).

---

## 7. Application: Bass Diffusion Model

### 7.1 Model Structure (Figure 2)

Standard Bass diffusion model variant:
- Time: 0 to 15
- Market Size: 1,000,000 people
- Initial adopters: 1
- Contact rate: 100 people/person/year
- Adoption fraction: 0.015 (dimensionless)
- Two stocks: Potential Adopters, Adopters
- One flow: Adopting

### 7.2 Feedback Loops

**Balancing loop B1:**
- probability of contact with potentials -> potentials contacts with adopters -> adoption from word of mouth -> adopting -> potential adopters -> probability of contact with potentials

**Reinforcing loop R1:**
- adopter contacts -> potentials contacts with adopters -> adoption from word of mouth -> adopting -> adopters -> adopter contacts

### 7.3 Results (Table 3, Figure 3)

The LTM analysis reproduces the standard explanation (Richardson 1995, Kampmann and Oliva 2017):

**Key finding:** Loop dominance shifts at the inflection point, when the stock reaches half its maximum value.

Table 3 shows link scores and loop scores at five time points (T=1, 9.5, 9.5625, 9.625, 15):

| Metric | T=1 | T=9.5 | T=9.5625 | T=9.625 | T=15 |
|--------|-----|-------|----------|---------|------|
| B1 loop score | 0.000 | -9.958 | -9,358 | -10.91 | -1.000 |
| B1 relative score | 0.000 | -0.465 | -0.488 | -0.512 | -1.000 |
| R1 loop score | 1.000 | 11.46 | 9,806 | 10.41 | 0.000 |
| R1 relative score | 1.000 | 0.535 | 0.512 | 0.488 | 0.000 |

The dominance shift occurs between T=9.5625 and T=9.625 (where the inflection occurs). Both relative scores pass through 0.5.

**Link score analysis:** Most links have score 1.000 (single input). The only changing links are:
- B1: "probability of contact with potentials -> potentials contacts with adopters" (the key link)
- R1: "adopter contacts -> potentials contacts with adopters" (counterpart)

These two links are at the junction between the reinforcing and balancing loops (structurally confirmed as most important).

### 7.4 Asymptotic Behavior (Figure 4)

- As dt approaches 0, both loop score magnitudes approach infinity at the inflection point.
- This happens because delta_z values in link scores approach 0 (the two competing links cancel).
- In single-stock systems, instants where loop scores approach infinity = shifts in feedback loop dominance.
- Log-scale plot (Figure 4) of absolute loop scores shows this clearly.
- **Key distinction from PPM/Loop Impact:** In LTM, infinities occur at dominance shifts (inflection point). In PPM-based approaches, infinities occur at max/min stock values (where curvature stops changing), and zeros represent inflection points.
- This is why relative loop scores are essential: they normalize away the magnitude explosion while preserving the smooth transition from positive to negative dominance.

---

## 8. Application: Yeast Alcohol Model

### 8.1 Model Structure (Figure 5)

Model equations:
- `B = C * (1.1 - 0.1 * A) / b1`  (birth rate)
- `D = C * EXP(A - 11) / d1`  (death rate)
- `dA/dt = p * C`  (alcohol production)

Initial conditions: A = 0, B = 1, b1 = 16, d1 = 30, p = 0.01
dt = 0.5

Produces overshoot and collapse behavior in C (yeast cells).

Note: There is a known **formulation flaw** -- B can take negative values and R's polarity changes when alcohol is high, causing R to act as an additional "death loop." This flaw is preserved for consistency with prior analyses.

### 8.2 Feedback Loops (Four loops, single cycle partition)

- **R** (reinforcing): births of cells C, characterized by b1 and Alcohol. Late in simulation, acts as balancing due to formulation flaw.
- **B1** (balancing): natural death of cells.
- **B2** (balancing): slowing of cell birth due to alcohol.
- **B3** (balancing): increasing cell death due to alcohol.

### 8.3 Results (Table 4, Figure 6)

Four behavioral phases identified:

| Phase | Time Range | Dominant Loop | Description |
|-------|-----------|---------------|-------------|
| 1     | 0-51.5    | R             | Exponential growth of C |
| 2     | 52-66     | B2            | Slowing growth due to alcohol |
| 3     | 66.5-75   | B3            | Collapse from alcohol toxicity |
| 4     | 75.5-100  | B1            | Natural death dominates at low C |

### 8.4 Comparison with Other Methods

**vs. Ford's behavioral approach (Phaff et al., 2006):** LTM identifies the same four phases. Only disagreement: Phase 3 -- LTM says B3 alone dominates, Ford says B2 and B3 together. (PPM and Loop Impact also identify B3 alone.)

**vs. EEA (Phaff et al., 2006):**
- Phase 1: Both agree R dominant, B2 restraining.
- Phase 2: Both agree B2 dominant, R still significant.
- Phase 3: EEA points to B1 and B3 together; LTM finds B3 solely dominant (caveat noted).
- Phase 4: Both agree B1 dominant over B3.

**vs. PPM (Mojtahedzadeh, 2008) and Loop Impact (Hayward and Boswell, 2014):** Agreement in principal; slight differences due to Hayward/Boswell using uniflow model variant.

### 8.5 Additional Insights from LTM

- Time 74-76: R becomes a significant (but not dominant) balancing loop due to the formulation flaw (excess alcohol causes births to run negative).
- At time 74, no single loop is dominant.
- B2 and B3 have local maxima in magnitude as they trade dominance.
- The number of stocks in a loop has no direct relationship to the number of relative loop score magnitude maxima.

---

## 9. Application: Inventory Workforce Model

### 9.1 Model Structure (Figure 7)

Version from Goncalves (2009), based on Mass and Senge (1975). Oscillatory two-stock model.
- Time: 0 to 60
- Stocks: Inventory, Workers
- External demand signal: graphical function acting as step function (increase between times 1 and 2), triggering dampened oscillation.

### 9.2 Feedback Loops (Two cycle partitions)

**Cycle Partition 1:**
- **B1 (Major balancing):** Inventory -> inventory gap -> desired change in inventory -> desired production -> desired workers -> workers gap -> hiring or firing -> Workers -> producing (back to Inventory)
- **B2 (Minor balancing):** Workers -> workers gap -> hiring or firing (back to Workers)

**Cycle Partition 2:**
- **B3 (Expected demand loop):** Expected demand -> changing expected (back to Expected demand)

B3 is in a separate partition because expected demand is driven only by demand itself, not coupled with inventory/workforce.

### 9.3 Results (Figure 8)

Three parameterizations analyzed (varying "time to hire or fire"):

**Key findings:**
- B1 dominates the oscillatory behavior in all parameterizations.
- B2 has a contribution dependent on the "time to hire or fire" parameter.
- Before the demand shock: model is in equilibrium, all link scores are 0, LTM cannot inform analysis.
- Increasing "time to hire or fire" increases B2's contribution relative to B1, causing oscillations to become more pronounced (less damped) and last longer.
- Time to hire or fire also directly impacts B1 (independent of B2).

### 9.4 Comparison with Other Methods

**vs. EEA (Goncalves, 2009):** Matches -- oscillatory mode from B1 loop gains, damping from B2.

**vs. PPM (Mojtahedzadeh, 2008; Hayward and Roach, 2017):**
- PPM shows behavior dominated by both B1 and B2 in a cyclical process (shifting dominance).
- Mojtahedzadeh acknowledges PPM's loop dominance pattern is "not suitable for analyzing causes of oscillation" and uses pathway frequency and stability factors instead.
- With those additional PPM metrics, same conclusion: B1 is source of oscillation, B2 responsible for dampening.
- Kampmann and Oliva (2008) explained: PPM-based methods are problematic in sinusoidal oscillators because the sign of PPM changes even though relative loop contributions remain constant.
- LTM does not suffer from this problem -- loop scores maintain consistent relative contributions during oscillation.

---

## 10. Discussion and Conclusions

### 10.1 Benefits of LTM

1. **General applicability:** Works on all models without modification, including discrete and discontinuous models. No model manipulation required.

2. **Simple, interpretable results:** Results are behavior-over-time graphs. Uses existing practitioner skill sets. No additional training needed to read relative loop/link scores.

3. **Computational and conceptual simplicity:** No complex mathematical constructs beyond what practitioners already use. The most challenging concept is the "partial change" (delta_xz), which is unfamiliar in terminology but not inherently complex. Transparent method leads to better understanding.

4. **Easy implementation:** Relatively easily implemented in existing simulation engines. No structural modifications to engines required. On par with Loop Impact method; considerable advantage over EEA and PPM. Based on experience implementing engines behind Stella and Vensim.

### 10.2 Weaknesses

1. **Cannot analyze equilibrium states:** When all stocks are unchanging, all loop scores are 0 by definition. Example: "bathtub" population model where birth fraction equals death fraction. EEA can provide information under equilibrium (if model is close to linear).
   - Unsatisfactory workaround: introducing minute changes to measure effects -- but this would break discrete/discontinuous model validity.
   - Alternative: model author can use STEP function to offset from equilibrium.

2. **Focus on endogenous behavior only:** Problematic for models where behavior is driven by external forcing functions dominating feedback effects. The inventory workforce model partially exhibits this (external demand signal creates a separate cycle partition). For highly forced models, Loop Impact method (Hayward and Boswell, 2014) is better. Link scores *could* measure exogenous contributions with future work.

### 10.3 Future Work

1. **Monte Carlo + LTM:** Combine to encompass more potential behavior sets; use during extreme condition testing to verify right results for right reasons; use optimizers to maximize/minimize loop scores for policy recommendations; include loop scores in sensitivity analyses to measure robustness.

2. **Visualization tools:** Animated stock-and-flow diagrams where links/flows change color and size based on polarity/link score. Automated CLD generation collapsing "unimportant" static links (scores of 0, +1.0, -1.0). Structurally correct, minimal CLDs showing dynamic components.

3. **Larger and more varied models:** Necessary to increase confidence in general utility.

---

## 11. Appendix A: Link Scores and Partial Derivatives

### Alternative Formulations

The link score (Equation 1) can be recast using partial differences for easier comparison with other methods.

**Equation 5** (partial difference form):

```
LS(x -> z) = (delta_xz / delta_x) * |delta_x / delta_z|

           = 0  if delta_z = 0 or delta_x = 0
```

First term: partial difference (has direction and sensitivity of z to x).
Second term: adjusts potential contribution to realized contribution using actual changes.

**Equation 6** (dividing by dt):

```
LS(x -> z) = (delta_xz / delta_x) * |(delta_x / dt) / (delta_z / dt)|

           = 0  if delta_z = 0 or delta_x = 0
```

**Equation 7** (continuous limit, as dt -> 0):

```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|

           = 0  if z_dot = 0 or x_dot = 0
```

Where dz/dx is the partial derivative, x_dot is dx/dt, z_dot is dz/dt.

This shows the relationship to partial derivatives (key to PPM and Loop Impact) and linearized representations (key to EEA). The second term `|x_dot / z_dot|` converts potential contribution (sensitivity) to realized contribution. This is why LTM gives results largely in line with other approaches when normalized, but not identical in absolute value.

---

## 12. Appendix B: Analytic Characteristics of Link Scores

### 12.1 Invariance Under Formulation (Chain Rule Property)

It should not matter whether we connect two variables with one complicated equation or three variables with two simpler equations.

**Proof sketch:** Consider two formulations:
1. Direct: `z = f(w, x, y)`
2. Indirect: `z = g(u, y)` where `u = h(w, x)`

Compute link score from x to u (Equation 8):
```
LS(x -> u) = |delta_xu / delta_u| * sign(delta_xu / delta_x)
```

Compute link score from u to z (Equation 9):
```
LS(u -> z) = |delta_uz / delta_z| * sign(delta_uz / delta_u)
```

Composite (product):
```
LS(x -> z) = |delta_xu / delta_u| * |delta_uz / delta_z| * sign(delta_xu/delta_x) * sign(delta_uz/delta_u)
```

Multiply by |delta_x / delta_x| = 1:
```
LS(x -> z) = |delta_xu / delta_x| * |delta_uz / delta_u| * |delta_x / delta_z| * sign(...) * sign(...)
```

Apply chain rule for partial differences (`delta_xu/delta_x * delta_uz/delta_u = delta_xz/delta_x` when canceling delta_u terms):
```
LS(x -> z) = |delta_xz / delta_z| * sign(delta_xu/delta_x) * sign(delta_uz/delta_u)
```

**Caveat:** This equivalence **fails if delta_u = 0** even when both delta_x and delta_z are nonzero. If the intermediate variable doesn't change, the link score becomes 0 even when input and ultimate output are changing. The paper notes this can happen (e.g., Bass Diffusion with total population computed by adding stocks) and the 0 value helpfully shows the potential feedback is not real.

### 12.2 Isolated Loop Score is Always +/-1

For a single positive or negative loop, the loop score will always be +1 or -1, regardless of the gain around the loop.

**Example:** Net population growth model.
- Link score from stock to flow: 1 (only the stock changes the flow).
- With only a single net flow, link score from flow to stock: also 1.
- Product = 1.

For exponential drain: link from flow to stock is -1, so loop score = -1.

**This value of 1 is true regardless of growth rate or residence time.** The loop score measures relative structural dominance, not gain or speed of change.

**When additional loops are added:** For example, adding deaths to a births-only population model gives link scores from flows into the stock based on their relative value. Both loop scores will have magnitude > 1. The closer the flows are to each other, the bigger the scores. This is why relative loop scores are the basis for analysis, and emphasizes how distinct loop score is from loop gain.

---

## 13. Key Figures Summary

### Figure 1
Pseudocode for calculating all link scores in a model after computing one dt. Shows the for-each-variable loop, stock vs. non-stock branching, and the ceteris paribus recalculation.

### Figure 2
Stock-and-flow diagram of the Bass diffusion model: two stocks (Potential Adopters, Adopters), one flow (Adopting), auxiliaries (contact rate, adoption fraction, probability of contact, adopter contacts, potentials contacts with adopters, adoption from word of mouth, market size).

### Figure 3
Bass diffusion relative loop scores plotted over time against Adopters. Shows R1 and B1 crossing at the inflection point (~T=9.6), with R1 dominant early and B1 dominant late.

### Figure 4
Log-scale plot of absolute loop score values for Bass diffusion. Shows both scores approaching infinity at the inflection point (where the competing links cancel, driving delta_z toward 0).

### Figure 5
Stock-and-flow diagram of the yeast alcohol model: stock C (yeast cells), stock A (alcohol), flows B (births) and D (deaths), parameters b1, d1, p.

### Figure 6
Yeast alcohol relative loop scores plotted against C. Shows four-phase behavior with R, B2, B3, and B1 dominating in succession. Shows R becoming negative (balancing) around time 74 due to formulation flaw.

### Figure 7
Stock-and-flow diagram of the inventory workforce model: stocks Inventory and Workers, plus Expected Demand. Shows demand signal, production, hiring/firing flows, and gap calculations.

### Figure 8
Three panels showing LTM analysis of inventory workforce model with different "time to hire or fire" values, demonstrating how this parameter affects the relative contributions of B1 and B2 to the oscillatory behavior. B1 dominates in all cases; B2 contribution increases with longer time to hire or fire.

---

## 14. Terminology Reference

| Term | Definition |
|------|-----------|
| **Link score** | Dimensionless measure of contribution and polarity of a link between independent and dependent variable at a point in time |
| **Loop score** | Product of all link scores in a feedback loop; measures loop's contribution to model behavior |
| **Relative loop score** | Loop score normalized by sum of absolute loop scores in the cycle partition; range [-1, 1] |
| **Partial change** (delta_xz) | Change in z that would occur if only x changed (ceteris paribus) |
| **Cycle partition** | Subset of model where all stocks are connected by feedback loops |
| **Dominant loop** | Loop (or set) contributing >= 50% of observed change across all stocks |
| **ILS** | Independent Loop Set (Kampmann, 2012) |
| **SILS** | Shortest Independent Loop Set (Oliva, 2004) |
| **EEA** | Eigenvalue Elasticity Analysis |
| **PPM** | Pathway Participation Metric |

---

## 15. Implementation Reference

The method was implemented by modifying the open-source sd.js engine (Powers, 2019) -- the same engine that is the ancestor of Simlin's engine. Key implementation requirement: the ability to re-evaluate equations with ceteris paribus inputs (current value of one source, previous values of all other sources).
