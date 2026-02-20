# Detailed Summary: "Improving Loops that Matter" (Schoenberg, Hayward, Eberlein, 2023)

**Full Title:** Improving Loops that Matter
**Authors:** William Schoenberg (University of Bergen / isee Systems), John Hayward (University of South Wales), Robert Eberlein (isee Systems)
**Published:** System Dynamics Review, Vol. 39, No. 2 (April/June 2023), pp. 140-151
**DOI:** 10.1002/sdr.1728
**Paper Type:** Notes and Insights (correction/improvement to the 2020 LTM method)

---

## 1. Context and Relationship to Previous Work

This paper is a **correction** to the original LTM method published in Schoenberg, Davidsen, and Eberlein (2020), "Understanding model behavior using the loops that matter method," System Dynamics Review 36(2): 158-190.

The 2020 paper introduced two distinct formulas for computing link scores:
1. An **instantaneous link score** for connections between auxiliaries, flows, and stocks (non-integration links)
2. A **flow-to-stock link score** specifically for the integration relationship between flows and their stocks

This 2023 paper identifies a **fundamental flaw** in the original flow-to-stock link score formula: it is **sensitive to flow aggregation**. That is, combining two separate flows into one net flow (a purely cosmetic/structural change with no mathematical effect on the model) **changes the LTM analysis results**. The paper provides a corrected formula that eliminates this sensitivity.

The corrected formula has been implemented in **Stella Architect version 2.1** and all subsequent versions (isee Systems, 2021).

---

## 2. Background: Link Scores in LTM

### 2.1 The Instantaneous Link Score (Eq. 2, unchanged)

For a link x -> z where z is a flow or auxiliary defined by z = f(x, y), the link score is:

```
                  | Delta_x(z) |
LS(x -> z) =     | ---------- | * sign(Delta_x(z) / Delta(x))
                  |  Delta(z)  |

              = 0   if Delta(z) = 0 or Delta(x) = 0
```

Where:
- `Delta_x(z)` is the **partial change** in z due to x alone (with y held constant)
- `Delta(z)` is the total change in z
- The first term measures the **proportion** of the change in z originating from x
- The second term measures the **polarity** of the link using Richardson's (1995) method
- This equation is from the original 2020 paper and is **NOT changed** in this correction

### 2.2 The Original (FLAWED) Flow-to-Stock Link Score (Eq. 1, from 2020 paper)

For a stock S with inflow i and outflow o, the original flow-to-stock link scores were:

```
Original Inflow:   LS(i -> S) = |i / (i - o)| * (+1)

Original Outflow:  LS(o -> S) = |o / (i - o)| * (-1)
```

Where:
- `i` is the value of the inflow
- `o` is the value of the outflow
- `i - o` is the net flow (rate of change of S)
- The contribution is measured as the **value** of each flow relative to the net flow

**Critical observation:** This formula uses the **value** of the flow, not the **change** in the flow. This is the root cause of the aggregation sensitivity problem.

---

## 3. The Problem: Flow Aggregation Sensitivity

### 3.1 Problem Demonstration with Example Model

The paper demonstrates the problem with a concrete example using two mathematically equivalent model structures.

#### Model Structure 1: Disaggregated Flows (Figure 1)

A stock S with separate inflow (in) and outflow (out):
- `S = integral(in - out)`, initial value = 100

| Variable | Time 1 | Time 2 |
|----------|--------|--------|
| in       | 5      | 10     |
| out      | 4      | 5      |
| S        | 101    | 106    |

Using the **original** Eq. 1:

```
LS_magnitude(in -> S) = |in / (in - out)| = |10 / (10 - 5)| = |10/5| = 2.0

LS_magnitude(out -> S) = |out / (in - out)| = |5 / (10 - 5)| = |5/5| = 1.0
```

#### Model Structure 2: Aggregated Net Flow (Figure 2)

The same model restructured with a single net flow:
- `net = in - out` (net is now an auxiliary)
- `S = integral(net)`, initial value = 100

| Variable | Time 1 | Time 2 | Variable Change | Partial Change in net | Link Score Magnitude |
|----------|--------|--------|-----------------|----------------------|---------------------|
| in       | 5      | 10     | Delta(in) = 5   | Delta_in(net) = (10-4) - (10-5) = 1 ... actually = 5 | see below |
| out      | 4      | 5      | Delta(out) = 1  | Delta_out(net) = (5-5) - (5-4) = -1 | see below |
| net      | 1      | 5      | Delta(net) = 4  | -- | -- |

Computing link score from `out` to `net` using the instantaneous Eq. 2:
```
Delta_out(net) = (in_t2 - out_t2) - (in_t2 - out_t1) = (10 - 5) - (10 - 4) = 5 - 6 = -1
Delta(net) = net_t2 - net_t1 = 5 - 1 = 4

LS_magnitude(out -> net) = |Delta_out(net) / Delta(net)| = |-1 / 4| = 0.25
```

Since the link from net to S has a score of 1 (single flow to stock), the total score from out to S is:
```
LS(out -> S) = LS(out -> net) * LS(net -> S) = 0.25 * 1 = 0.25
```

#### The Discrepancy

- **Disaggregated model (Eq. 1):** LS_magnitude(out -> S) = **1.0**
- **Aggregated model (Eq. 2):** LS_magnitude(out -> S) = **0.25**

These are **mathematically identical models** yet produce **different LTM analysis results**. The original flow-to-stock formula (Eq. 1) overweighs the impact of relatively small changes because it uses the **value** of the flow rather than the **change** in the flow.

### 3.2 Root Cause Analysis

The fundamental issue is that Eq. 1 divides the flow **value** (`out = 5`) by the net flow (`i - o = 5`), while Eq. 2 divides the flow **change** (`Delta_out(net) = -1`) by the total change (`Delta(net) = 4`).

The flow values can be large relative to the net flow (e.g., two large flows nearly canceling out), which causes all scores to become very large in magnitude as a stock approaches equilibrium. How large depends on how the flows are specified (aggregated vs. disaggregated), which is a purely structural/cosmetic choice.

---

## 4. The Solution: Updated Flow-to-Stock Link Score (Eq. 3)

### 4.1 The Corrected Formula

```
Updated Inflow:   LS(i -> S) = | Delta(i) / (Delta(S_t) - Delta(S_{t-dt})) | * (+1)

Updated Outflow:  LS(o -> S) = | Delta(o) / (Delta(S_t) - Delta(S_{t-dt})) | * (-1)
```

Where:
- `Delta(i)` = the **change** in the inflow value (first-order partial change in S w.r.t. the flow)
- `Delta(o)` = the **change** in the outflow value
- `Delta(S_t) - Delta(S_{t-dt})` = the change in the net flow = the **second-order change** in S
- `Delta(S_t) = S_t - S_{t-dt}` is the net flow at time t (change in stock over one timestep)

### 4.2 Conceptual Interpretation

- The **numerator** (`Delta(i)` or `Delta(o)`) is the first-order partial change in the stock S with respect to the flow
- The **denominator** (`Delta(S_t) - Delta(S_{t-dt})`) is the change in the net flow, which is the second-order change in S
- The flow-to-stock link score magnitude now measures: **the first-order partial change in the stock due to the flow, relative to the second-order change of the stock**
- This is conceptually different from the instantaneous link score, which is necessary to account for the integration process
- However, from an **arithmetic/operational** perspective, the instantaneous and updated flow-to-stock link score equations now produce the **same set of calculations**

### 4.3 Verification with the Example

Using the corrected Eq. 3 with the disaggregated model:

| Variable | Time 1 | Time 2 | Variable Change | Link Score Magnitude |
|----------|--------|--------|-----------------|---------------------|
| in       | 5      | 10     | Delta(in) = 5   | \|5 / 4\| = 1.25 |
| out      | 4      | 5      | Delta(out) = 1  | \|1 / 4\| = 0.25 |
| S        | 101    | 106    | Delta(S_t) - Delta(S_{t-dt}) = 5 - 1 = 4 | -- |

Now comparing:
- **Disaggregated model (corrected Eq. 3):** LS_magnitude(out -> S) = **0.25**
- **Aggregated model (Eq. 2):** LS_magnitude(out -> S) = **0.25**

The results now match regardless of flow aggregation structure. Not only do the final results match, but **all intermediate steps** are identical.

### 4.4 Implementation Choice

The corrected formula demonstrates that there is **no need for a separate calculation method** for measuring the link score between flows and stocks, as long as all flows are aggregated during analysis.

The paper notes that it is now up to the implementor whether to:
1. Use Eq. 3 directly with disaggregated flows, or
2. Automatically aggregate all flows into net flows and then use a link score of 1 for all net flow-to-stock links

Both approaches produce identical results because the corrected formula is **aggregation-invariant**.

---

## 5. Placing LTM in the Literature

### 5.1 Continuous-Time Form of the Link Score (Eq. 5)

The instantaneous link score (Eq. 2) can be restated by splitting the fraction:

```
LS(x -> z) = (Delta_x(z) / Delta(x)) * |Delta(x) / Delta(z)|
            = 0   if Delta(z) = 0 or Delta(x) = 0
```

This is Eq. 4. Letting all deltas approach 0 (dt -> 0):

```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|
            = 0   if z_dot = 0 or x_dot = 0
```

This is Eq. 5, where:
- `dz/dx` (partial derivative) is the **gain** between adjacent auxiliary variables, as defined by Kampmann (2012, p. 373) and Richardson (1995, p. 75)
- `x_dot / z_dot` is the ratio of time derivatives
- These gains are used in the **Pathway Participation Metric (PPM)** (Mojtahedzadeh et al., 2004, eq. 3) and the definition of **impact** (Hayward and Boswell, 2014, appendix 2)

**Important property:** The link gains obey the chain rule of partial differentiation, so `dz/dx` remains the gain regardless of the number of auxiliary variables between x and z. Although the link score weights these gains by `|x_dot / z_dot|`, these weights cancel when applying the chain rule. Therefore, the path score is always the gain multiplied by the relative time derivative of the two endpoint variables.

### 5.2 Continuous-Time Form of the Updated Flow-to-Stock Link Score (Eq. 6)

For the flow-to-stock link, letting dt approach zero:

```
LS(i -> S) = |di/dt / (d^2S/dt^2)|        (Eq. 6, for inflow i)
```

The denominator is the second derivative of S (the acceleration of S), and the numerator is the rate of change of the inflow.

### 5.3 Link Score Between Adjacent Stocks (Eq. 7)

For two stocks S1 and S2 connected by flow f (where S1 influences f which feeds into S2):

```
LS(S1 -> S2) = LS(S1 -> f) * LS(f -> S2)
             = (df/dS1) * |S1_dot / f_dot| * |f_dot / S2_ddot|
             = (df/dS1) * |S1_dot / S2_ddot|
```

This is Eq. 7. Notice that `f_dot` cancels in the chain, which is a key algebraic property.

### 5.4 Relationship to Impact (Eq. 10)

The relationship between stocks S2 and S1 connected by flow f can be written as:

```
dS2/dt = f(S1, ...) + ...        (Eq. 8)
```

Differentiating (following Hayward and Roach, 2017, appendix C; Mojtahedzadeh et al., 2004, appendix 1):

```
d^2S2/dt^2 = (df/dS1) * (dS1/dt) + ...
           = [(df/dS1) * (S1_dot / S2_dot)] * (dS2/dt) + ...    (Eq. 9)
```

The bracketed expression is the **impact** of S1 on S2:

```
Impact(S1 -> S2) = (df/dS1) * (S1_dot / S2_dot)        (Eq. 10)
```

### 5.5 Relating Link Score to Impact (Eq. 11)

Comparing Eqs. 7 and 10:

```
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * [Sign(S1_dot) / Sign(S2_dot)]    (Eq. 11)
```

The link score and impact differ in two aspects:

1. **Weighting by acceleration:** The factor `|S2_dot / S2_ddot|` weights the impact by the ratio of the target stock's velocity to its acceleration
2. **Polarity convention:** The `Sign(S1_dot) / Sign(S2_dot)` factor reflects that:
   - **LTM link score** measures **structural polarity** (based on the model structure)
   - **Impact (and PPM)** measures **behavioral polarity** (whether the link contributes to exponential or logarithmic behavior)

### 5.6 Single-Stock Models (Eq. 12-13)

For single-stock models where S2 = S1 (first-order loop), impact equals the loop gain G1:

```
Impact(S1 -> S1) = df/dS1 = G1        (Eq. 12, Kampmann 2012 definition)
```

The loop score becomes a **weighted loop gain**:

```
LS(S1 -> S1) = G1 / |S1_ddot / S1_dot|        (Eq. 13)
```

**Key result:** Because all loop scores in a single-stock model are weighted by the same factor `|S1_ddot / S1_dot|`, the **relative loop score** (normalized across all loops) is **identical to the PPM** for single-stock models.

### 5.7 Multi-Stock Models (Eq. 14)

For a two-stock loop where S1 influences S2 which influences S1:

```
LS(S1 -> S1) = LS(S1 -> S2) * LS(S2 -> S1)

Using the loop impact theorem (Hayward and Boswell, 2014, appendix 3):
Impact(S1 -> S2) * Impact(S2 -> S1) = G2  (the second-order loop gain)

Therefore:
LS(S1 -> S1) = G2 * |S1_dot / S1_ddot| * |S2_dot / S2_ddot|    (Eq. 14)
```

For multi-stock models, LTM will give **different** results than PPM and Loop Impact. However, if link scores on a **single stock** are compared, the results are the same as the other methods (except for polarity convention).

---

## 6. Interpretation and Properties of Loop Scores

The paper identifies three key implications of Eq. 14 for the meaning of loop scores:

### 6.1 Structural Polarity
Loop scores always measure the **structural polarity** of loops because of the absolute values of the loop impacts in the denominator. This is in contrast to PPM/Loop Impact which measure behavioral polarity.

### 6.2 Behavior at Equilibrium
If a stock is not changing (reaching a maximum, minimum, or equilibrium value), i.e., as `S_dot -> 0`, then the loop score approaches 0. As a corollary, when loop gains are 0, the loop score is 0, meaning **inactive loops are never explanatory**.

### 6.3 Behavior at Inflection Points
As the acceleration in a stock ceases (at inflection points, when stocks are changing the most), i.e., as `S_ddot -> 0`, then the loop score approaches infinity. This demonstrates:
- LTM **favors loops with large gains that pass through stocks changing the most**
- The **relative loop score** (normalized across all loops) is necessary to make the infinities at inflection points interpretable
- Dominance is fundamentally a measure of **relative importance**, so loop scores are only meaningful in relation to each other

The paper notes one exception: the sum of the absolute values of all loop scores may have some use to express the magnitude of the change in the model at an instant in time.

---

## 7. How LTM Differs from PPM and Loop Impact

| Aspect | LTM (Corrected) | PPM / Loop Impact |
|--------|-----------------|-------------------|
| **Polarity** | Structural (based on model structure) | Behavioral (curvature of behavior) |
| **Scope** | Model-wide (single measure per loop) | Per-stock (measure for each stock in loop) |
| **Chaining** | Link scores chain via multiplication through multiple stocks | Separate analysis per stock |
| **Single-stock** | Relative loop scores identical to PPM | -- |
| **Multi-stock** | Different results due to cross-stock weighting | Different results |
| **Dominance definition** | Applies to entire model (or connected subcomponent) | Applies to individual stocks |

**LTM's definition of dominance** (from Schoenberg et al., 2020, p. 159): A loop (or set of loops) is dominant if it describes at least 50% of the observed change in behavior across **all stocks** in the model over the selected time period. This model-wide perspective is unique to LTM and is enabled by the structural polarity convention, which allows chaining through multiple stocks (Eq. 14).

---

## 8. Figures

### Figure 1
A stock-and-flow diagram showing a stock S with a separate inflow (in) and outflow (out). This represents the disaggregated flow structure used in Table 1.

### Figure 2
A stock-and-flow diagram showing the same model restructured with a single net flow auxiliary (net = in - out) feeding into stock S. The variables `in` and `out` are now auxiliaries feeding into `net`. This represents the aggregated flow structure used in Table 2.

---

## 9. Key Differences from the 2020 Paper

| Aspect | 2020 Paper | 2023 Correction |
|--------|-----------|-----------------|
| **Flow-to-stock formula** | Eq. 1: uses flow **values** divided by net flow | Eq. 3: uses flow **changes** divided by change in net flow |
| **Sensitivity** | Sensitive to flow aggregation | Aggregation-invariant |
| **Conceptual basis** | Portion of net change from each flow | First-order partial change relative to second-order change |
| **Separate calculation** | Needed separate formula for flow-to-stock links | Can use same arithmetic as instantaneous links |
| **Near equilibrium** | All scores become very large; magnitude depends on flow structure | Scores approach 0 (stock not changing) |
| **Relationship to PPM** | Unclear | Clearly derived from PPM/Loop Impact with known differences |
| **Implementation** | Original Stella Architect 2.0 | Updated in Stella Architect 2.1+ |

---

## 10. Summary of All Equations

### Unchanged from 2020:

**Eq. 2 (Instantaneous Link Score):**
```
LS(x -> z) = |Delta_x(z) / Delta(z)| * sign(Delta_x(z) / Delta(x))
           = 0   when Delta(z) = 0 or Delta(x) = 0
```

### Deprecated (from 2020):

**Eq. 1 (Original Flow-to-Stock Link Score -- FLAWED):**
```
LS(i -> S) = |i / (i - o)| * (+1)      [inflow]
LS(o -> S) = |o / (i - o)| * (-1)      [outflow]
```

### New in 2023:

**Eq. 3 (Updated Flow-to-Stock Link Score -- CORRECTED):**
```
LS(i -> S) = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)      [inflow]
LS(o -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)      [outflow]
```

**Eq. 4 (Restated link score, splitting the fraction):**
```
LS(x -> z) = (Delta_x(z) / Delta(x)) * |Delta(x) / Delta(z)|
```

**Eq. 5 (Continuous-time limit of link score, dt -> 0):**
```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|
```

**Eq. 6 (Continuous-time limit of updated flow-to-stock link score):**
```
LS(i -> S) = |di/dt / d^2S/dt^2|
```

**Eq. 7 (Link score between adjacent stocks):**
```
LS(S1 -> S2) = (df/dS1) * |S1_dot / S2_ddot|
```

**Eqs. 8-9 (Derivation of impact from stock dynamics):**
```
dS2/dt = f(S1, ...) + ...
d^2S2/dt^2 = (df/dS1)(dS1/dt) + ... = [Impact(S1->S2)] * (dS2/dt) + ...
```

**Eq. 10 (Impact between stocks):**
```
Impact(S1 -> S2) = (df/dS1) * (S1_dot / S2_dot)
```

**Eq. 11 (Relationship between link score and impact):**
```
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * Sign(S1_dot) / Sign(S2_dot)
```

**Eq. 12 (Impact as loop gain for single-stock models):**
```
Impact(S1 -> S1) = df/dS1 = G1
```

**Eq. 13 (Loop score as weighted loop gain):**
```
LS(S1 -> S1) = G1 / |S1_ddot / S1_dot|
```

**Eq. 14 (Two-stock loop score):**
```
LS(loop) = G2 * |S1_dot / S1_ddot| * |S2_dot / S2_ddot|
```

---

## 11. Limitations and Caveats

1. **The paper does not provide new example models with full LTM analysis.** It focuses on demonstrating the corrected formula using a minimal example and then derives the theoretical relationships. The reader is expected to refer to Schoenberg et al. (2020) for full model analyses.

2. **Multi-stock loop dominance:** LTM provides a single measure per loop for the whole model, whereas PPM/Loop Impact provide per-stock measures. The paper acknowledges this leads to different results for multi-stock models but argues the difference is justified by LTM's model-wide perspective.

3. **Infinities at inflection points:** The loop score approaches infinity when stock acceleration approaches zero. While the relative loop score normalizes this, the paper notes this is an inherent property rather than a limitation.

4. **Implementation note:** The corrected formula is implemented in Stella Architect 2.1+. The paper leaves open whether implementors should aggregate flows automatically or use Eq. 3 directly, as both approaches are now equivalent.

5. **Polarity difference from PPM/Loop Impact:** LTM measures structural polarity while PPM measures behavioral polarity. This is presented as a design choice, not a limitation, but users should be aware that the same loop may have different reported signs depending on the method used.

---

## 12. References (Key Citations)

- **Schoenberg, Davidsen, Eberlein (2020)** - "Understanding model behavior using the loops that matter method" - The original LTM paper that this corrects
- **Schoenberg and Eberlein (2020)** - "Seamlessly integrating loops that matter into model development and analysis" - Practical integration paper
- **Mojtahedzadeh et al. (2004)** - "Using DIGEST to implement the pathway participation method" - PPM method that LTM is related to
- **Hayward and Boswell (2014)** - "Model behaviour and the concept of loop impact" - Loop Impact method
- **Hayward and Roach (2017)** - "Newton's laws as an interpretive framework in system dynamics" - Derivation of impact equations
- **Kampmann (2012)** - "Feedback loop gains and system behaviour" - Definition of loop gain G1
- **Richardson (1995)** - "Loop polarity, loop dominance, and the concept of dominant polarity" - Polarity measurement method used in link scores
