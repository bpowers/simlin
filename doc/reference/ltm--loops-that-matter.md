# Loops That Matter (LTM): Technical Reference

This document synthesizes five papers and the associated PhD thesis that collectively
define the Loops That Matter method for loop dominance analysis in system dynamics models:

1. **Schoenberg, Davidsen, and Eberlein (2020)** -- "Understanding model behavior using
   the Loops that Matter method." *System Dynamics Review* 36(2): 158--190. *Foundational
   paper introducing link scores, loop scores, and dominance analysis.*

2. **Schoenberg, Hayward, and Eberlein (2023)** -- "Improving Loops that Matter." *System
   Dynamics Review* 39(2): 140--151. *Corrects the flow-to-stock link score formula to
   eliminate sensitivity to flow aggregation.*

3. **Eberlein and Schoenberg (2020)** -- "Finding the Loops that Matter." *Conference
   paper. Describes the strongest-path loop discovery algorithm for models too large for
   exhaustive loop enumeration.*

4. **Schoenberg and Eberlein (2020)** -- "Seamlessly Integrating Loops That Matter into
   Model Development and Analysis." *Conference paper. Production implementation in the
   Stella family of products: macro handling, discrete elements, simplified CLDs, and
   builtin functions.*

5. **Schoenberg (2020)** -- "Loops that Matter." *PhD Thesis, University of Bergen.
   Article-based dissertation providing the framing narrative, the LoopX visualization
   tool, Feedback System Neural Networks (FSNN) for causal inference, and a synthesis of
   all five articles.*

The papers are presented here in logical order: the foundational method first, then its
correction, then the scalability solution, then the production integration challenges,
and finally the broader thesis-level synthesis.

---

## 1. Motivation and Problem Statement

The relationship between model structure and behavior is central to system dynamics. Given
a model, practitioners must understand *which feedback loops drive behavior at each point
in time* -- this is the **loop dominance analysis** problem.

Ford (1999) identified two needs: (i) automated analysis tools applicable to models with
many loops, and (ii) a clear and unambiguous definition of loop dominance.

Sterman (2000, chapter 21) identified three specific challenges for the field:
1. "Automated identification of dominant loops and feedback structure"
2. "Visualization of model behavior"
3. "Linking behavior to generative structure"

Despite 40+ years of research and many publications, none of the prior approaches have
achieved widespread adoption. The thesis attributes this to two factors: (1) each approach
has systematic limitations and blind spots, and (2) all of them require the practitioner
to do significant work beyond normal modeling. Experienced practitioners rely on intuition;
less experienced modelers are overwhelmed by model-building itself and cannot also learn a
complex analytical toolset.

### 1.1 Prior Approaches

**Eigenvalue Elasticity Analysis (EEA):**
- Originated with Forrester (1982), formalized by Kampmann (2012), refined by Saleh et al.
  (2002, 2006, 2010), Oliva (2004, 2016), Naumov and Oliva (2018)
- Characterizes behavior as weighted behavior modes via decoupled eigenvalues
- Kampmann (2012) developed Independent Loop Sets (ILS); Oliva (2004) refined these into
  Shortest Independent Loop Sets (SILS), composed only of geodetic (shortest) loops
- Can identify leverage points for policy intervention
- *Strengths:* Most comprehensive structural analysis; can analyze equilibrium states;
  identifies distinct behavior modes (growth, oscillation, etc.)
- *Weaknesses:* Requires model linearization; limited to continuously differentiable
  systems; current tools modify model equations (e.g., changing a discrete integer-only
  variable from 2 to 2.1), introducing logical errors; requires specialized knowledge

**Pathway Participation Metric (PPM):**
- Mojtahedzadeh et al. (2004)
- Traces causal pathways from a specific stock to ancestor stocks
- Partitions behavior into phases where slope and convexity are maintained (7 behavior
  patterns)
- Determines dominance by making minute changes to a stock and tracing which pathway has
  the largest magnitude effect
- *Strengths:* No model modification required; works with discontinuous models; converges
  on a unique piece of dominant structure
- *Weaknesses:* Focused on single stocks, not whole-model behavior; criticized for
  inability to cleanly explain oscillatory behavior (Kampmann and Oliva, 2009): sign
  changes during sinusoidal oscillation even though relative loop contributions are
  constant; may fail when two pathways have similar importance

**Loop Impact Method:**
- Hayward and Boswell (2014), simplified from PPM
- Implementable in standard SD software by adding equations -- no engine changes required
- Focuses on direct impact one stock has on another, chaining impacts for a loop metric
- Product of impacts equals loop gain
- *Strengths:* Implementable without engine modification; more intuitive framing
- *Weaknesses:* Still stock-specific rather than model-wide; like PPM, treats integration
  links using the second derivative rather than the first

**Ford's Behavioral Approach:**
- Ford (1999) -- qualitative approach based on practitioner intuition
- Identifies behavioral phases by examining stock behavior
- *Strengths:* Intuitive; no specialized tools needed
- *Weaknesses:* Subjective; does not scale; no quantitative rigor

### 1.2 How LTM Compares

| Property | EEA | PPM | Loop Impact | LTM |
|----------|-----|-----|-------------|-----|
| Scope | Whole model | Single stock | Single stock | Whole model (cycle partition) |
| Integration link treatment | Second derivative | Second derivative | Second derivative | First derivative |
| Works at equilibrium? | Yes | No | No | No |
| Identifies behavior modes? | Yes | Partial (7 patterns) | No | No |
| Requires linearization? | Yes | No | No | No |
| Works on discontinuous models? | No | Yes | Yes | Yes |
| Requires model modification? | Yes (perturbation) | No | Yes (added equations) | No |
| Computational complexity | High | Moderate | Low | Low-moderate |
| Identifies leverage points? | Yes | No | No | No |
| Tool availability | Specialized | Specialized | In-model | Stella 2.0+ checkbox |

LTM sacrifices equilibrium analysis and behavior mode identification in exchange for
generality (works on any model type), simplicity (no specialized math), and accessibility
(single checkbox in production software).

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

This model-wide perspective is unique to LTM and is enabled by the structural polarity
convention, which allows chaining through multiple stocks. PPM and Loop Impact provide
per-stock measures instead.

A standard feedback loop is a set of interconnections forming a closed path from a variable
back to itself, including at least one state variable (stock).

### 2.2 Three Metrics

LTM introduces three dimensionless metrics, all computed at each simulation timestep:

1. **Link score** -- the contribution and polarity of a single causal link
2. **Loop score** -- the contribution of a feedback loop to model behavior (product of
   constituent link scores)
3. **Relative loop score** -- the loop score normalized by the sum of absolute loop scores,
   yielding a value in [-1, 1] representing the fractional contribution

Additionally, **path scores** (the product of link scores along a multi-step path) enable
handling of macros and model simplification (see Section 5).

### 2.3 Key Properties

- Calculations are done directly on original model equations during simulation.
- No model transformation, canonical form, or eigenvalue computation required.
- Uses only values computed during a regular simulation (plus one ceteris-paribus
  re-evaluation per link per timestep).
- Applicable to discrete, discontinuous, and agent-based models as long as the structure is
  a network of equations evaluated at known time points.
- Does NOT affect model validity.
- Constants and parameters: variables whose values do not change have link scores of 0 (by
  definition, Delta(x) = 0). However, constants are still significant to loop dominance
  because they condition the link scores of other links: a parameter appearing in an
  equation affects how much of z's change is attributable to x vs. y, even though the
  parameter itself is not changing.

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
If z does not change, all links into z have score 0. Note that a variable whose outgoing link
scores are all 0 can still matter indirectly -- it may appear as a parameter in other equations
and therefore affect other link scores.

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
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * Sign(S1_dot) * Sign(S2_dot)
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
  This is because the loop score measures the *fraction* of behavior attributable to the
  loop, and with no other loops, 100% of behavior is attributable to it. This is a key
  distinction from loop gain.
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

Three key implications of the multi-stock loop score formula
(LoopScore = G_n * product(|Si_dot / Si_ddot|)):
1. **Structural polarity:** Loop scores always measure structural polarity because of the
   absolute values of the loop impacts in the denominator.
2. **Equilibrium behavior:** If a stock is not changing (at max/min or equilibrium), the
   loop score approaches 0. Inactive loops are never explanatory.
3. **Inflection behavior:** As acceleration ceases (at inflection points, when stocks are
   changing the most), the loop score approaches infinity. LTM favors loops with large
   gains that pass through stocks changing the most.

The sum of absolute values of all loop scores may have some use to express the magnitude of
the change in the model at an instant in time, but dominance is fundamentally a measure of
relative importance.

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
- The sum of absolute relative loop scores equals 1.0 (or 100% when expressed as
  percentages)

### 4.5 Loop Polarity

#### Structural Polarity

Determined from model structure:
- **Reinforcing (R):** Even number of negative links -> positive loop score
- **Balancing (B):** Odd number of negative links -> negative loop score
- **Undetermined (U):** Any link has unknown polarity (a conservative classification)

#### Runtime Polarity

Some models contain links (and therefore loops) that change polarity during simulation.
For example, in the yeast alcohol model, a link from yeast concentration to growth is
first positive, then negative (a formulation flaw, but it demonstrates the phenomenon).
Any loop containing such a link has expressed both positive and negative polarities at
different points in time.

The Stella implementation uses a **loop polarity classification scheme**:

| Label | Meaning |
|-------|---------|
| Rx | Reinforcing (index x) |
| Bx | Balancing (index x) |
| Rux | Unknown polarity, predominantly reinforcing |
| Bux | Unknown polarity, predominantly balancing |
| Ux | Unknown polarity with no clear predominance |

The Ru and Bu designations are assigned when the **polarity confidence value is above
0.99** (calculated using the confidence formula described in Section 13.7). This cutoff
allows a well-reasoned factual interpretation of diagrams where the polarity-changing
nature of links is not important over the course of the simulation.

---

## 5. Path Scores and Composite Link Scores

### 5.1 Path Score

An important attribute of link scores is that they can be **multiplied together** to give
the effect along a path between an input and an output. This product is called the **path
score**:

```
PathScore(x -> ... -> z) = LS(x -> a) * LS(a -> b) * ... * LS(y -> z)
```

Path scores are the foundation for:
1. **Loop scores** (a path score around a closed loop)
2. **Composite link scores** for macros (Section 6)
3. **Simplified link scores** in simplified CLDs (Section 13)

The chain rule invariance property (Section 4.2) guarantees that the path score equals the
link score that would be computed if all intermediate variables were eliminated and the
entire path were a single equation from x to z. This means structurally equivalent models
(differing only in how many intermediate auxiliary variables they use) produce identical
path scores.

### 5.2 Composite Link Score (for Macros)

When a model uses macros (DELAY, SMOOTH, etc.), multiple internal causal pathways may exist
between the macro's apparent inputs and outputs. The **composite link score** is the path
score of the expanded pathway with the **largest magnitude** at each calculation interval.

This is described fully in Section 6.

### 5.3 Composite Relative Loop Score (for Simplified CLDs)

When generating simplified CLDs, each simplified loop may represent multiple full-model
loops. The **composite relative loop score** for a simplified loop is the direct sum of the
relative loop scores from all full loops that map to that simplified loop:

```
CompositeRelativeLoopScore(SL) = sum_{L maps to SL}(RelativeLoopScore(L))
```

Since each full loop maps to exactly one simplified loop, the sum of absolute composite
relative loop scores across all simplified loops represents the **total fraction of model
behavior explained by the simplified CLD**. Numbers closer to 100% mean the simplified CLD
captures more of the model's dynamics.

This is described fully in Section 13.6.

---

## 6. Handling Macros (DELAY, SMOOTH, etc.)

Macros are one of the most significant practical challenges for implementing LTM in a
production environment. This section synthesizes the detailed treatment from Schoenberg and
Eberlein (2020, "Seamlessly Integrating") and the thesis.

### 6.1 The Problem

Macros like DELAY3 or SMOOTH incorporate complex hidden internal structure. From the
practitioner's perspective, there is a direct link between "input" and "output using macro."
But the full set of relationships underlying the macro reveals a much less direct path.

The challenges are:
1. Multiple causal pathways exist through the macro with differing strengths and
   potentially differing polarities.
2. There may be feedback loops **within** the macro equations themselves.
3. Expanding macros to show all internal variables would be confusing, generate meaningless
   variable names, and undermine one of the main reasons for using macros (preventing
   clutter).

### 6.2 Internal Structure: The DELAY3 Example

A DELAY3 macro expands into **3 stocks, 4 flows, and multiple causal pathways**. What
appears to be two simple direct links on the diagram (input -> output, delay time ->
output) is actually **seven distinct causal pathways** if we include influences to flows
both directly and through upstream stocks.

Additionally, the structure itself contains **three feedback loops** internal to the macro
(the first-order drains within each delay stage). These internal loops generally do not
produce behavior by themselves -- they are an implementation detail of the delay mechanism.

### 6.3 The Composite Link Score Solution

The solution is a simple heuristic applied at each calculation interval:

1. **Compute the path score** for every pathway through the macro (product of link scores
   along each internal pathway).
2. If there is **only one pathway** through the macro, the composite link score equals that
   path score. This is identical to the score that would have been computed if the macro
   had been fully expanded into the model.
3. If there are **multiple pathways**, choose the path score with the **largest magnitude**
   (whether positive or negative).

**Why this works:**
- **Single path case:** The loop score for any loop through the macro is exactly what it
  would have been if the macro had been expanded. No information is lost.
- **Multiple path case:** The loop score reflects the biggest (most important) of all the
  loops involving the macro. Each full-model loop through the macro traverses one specific
  internal pathway; the composite picks the strongest one.

**Dynamic pathway selection:** The dominant pathway through a macro is **not necessarily
fixed** throughout the simulation run. At different timesteps, different internal pathways
may have the largest path score. This is correct behavior -- the most important causal
mechanism within a macro can change over time, just as loop dominance itself changes.

### 6.4 Loop Trimming for Macros

Two kinds of loop trimming are needed:

1. **Internal loop suppression:** Loops that are entirely internal to the macro (such as
   the first-order drain loops within DELAY3) are **dropped altogether** and not reported
   to the user. These loops are structural artifacts of the macro's implementation.

2. **Loop collapsing:** Loops that pass through the macro (entering from one external
   variable and exiting to another) must have their internal segments collapsed. The
   reported loop shows only the macro's external inputs and outputs, not the internal
   variables. The link score for the macro segment is the composite link score.

### 6.5 Edge Cases

**Step input to a DELAY3 with no feedback:** If a DELAY3 macro receives a step input (and
nothing else -- i.e., the macro is not part of a feedback loop), the composite link score
will always be 0. This happens because:

- The input changes only at a single point in time when the output is not yet changing
  (input has changed, output has not responded yet, so one internal link score is 0)
- Once the output starts changing, the input is no longer changing (input is now constant,
  so the link from input to the first internal stock has score 0)
- Since the reported link score is the product of internal link scores (one of which is
  always 0 at any given time), the composite is always 0

This makes macros somewhat inscrutable when they are not actually part of any active
feedback structure -- but this is correct behavior. The macro is not contributing to any
feedback loop, so reporting a score of 0 is appropriate.

### 6.6 Rejected Alternatives

Three alternative approaches were considered and rejected:

1. **Post-processing all path scores and picking the best one:** This would have the
   advantage of invariant macro structure (the same pathway is always reported), but it
   would change loop scores relative to the fully expanded case. The per-timestep approach
   was preferred because it preserves fidelity with the expanded model.

2. **Predefined pathways for link score computation:** Fixing which internal pathway
   represents the macro a priori. Rejected for the same reason -- it would not reflect the
   dynamic nature of which pathway is actually most important at each point in time.

3. **Expanding all macros to expose internal variables to the user:** Rejected because it
   would be confusing, generate meaningless variable names (what does "DELAY3_stage_2_of_
   production_start_rate" mean to a practitioner?), and undermine the purpose of macros.

### 6.7 Implementation Implications for Modules

The macro handling approach generalizes to any model structure where complex internal
structure is hidden behind a simplified external interface. This is directly relevant to
**modules** (submodels), which are a form of macro at a larger scale:

- A module has external inputs and outputs, with potentially complex internal structure
  including stocks, flows, feedback loops, and multiple causal pathways.
- The composite link score approach applies: compute path scores for all pathways through
  the module, and use the largest-magnitude path score at each timestep.
- Internal loops within the module that do not connect to external variables should be
  suppressed from the parent model's loop analysis.
- Loops that pass through the module should be collapsed to show only the module's
  interface variables.
- The PATHSCORE builtin (Section 10) can be used to examine specific internal pathways
  when deeper understanding of the module's contribution is needed.

---

## 7. Handling Discrete Elements

### 7.1 The Problem

Some modeling environments include discrete elements (Conveyors, Queues, Ovens) and
builtin functions like PREVIOUS that retain state. Unlike macros, there is no rigorous way
to expand these into structures amenable to complete link score computation because:

- Internal structures cannot practically be exposed. A conveyor can have **thousands of
  individual elements** waiting to be used at a later time.
- Following paths through such elements is not feasible -- the internal structure is not a
  standard stock-and-flow network.

### 7.2 The Perfect Mixing Approximation

The solution treats the instantaneous response to changing inputs **as if it were the
eventual response** -- analogous to the perfect mixing assumption in traditional SD models.

**Rationale:** For a normal stock with proportional outflow, a change in stock value causes
the outflow to change immediately (at the next timestep). For a conveyor, the change is not
immediate but eventual (items must traverse the full conveyor length), but the basic
character of the response is the same: an increase in input eventually leads to an increase
in output. Treating the response as instantaneous:

- Gets **polarity** correct (the direction of influence is the same whether instantaneous
  or delayed)
- Gets **magnitude** approximately correct (the size of the eventual effect is captured)
- May distort the **time profile** of loop dominance (reporting the effect earlier than it
  actually occurs)

### 7.3 Practical Validation

The workforce training model example (Section 11.5) demonstrates that LTM works well on
models with conveyors and non-negative stocks. The perfect mixing approximation correctly
identifies:
- Hidden feedback loops within the conveyor (between its contents and its output)
- Structural changes when non-negative constraints become active
- Different feedback structures under different parameterizations

---

## 8. Cycle Partitions

For models with a single cycle partition (every stock has a feedback path to/from every
other stock), all loops are compared against each other.

For models with multiple cycle partitions, only loops within the same partition are
compared. Loop scores between partitions are not meaningful because the stocks are
structurally independent.

LTM does **not** require loops to be independent (unlike EEA which uses ILS/SILS). All
connected loops are considered. Restricting to independent loops can filter out important
ones, as demonstrated by the three-party arms race model (Section 12.2).

---

## 9. Computational Considerations

### 9.1 When Computations Occur

- First computation after model initialization and first timestep
- Computed at each dt using Euler integration
- In principle compatible with Runge-Kutta and other integration methods
- The corrected flow-to-stock formula requires values from two previous timesteps
  (Delta(S_t) and Delta(S_{t-dt})), so link scores for flow-to-stock links are undefined
  for the first two timesteps

### 9.2 Equation Re-evaluation Cost

For each non-stock variable with equation `z = f(x1, x2, ..., xk)`, the equation must be
re-evaluated **k times** per timestep (once per input variable, holding others at previous
values). For a typical model where most variables have 2-4 inputs, this roughly doubles the
number of equation evaluations vs. normal simulation. For variables with many inputs (e.g.,
a lookup function of 10 variables), the cost is proportionally higher.

### 9.3 Storage Requirements

Previous values of **all** variables must be retained between timesteps. The delta
computation requires both current and previous values for every variable. This doubles
memory usage for variable storage.

For the corrected flow-to-stock formula, the net flow from the *previous* timestep
(Delta(S_{t-dt})) must also be stored, requiring an additional value per stock.

### 9.4 Performance

For models with 2-20 stocks and fewer than 50 feedback loops, the computational burden is
dominated by loop finding, not score computation. Analysis of all models in the 2020 paper
(including Forrester's 10-stock market growth model) takes less than 1 second total.

Link scores are computed for every link at every dt during simulation. For a model with L
total links and T timesteps, this is L*T link score computations. Each link score
computation is O(1) given the precomputed ceteris paribus values.

Total overhead for typical models: LTM roughly doubles simulation time. For models with
many inputs per variable or many feedback loops, the overhead can be higher.

---

## 10. LOOPSCORE and PATHSCORE Builtins

The Stella implementation includes two builtin functions that give practitioners direct
control over which loops and paths are analyzed.

### 10.1 PATHSCORE

- Can be used **in model equations** during simulation
- Computes the raw path score (product of link scores) along a specified path
- Available at every timestep, not just at the end of simulation
- Can be used to examine specific internal pathways through macros or modules
- Returns the instantaneous (non-relative) score

### 10.2 LOOPSCORE

- Allows practitioners to specify **any arbitrary feedback loop** and compute its score
  over time
- Reports **relative** loop scores (normalized by all known loops)
- Only available at the **end of simulation** (because relative scores require
  normalization across all loops, which requires the full set of loop scores to be known)
- **Guarantees the specified loop will be reported** regardless of whether the strongest
  path algorithm discovered it during simulation
- Enables **cross-run comparisons** of how active a loop is under different scenarios

### 10.3 Why Both Are Needed

The strongest path algorithm (Section 12) is heuristic -- its results change with
parameterization. A loop that is important in one scenario may not be discovered in another.
The LOOPSCORE builtin solves this by allowing practitioners to track specific loops
regardless of the discovery algorithm's output.

PATHSCORE serves a different need: examining the internal workings of macros, modules, or
specific causal pathways during simulation, without waiting for the full analysis to
complete.

---

## 11. Application Examples

### 11.1 Bass Diffusion Model

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

### 11.2 Yeast Alcohol Model

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

### 11.3 Inventory Workforce Model

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

### 11.4 Economic Cycles Model (Mass, 1975)

**Characteristics:** 163 variables, 17 stocks, 494 feedback loops (plus 4 additional
two-variable stock/flow balancing loops in separate cycle partitions that do not affect
model behavior).

**Simplified CLD:** A machine-generated simplified CLD with link inclusion threshold over
100%, loop inclusion threshold 2.4%, and flows not automatically kept, produces **9
simplified feedback loops** representing the combined effects of **21 full feedback loops**.
These 9 simplified loops explain **59.7%** of the behavior across the entire simulation.

Of the remaining 40.3%:
- **31.2%** comes from 469 relatively unimportant loops (each individually producing less
  than 2% of cumulative behavior)
- **8.9%** comes from 4 remaining loops consisting of two sets of paired feedback loops
  (one balancing and one reinforcing in each pair) that perfectly cancel each other at all
  time points, making all 4 irrelevant to observed behavior

**Loop dominance analysis (one complete cycle, time 9 to 12.5):**

The loop dominance pattern repeats twice in the analysis period. The progression within
each half-cycle:

1. **B1 starts nearly completely dominant.** B1 describes the major drivers of hiring labor
   based on vacancies created by backlog driven directly by inventory changes.
2. **R1 and B3 become active as B1 wanes.** These are a pair that **perfectly destructively
   interfere** -- they cancel each other out because their only difference is the specific
   route through which perceived rate of increase in price affects vacancies. R1
   (reinforcing) represents the effect of perceived price increase on backlog (reinforcing
   vacancies); B3 (balancing) represents the same price perception effect on inventory
   (balancing vacancies).
3. **B4 becomes important.** Like B1, B4 describes changes in labor due to vacancies, but
   through a long delay from the perception of price (inventory changes affecting unit
   costs). After B4's delayed price adjustment effect plays out, B1 becomes dominant again.
4. **B2 becomes active,** representing the direct effect of backlog changes on labor
   through termination.
5. **B2 becomes the dominant loop,** driving changes in labor through termination at the
   inflection points.
6. **B2 yields back to B1,** starting the cycle anew.

Every other progression through this cycle changes whether labor is growing or shrinking
(since B1 is an oscillatory loop). This cogent explanation of a 163-variable, 494-loop
model demonstrates the toolset's power to simplify complex model understanding while
providing objective clarity on the causes of behavior.

### 11.5 Discrete Workforce Training Model

**Structure:** A workforce training model using a **conveyor** (pipeline delay) for the
training process and a **non-negative stock** for Workers.

**Two parameterizations analyzed** (identical except for "time to adjust": 5 vs 2):

**Case 1 (time to adjust = 5):** The simplified CLD shows **two balancing loops**:
- One involving apprentices -> finishing training -> workers -> adjustment -> hiring
- One showing the hidden feedback loop within the conveyor between Apprentices and
  finishing training (the conveyor directly affects its own output)

**Case 2 (time to adjust = 2):** The non-negative stock becomes active and constrains the
outflow "leaving." The simplified CLD shows **four loops**:
- The same two balancing loops from Case 1
- An additional balancing loop (between leaving and workers)
- An additional reinforcing loop (across the full chain without passing through the
  adjustment variable)

**Key insights:**
1. LTM identifies **hidden feedback loops in discrete structures**. The conveyor's internal
   feedback (output depends on its own contents) is correctly surfaced.
2. The **feedback complexity of the model changes with its parameterization.** The
   non-negative constraint creates additional loops only when it becomes active.
3. LTM is fully capable of performing analyses on **discrete systems** without losing
   insight capability. The perfect mixing approximation works well in practice.

### 11.6 Population Models (Pedagogical Examples)

**Births only:** One reinforcing loop (R1) that accounts for 100% of behavior. Students
can adjust growth rate to develop intuition for exponential growth.

**Births and deaths (average lifetime = 20):** Two loops: R1 (+67%), B1 (-33%). Still
exponential growth but slower.

**Equilibrium case (average lifetime = 10):** Nothing changes and no loops are reported.
This is a **learning moment**: the lack of reported loops reflects that two opposing loops
are perfectly balanced -- either cosmic coincidence or meaningful. This naturally leads to
introducing carrying capacity.

**Births, deaths, and carrying capacity:** Three loops: R1 (+50%), B1 (-36.17%),
B2 (-13.83%). This dramatically shows the difference between a fragile equilibrium and one
resulting from **shifting loop dominance**. The capacity constraint loop (B2) is at first
inactive but then becomes the bigger of the two balancing loops.

**Pedagogical value:** LTM helps even with "obvious" models because "simply obvious" means
different things to novices versus experienced modelers. The extra quantitative cues make
discussion faster, more informative, and better remembered. Highlighting a balancing loop
on the stock-and-flow diagram (e.g., the outflow "deaths" loop which lacks a visible
arrowhead back to the stock) helps students see loops that are structurally present but
visually hidden.

---

## 12. Loop Discovery: The Strongest Path Algorithm

### 12.1 The Loop Enumeration Problem

For small models, all feedback loops can be enumerated (e.g., using Tarjan, 1973).
For large models, the number of loops grows up to the factorial of the number of stocks:
- **Urban Dynamics:** 43,722,744 loops
- **World3-03:** 330,574 loops

Exhaustive enumeration is impractical for such models. More importantly, restricting
analysis to independent loop sets misses dynamically important loops.

### 12.2 Why Independent Loop Sets Are Insufficient

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

### 12.3 Why Composite Networks Fail

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

This was demonstrated with a decoupled two-stock model where both stocks start at 1, with
conditional IF/THEN equations creating loops that are active at different times. The model
has two minor loops (each stock's self-loop) and one major loop (stock-to-stock). Only one
loop is active at any given time, but the composite network assigns scores that misrank
their importance.

### 12.4 The Adopted Strategy: Per-Timestep Discovery

Instead of a composite network, run loop discovery **at each simulation timestep** (or a
subset). This requires many more discovery passes, but each pass converges quickly because
actual link scores provide strong pruning guidance.

For models with fewer than ~1,000 total loops, exhaustive enumeration on a composite
(max-based) network is used instead, since it is guaranteed complete and fast enough.

### 12.5 Algorithm Description

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
        Call Check_outbound_uses(link.variable, score * link.score)
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

### 12.6 Why This Is a Heuristic

Because the algorithm maximizes (rather than minimizes), it does **not guarantee finding
the truly strongest loop**. In Dijkstra's shortest path algorithm, minimization guarantees
optimality: once a node is reached with some cost, any later path through that node must
cost at least as much. With maximization, this guarantee does not hold -- a weaker path to
a node may be part of a stronger overall loop.

The failure mechanism: if the search reaches variable b via a high-scoring intermediate path
(setting b's best_score high), a later direct path to b with a lower cumulative score will
be pruned. If that direct path would have completed a stronger loop than any loop found
through the intermediate path, the strongest loop is missed.

The Eberlein and Schoenberg (2020) paper demonstrates this with a 4-node graph (Figure 7).
The algorithm finds loop a->d->c->a (score 100) but misses the stronger loop a->b->c->a
(score 1000). The miss occurs because the search from a reaches b via the path a->d->...->b
with a high cumulative score, and when the direct a->b path is later considered, b's
best_score causes it to be pruned.

The authors note that starting the search from different stocks can recover missed loops
(starting from b or c in the example above might find the missed loop). However, it is
theoretically possible to construct graphs where the strongest loop is missed regardless of
starting stock.

**Mitigating factors:**
- The algorithm runs from every stock at every timestep, providing many opportunities to
  discover each loop
- Empirically, missed loops are **structurally very similar** to found loops -- they are
  "siblings" that share most of their path but differ by a few links (see Section 12.7)
- The set of discovered loops **can change based on the parameterization** of a simulation
  run. Just as the most important loops change with different parameters, so will the loops
  actually identified. This is a feature: the algorithm dynamically adapts to what is
  actually important in the current scenario.

### 12.7 Completeness Evaluation

Tested against exhaustive enumeration on models small enough:

**Market Growth Model (19 loops):** All 19 found. (Note: with macro expansion, the count
is 23 -- the difference of 4 is from internal DELAY/SMOOTH feedback loops.)

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

### 12.8 Performance on Large Models

**Urban Dynamics (43,722,744 loops):**
- Discovered 20,172 loops
- After 0.1% contribution cutoff: < 200 retained
- Computation time: 10-20 seconds (8th gen Intel Core i7)

**World3-03 (330,574 loops):**
- Discovered 2,709 loops
- After 0.1% contribution cutoff: 112 retained
- Computation time: ~4 seconds

### 12.9 Failed Approaches

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

## 13. Visualization: Simplified CLDs and Animated Diagrams

This section synthesizes the visualization and simplification work from the LoopX tool
(thesis Article #2) and its production evolution in the Stella integration (Schoenberg and
Eberlein, 2020). The goal is to fulfill Sterman's three challenges: automated
identification, visualization of behavior, and linking behavior to generative structure.

### 13.1 Goals and Motivation

If tools for discovering the origins of behavior are simply part of the model development
experience -- turned on by a checkbox, automatically reporting loop and link information --
they will become part of how people understand the models they build. For experienced
practitioners, the tools reinforce and challenge beliefs about what drives behavior. For
less experienced modelers, they offer discoverable, easy-to-communicate pathways to
understanding.

The key design principle: LTM analysis should be essentially **invisible** until the user
wants to see it. Activation by checkbox, automatic reporting, no specialized knowledge
required.

### 13.2 Machine-Generated CLD Layout

#### The Edge Curving Heuristic

Standard force-directed graph algorithms (including neato/Graphviz) produce layouts where
connected nodes are near each other, but the resulting edge layouts don't emphasize feedback
loops. Lombardi-style diagrams produce nice curves but don't handle directed cycles. This
gap motivated a novel **edge curving heuristic** contributed to the public Graphviz
codebase.

The algorithm works on an edge-by-edge basis after node positions have been determined by
the force-directed layout:

1. For each edge, find the **shortest feedback loop (length > 2)** that the edge is a
   member of (using the model's dependency structure).
2. Compute the **centroid** (average center) of all nodes in that shortest loop.
3. The edge is drawn as a **circular arc** whose center of curvature is that centroid.
4. **Two-node exception:** Loops of length 2 (mutual connections between two nodes) are
   handled separately, producing paired directed edges rendered as elongated ellipse
   structures to avoid overlapping.

The key insight: force-directed layout algorithms (like Kamada-Kawai) already place
connected nodes near each other. By curving each edge toward the center of its shortest
loop, the visual result is that **feedback loops appear as recognizable circular
structures** in the 2D layout. This directly emphasizes the feedback structure that is
central to SD understanding.

#### Neato Configuration for Quality CLDs

The LoopX implementation uses:
1. `overlap = 'prism'` -- Prism algorithm (Gansner & Hu, 2010) to remove overlapping
   variable names with minimal layout disturbance
2. `mode = 'KK'` -- Kamada-Kawai gradient descent for node placement
3. `model = 'shortpath'` -- Shortest path between node pairs as ideal spring length
4. `splines = 'curved'` -- Invoke the edge curving algorithm

#### Initialization with SFD Positions

To avoid degenerate layouts, neato uses the position of each variable in the stock-and-flow
diagram as its initial position. This preserves local clusters from the original SFD
(assumes a well-laid-out SFD keeps related variables near each other).

#### Quality Assessment

The edge curving heuristic produces CLDs that are substantially improved over the earlier
Forio Model Explorer (Schoenberg, 2009), which produced diagrams "of significantly less
quality" than expert hand-drawn CLDs. The curved edges follow Richardson (1986) best
practices for CLD aesthetics and make machine-generated CLDs visually competitive with
hand-drawn versions. The technique is independent of LTM and works for any directed network
data.

### 13.3 Model Simplification Parameters

Two threshold parameters control the tradeoff between descriptive power and cognitive
simplicity:

#### Link Inclusion Threshold (range [0%, 100%+])

Purpose: Filter out auxiliary variables whose connections don't change much over the
simulation -- these variables exist for equation simplification rather than dynamic
complexity.

Based on **Relative Link Variance**:

```
RelativeLinkVariance(x -> y) = max(|RelativeLinkScore(x -> y)|)
                               - min(|RelativeLinkScore(x -> y)|)
```

Where the Relative Link Score is the link score normalized across all determinants of the
dependent variable y. This measures the change in the percentage contribution of x to y
over the full simulation.

- Only variables with at least one incoming link whose relative link variance >= threshold
  are included in the simplified CLD
- If a stock is included, its flows are automatically included (stocks require flows to
  change; see Section 13.10 for an option to override this)
- Variables with 0 variance are constants or linear pass-throughs -- good candidates for
  elimination
- Variables with high variance point to sources of non-linearity -- important for
  understanding dynamic complexity

The link inclusion threshold acts as a **"surgical scalpel"** -- it removes variables that
exist for equation simplification rather than dynamic complexity.

#### Loop Inclusion Threshold (range [0%, 100%])

Purpose: Filter out feedback loops (and their associated stocks/flows) that have low
average contribution to behavior.

- Represents the average magnitude of the relative loop score across the simulation period
- Only loops with average magnitude >= threshold have their stocks and flows automatically
  included
- Delayed averaging: starts from the first instant the loop becomes active (avoids
  penalizing loops during initialization)
- A threshold of 0.01 (1%) means the loop contributes only 1% of total behavior on average

#### Simplification Examples (Market Growth Model)

| Thresholds | Variables | Description |
|-----------|-----------|-------------|
| Link 0%, Loop 0% | All (48) | Full CLD, all feedback complexity |
| Link 100%, Loop 0% | ~22 | Less than half the variables; all loops represented |
| Link 100%, Loop 20% | ~17 | Further reduced; 7 stocks |
| Link 100%, Loop 100% | 4 | Maximally simplified: single most dominant loop |

The tradeoff is **descriptive power vs. ease of cognition**, best decided case-by-case.

### 13.4 Generating Simplified Links

After determining which variables to keep, the simplified CLD needs to establish which
links exist between the remaining variables. This is non-trivial because the original model
may have multi-step pathways between two kept variables.

#### LoopX Approach (Early Implementation -- Flawed)

The original LoopX implementation first selected variables to include, then found all
pathways connecting those variables using a depth-first search through the full equation
network, then detected loops in the simplified diagram. This had two shortcomings:

1. **Computational:** Loop detection in the simplified graph is expensive and unreliable.
2. **Conceptual:** The DFS reconnection process ignored *why* a variable was kept. It
   searched the entire equation network and brought forth links (and therefore loops) of
   "demonstrable unimportance" in highly simplified CLDs.

Additionally, the DFS only found the *first* valid pathway between two kept variables,
which might not be the most important one. If multiple distinct causal pathways connected
two kept variables (through different intermediate variables), only one was represented.

#### Stella Approach (Production Implementation -- Improved)

The production implementation uses a fundamentally different approach based on the
**original model loops** rather than reconnecting kept variables from scratch:

1. When a variable is kept by the link inclusion threshold, the system records **which
   specific link(s)** justified keeping that variable (the links with relative link
   variance above the threshold).
2. For each kept link, the system identifies the **strongest feedback loop** that the link
   is a member of.
3. This creates a direct mapping from each kept variable to the specific feedback loop(s)
   that made it important.
4. Simplified links are only generated along pathways that are part of these identified
   important loops, rather than any arbitrary pathway through the equation network.

This ensures that simplified links are **causally relevant** -- they exist because they are
part of loops that actually matter, not because a pathway happens to exist in the equation
network.

### 13.5 Two-Way Loop Mapping

The improved implementation maintains a **bidirectional mapping** between full-model loops
and simplified-diagram loops:

- **Full to simplified:** Given any full-model loop, determine which simplified loop (if
  any) it maps to.
- **Simplified to full:** Given any simplified loop, look up all the full-model loops it
  represents. Multiple full loops can map to the same simplified loop (when they differ
  only in intermediate variables that were filtered out).

This mapping enables:
- Computing the fraction of total model behavior the simplified CLD explains
- Attaching scores to simplified loops that faithfully represent the original analysis
- Ensuring that important connections are included

A small amount of **loop closure** is performed to improve layout aesthetics, which can add
some aesthetically pleasing but unimportant loops. These artifact loops are identifiable by
their low composite relative loop scores.

### 13.6 Composite Relative Loop Score

Because each simplified CLD loop may represent multiple full-model loops, each simplified
loop gets a composite relative loop score:

```
CompositeRelativeLoopScore(SL) = sum_{L maps to SL}(RelativeLoopScore(L))
```

Since relative loop scores are normalized and each full loop maps to exactly one simplified
loop, the **sum of absolute composite relative loop scores** for all simplified loops
represents the **fraction of full model behavior explained by the simplified CLD**.

**Example (Economic Cycles model):** 9 simplified loops representing 21 full loops explain
59.7% of total behavior. Of the remaining 40.3%: 31.2% comes from 469 individually
unimportant loops (<2% each), and 8.9% comes from 4 perfectly canceling loop pairs.

This quality metric is a major improvement over LoopX, which had no way to measure how much
behavior the simplified CLD captured.

### 13.7 Polarity Confidence

A simplified link may represent several different causal pathways with different strengths
and potentially different polarities. When a simplified link over-abstracts pathways of
both reinforcing and balancing polarities, the displayed polarity may be misleading.

The **polarity confidence** metric:

```
confidence = |r - |b|| / (r + |b|)
```

Where:
- `r` = sum of the single highest magnitude instantaneous reinforcing pathway scores
  across the entire simulation
- `b` = sum of the single highest magnitude instantaneous balancing pathway scores across
  the entire simulation

**Interpretation:**
- Confidence = 1 when only one polarity is present (either r or b is 0)
- Confidence approaches 0 when both polarities contribute equally
- A confidence value of **0.99 or lower** triggers the link to be displayed in **gray**
  (representing mixed/unknown polarity)

This makes it abundantly clear when the simplified CLD is over-simplified at a particular
point. Adjusting the link inclusion threshold down slightly can expand the over-simplified
link into its key constituent pathways, resolving the ambiguity. The Forrester (1968)
Market Growth model demonstrates this: a gray link appears in the simplified CLD, and
lowering the link inclusion threshold by one one-hundredth of a percent expands it into
its constituent pathways.

### 13.8 Link Thickness and Polarity in Simplified CLDs

For simplified links representing multi-step pathways, the displayed thickness and polarity
use the causal pathway with the **largest path score magnitude averaged across time**.

**Rationale:** If a variable is included because it has a strong link, and a loop is
included because it is strong, then the representation should reflect that strongest
pathway's average strength. This produces satisfying results for links representing
pathways of the same polarity.

For mixed-polarity links (where the confidence metric is below 0.99), the link is rendered
gray regardless of the pathway's magnitude.

### 13.9 Disconnected Simplified CLDs

The simplification process can produce **disconnected** simplified CLDs. This occurs when
a model has two strong minor feedback loops tied together by a weak major feedback loop.
When thresholds are set high, the weak major loop disappears, leaving two unconnected minor
loops as the primary drivers of behavior.

This is a **valid and faithful** representation of the underlying feedback structure. The
two subsystems are essentially independent in terms of their behavioral contribution --
the coupling between them is negligible. Displaying them as disconnected accurately
represents this.

### 13.10 Flow Inclusion Toggle

In the original definition of link and loop inclusion thresholds, anytime a stock is kept,
so are its flows, regardless of the relative link scores of those flows. In large models
with many stocks and long feedback loops, this produces extra flows that add nothing to
understanding.

A third simplification parameter -- a **boolean** controlling whether flows are
automatically kept when a stock is kept -- gives the user control over flow inclusion in
the simplified CLD. Allowing users to optionally exclude flows produces cleaner simplified
CLDs.

### 13.11 Animated Diagrams

Both SFDs and simplified CLDs can be animated:

- **Color** represents link polarity (red = negative/balancing, green = positive/
  reinforcing, gray = mixed/unknown)
- **Thickness** represents magnitude of the relative link score
- Users can scrub through simulation time, pin specific time points, and adjust thresholds
  live
- A loop table shows instantaneous contribution and average contribution for each loop,
  sorted by importance. Loop identifiers can be pressed to highlight all variables and
  links in that loop across all diagrams.
- All LTM metrics are available as CSV for external analysis.

### 13.12 Link Thickness Animation Choices

Four approaches to link thickness were analyzed in the thesis:

1. **Raw link scores:** Would explode to infinity near equilibrium transitions (the
   denominator in link scores approaches 0). Unsuitable for direct visualization.

2. **Relative link magnitude** (normalized across all inputs of a dependent variable): This
   is the approach used. Yields a fraction in [0, 1]. The normalization is per-variable, so
   loops are not directly identifiable from thickness alone -- a thick link only means it
   dominates *its particular variable's* inputs, not that it is part of a dominant loop.

3. **Loop-score-based thickness:** Each link colored/sized based on the loop scores of the
   loops it participates in. Rejected because in models where important loops share many
   links, representing a link's membership in multiple loops of different polarities does
   not scale visually.

4. **Global normalization** (across all links at a timestep, or across all timesteps): Both
   have problems. Per-timestep normalization makes equilibrium periods (when most scores
   are near 0 but ratios blow up) dominate the visual. Per-simulation normalization makes
   still-active model parts appear increasingly bright as other parts reach quiescence,
   creating misleading impressions of relative importance.

### 13.13 Split Flow Rendering in SFDs

In standard SFD notation, a flow between two stocks has an implicit dual role: it is an
outflow from one stock and an inflow to another. From an LTM perspective, the flow-to-stock
link has a different score (and potentially different polarity) for each stock.

LoopX renders biflows (flows connecting two stocks) **split in half**: the pipe section
before the valve is colored/sized based on its contribution to the source stock, the
section after the valve shows its contribution to the destination stock.

This makes the **hidden information links** in SFDs visible -- in particular, the outflow
relationship from a stock to its outgoing flow, which has no arrowhead in standard SFD
notation but is a real causal link that participates in feedback loops. This is important
pedagogically: novice modelers often miss that a stock's outflow represents a feedback
connection from the stock to the rest of the system.

### 13.14 Scalability Concerns

The visualization pipeline works well for models with up to ~50 variables and <50 feedback
loops. For larger models (e.g., T-21 or Urban Dynamics), the loop finding itself is fast
(Section 12.8), but the simplified CLD generation process (mapping full loops to simplified
loops, generating simplified links, layout computation) needs further engineering. The
visualization pipeline is the bottleneck, not the loop discovery algorithm.

---

## 14. FSNN: Causal Inference from Data

The thesis includes a novel extension of LTM beyond analyzing known models: **Feedback
System Neural Networks (FSNN)** for inferring causal structure from observational data.
This is tangential to model simulation but relevant to the broader LTM ecosystem.

### 14.1 Motivation

Existing causal inference methods (Granger causality, PC algorithm, Bayesian networks) are
designed for **directed acyclic graphs (DAGs)** and fundamentally cannot represent
feedback. FSNN fills this gap by combining neural network universal approximation with LTM
analysis to identify causal relationships in systems with feedback loops.

### 14.2 Method (Four Steps)

1. **Construct a system of ODEs** where each state variable's derivative is a separate
   neural network (MLP) that takes all state variables as inputs:
   `dState_i/dt = f_i(State_1, State_2, ..., State_n)`

   Unlike Neural ODEs (Chen et al., 2018) where state variables are latent, FSNN states
   are explicitly the **observed real system variables** (e.g., population, temperature).
   This is what makes FSNN interpretable.

2. **Train** by minimizing squared error between training data and calculated state values.
   All weights start at 0 (representing "no causal structure"). Multiple distinct
   initializations (different initial conditions) should be used simultaneously.

3. **Analyze** the trained model using LTM link scores along all direct pathways
   (State_i -> f_j -> dState_j/dt). This produces an n x n matrix of pathway scores at
   each time point: zero or near-zero means no causal influence; non-zero means causal
   influence with sign indicating polarity.

4. **Validate** against known ground truth (for synthetic data) or empirical
   experimentation (for real data).

### 14.3 Key Results

Tested on a three-state nonlinear oscillatory system (4 balancing loops, dampened
oscillation):
- Correctly identifies all 5 existing causal relationships and their polarities
- Correctly identifies all 4 non-existing relationships as zero or negligible
- Monte Carlo validation with 100 random initializations shows tight prediction bounds
  within the training data range
- Performance degrades exponentially beyond the training data range

### 14.4 Challenges

1. **Singularity problem:** Multiple causally-different models can produce
   behaviorally-identical outputs. The trained model is one possible explanation, not a
   proven fact. Validation requires empirical experimentation beyond behavior-matching.

2. **Manifold coverage:** Each initialization explores a limited region of the state space
   manifold. More initializations > more temporal samples from a single initialization
   (diminishing returns from trajectory convergence).

3. **Confounders:** With a single initialization, correlated trajectories can create false
   causal links. Multiple initializations from different starting states break these false
   correlations precipitously.

4. **Activation function scaling:** tanh saturation at large values masks real differences.
   Linear rescaling assumes roughly uniform data distribution. Logarithmic scaling may be
   needed for exponentially distributed data.

5. **Endogenous forcing:** The method forces endogenous explanations. Including an
   extraneous time series warps the payoff surface toward degeneracy.

---

## 15. Weaknesses and Limitations

### 15.1 Cannot Analyze Equilibrium States

When all stocks are unchanging, all loop scores are 0 by definition. EEA can provide
information under equilibrium for near-linear models.

Workarounds: introduce minute perturbations via STEP function, or analyze the transient
approach to equilibrium.

### 15.2 Focus on Endogenous Behavior

LTM focuses on feedback loops (endogenous structure). For models dominated by external
forcing functions, feedback effects may be small and the analysis less informative. The Loop
Impact method (Hayward and Boswell, 2014) may be better suited for highly forced models.

Link scores could in principle measure exogenous contributions, but this is not currently
part of the method.

### 15.3 Approximate Integration Sensitivity

The method as described uses Euler integration. Compatibility with Runge-Kutta and other
methods has not been fully explored (though in principle it should work at the level of
saved timesteps).

### 15.4 Heuristic Nature of Loop Discovery

The strongest-path algorithm does not guarantee finding the truly strongest loop, though
empirically it finds loops that are structurally very similar to the strongest. This is
acceptable for practical analysis but means the method cannot prove it has found *all*
important loops. The LOOPSCORE builtin (Section 10) mitigates this by allowing practitioners
to track specific loops regardless of the discovery algorithm.

### 15.5 Cannot Identify Behavior Modes

Unlike EEA, LTM does not decompose behavior into distinct modes (exponential growth,
oscillation, etc.). It reports which loops are dominant but not *what kind of behavior* they
are generating. A practitioner must infer the behavior mode from the combination of loop
polarities and time-varying dominance patterns.

### 15.6 Reports Only on Observed Behavior

LTM only analyzes behavior that actually occurs during a specific simulation run with
specific parameter values. Alternative scenarios, counterfactuals, and sensitivity to
parameters are not covered by a single LTM analysis. Monte Carlo + LTM (running many
simulations with varied parameters) is identified as a promising future direction.

### 15.7 Cannot Identify Leverage Points

Unlike EEA, LTM does not directly identify where structural changes (policy interventions)
would most affect behavior. It identifies which loops dominate, but not where to intervene.

---

## 16. Models Analyzed Across the Papers

| Model | Stocks | Variables | Loops | Papers |
|-------|--------|-----------|-------|--------|
| Simple Population | 1 | ~4 | 1-3 | Integration |
| Bass Diffusion (1969) | 2 | ~10 | 2 | Core, LoopX, Integration |
| Yeast Alcohol | 2 | ~5 | 4 | Core |
| Workforce Training (discrete) | 2 | ~10 | 2-4 | Integration |
| Inventory Workforce (Goncalves, 2009) | 3 | ~12 | 3 | Core |
| Three-State ODE (synthetic) | 3 | ~8 | 4 | FSNN (thesis) |
| Three-Party Arms Race | 3 | ~18 | 8 | Discovery |
| Market Growth (Forrester, 1968) | 10 | 48 | 19 (23 with macro expansion) | LoopX, Discovery, Integration |
| Service Quality (Oliva & Sterman, 2001) | ~15 | ~50 | 104+ | Discovery |
| Economic Cycles (Mass, 1975) | 17 | 163 | 494 | Discovery, Integration |
| Urban Dynamics (Forrester, 1969) | ~20 | ~100+ | 43,722,744 | Discovery |
| World 3-03 (Meadows, 2004) | ~30 | ~200+ | 330,574 | Discovery |

---

## 17. Notation Reference

| Symbol | Definition |
|--------|-----------|
| LS(x -> z) | Link score from variable x to variable z |
| Delta(z) | Total change in z: z(t) - z(t-dt) |
| Delta(x) | Change in x: x(t) - x(t-dt) |
| Delta_x(z) | Partial change in z due to x alone (ceteris paribus) |
| Delta(S_t) | Net flow at time t: S(t) - S(t-dt) |
| Delta(S_{t-dt}) | Net flow at time t-dt: S(t-dt) - S(t-2dt) |
| LoopScore(L) | Product of all link scores in loop L |
| RelativeLoopScore(L) | LoopScore(L) / sum(\|LoopScore(Y)\|) for all loops Y |
| CompositeRelativeLoopScore(SL) | Sum of RelativeLoopScore for all full loops mapping to simplified loop SL |
| PathScore(x -> z) | Product of link scores along a multi-step path from x to z |
| dz/dx | Partial derivative of z with respect to x |
| x_dot, z_dot | Time derivatives dx/dt, dz/dt |
| S_ddot | Second time derivative d^2S/dt^2 |
| G_n | n-th order loop gain |
| Impact(S1 -> S2) | (df/dS1) * (S1_dot / S2_dot) |
| RelativeLinkVariance(x -> y) | max(\|RelativeLinkScore\|) - min(\|RelativeLinkScore\|) over simulation |
| confidence | \|r - \|b\|\| / (r + \|b\|) -- polarity confidence metric |

---

## 18. Terminology

| Term | Definition |
|------|-----------|
| **Link score** | Dimensionless measure of contribution and polarity of a link at a point in time |
| **Loop score** | Product of all link scores in a feedback loop; measures the loop's contribution to model behavior |
| **Relative loop score** | Loop score normalized by sum of absolute loop scores; range [-1, 1] |
| **Path score** | Product of link scores along a path; equals the link score that would exist if the path were a single direct link |
| **Partial change** | Change in z that would occur if only x changed (ceteris paribus) |
| **Cycle partition** | Subset of model where all stocks are connected by feedback loops |
| **Dominant loop** | Loop (or set) contributing >= 50% of change across all stocks |
| **Structural polarity** | Polarity determined from model structure (number of negative links) |
| **Behavioral polarity** | Polarity determined from curvature of behavior (used by PPM/Impact) |
| **Composite link score** | Path score of the dominant (largest magnitude) pathway through a macro at each timestep |
| **Composite relative loop score** | Sum of relative loop scores of all full loops mapping to a simplified loop |
| **Link inclusion threshold** | [0, 1+] parameter filtering variables by maximum relative link variance of incoming links |
| **Loop inclusion threshold** | [0, 1] parameter filtering loops by average magnitude of relative loop score |
| **Relative link variance** | max - min of the absolute relative link score over the simulation; measures link dynamism |
| **Polarity confidence** | \|r - \|b\|\| / (r + \|b\|); measures whether a simplified link has consistent polarity |
| **Flow inclusion toggle** | Boolean controlling whether flows are automatically kept when a stock is included |
| **Strongest path algorithm** | Dijkstra-like heuristic for finding high-scoring feedback loops |
| **Composite feedback structure** | Network with one score per link aggregated over all timesteps (rejected for discovery) |
| **Perfect mixing approximation** | Treatment of discrete elements as if their eventual response were instantaneous |
| **Loop polarity labels** | Rx (reinforcing), Bx (balancing), Rux (predominantly reinforcing), Bux (predominantly balancing), Ux (unknown) |
| **LOOPSCORE** | Builtin function: specify a loop, get its relative score across the simulation |
| **PATHSCORE** | Builtin function: compute raw path/loop scores during simulation |
| **FSNN** | Feedback System Neural Network: ODE system with neural net derivatives for causal inference |
| **ILS** | Independent Loop Set (Kampmann, 2012) |
| **SILS** | Shortest Independent Loop Set (Oliva, 2004) |
| **EEA** | Eigenvalue Elasticity Analysis |
| **PPM** | Pathway Participation Metric |
| **LoopX** | Web-based LTM visualization tool (prototype, precursor to Stella integration) |
| **Ceteris paribus** | "All other things being equal": varying one input while holding others at previous-timestep values |

---

## 19. Summary of Key Formulas

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

### Path Score

```
PathScore(x -> ... -> z) = product of LS(link_i) along the path
```

### Composite Link Score (for Macros)

```
CompositeLinkScore = max_magnitude(PathScore(pathway_i)) for all internal pathways
```

### Composite Relative Loop Score (for Simplified CLDs)

```
CompositeRelativeLoopScore(SL) = sum_{L maps to SL}(RelativeLoopScore(L))
```

### Polarity Confidence

```
confidence = |r - |b|| / (r + |b|)
```

### Relative Link Variance (for CLD Simplification)

```
RelativeLinkVariance(x -> y) = max(|RelativeLinkScore(x -> y)|)
                               - min(|RelativeLinkScore(x -> y)|)
```

### Continuous-Time Forms

```
LS(x -> z) = (dz/dx) * |x_dot / z_dot|
LS(i -> S) = |di/dt / d^2S/dt^2|
LS(S1 -> S2) = (df/dS1) * |S1_dot / S2_ddot|
```

### Relationship to Impact

```
LS(S1 -> S2) = Impact(S1 -> S2) * |S2_dot / S2_ddot| * Sign(S1_dot) * Sign(S2_dot)
```

### Multi-Stock Loop Score

```
LoopScore = G_n * product_i(|Si_dot / Si_ddot|)
```

where G_n is the n-th order loop gain and the product runs over all stocks in the loop.

---

## References

- Chernobelskiy, R., Cunningham, K., Goodrich, M., Kobourov, S., and Trott, L. (2011).
  "Force-directed Lombardi-style graph drawing." In *Graph Drawing*, Springer, 320--331.
- Chen, R. T. Q., Rubanova, Y., Bettencourt, J., and Duvenaud, D. (2018). "Neural
  ordinary differential equations." *NeurIPS* 31.
- Cybenko, G. (1989). "Approximation by superpositions of a sigmoidal function."
  *Mathematics of Control, Signals, and Systems* 2(4): 303--314.
- Dijkstra, E. W. (1959). "A note on two problems in connexion with graphs." *Numerische
  Mathematik* 1(1): 269--271.
- Eberlein, R. (1989). "Simplification and analysis of system dynamics models." In
  *Computer-Based Management of Complex Systems*, Springer, 251--259.
- Eberlein, R. and Schoenberg, W. (2020). "Finding the loops that matter."
- Ford, D. N. (1999). "A behavioral approach to feedback loop dominance analysis." *System
  Dynamics Review* 15(1): 3--36.
- Forrester, J. W. (1968). "Market growth as influenced by capital investment." *Industrial
  Management Review (MIT)* 9(2): 83--105.
- Forrester, J. W. (1969). *Urban Dynamics.* Cambridge, Mass: MIT Press.
- Forrester, J. W. (1982). "System dynamics: some personal observations." In *Elements of
  the System Dynamics Method*, Productivity Press, 199--226.
- Gansner, E. R. and Hu, Y. (2010). "Efficient, proximity-preserving node overlap
  removal." *Journal of Graph Algorithms and Applications* 14(1): 53--74.
- Goncalves, P. (2009). "Behavior modes, pathways and overall trajectories." *System
  Dynamics Review* 25(2): 163--195.
- Granger, C. W. J. (1969). "Investigating causal relations by econometric models and
  cross-spectral methods." *Econometrica* 37(3): 424--438.
- Guneralp, B. (2006). "Towards coherent loop dominance analysis." *System Dynamics
  Review* 22(3): 263--289.
- Hayward, J. and Boswell, G. P. (2014). "Model behaviour and the concept of loop
  impact." *System Dynamics Review* 30(1-2): 29--57.
- Hayward, J. and Roach, P. A. (2017). "Newton's laws as an interpretive framework in
  system dynamics." *System Dynamics Review* 33(3-4): 183--218.
- Huang, J., Howley, E., and Duggan, J. (2012). "Observations on the shortest independent
  loop set algorithm." *System Dynamics Review* 28(3): 276--280.
- Kamada, T. and Kawai, S. (1989). "An algorithm for drawing general undirected graphs."
  *Information Processing Letters* 31(1): 7--15.
- Kampmann, C. E. (2012). "Feedback loop gains and system behaviour (1996)." *System
  Dynamics Review* 28(4): 370--395.
- Kampmann, C. E. and Oliva, R. (2009). "Structural dominance analysis and theory
  building in system dynamics." *Systems Research and Behavioral Science* 26(4): 505--519.
- Mass, N. J. (1975). *Economic Cycles: An Analysis of Underlying Causes.* Cambridge,
  Massachusetts.
- Meadows, D. H., Randers, J., and Meadows, D. L. (2004). *The Limits to Growth: The
  30-Year Update.*
- Mojtahedzadeh, M., Andersen, D., and Richardson, G. (2004). "Using DIGEST to implement
  the pathway participation method." *System Dynamics Review* 20(1): 1--20.
- Naumov, S. and Oliva, R. (2018). "Refinements to eigenvalue-based loop dominance
  analysis." In *Feedback Economics*, Springer, 93--118.
- Oliva, R. (2004). "Model structure analysis through graph theory: partition heuristics
  and feedback structure decomposition." *System Dynamics Review* 20(4): 313--336.
- Oliva, R. (2016). "Structural dominance analysis of large and stochastic models."
  *System Dynamics Review* 32(1): 26--51.
- Oliva, R. and Sterman, J. D. (2001). "Cutting corners and working overtime: quality
  erosion in the service industry." *Management Science* 47(7): 894--914.
- Powers, B. (2019). sd.js: System dynamics engine in JavaScript. Open source.
- Richardson, G. P. (1986). "Problems with causal-loop diagrams." *System Dynamics
  Review* 2(2): 158--170.
- Richardson, G. P. (1995). "Loop polarity, loop dominance, and the concept of dominant
  polarity." *System Dynamics Review* 11(1): 67--88.
- Saleh, M. (2002). "The characterization of model significance: a systems dynamics
  perspective." PhD Thesis, MIT.
- Saleh, M., Oliva, R., Kampmann, C. E., and Davidsen, P. I. (2010). "A comprehensive
  analytical approach for policy analysis of system dynamics models." *European Journal of
  Operational Research* 203(3): 673--683.
- Saysel, A. K. and Barlas, Y. (2006). "Model simplification and validation with indirect
  structure validity tests." *System Dynamics Review* 22(3): 241--262.
- Schoenberg, W. (2009). "The Forio model explorer." SD Conference.
- Schoenberg, W. (2020). "LoopX: Visualizing and understanding the origins of dynamic
  model behavior." arXiv:1909.01138.
- Schoenberg, W. (2020). "Feedback system neural networks for inferring causality in
  directed cyclic graphs." arXiv:1908.10336.
- Schoenberg, W. (2020). "Loops that Matter." PhD Thesis, University of Bergen.
- Schoenberg, W. and Eberlein, R. (2020). "Seamlessly integrating loops that matter into
  model development and analysis." arXiv:2005.14545.
- Schoenberg, W., Davidsen, P., and Eberlein, R. (2020). "Understanding model behavior
  using the loops that matter method." *System Dynamics Review* 36(2): 158--190.
- Schoenberg, W., Hayward, J., and Eberlein, R. (2023). "Improving loops that matter."
  *System Dynamics Review* 39(2): 140--151.
- Schoenenberger, L. K., Schmid, A., and Tanase, R. (2015). "Structural analysis and
  archetypes in system dynamics." In *Proceedings of the 33rd International Conference of
  the System Dynamics Society.*
- Sterman, J. D. (2000). *Business Dynamics: Systems Thinking and Modeling for a Complex
  World.* McGraw-Hill.
- Sugihara, G., May, R., Ye, H., Hsieh, C., Deyle, E., Fogarty, M., and Munch, S.
  (2012). "Detecting causality in complex ecosystems." *Science* 338(6106): 496--500.
- Tarjan, R. (1973). "Enumeration of the elementary circuits of a directed graph." *SIAM
  Journal on Computing* 2(3): 211--216.
