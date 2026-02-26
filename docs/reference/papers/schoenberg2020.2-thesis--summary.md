# Detailed Summary: "Loops that Matter"

**Author:** William Schoenberg
**Type:** PhD Thesis, University of Bergen, Department of Geography
**Year:** 2020
**ISBN:** 9788230850848 (print), 9788230866122 (PDF)
**Supervisors:** Pal Davidsen (primary, University of Bergen), Robert Eberlein (co-supervisor, isee systems), Birgit Kopainsky (co-supervisor, University of Bergen)

---

## Thesis Overview and Structure

This thesis is an article-based (Scandinavian-style) PhD dissertation that presents a body of work centered on the **Loops that Matter (LTM)** method for understanding structural dominance in system dynamics models. The thesis consists of a framing narrative (introduction, scientific environment, objectives, article summaries, and discussion) followed by five appended research articles:

1. **Article #1:** "Understanding Model Behavior Using the Loops that Matter" (Schoenberg, Davidsen, Eberlein) -- The core LTM method paper (System Dynamics Review, accepted for publication)
2. **Article #2:** "LoopX: Visualizing and Understanding the Origins of Dynamic Model Behavior" (Schoenberg) -- A web-based visualization tool (arXiv:1909.01138)
3. **Article #3:** "Feedback System Neural Networks for Inferring Causality in Directed Cyclic Graphs" (Schoenberg) -- Applying LTM to data-driven causal inference (arXiv:1908.10336)
4. **Article #4:** "Finding the Loops that Matter" (Eberlein and Schoenberg) -- A heuristic algorithm for discovering important loops in large models (arXiv:2006.08425)
5. **Article #5:** "Seamlessly Integrating Loops That Matter into Model Development and Analysis" (Schoenberg and Eberlein) -- Production implementation in Stella (arXiv:2005.14545)

Article #1 is already summarized in detail in [schoenberg2020-loops-that-matter--summary.md](schoenberg2020-loops-that-matter--summary.md). Article #4 is summarized in [eberlein2020-finding-the-loops-that-matter--summary.md](eberlein2020-finding-the-loops-that-matter--summary.md). Article #5 is summarized in [schoenberg2020.1-seamlessly-integrating-ltm--summary.md](schoenberg2020.1-seamlessly-integrating-ltm--summary.md). This document focuses on the thesis framing chapters and provides detailed summaries of Articles #2 and #3 (which have no standalone summaries), while also capturing the thesis-level synthesis of all five articles.

---

## 1. Introduction and Motivation

### 1.1 The Central Problem

The thesis addresses what Sterman (2000, chapter 21 of *Business Dynamics*) identified as key challenges for the future of system dynamics:

1. **"Automated identification of dominant loops and feedback structure"** -- Automatically determine which feedback loops drive model behavior
2. **"Visualization of model behavior"** -- Animate structural diagrams to show dynamic behavior
3. **"Linking behavior to generative structure"** -- Create tools that connect observed behavior back to the causal structure generating it

Prior to this work, loop dominance analysis methods existed (EEA, PPM, Loop Impact) but none had been widely adopted by practitioners. Ford (1999, p.4-5) stated the requirements clearly: *"To rigorously analyze loop dominance in all but small and simple models and effectively apply analysis results, system dynamicists need at least two things: (1) automated analysis tools applicable to models with many loops and (2) a clear and unambiguous understanding of loop dominance and how it impacts system behavior."* The thesis argues that existing methods failed both tests because:

- They require the practitioner to do significant work beyond normal modeling
- They require specialized knowledge (eigenvalue analysis, linearization)
- They are hard to integrate into existing workflows
- Results are difficult to interpret and communicate

### 1.2 Research Questions

The thesis formally states two research questions (Section 1.2):

1. **"How can the origins of behavior be algorithmically discovered in any time dependent mathematical model, discrete or continuous, linear or non-linear, feedback rich or not?"** -- Addressed by the LTM method (Article #1), the strongest path algorithm (Article #4), and the FSNN approach (Article #3)
2. **"How can the origins of behavior in any mathematical system be visualized, animated and simplified such that practitioners can most easily understand the relationships between the causal mathematical structure of models and observed behavior?"** -- Addressed by LoopX (Article #2) and the Stella integration (Article #5)

A recurring theme is the emphasis on **practicality over theoretical novelty**: the thesis explicitly acknowledges that LTM may not produce fundamentally new insights compared to prior methods, but that making loop dominance analysis accessible to a wide audience of practitioners is itself a significant contribution.

---

## 2. Scientific Environment

### 2.1 System Dynamics Foundations

The thesis is grounded in the system dynamics tradition originating with Jay Forrester at MIT. Key concepts reviewed:

- **Stocks and flows** as the fundamental building blocks of dynamic systems
- **Feedback loops** (reinforcing and balancing) as the structural drivers of behavior
- The distinction between **endogenous** behavior (generated by feedback structure) and **exogenous** behavior (driven by external inputs)
- The seven basic behavior modes in SD: exponential growth, goal seeking, oscillation, S-shaped growth, S-shaped growth with overshoot, overshoot and collapse, and overshoot and oscillation
- **Causal Loop Diagrams (CLDs)** vs. **Stock-and-Flow Diagrams (SFDs)** as complementary model representations

### 2.2 Prior Work on Loop Dominance Analysis

The thesis provides a comprehensive literature review covering four families of approaches:

**Eigenvalue Elasticity Analysis (EEA):**
- Originated with Forrester (1982), formalized by Kampmann (2012), refined by Saleh et al. (2002, 2006, 2010), Oliva (2004, 2016), Naumov and Oliva (2018)
- Built on the observation that a linear system's behavior can be expressed as a weighted combination of behavior modes, each characterized by a decoupled (or pairwise coupled) set of eigenvalues
- EEA examines both link and loop significance with regard to the dynamic behavior of the model by identifying the relationship, expressed in the form of elasticity, between the parameters that make up the gains of individual feedback loops and the eigenvalues (and sometimes eigenvectors) that characterize dynamic behavior
- The significance of a loop is expressed by the eigenvalue elasticity of its gain: how strongly a change in loop gain impacts the eigenvalue associated with the behavior of interest
- Uses Independent Loop Set (ILS, Kampmann 2012) or Shortest Independent Loop Set (SILS, Oliva 2004) to manage combinatorial complexity: SILS composed only of geodetic (shortest) loops which are both unique and easily discoverable
- Can identify leverage points for policy intervention (where structural changes most affect behavior)
- **Strengths:** Most comprehensive structural analysis; can analyze equilibrium states; identifies distinct behavior modes (growth, oscillation, etc.); can identify leverage points for controlling system behavior
- **Weaknesses:** Requires model linearization; limited to continuously differentiable systems; requires specialized knowledge; current tools modify model equations (e.g., changing a discrete integer-only variable from 2 to 2.1) which introduces logical errors and affects validity

**Pathway Participation Metric (PPM):**
- Mojtahedzadeh et al. (2004)
- Starting point is the behavior of a single variable, typically a stock
- Partitions the stock's behavior into phases where slope and convexity are maintained (i.e., first and second time derivatives do not change sign), characterized by 7 behavior patterns (Mojtahedzadeh et al., 2004)
- Determines dominance by tracing causal pathways between stocks and their ancestor stocks, measuring the magnitude of the change in the net flow of the stock under study by making minute changes to that stock, then comparing these changes to determine which pathway of the largest magnitude explains the observed behavior pattern during that phase
- Relationship to LTM: PPM considers the link from a flow to a stock in the same manner as algebraic links -- it measures the change in stock value relative to the change in value of the associated flow (the second derivative). LTM's Equation 2 instead uses the flow value relative to the change in the stock (the first derivative), which allows a single number to represent a whole loop's contribution.
- **Strengths:** No model modification required; works with discontinuous models; more directly applicable; converges on a unique piece of structure as the one most influential (Mojtahedzadeh, 1996)
- **Weaknesses:** Focused on single stocks, not whole-model behavior; criticized for inability to clearly explain oscillatory behavior (Kampmann and Oliva, 2009); may fail to identify structure when two pathways have similar importance (Kampmann and Oliva, 2009)

**Loop Impact Method:**
- Hayward and Boswell (2014), simplified from PPM
- Can be implemented in standard SD software by adding equations to the model (no engine changes required)
- Key difference from PPM: does not identify dominant pathways (impacts from one stock to another); instead focuses on the direct impact one stock has on another stock to identify which loops dominate the behavior of the selected stock
- Pathways are chained together according to model structure to yield Loop Impact metrics identifying which loop dominates the selected stock
- Also identifies instances where multiple loops are required to explain behavior
- Hayward and Roach (2017) developed a Newtonian physics framework around Loop Impact, explaining the model as a series of interacting forces
- **Strengths:** Implementable without engine modification; more intuitive framing
- **Weaknesses:** Still stock-specific rather than model-wide; like PPM, treats integration links using second derivative rather than first

**Ford's Behavioral Approach:**
- Ford (1999) -- qualitative approach based on practitioner intuition
- Identifies behavioral phases by examining stock behavior
- **Strengths:** Intuitive; no specialized tools needed
- **Weaknesses:** Subjective; doesn't scale; no quantitative rigor

### 2.3 Model Simplification and Aggregation

The thesis reviews prior work on simplifying complex models for understanding:

- **Richardson (1986):** Cautions about information loss when aggregating CLDs
- **Eberlein (1989):** Model simplification using linearization
- **Saysel and Barlas (2006):** Aggregation methods
- **Schoenenberger et al. (2015, 2017):** Variety filters, structural model partitioning, ADAS method (algorithmic detection of archetypal structures)
- **ILS/SILS (Kampmann 2012, Oliva 2004):** Reduce number of loops to analyze, but may miss important non-independent loops
- **Forio Model Explorer (Schoenberg, 2009):** Early attempt at force-directed graph visualization of models; tests comparing these force-directed graphs to hand-drawn CLDs were inconclusive, "showing no reported differences in learning outcomes, but diagrams generated were of significantly less quality" than expert hand-drawn CLDs

### 2.4 Visualization and Graph Drawing

The thesis reviews graph visualization techniques relevant to CLD generation:

- **Force-directed layouts:** Kamada and Kawai (1989), and variants; produce layouts where connected nodes are near each other
- **Lombardi-style diagrams:** Chernobelskiy et al. (2011) -- aesthetically pleasing circular-arc edges meeting at vertices with equi-angular spacing, but fundamentally lack concepts of directed edges and cycles that SD requires; designed for undirected acyclic graphs
- **Graphviz/neato:** Standard force-directed graph tools; modified by Schoenberg to support curved edges that emphasize feedback loops
- **Key challenge:** Standard graph drawing algorithms don't account for directed cycles (feedback loops), which are central to SD understanding

---

## 3. The Loops That Matter Method (Summary)

This section of the thesis synthesizes the LTM method as presented across Articles #1 and #5. The full technical details are in [schoenberg2020-loops-that-matter--summary.md](schoenberg2020-loops-that-matter--summary.md). Key points reiterated in the thesis framing:

### 3.1 Core Metrics

**Link Score (Equation 1):** For a dependent variable `z = f(x, y)`, the link score for link `x -> z`:

```
                | delta_xz |
LS(x -> z)  =  | -------- | * sign(delta_xz / delta_x)       if delta_z != 0 AND delta_x != 0
                | delta_z  |

             =  0                                              if delta_z = 0 OR delta_x = 0
```

Where `delta_xz` (the "partial change") is the amount z would have changed if only x had changed (ceteris paribus): `f(x_current, y_previous) - z_previous`. The link score magnitude is not a partial derivative (sensitivity) but rather the **fraction of actual change** in z attributable to x. This distinction matters: a partial derivative measures how much z *would* change per unit change in x, while the link score measures how much of z's *actual* change was caused by x's actual change.

The ceteris paribus computation requires re-evaluating `z = f(...)` once per input variable, holding all other inputs at their previous-timestep values. This is the primary computational cost of LTM: for a variable with k inputs, k additional equation evaluations are needed per timestep.

**Link Score for Integration (Equation 2):** For stock `s = integral(i - o)`:

```
Inflow:   LS(i -> s) = |i / (i - o)| * (+1)
Outflow:  LS(o -> s) = |o / (i - o)| * (-1)
```

This is a critical design choice that distinguishes LTM from PPM and Loop Impact. PPM and Loop Impact treat the integration link the same as algebraic links -- they measure the change in stock value relative to the change in the flow value (the second derivative of the stock). LTM instead uses the flow value relative to the change in the stock (the first derivative), which makes link scores along a flow-to-stock-to-flow pathway reduce to a single number representing the whole loop's contribution. This is what enables LTM to be a **whole-model metric** rather than a stock-specific one.

**Loop Score (Equation 3):** Product of all link scores in a loop:

```
Loop Score(L_x) = LS(s1 -> t1) * LS(s2 -> t2) * ... * LS(sn -> tn)
```

**Relative Loop Score (Equation 4):** Normalized to [-1, 1]:

```
Relative Loop Score(L_X) = Loop Score(L_X) / sum_Y(|Loop Score(L_Y)|)
```

Where the sum is over all loops Y in the same **cycle partition** as loop X. A cycle partition is a subset of the model where all stocks are connected by at least one feedback loop. For models where some stocks do not share any feedback loops, each disconnected subcomponent of inter-related feedback loops is considered individually. Loops are only compared against other loops within the same cycle partition.

### 3.2 Key Properties

- **Computationally simple:** Uses only values computed during normal simulation; roughly doubles equation evaluations. No model linearization, no eigenvalue computation, no specialized mathematics.
- **No model modification:** Works on original equations directly. Contrast with EEA which requires perturbing parameter values and may introduce logical errors in discontinuous models.
- **Generally applicable:** Works on discrete, discontinuous, and agent-based models. EEA requires continuously differentiable equations; LTM has no such restriction because it uses actual computed values rather than derivatives.
- **Whole-model metric:** Loop scores describe behavior of all stocks in a connected cycle partition, not just individual stocks. This is a direct consequence of the integration link score design (Equation 2).
- **Chain rule property:** Link scores can be multiplied along paths, and the result is invariant under model reformulation. Proved in Appendix II of Article #1: for a chain `x -> y -> z` where `y = f(x)` and `z = g(y)`, the product `LS(x -> y) * LS(y -> z)` equals the link score `LS(x -> z)` that would be computed if `y` were eliminated and `z = g(f(x))` were computed directly. This ensures that structurally equivalent models (differing only in how many intermediate auxiliary variables they use) produce identical loop scores.
- **Isolated loop always scores +/-1:** A single loop with no competition always has relative loop score magnitude 1 regardless of the loop's gain. This is because the loop score measures the *fraction* of behavior attributable to that loop, and with no other loops, 100% of behavior is attributable to the single loop.
- **Constants and parameters:** Variables whose values do not change have link scores of 0 (by definition, delta_x = 0). However, constants are still significant to loop dominance analysis because they condition the link scores of other links: a parameter appearing in an equation affects how much of z's change is attributable to x vs. y, even though the parameter itself is not changing.

### 3.3 Limitations

- **Cannot analyze equilibrium:** When all stocks are unchanging, all net flows are 0, so all integration link scores produce 0/0 = 0. This is a fundamental limitation: the method measures the fraction of *actual change* driven by each loop, and when there is no change, there is nothing to attribute.
- **Focused on endogenous behavior:** Less useful for models dominated by external forcing functions. The method attributes behavior to feedback loops, but exogenous inputs are not part of loops. Models where exogenous drivers dominate will have low total loop scores.
- **Cannot identify behavior modes:** Unlike EEA, does not decompose behavior into distinct modes (exponential growth, oscillation, etc.). LTM reports which loops are dominant but not *what kind of behavior* they are generating. A practitioner must infer the behavior mode from the combination of loop polarities and time-varying dominance patterns.
- **Reports only on observed behavior:** LTM only analyzes the behavior that actually occurs during a specific simulation run with specific parameter values. Alternative scenarios, counterfactuals, and sensitivity to parameters are not covered by a single LTM analysis.

### 3.4 Relationship Between LTM and Other Methods

The thesis framing provides an explicit comparison of LTM against each prior method:

| Property | EEA | PPM | Loop Impact | LTM |
|----------|-----|-----|-------------|-----|
| Scope | Whole model | Single stock | Single stock | Whole model (cycle partition) |
| Integration link treatment | Second derivative | Second derivative | Second derivative | First derivative (Eq. 2) |
| Works at equilibrium? | Yes | No | No | No |
| Identifies behavior modes? | Yes | Partial (7 patterns) | No | No |
| Requires linearization? | Yes | No | No | No |
| Works on discontinuous models? | No | Yes | Yes | Yes |
| Requires model modification? | Yes (perturbation) | No | Yes (added equations) | No |
| Computational complexity | High | Moderate | Low | Low-moderate |
| Identifies leverage points? | Yes | No | No | No |
| Tool availability | Specialized | Specialized | In-model | Stella 2.0 checkbox |

LTM sacrifices equilibrium analysis and behavior mode identification in exchange for generality (works on any model type), simplicity (no specialized math), and accessibility (single checkbox in production software).

---

## 4. Article #2: LoopX -- Visualizing and Understanding the Origins of Dynamic Model Behavior

**Author:** William Schoenberg
**Status:** arXiv preprint (arXiv:1909.01138)

### 4.1 Overview and Goals

LoopX is a **web-based software tool** that addresses three challenges identified by Sterman (2000):

1. **How can high-quality CLDs be machine-generated** from the network of model interconnections?
2. **How can models be aggregated and simplified** without losing information important to model understanding while retaining relative simplicity?
3. **How can the results of an LTM analysis be easily visualized and communicated?**

### 4.2 Software Architecture (Figure 1)

LoopX takes an XMILE model file as input and produces two types of animated output:

```
XMILE Model File
    |
    +--> Model Equations --> sd.js Simulation Engine --> LTM Metrics --> Animated SFD
    |                        (with integrated LTM)        |
    +--> Variables/Connectors --> Graphviz neato --> CLD Layout --> Animated Simplified CLD
                                                                    (via LTM Metrics)
```

Components:
- **sd.js engine** (Powers, 2019): Modified to compute LTM metrics during simulation
- **Graphviz neato**: Force-directed graph layout, modified for curved edges
- **Web frontend**: Renders animated diagrams with user controls

### 4.3 Challenge 1: Machine-Generated CLDs

**The edge curving problem:** Standard force-directed graph algorithms (including neato) produce either straight edges or curves that don't emphasize feedback loops. Lombardi-style diagrams produce nice curves but don't handle directed cycles.

**Solution -- the edge curving heuristic:** A new algorithm contributed to the public neato/Graphviz codebase. The heuristic works on an edge-by-edge basis after node positions have been determined by the force-directed layout:

1. For each edge, find the **shortest feedback loop (length > 2)** that the edge is a member of (using the model's dependency structure)
2. Compute the **average center** (centroid) of all nodes in that shortest loop
3. The edge is drawn as a **circular arc** whose center of curvature is that average center
4. **Two-node exception:** Loops of length 2 (two nodes connected by edges in both directions) are handled separately, producing paired directed edges rendered as elongated ellipse structures to avoid overlapping

The key insight is that force-directed layout algorithms (like Kamada-Kawai) already place connected nodes near each other. By curving each edge toward the center of its shortest loop, the visual result is that **feedback loops appear as recognizable circular structures** in the 2D layout. This directly emphasizes the feedback structure that is central to SD understanding, addressing a gap where standard graph drawing algorithms produce layouts that obscure the circular nature of feedback.

**Initialization with SFD positions:** To avoid degenerate layouts, neato uses the position of each variable in the stock-and-flow diagram as its initial position. This preserves local clusters from the original SFD (assumes a well-laid-out SFD keeps related variables near each other).

**Neato configuration for quality CLDs:**
1. `overlap = 'prism'` -- Prism algorithm (Gansner & Hu, 2010) to remove overlapping variable names with minimal layout disturbance
2. `mode = 'KK'` -- Kamada-Kawai gradient descent for node placement
3. `model = 'shortpath'` -- Shortest path between node pairs as ideal spring length
4. `splines = 'curved'` -- Invoke the new edge curving algorithm

### 4.4 Challenge 2: Model Aggregation and Simplification

LoopX introduces **two parameters** for controlling CLD simplification:

**Parameter 1: Link Inclusion Threshold (range [0, 1])**

Purpose: Filter out auxiliary variables whose connections don't change much over the simulation.

Based on **Relative Link Variance** (Equation 5):

```
Relative Link Variance(x -> y) = max(abs(Relative Link Score(x -> y)))
                                 - min(abs(Relative Link Score(x -> y)))
```

Where the Relative Link Score is the link score normalized across all determinants of the dependent variable y. This measures the change in the percentage contribution of x to y over the full simulation.

- Only variables with at least one incoming link whose relative link variance >= threshold are included
- If a stock is included, its flows are automatically included (stocks require flows to change)
- Variables with 0 variance are constants or linear pass-throughs -- good candidates for elimination
- Variables with high variance point to sources of non-linearity -- important for understanding

**Parameter 2: Loop Inclusion Threshold (range [0, 1])**

Purpose: Filter out feedback loops (and their associated stocks/flows) that have low average contribution.

- Represents the average magnitude of the relative loop score across the simulation period
- Only loops with average magnitude >= threshold have their stocks and flows automatically included
- Delayed averaging: starts from the first instant the loop becomes active (avoids penalizing loops during initialization)
- A threshold of 0.01 means the loop contributes only 1% of total behavior on average

**Generating simplified links:** After determining which variables to keep, the simplified CLD needs to establish which links exist between the remaining variables. This is non-trivial because the original model may have multi-step pathways between two kept variables:

- A **depth-first search** tests each candidate simplified link by traversing the full disaggregated model's dependency structure
- For each pair of kept variables (source, target), the search verifies that a causal pathway exists from source to target in the full model that **does not pass through any other variable that is also in the simplified set**
- If such a pathway exists, a simplified link is created between the source and target
- All kept simplified links are valid in both the aggregated and disaggregated model
- The link score for a simplified link is the **product** of link scores along the pathway (chain rule property), which preserves consistency across aggregation levels
- **Limitation:** The DFS only finds the *first* valid pathway, which may not be the most important one. If multiple distinct causal pathways connect two kept variables (through different intermediate variables), only one is represented in the simplified link. This is one of the key problems identified for future improvement.

### 4.5 Application: Forrester's 1968 Market Growth Model

The Market Growth model (Forrester, 1968) with all macros expanded contains:
- 10 stocks, 48 variables, 23 feedback loops (including macro-internal loops)

**Note on loop count:** Article #2 reports 23 loops for Market Growth with macros expanded. Article #4 reports 19 loops for the same model (Morecroft 1983 replication). The difference of 4 loops is due to macro expansion: DELAY and SMOOTH macros contain internal feedback loops that are counted when macros are expanded (Article #2) but hidden when macros are treated as atomic units (Article #4, and the Article #5 production implementation which trims macro-internal loops from reported results).

LoopX generates progressively simplified CLDs:

| Thresholds | Variables | Description |
|-----------|-----------|-------------|
| Link 0%, Loop 0% | All (48) | Full CLD, all feedback complexity |
| Link 100%, Loop 0% | ~22 | Less than half the variables; all 23 loops represented; 10 stocks, 12 flows, 2 auxiliaries |
| Link 100%, Loop 20% | ~17 | 7 stocks, 10 flows, 2 auxiliaries |
| Link 100%, Loop 100% | 4 | Maximally simplified: 2 flows, 2 auxiliaries; captures the single most dominant loop |

The tradeoff across these is **descriptive power vs. ease of cognition**, best decided case-by-case.

### 4.6 Challenge 3: Visualization of LTM Results

**Animated diagrams:** In both SFDs and CLDs:
- **Color** represents link polarity (red = negative/balancing, green = positive/reinforcing)
- **Thickness** represents magnitude of the relative link score
- For simplified CLDs, relative link scores along multi-step pathways are **multiplied** to get the simplified link's score

**Split flow rendering in SFDs:** In standard SFD notation, a flow between two stocks has an implicit dual role: it is an outflow from one stock and an inflow to another. From an LTM perspective, the flow-to-stock link has a different score (and potentially different polarity) for each stock. LoopX renders biflows (flows connecting two stocks) **split in half**: the pipe section before the valve is colored/sized based on its contribution to the source stock, the section after the valve shows its contribution to the destination stock. This makes the **hidden information links** in SFDs visible -- in particular, the outflow relationship from a stock to its outgoing flow, which has no arrowhead in standard SFD notation but is a real causal link that participates in feedback loops. This is important pedagogically: novice modelers often miss that a stock's outflow represents a feedback connection from the stock to the rest of the system.

**Loop table:** Shows instantaneous contribution and average contribution for each loop, sorted by importance. Loop identifiers can be pressed to highlight all variables and links in that loop across all diagrams.

**Animation timeline:** Users can scrub through simulation time, pin specific time points, and adjust thresholds live.

**Data export:** All LTM metrics available as CSV for external analysis.

### 4.7 Discussion and Limitations

**CLD generation quality:**
- Curved edges appear naturally drawn, following Richardson (1986) best practices for CLD aesthetics
- Generated CLDs minimize non-feedback-looking structures while keeping short loops close in 2D space
- The quality is substantially improved over Schoenberg (2009)'s Forio Model Explorer, which produced diagrams "of significantly less quality" than expert hand-drawn CLDs. The edge curving heuristic was the key advance that made machine-generated CLDs visually competitive.
- CLD generation is independent of LTM -- the edge curving technique and force-directed layout work for any directed network data, not just SD models

**Simplified CLD quality:**
- Link score multiplication ensures consistency across aggregation levels (the chain rule property)
- Information loss is controlled by the user via threshold parameters
- Loop inclusion threshold can cause loss of stock representation if those stocks only appear in filtered loops
- Link inclusion threshold rarely causes loss of feedback loops (because flow-to-stock links have high variance near equilibrium transitions)
- The link inclusion threshold acts as a "surgical scalpel" -- it removes variables that exist for equation simplification rather than dynamic complexity

**Problems with simplified CLDs:**
- No quality indicator for the generated diagram (user must manually check which important loops are represented)
- Simplified links may represent multiple causal pathways; the simplification method only chooses the first valid one found, which may not be the most important

**Link thickness animation choices (detailed analysis of four alternatives):**

1. **Raw link scores:** Would explode to infinity near equilibrium transitions. When a stock approaches equilibrium, its net flow approaches 0, and the integration link score (Equation 2) has (i - o) in the denominator, causing the score to grow without bound. This makes raw scores unsuitable for direct visual representation.

2. **Relative link magnitude** (normalized across all inputs of a dependent variable): This is what was chosen. The relative link score divides the link score by the sum of all link scores for that dependent variable, yielding a fraction in [0, 1]. However, this normalization is per-variable, so loops are not directly identifiable from thickness alone -- a thick link only means it dominates *its particular variable's* inputs, not that it is part of a dominant loop.

3. **Loop-score-based thickness:** Each link would be colored/sized based on the loop scores of the loops it participates in. Rejected because in models where important loops share many links, representing a link's membership in multiple loops simultaneously doesn't scale visually. A link could be in 5 important loops of different polarities, and there is no clear way to render this.

4. **Global normalization** (across all links at a time step, or across all time steps): Both have problems. Normalizing across a time step means equilibrium periods (when most scores are near 0 but their ratios blow up) dominate the visual. Normalizing across all time steps means that when different parts of the model reach equilibrium at different times, the still-active parts appear increasingly bright relative to the quiescent parts, creating misleading impressions of relative importance.

**Use cases for simplified CLDs:**
1. Education (accurate but simpler depictions)
2. Model construction (verify understanding of key dynamics)
3. Policy maker communication (overviews of key structure)
4. Pre-processing for ADAS archetype detection (reduced search space)

### 4.8 Scalability Concerns

LoopX works well for models with up to ~50 variables and <50 feedback loops. For larger models (like T-21), significant re-engineering is needed for the simplified link finding process.

---

## 5. Article #3: Feedback System Neural Networks (FSNN) for Inferring Causality in Directed Cyclic Graphs

**Author:** William Schoenberg
**Status:** arXiv preprint (arXiv:1908.10336)

### 5.1 Motivation and Problem Statement

This article extends LTM from analyzing *known* system dynamics models to **inferring causal structure from observational data**. The key insight: if the LTM link score can measure the contribution of one variable to another in a known model, can we construct a model from data and then use link scores to identify which causal relationships are real?

**The gap in existing causal inference:**
- **Granger causality** (Granger, 1969): Tests whether past values of one time series improve prediction of another. Can identify specific causal links in simple systems, but fails to generalize to complex non-linear feedback systems where multiple variables simultaneously influence each other. Also assumes stationarity.
- **Causal network learning algorithms** (PC algorithm, Bayesian network learning, etc.): Start from an empty or fully connected network and iteratively test/score edges. Fundamental limitation: they assume **directed acyclic graphs (DAGs)**, which by definition cannot represent feedback. Any system with feedback loops -- the core subject of system dynamics -- is outside their representational capacity.
- **Convergent Cross-Mapping** (Sugihara et al., 2012): Tests for causation by measuring whether the state of one variable can be recovered from the manifold of another. Works for coupled dynamical systems but requires very long time series and struggles with noise.
- **Machine learning** (deep learning, random forests, etc.): Excel at prediction but produce "black box" models that fail to reveal causal mechanisms. The trained weights do not correspond to interpretable causal relationships.
- **Neural ODEs** (Chen et al., 2018): Learn dynamics as continuous-time neural networks, but the state variables are part of a hidden (latent) network, making them uninterpretable. You can predict behavior but cannot extract causal structure.

The fundamental gap: no existing method can infer **directed cyclic causal graphs** (systems with feedback loops) from observational data while also revealing the causal structure in an interpretable way.

**The FSNN proposition:** Build a system of ODEs where each state variable's derivative is a function of all other state variables (via neural networks), train it on observational data, then analyze the trained model using LTM link scores to identify which causal connections actually matter.

### 5.2 The FSNN Method (Four Steps)

**Step 1: Construct the system of ODEs**

- Enumerate the state variables of the ground truth system from observed data
- For each state variable, set its derivative to be a function of every other state variable as well as itself: `dState_i/dt = f_i(State_1, State_2, ..., State_n)`
- Each `f_i` is implemented as a **separate multilayer perceptron (MLP) neural network** with:
  - Input layer: current values of all state variables
  - Hidden layers: individually sized per best practice
  - Single output: the derivative of the target state variable
- A-priori knowledge can be incorporated by removing input nodes for known non-existent relationships (e.g., if a researcher knows that State_3 cannot directly influence State_1, the State_3 input to f_1 is removed)
- **Critical architectural distinction from Neural ODEs (Chen et al., 2018):** In Neural ODEs, the state variables are latent -- they are part of a hidden network and do not correspond to observable quantities. In FSNN, the states are explicitly the **observed real system variables** (e.g., population, temperature, pressure). This is what makes FSNN interpretable: the neural networks learn the *relationships between observable quantities*, not a latent representation. The tradeoff is that FSNN requires knowing which variables are the state variables of the system, while Neural ODEs discover latent states automatically.
- The relationship between state variables is fully general: each `f_i` can learn arbitrary nonlinear functions, including thresholds, saturation, and complex multi-variable interactions

**Step 2: Train/parameterize the model**

- **Payoff function:** Minimize total squared error between training data and calculated state values across all state variables and all time points. Formally: `payoff = -sum_t sum_i (data_i(t) - calc_i(t))^2` (negative because the optimizer maximizes).
- **ODE solver:** Any ODE solver can be used. The example uses **Runge-Kutta 4** with a fixed time step, matching the time step used to generate training data.
- **Optimization algorithm:** Powell's **BOBYQA** (Bound Optimization BY Quadratic Approximation) via DLib 19.7. BOBYQA is derivative-free, which is important because backpropagation through the full ODE integration would be complex. The optimizer adjusts all neural network weights and biases simultaneously.
- **Initialization of weights:** All weights and biases start at **exactly 0**. This is a deliberate choice representing "no causal structure" -- with all-zero weights, every `f_i` outputs 0 (no change), meaning the initial model predicts all states are constant. The optimizer then incrementally builds causal structure as needed to match the data.
- **Multiple training initializations:** Multiple distinct initializations of the ground truth system (different initial conditions for the state variables) should be used simultaneously in the same training run. All initializations contribute to the same payoff function. This is critical for breaking confounders (see Section 5.4).

**Step 3: Analyze the trained model using LTM link scores**

- Simulate the trained model and compute LTM link scores along all **direct pathways** from each state variable through the neural network to the derivative of every other state variable
- For n state variables, this produces an **n x n matrix** of pathway link scores at each time point: entry (i, j) gives the signed percentage of State_j's behavior that is driven by State_i at that instant
- A zero (or near-zero) pathway link score means State_i does not causally influence State_j. A non-zero score means it does, and the sign indicates polarity (reinforcing vs. balancing).
- Unlike full feedback loop analysis (which generates up to factorial numbers of loops), this pathway analysis is manageable: only n^2 directed relationships to examine
- The analysis is done at the **pathway level** (State_i -> f_j -> dState_j/dt), not at the full feedback loop level. Loop-level analysis would be possible but is not needed for causal structure identification -- knowing which directed relationships exist is sufficient.

**Step 4: Validate and interpret**

- Compare causal structure of the generated model against known ground truth (for synthetic data)
- For real-world data: validate through empirical experimentation

### 5.3 Application: Three-State Nonlinear Oscillatory System

**Ground truth system (Table 1):**

```
State_1 = flow_2 - flow_1     (Units)
State_2 = flow_3 - flow_2     (Units)
State_3 = flow_4 - flow_3     (Units)

flow_1 = (g - f(State_3)) / t    (Units/Time)
flow_2 = State_1 / t             (Units/Time)
flow_3 = State_2 / t             (Units/Time)
flow_4 = State_3 / t             (Units/Time)

t = 5          (Time)
g = 75         (Units)
f = nonlinear sigmoid function, x range [0,100], y range [0,100], inflection at (50,50)
```

This produces **4 feedback loops** (all balancing) and exhibits **dampened oscillation**.

**Training setup:**
- Two distinct initializations: (29, 96, 4) and (22, 11, 78)
- 100 time points per initialization, sampled at each time unit
- dt = 1/4, RK4 solver
- Each neural net: 3 hidden layers (8, 6, 4 nodes), tanh activation
- All inputs linearly rescaled to [-1, 1] by dividing by 100 (externally determined maximum magnitude)
- Output multiplied by 100 to recover proper scale

**Results:**

1. **Behavioral match (Figure 1):** The generated model closely reproduces the ground truth behavior across both training initializations

2. **Causal structure identification (Figure 2):** The 3x3 pathway link score matrix shows scores for all 9 possible directed relationships (State_i -> State_j for i,j in {1,2,3}). Analysis correctly identifies:
   - **Which causal links exist** (non-zero link scores) and which don't (zero or near-zero link scores): the ground truth has 5 non-zero relationships (State_1->State_2, State_2->State_1, State_2->State_3, State_3->State_2, State_3->State_1), and the generated model correctly shows these 5 as non-zero while the remaining 4 are zero or negligible
   - **The polarity** of each link (reinforcing vs. balancing): all correctly identified
   - **The magnitude profiles differ:** The generated model is NOT the singular well-defined model of the ground truth. The exact time-varying patterns of link scores differ (the internal structure of the neural networks is different from the ground truth equations), but the high-level causal features are all correct. This illustrates the singularity problem: multiple models can produce correct causal identification while having different internal structures.

3. **Monte Carlo validation (Figure 3):** 100 random initializations (Sobol sequences, sum of initial states constrained to [30, 150]):
   - 95% confidence bounds of prediction error are close to 0
   - 75% confidence bounds are very tight
   - The model is an accurate recreation within the behavioral space of the training data

4. **Failure at boundary (Figure 4):** When initial sum of states exceeds ~150, the maximum prediction error across all states and time points grows exponentially. This is because the training data (where initial sum was 125 and 111) never explored regions where multiple states simultaneously have values > 70. In those unexplored regions, the neural networks are extrapolating beyond the manifold surfaces they learned, and the tanh activation function's saturation behavior produces increasingly inaccurate derivatives. This demonstrates the critical importance of training data coverage: **the generated model is only reliable within the behavioral space explored during training**.

### 5.4 Theoretical Analysis: Challenges with Nonlinear Feedback Systems

**The singularity problem:** The generated model is not the singular well-defined model for the ground truth system. Multiple causally-different models can produce behaviorally-identical outputs within a given range. The thesis calls this the "degenerate payoff surface" problem:

- The neural network is a **universal approximator** (Cybenko, 1989), capable of representing an infinitely wide range of behavior patterns. This means many different weight configurations can produce the same behavioral output.
- After training, the generated model is just one possible explanation -- a hypothesis about causal structure, not a proven fact.
- The distinction between the singular well-defined model and a set of behaviorally-equivalent models is "miniscule to insignificant in the behavioral space, but vastly different in the structural (causal) space." Two models that are indistinguishable by behavior may attribute causation to completely different variables.
- The payoff surface has regions of degeneracy where multiple weight configurations score equally well. The optimizer finds *one* of these configurations, which may or may not reflect the true causal structure.
- **Ultimately, validation of inferred causal structure requires empirical experimentation beyond behavior-matching.** Matching behavior is necessary but not sufficient for establishing causation. This is analogous to the fundamental problem of observational studies in statistics.

**The manifold perspective:** The ground truth system can be understood as an n-sized set of n-dimensional functions (one per state variable's derivative), each producing a manifold in state space. The derivative of State_i is a function of the current values of all state variables, so as the system evolves, it traces out a trajectory on the surface of these n-dimensional manifolds. Training explores these manifold surfaces:

- Each initialization starts at a specific point in state space and traces a trajectory that settles into a specific area of the manifolds (except for chaotic systems). The neural networks learn to approximate the manifold surfaces from these trajectories.
- Adding more samples to one initialization yields **diminishing returns**: the system quickly converges to a trajectory that covers only a limited region of the manifold. More temporal samples along the same trajectory add little new manifold information.
- Adding **new initializations** is far more valuable: each explores a new region of the manifolds from a different starting point, revealing manifold structure that the first initialization never visited. This helps break false learned causations (confounders).
- For non-chaotic systems, three categories at the limit of time: (1) **oscillatory** (standing oscillation -- the trajectory cycles, covering a fixed manifold region), (2) **equilibrium** (steady state -- the trajectory converges to a point), (3) **chaotic** (trajectory never settles, continuously exploring new manifold regions)
- In oscillatory and equilibrium systems, each initialization eventually stops providing new manifold information as the trajectory converges. Chaotic systems are a special case where a single initialization may explore the manifold extensively, but chaotic sensitivity to initial conditions makes optimization difficult.
- **Verification of initialization independence:** Each set of initial states must be verified as "indeed distinct" -- the initial states of one training initialization should not appear as calculated values in any other initialization's trajectory. If they do, the two initializations are exploring the same manifold region and no new information is gained. For real-world systems, this means studying multiple unrelated instances of the system where each instance shares the same causal structure but operates from genuinely different states.

**The confounders problem:** With a single initialization, two state variables may exhibit correlated trajectories (both rising, both falling) due to a third variable's influence rather than direct causation between them. The optimizer may learn a false direct causal link between the correlated variables because this "explains" the observed data. Multiple initializations break these false correlations because:

- In different initializations with different starting conditions, the first and second states may exhibit **opposite** trajectories (one rising while the other falls), even though the third state's influence is the same
- This forces the optimizer to find a model where the causal path goes through the third state (the true cause), not directly between the two confounded states
- Even with just two initializations, the incidence of confounders drops **precipitously** -- the false correlation present in one initialization is contradicted by the other
- The thesis draws an explicit analogy to observational studies in epidemiology: more diverse observations reduce confounding

**The activation function / scaling problem:** The tanh activation function combined with linear rescaling creates a fundamental signal attenuation issue at large values:

- **tanh saturates:** For inputs much greater than 1 or much less than -1, tanh "planes out" to +/-1. After linear rescaling to [-1, 1], a value of 0.7 and a value of 1.0 both produce tanh outputs near 1.0. The system responds similarly to a value of 2x vs x when both are in the saturation region.
- **Impact on causal inference:** The saturation masks real differences between large values. If State_1 has magnitude 80 vs 90 (both mapping to ~0.8 and ~0.9 after rescaling), the neural network may not be able to distinguish these, even though the difference may be causally important.
- **Data distribution matters:** Linear rescaling assumes roughly uniform data distribution. For systems with exponentially distributed data (where most observations cluster near 0 and rare events produce large values), the linear scaling is "fundamentally incompatible" because the rare large values are compressed into the saturated region while the common small values occupy only a small portion of tanh's sensitive range.
- **Logarithmic scaling** may be needed for exponentially distributed data, compressing the range of large values while expanding the range of small values.
- The interaction between activation function, scaling function, and data distribution is identified as an important open research question. Different activation functions (ReLU, sigmoid, etc.) may be better suited to different data distributions, but this has not been studied in the FSNN context.

### 5.5 Requirements for Real-World Application

For the method to work on unknown systems, several challenging requirements must be met:

- **Multiple unrelated instances:** Multiple genuinely independent instances of the system must be studied, each sharing the same causal structure but operating from different initial states. For ecological systems, this might mean studying multiple lakes; for economic systems, multiple firms. The instances must be verified as independent (no shared trajectory regions).
- **Clean data:** Measurement error, missing data, and extraneous time series are all problematic. The FSNN method has no way to distinguish measurement noise from actual causal signals -- noise will be fitted as if it were real structure.
- **Endogenous forcing:** The design of the method fundamentally forces endogenous explanations. Every state variable's derivative is expressed as a function of other state variables. If an extraneous time series is included (one that has no causal relationship to the system), the optimizer will nonetheless find a way to incorporate it into the learned causal structure, because the universal approximation capability of the neural network allows it to fit arbitrary patterns. The extraneous variable's inclusion "warps the payoff surface towards degeneracy" -- many more weight configurations can achieve similar payoff, and many of these configurations involve false causal links to the extraneous variable.
- **Known magnitude range:** The magnitude of state variables must be approximately known for input rescaling. If the range is underestimated, values will saturate in the activation function. If overestimated, values will cluster near 0 where tanh is approximately linear, reducing the neural network's nonlinear capacity.
- **Known state variables:** The method requires knowing which observed quantities are the state variables (stocks) of the system. In SD terms, you must know which variables accumulate over time. This is a non-trivial assumption for real-world systems where the stock structure may not be obvious.
- **Stationarity of causal structure:** All training instances must share the same causal structure. If the system undergoes structural changes (a feedback loop appears or disappears), the method will learn an "average" structure that may not accurately represent any specific configuration.

### 5.6 Conclusions

The FSNN approach represents a significant conceptual advance: applying system dynamics thinking (feedback, stocks, flows, causal structure) to the data-driven machine learning world. The key intellectual contribution is bridging two traditionally separate fields:

- **From SD to ML:** The stock-and-flow formulation provides an interpretable structure for the neural network model. Instead of a black-box prediction system, the FSNN produces a model whose state variables are observable quantities and whose learned relationships can be analyzed using LTM link scores.
- **From ML to SD:** Universal approximation via neural networks removes the need to specify functional forms a priori. Instead of guessing "is this a linear relationship? exponential? logistic?", the neural network learns the appropriate function from data.

The thesis explicitly positions FSNN as complementary to existing causal inference: where Granger causality, PC algorithms, and Bayesian networks assume acyclic causal structures, FSNN is specifically designed for **cyclic** (feedback) systems. This fills a genuine gap in the causal inference landscape.

If the considerable challenges (manifold coverage, confounders, scaling, measurement error, state variable identification) can be addressed, the method has the potential to identify causal structure in complex feedback systems where existing causal inference methods fundamentally fail. The thesis acknowledges this is a research program, not a finished tool -- "considerable further study is warranted."

---

## 6. Article #4: Finding the Loops that Matter (Summary)

**Authors:** Robert Eberlein and William Schoenberg
**Full summary:** [eberlein2020-finding-the-loops-that-matter--summary.md](eberlein2020-finding-the-loops-that-matter--summary.md)

### 6.1 The Problem

For small models, all feedback loops can be enumerated (Tarjan, 1973) and scored. For large models, the number of loops grows super-linearly with the number of stocks (potentially proportional to the factorial of the number of stocks). The Urban Dynamics model has **43,722,744** feedback loops -- exhaustive enumeration and scoring is impractical.

### 6.2 Why Independent Loop Sets Are Insufficient

The ILS (Kampmann, 2012) and SILS (Oliva, 2004) reduce the set of loops to analyze by selecting only topologically independent loops. But:

- This is a static structural analysis that ignores dynamic behavior
- The most important loops at a given point in simulation may be non-independent
- Demonstrated with the three-party arms race model: the two long reinforcing loops (A-to-B-to-C and A-to-C-to-B) are NOT in the SILS set, but dominate long-term behavior (Figure 3 of Article #4)

### 6.3 The Composite Feedback Structure Problem

Different loops are important at different times. A loop that is inactive early may be dominant later (demonstrated with the decoupled two-stock IF/THEN model -- Figure 4 of Article #4). This means loop discovery must account for all links that are active at **any point** during the simulation.

**Composite link score approaches tried and rejected:**
- **Maximum link score magnitude over time:** Always gives 1 for every link near a stock's own flow; biased toward longer loops; scores can exceed 1.0E300
- **Average link score magnitude over time:** Biased toward shorter loops; small equation changes can make major loops score 0

**Solution adopted:** Run the strongest path algorithm at **every (or nearly every) time step**, using instantaneous link scores. This is more passes but each pass converges quickly.

### 6.4 The Strongest Path Algorithm

An adaptation of Dijkstra's shortest path algorithm for **maximization** of multiplicative path scores:

```
For each time step in the simulation:
  1. Compute link scores for every connection
  2. For every variable, sort outbound links by absolute link score (descending)
  3. For every stock in the model, start a search:
     a. Follow outbound links in order, multiplying by link score
     b. If we reach the starting stock: record the loop and its score (check uniqueness)
     c. If the variable is being visited (cycle not through start): return (will be found from another start)
     d. If the variable was visited before with a higher score: return (prune)
     e. If unvisited or lower prior score: record score, continue recursively
```

**Key insight:** Sorting edges by score means the first visit to a variable is likely the highest-scoring path, making pruning effective and the algorithm fast.

**Caveat:** Because this maximizes rather than minimizes, the algorithm is **heuristic** -- it can miss the strongest loop in certain graph configurations (Figure 7 of Article #4 demonstrates this). However, empirically it finds loops that are very similar to the truly strongest ones.

**Computational complexity:** Roughly proportional to the square of the number of variables per pass. Sorting edges by score provides ~3x speedup.

### 6.5 Completeness Results

| Model | Total Loops | Loops Found | Notes |
|-------|------------|-------------|-------|
| Market Growth (Forrester, 1968; Morecroft 1983) | 19 | 19 | All found (19 loops without macro expansion; 23 with macro expansion per Article #2) |
| Service Quality (Oliva & Sterman, 2001) | 104 (main set) | 76 | Of top 15, only 8th missing; misses are siblings of found loops |
| Economic Cycles (Mass, 1975) | 494 | 261 | Of top 40, only 22nd and 40th missing |
| Urban Dynamics (Forrester, 1969) | 43,722,744 | 20,172 | Truncated to <200 at 0.1% contribution threshold; 10-20 seconds on i7 |
| World 3-03 (Meadows, 2004) | 330,574 | 2,709 | Truncated to 112 at 0.1% threshold; ~4 seconds |

Missing loops are typically **siblings** of discovered loops -- slightly longer or shorter variants that share most of their elements. The algorithm has high confidence of finding loops *similar to* the strongest ones.

### 6.6 Paths Not Taken

The paper documents several abandoned approaches (valuable for future researchers):

1. **Composite score with total potential remaining:** Computed product of strongest outbound link from every unvisited variable as an upper bound on remaining potential. Worked well for averages but not for identifying the most powerful loops at any given time.

2. **Trimming the feedback structure:** Removing weak links to reduce graph density. First attempt (after N loops found) removed strongest loops first -- counterproductive. Second attempt removed weakest links -- removed links that were needed to complete strong loops. Both failed.

3. **Stock-to-stock compaction:** Collapsing all auxiliaries to create a smaller search graph. Didn't improve speed (the number of paths is what matters, not the number of nodes), and eliminated parallel pathways (potential loops).

---

## 7. Article #5: Seamlessly Integrating Loops That Matter into Model Development and Analysis (Summary)

**Authors:** William Schoenberg and Robert Eberlein
**Full summary:** [schoenberg2020.1-seamlessly-integrating-ltm--summary.md](schoenberg2020.1-seamlessly-integrating-ltm--summary.md)

This article describes the production implementation of LTM in the **Stella** family of products (Version 2.0+, including Professional, Architect, and online tools). Key challenges and solutions beyond what LoopX demonstrated:

### 7.1 Macros (DELAY, SMOOTH, etc.)

Macros like DELAY3 hide complex internal structure (3 stocks, 4 flows, internal feedback loops). From the practitioner's perspective, a DELAY3 looks like a single link from input to output.

**Solution -- composite link score:** The link score for a pathway through a macro is the **path score of the expanded pathway with the largest magnitude** (positive or negative). This:
- Preserves loop score integrity (if single internal path, identical to expansion)
- If multiple internal paths (DELAY3 has 7 distinct causal pathways): uses the largest
- Can be computed during simulation with no post-processing
- Supports the PATHSCORE builtin function
- Internal macro loops are trimmed from reported loops
- Internal macro variables are removed from reported results

**Edge case:** A DELAY3 with a step input will report link score 0 because when input changes, output hasn't changed yet. Once output starts changing, input is no longer changing. Since the reported score is a product of internal link scores and one is always 0, the composite is always 0. This makes macros somewhat inscrutable when they're not part of an active feedback loop.

### 7.2 Discrete Variables and Stateful Functions

Stella includes Conveyors, Queues, Ovens, and functions like PREVIOUS that retain state. These have internal structures that can't practically be exposed for complete link score computation (a conveyor can have thousands of individual elements).

**Solution:** Approximate their response as if the instantaneous response is the eventual response (analogous to perfect mixing in traditional SD). This may distort the time profile of loop dominance but gets polarity and magnitude correct. Works well in practice.

### 7.3 Too Many Loops

Uses the strongest path algorithm (Article #4) to find only the important loops. Additionally:
- **LOOPSCORE builtin function:** Allows practitioners to specify any arbitrary feedback loop and compute its score over time, regardless of whether the strongest path algorithm found it. Reports relative values, available at end of simulation.
- **PATHSCORE builtin function:** Computes raw loop/path scores during simulation. Can be used in model equations.

These builtins solve the problem that the strongest path algorithm's results change with parameterization -- a loop important in one scenario may not be discovered in another.

### 7.4 Simplified CLD Improvements Over LoopX

The LoopX prototype had a significant design flaw in how it constructed simplified CLDs: after filtering variables, it searched the **entire network of model equations** to reconnect kept variables, which "ignored why a certain variable was kept and brought forth links (and therefore loops) of demonstrable unimportance in highly simplified CLDs." The Stella implementation fixes this with a fundamentally different approach:

**Evolution of the simplification algorithm:**
1. When a variable is kept by the **link inclusion threshold**, the system now also records *which specific link(s)* justified keeping that variable (the links with relative link variance above the threshold)
2. For each kept link, the system identifies the **strongest feedback loop** that the link is a member of
3. This creates a direct mapping from each kept variable to the specific feedback loop(s) that made it important
4. Simplified links are only generated along pathways that are part of these identified important loops, rather than any arbitrary pathway through the equation network

**Two-way loop mapping:** Uses **original model loops** mapped to simplified loops (many full loops can map to one simplified loop). This mapping is bidirectional: given a simplified loop, you can look up all the full loops it represents; given a full loop, you can determine its simplified representative.

This enables computing the **fraction of total model behavior explained** by the simplified CLD (see composite relative loop score below). Small amount of loop closure is added to improve aesthetics of the simplified CLD layout.

**Composite relative loop score:** Each simplified loop has a composite score = direct sum of the relative loop scores from all full loops that map to it. The sum of absolute composite scores across all simplified loops in the CLD represents the **total explanatory power** (fraction of model behavior explained). Numbers closer to 100% = better simplification. For example, in the Market Growth model, the Stella simplified CLD explains 96.3% of total behavior (Figure 6 of thesis framing, showing Stella Architect), a significant improvement over LoopX which had no such quality metric.

**Disconnected simplified CLDs:** Can occur when two strong minor feedback loops are connected by a weak major loop. When thresholds filter the weak loop, the two strong loops appear disconnected. This is a valid representation of the model's behavior.

### 7.5 Link Thickness and Polarity in Simplified CLDs

**Polarity confidence (Equation 3):** For links representing pathways of potentially mixed polarity:

```
confidence = |r - |b|| / (r + |b|)
```

Where `r` = sum of highest-magnitude instantaneous reinforcing pathway scores over simulation, `b` = sum of highest-magnitude instantaneous balancing pathway scores.

- Confidence of 1: link always has same polarity
- Confidence near 0: link represents pathways of mixed polarity
- Below 0.99 confidence: link is rendered **gray** to signal over-simplification and mixed polarity

### 7.6 Loops with Unknown Polarity

Some links change polarity during simulation (e.g., the formulation flaw in the yeast alcohol model). Loops containing such links have expressed both positive and negative polarities at different times.

**Labeling scheme:**
- Rx, Bx: Standard reinforcing/balancing
- Rux, Bux: Unknown polarity but **predominantly** reinforcing/balancing (confidence > 0.99)
- Ux: Unknown polarity with no clear predominance

### 7.7 Additional Simplification Option

A boolean parameter controls whether flows are automatically kept when their stock is kept. In large models with long loops and many stocks, automatically keeping all flows adds clutter without understanding. Allowing users to optionally exclude flows produces cleaner simplified CLDs.

### 7.8 Demonstrations

**Pedagogy (Population Model):**
- Births only: R1 = 100% (exponential growth, obvious)
- Births + deaths (lifetime = 20): Text states R1 = 67%, B1 = 35%; Figure 5 shows precise values R1 = 66.67%, B1 = -33.33%. The text's "35%" for the balancing loop appears to be a rounding error in the paper; the mathematically correct value from the figure is 33.33%.
- Setting lifetime = 10: No loops reported (equilibrium -- LTM limitation)
- Adding carrying capacity: Text states "one reinforcing loop with a score of 50%, and two balancing loops with scores that add to -50%." The capacity constraint loop (B1) is at first inactive, but then becomes the bigger of the two balancing loops. Figure 7 shows specific instantaneous values at the end of simulation: R1 = 50.00%, B1 = -36.17%, B2 = -13.82%. Dominance shifts over time from R1-dominant to a three-way split.

**Complex Model (Mass's 1975 Economic Cycles):**
- 163 variables, 17 stocks, 494 feedback loops
- Simplified CLD: 9 simplified loops representing 21 full loops, explaining **59.7%** of total behavior
- Of remaining 40.3%: 31.2% from 469 unimportant loops (<2% each), 8.9% from 4 loops consisting of perfectly canceling reinforcing/balancing pairs
- Loop dominance analysis reveals oscillatory cycle driven by alternating dominance of B1 (direct hiring driven by inventory-backlog gap), B4 (delayed price adjustment effect on vacancies), and B2 (direct backlog-to-labor termination)
- R1 and B3 are perfectly destructive interference -- same loop path but opposite effects through perceived price changes on backlogs vs. inventory

**Discrete System (Workforce Training):**
- Model with conveyor (pipeline delay) and non-negative stock
- Two parameterizations (time to adjust = 5 vs. 2) produce different feedback structures
- LTM identifies hidden feedback loops in the conveyor (between Apprentices and finishing training)
- When time to adjust = 2: non-negative stock becomes active, creating additional balancing and reinforcing loops not present in the other parameterization
- Demonstrates LTM works on discrete/discontinuous systems without losing insight capability

---

## 8. Thesis Discussion and Synthesis

### 8.1 Contributions to Knowledge

The thesis argues five contributions:

1. **The LTM method itself** (Article #1): A computationally simple, generally applicable metric for measuring the contribution of feedback loops to model behavior. Not fundamentally new insights, but dramatically more accessible than prior methods.

2. **Automated CLD generation with edge curving** (Article #2): A technique for machine-generating CLDs from model structure that emphasizes feedback loops through a novel edge curving heuristic (contributed to Graphviz).

3. **LTM-based model simplification** (Articles #2 and #5): Two threshold parameters that systematically control the tradeoff between descriptive power and cognitive simplicity in model representations.

4. **The strongest path algorithm for loop discovery** (Article #4): A heuristic that makes LTM practical for large models by finding the most important loops without exhaustive enumeration.

5. **FSNN for causal inference in feedback systems** (Article #3): A novel approach to applying system dynamics thinking to data-driven causal inference, overcoming the DAG assumption of existing methods.

### 8.2 Practical Impact

The thesis was written concurrent with the production implementation in Stella 2.0 (isee systems). The LTM tools are:
- Turned on by a **single checkbox** ("Loop dominance analysis") in the simulation settings
- Automatically find important loops using the strongest path algorithm, compute all link and loop scores during simulation, and report results
- Display animated SFDs and auto-generated simplified CLDs with real-time threshold controls
- Include LOOPSCORE and PATHSCORE builtin functions for practitioners who want to examine specific loops
- Accessible to practitioners with **no specialized training** -- no knowledge of eigenvalues, linearization, or advanced mathematics required

This represents the first time any loop dominance analysis technique has been seamlessly integrated into mainstream modeling software. The thesis framing emphasizes this as the primary contribution: "the true contribution is not a methodological leap, but an accessibility leap." Prior methods (EEA, PPM) were more theoretically sophisticated in some ways, but the LTM method combined with the strongest path algorithm and the Stella UI is the first complete package that an average SD practitioner can use without additional training.

### 8.3 Relationship to Sterman's Challenges and the Research Questions

The two research questions map to Sterman's three challenges:
- **RQ1** (algorithmic discovery of behavioral origins) corresponds to Sterman's "automated identification of dominant loops"
- **RQ2** (visualization, animation, simplification) corresponds to Sterman's "visualization of model behavior" and "linking behavior to generative structure"

| Challenge | Prior State | After Thesis |
|-----------|-----------|-------------|
| Automated identification of dominant loops | EEA, PPM existed but not widely adopted | LTM automated, packaged in Stella |
| Visualization of model behavior | Static diagrams | Animated SFDs and CLDs with dynamic link/loop scores |
| Linking behavior to generative structure | Required specialized analysis | Simplified CLDs automatically connect behavior to minimal structural explanation |

### 8.4 Limitations Acknowledged

1. **Cannot report on unobserved behavioral modes:** LTM only analyzes the behavior that actually occurs during a specific simulation with specific parameters. Alternative scenarios, counterfactuals, and parameter sensitivity are not covered by a single analysis. This is in contrast to EEA, which can decompose behavior into distinct modes (growth, oscillation) and identify how parameter changes would affect each mode.
2. **Cannot report on loop dominance during equilibrium:** Fundamental limitation of the approach. When all net flows are zero, all link scores are 0/0 = 0. This means LTM provides no information about which feedback structure is *maintaining* an equilibrium, only about which structure is *driving change*. EEA can analyze equilibrium states.
3. **Scalability of simplified CLDs:** While the strongest path algorithm handles the Urban Dynamics model (43M loops, ~10-20 seconds on i7), the simplified CLD generation process (mapping full loops to simplified loops, generating simplified links) needs further engineering for truly giant models. The loop finding itself is fast; the visualization pipeline is the bottleneck.
4. **FSNN is early-stage:** The article is a "working paper" demonstrating feasibility on a synthetic system. Many challenges remain for real-world application: measurement error, appropriate scaling, manifold coverage, identifying state variables from observational data, and validating inferred causal structure without access to ground truth.
5. **LTM does not identify leverage points:** Unlike EEA, LTM does not directly identify where structural changes (policy interventions) would most affect behavior. It identifies which loops dominate, but not where to intervene.

### 8.5 Future Directions

1. **Monte Carlo + LTM:** Combine sensitivity analysis with loop dominance to measure structural robustness across parameter ranges. Run many simulations with varied parameters, compute LTM for each, and analyze how loop dominance patterns change. This would address the limitation that LTM only reports on one scenario at a time.
2. **Expanded FSNN research:** Address real-world data challenges (noise, missing data, extraneous variables, unknown state variables). Key questions: How much data is needed? How many initializations? How to handle measurement error? How to select activation functions and scaling for different data distributions?
3. **Cross-disciplinary application:** LTM and FSNN are applicable to data science, machine learning, and AI beyond the SD community. The thesis argues that the SD concept of feedback loop dominance has direct analogs in neuroscience (neural feedback circuits), ecology (predator-prey dynamics), and economics (market feedback).
4. **Improved simplified CLD quality metrics:** Automated assessment of whether a simplified CLD adequately represents model behavior. Currently, the composite relative loop score gives a global percentage, but there is no automated check for whether specific important dynamics are preserved or distorted by simplification.
5. **Effectiveness studies:** Measure whether the LTM tools actually improve learning outcomes and modeling practice in controlled experiments. The thesis notes that the prior Forio Model Explorer study was inconclusive, and more rigorous evaluation is needed.
6. **Loop power over time visualization:** Article #2 discusses the potential for showing loop power (dominance) as a function of time alongside the loop legend, enabling users to see dominance shifts without scrubbing the animation timeline.

---

## 9. Implementation Notes (Relevant to Simlin)

The thesis documents that the LTM method was originally implemented by modifying the **sd.js engine** (Powers, 2019). Key implementation details:

### 9.1 Computational Requirements

- **Equation re-evaluation:** For each non-stock variable with equation `z = f(x1, x2, ..., xk)`, the equation must be re-evaluated **k times** per timestep (once per input variable, holding others at previous values). For a typical model where most variables have 2-4 inputs, this roughly doubles the number of equation evaluations vs. normal simulation. For variables with many inputs (e.g., a lookup function of 10 variables), the cost is proportionally higher.
- **Storage:** Previous values of **all** variables must be retained between time steps. The delta computation requires both current and previous values for every variable. This doubles memory usage for variable storage.
- **Loop finding:** The strongest path algorithm runs at each dt (or a configurable subset of time steps), with complexity roughly proportional to the **square of the number of variables** per pass. The 3x speedup from edge sorting (Article #4) is critical for large models. For Urban Dynamics, the algorithm takes ~10-20 seconds total across all time steps on an i7 processor.
- **Link scores are computed for every link at every dt** during simulation. For a model with L total links and T time steps, this is L*T link score computations. Each link score computation is O(1) given the precomputed ceteris paribus values.
- **Total overhead:** For typical models, LTM roughly doubles simulation time. For models with many inputs per variable or many feedback loops, the overhead can be higher.

### 9.2 PATHSCORE and LOOPSCORE Builtins

- **PATHSCORE:** Can be used in model equations; computes the raw path score (product of link scores) along a specified path. Available during simulation.
- **LOOPSCORE:** Reports relative loop scores for a specified loop. Only available at end of simulation (because relative scores require normalization across all loops).

### 9.3 Macro Handling

For macros (DELAY, SMOOTH, etc.), the composite link score is the path score of the expanded pathway with the largest magnitude. Internal macro loops are trimmed from results. Internal macro variables are hidden.

### 9.4 Conveyor/Discrete Element Handling

Approximated as instantaneous response (perfect mixing analog). Gets polarity and magnitude right; may distort time profile of dominance.

---

## 10. Key Figures Summary

### Thesis Body Figures
- **Figure 1:** Simple population stock-and-flow diagram (births feedback loop)
- **Figure 2:** Simple population causal loop diagram
- **Figure 3:** Seven basic SD behavior modes (exponential growth, goal seeking, oscillation, S-shaped growth, S-shaped growth with overshoot, overshoot and collapse, overshoot and oscillation)
- **Figure 4:** Illustration of how reinforcing/balancing loops drive behavior patterns
- **Figure 5:** Stella Architect screenshot showing LTM analysis of the Market Growth model, with animated SFD, simplified CLD, loop legend showing composite relative loop scores totaling 96.3%
- **Figure 6:** Comparison of thesis goals vs. thesis articles mapping (which article addresses which goal)

### Article #2 Figures
- **Figure 1:** LoopX architecture schematic (XMILE -> sd.js + LTM -> animated SFD/CLD)
- **Figure 2:** Machine-generated CLD from Bass diffusion SFD (demonstrating edge curving quality)
- **Figure 3:** Stock-and-flow diagram of Forrester's 1968 Market Growth model (10 stocks)
- **Figure 4:** Full auto-generated CLD of Market Growth (link 0%, loop 0%)
- **Figure 5:** Simplified CLD (link 100%, loop 0%) -- less than half the variables
- **Figure 6:** Simplified CLD (link 100%, loop 20%) -- further reduced
- **Figure 7:** Maximally simplified CLD (link 100%, loop 100%) -- 4 variables
- **Figure 8:** LoopX screenshot -- animated SFD of Bass diffusion (split flow rendering)
- **Figure 9:** LoopX screenshot -- simplified CLD of Bass diffusion

### Article #3 Figures
- **Figure 1:** Generated model vs. ground truth on two training initializations (good fit)
- **Figure 2:** 3x3 matrix of pathway link scores comparing ground truth and generated model (correct causal links and polarities identified)
- **Figure 3:** Prediction error distribution across 100 Monte Carlo initializations (tight 95% bounds)
- **Figure 4:** Maximum error vs. sum of initial states (exponential degradation beyond training range)

### Article #4 Figures
- **Figure 1:** Three-party arms race model SFD
- **Figure 2:** Behavior of the 3 stocks in the arms race model
- **Figure 3:** Relative loop scores for all 8 loops (long loops dominate by time 50)
- **Figure 4:** Simple decoupled feedback loop model (demonstrating active/inactive loops)
- **Figure 5:** Behavior of the decoupled model
- **Figure 6:** Link scores showing time-varying activity of connections
- **Figure 7:** Failure case for direct Dijkstra adaptation (a->d->c->a found instead of stronger a->b->c->a)

### Article #5 Figures
- **Figure 1:** DELAY3 macro internal structure (3 stocks, 4 flows, 7 distinct causal pathways)
- **Figure 2:** Weakly coupled system that becomes disconnected when simplified (valid representation)
- **Figure 3:** Over-simplified Market Growth CLD with gray link (mixed polarity indicator) and its expansion
- **Figure 4:** Simple population model (births only) -- R1 = 100%
- **Figure 5:** Population with births and deaths -- R1 = 67%, B1 = -33%
- **Figure 6:** Highlighting the death loop (no arrowhead from stock to outflow)
- **Figure 7:** Population with carrying capacity -- shifting dominance
- **Figure 8:** Auto-generated simplified CLD of Mass's Economic Cycles (9 simplified loops, 59.7% explained)
- **Figure 9:** Composite relative loop scores during one cycle of Economic Cycles model
- **Figure 10:** Key indicator stocks showing dampened oscillation
- **Figure 11:** Workforce training model with conveyor and non-negative stock
- **Figure 12:** Two simplified CLDs under different parameterizations (time to adjust = 5 vs 2)

---

## 11. Complete Reference List of Models Analyzed

| Model | Stocks | Variables | Loops | Articles |
|-------|--------|-----------|-------|----------|
| Bass Diffusion (1969) | 2 | ~10 | 2 | #1, #2, #5 |
| Yeast Alcohol | 2 | ~5 | 4 | #1 |
| Inventory Workforce (Goncalves, 2009) | 3 | ~12 | 3 | #1 |
| Three-State ODE (synthetic) | 3 | ~8 | 4 | #3 |
| Three-Party Arms Race | 3 | ~18 | 8 | #4 |
| Market Growth (Forrester, 1968) | 10 | 48 | 19 (or 23 with macro expansion) | #2, #4, #5 |
| Service Quality (Oliva & Sterman, 2001) | ~15 | ~50 | 104+ | #4 |
| Economic Cycles (Mass, 1975) | 17 | 163 | 494 | #4, #5 |
| Simple Population | 1 | ~4 | 1-3 | #5 |
| Workforce Training (discrete) | 2 | ~10 | 2-4 | #5 |
| Urban Dynamics (Forrester, 1969) | ~20 | ~100+ | 43,722,744 | #4 |
| World 3-03 (Meadows, 2004) | ~30 | ~200+ | 330,574 | #4 |

---

## 12. Terminology Reference

| Term | Definition |
|------|-----------|
| **Link score** | Dimensionless measure of contribution and polarity of a link between independent and dependent variable at a point in time |
| **Loop score** | Product of all link scores in a feedback loop; measures loop's contribution to model behavior |
| **Relative loop score** | Loop score normalized by sum of absolute loop scores in the cycle partition; range [-1, 1] |
| **Partial change** (delta_xz) | Change in z that would occur if only x changed (ceteris paribus) |
| **Relative link score** | Link score normalized across all determinants of the dependent variable |
| **Relative link variance** | max - min of the absolute relative link score over the simulation; measures link dynamism |
| **Composite link score** | Path score of the expanded pathway through a macro with the largest magnitude |
| **Composite relative loop score** | Sum of relative loop scores of all full loops mapping to a simplified loop |
| **Link inclusion threshold** | [0,1] parameter filtering variables by maximum relative link variance of incoming links |
| **Loop inclusion threshold** | [0,1] parameter filtering loops by average magnitude of relative loop score |
| **Polarity confidence** | |r - |b|| / (r + |b|); measures whether a simplified link has consistent polarity |
| **Cycle partition** | Subset of model where all stocks are connected by feedback loops |
| **Dominant loop** | Loop (or set) contributing >= 50% of observed change across all stocks |
| **Strongest path algorithm** | Dijkstra-like heuristic for finding high-scoring feedback loops |
| **FSNN** | Feedback System Neural Network -- ODE system with neural net derivatives for causal inference |
| **ILS** | Independent Loop Set (Kampmann, 2012) |
| **SILS** | Shortest Independent Loop Set (Oliva, 2004) |
| **EEA** | Eigenvalue Elasticity Analysis |
| **PPM** | Pathway Participation Metric |
| **LoopX** | Web-based LTM visualization tool (prototype, precursor to Stella integration) |
| **Universal approximator** | Property of neural networks (Cybenko, 1989): a single hidden layer with enough neurons can approximate any continuous function to arbitrary precision |
| **Manifold** | In FSNN context: the n-dimensional surface in state space traced by the derivative function of a state variable as the system evolves; training data samples points on this surface |
| **Degenerate payoff surface** | In FSNN context: multiple weight configurations score equally well during optimization, making the specific causal structure found by the optimizer non-unique |
| **BOBYQA** | Bound Optimization BY Quadratic Approximation (Powell); derivative-free optimization algorithm used for FSNN training |
| **Ceteris paribus** | "All other things being equal"; the LTM method for computing partial changes by varying one input while holding others at previous-timestep values |
