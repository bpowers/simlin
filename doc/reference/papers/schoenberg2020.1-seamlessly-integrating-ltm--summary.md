# Detailed Summary: "Seamlessly Integrating Loops That Matter into Model Development and Analysis"

**Authors:** William Schoenberg, Robert Eberlein
**Venue:** System Dynamics Conference / Working Paper (2020)
**Relationship to prior work:** Follow-on to Schoenberg, Davidsen, and Eberlein (2019), "Understanding Model Behavior Using the Loops that Matter Method" (System Dynamics Review, Vol. 36, No. 2)

---

## 1. Abstract and Motivation

This paper describes the challenges of implementing the Loops that Matter (LTM) methodology into a **fully functioning model development environment** (the Stella family of products, starting with Version 2.0), along with the solutions developed. The focus is on making LTM a seamless, essentially invisible part of the modeling experience -- turned on by a checkbox, automatically reporting loop and link information, and enabling visual exploration of model structure crucial to driving behavior.

The central thesis: if tools for discovering the origins of behavior are simply part of the model development experience, they will become part of how people understand the models they build. For experienced practitioners, the tools reinforce and challenge beliefs about what drives behavior. For less experienced modelers, they offer discoverable, easy-to-communicate pathways to understanding.

**Key distinction from the prior paper:** The 2019 paper presented the LTM method itself (the mathematics, definitions, and validation against other methods). This paper addresses the **production engineering** -- what had to be solved to make LTM work on real practitioner-built models of arbitrary complexity, including macros, discrete elements, models with hundreds of feedback loops, and visualization challenges.

---

## 2. Existing Approaches to Loop Dominance Analysis (Brief Review)

The paper provides a condensed review of the two historical approaches, noting why neither has achieved widespread adoption:

### 2.1 Eigenvalue Elasticity Analysis (EEA)

- Determines what combination of behavior modes a given model structure produces (Saleh, 2002; Kampmann et al., 2006; Saleh et al., 2010; Oliva, 2016).
- Uses a linearization of the model and its associated eigenvalues and eigenvectors as the unit of analysis.
- Most encompassing method for structural analysis, but **limited in the set of models it can analyze** because it is fundamentally designed to work on continuously differentiable systems (Oliva, 2016).
- Current software tools for performing EEA **change model equations to meet that requirement**, which has measurable impacts on simulation results.
- Current applications rely on the independent loopset (Kampmann, 2012) or the unique shortest independent loopset (Oliva, 2004). These provide a reasonable number of loops and maintain loop independence, but **can limit the ability to understand models where behavior is fundamentally driven by loops outside of these sets** (as described by Eberlein and Schoenberg, 2020).

### 2.2 Pathway Participation Metric (PPM)

- Does not use eigenvalues; focuses on links between variables, tracing causal pathways between stocks and identifying the pathway most responsible for moving a stock in the direction of its net change (Mojtahedzadeh et al., 2004).
- PPM-based methods (including Hayward and Boswell's Loop Impact Method, 2014) study observed behavior modes relative to **individual stocks** in the model, rather than all stocks together.
- More directly applicable to a wider range of models because it does not require continuously differentiable systems.
- Criticized for failure to clearly explain oscillatory behavior (Kampmann & Oliva, 2009).

### 2.3 Why None Are In Common Use

Despite 40+ years of research and many publications, **none of these approaches are in common use** by a significant number of modelers or students. The paper attributes this to:
1. Each approach has systematic limitations and blind spots.
2. More importantly, **all of them require the practitioner to do significant work**. People with years of experience tend to rely on intuition; those with less experience are overwhelmed by model-building itself and cannot also learn a complex analytical toolset.

---

## 3. The Loops that Matter Method (Brief Recap)

The paper provides a condensed summary of LTM for readers who have not read the 2019 paper. Key points:

### 3.1 Core Properties

- Uses computations based on **actual variable values during simulation**.
- Can be applied to models of any size or complexity without regard to continuity.
- Produces analyses matching PPM and EEA when applied to the same models (Schoenberg et al., 2019).
- Like PPM, builds upon observations of how experienced modelers perform analysis: walking causal pathways to determine sources of observed behavior.
- All calculations done **directly on model equations**, making measurements of loop and link contributions easier to understand.
- Uses only values computed as part of the normal simulation, making it applicable to models with discrete characteristics.

### 3.2 Three Metrics

1. **Link score** -- contribution of a link at one instant in time
2. **Loop score** -- contribution of a feedback loop (product of link scores around the loop)
3. **Relative loop score** -- normalized loop score (absolute values sum to 100%)

### 3.3 Link Score (Equation 1)

```
                | delta_xz |        / delta_xz \
LS(x -> z) =   | -------- | * sign |  --------  |      if delta_z != 0 AND delta_x != 0
                | delta_z  |        \  delta_x  /

            =   0                                        if delta_z = 0 OR delta_x = 0
```

Where delta_xz is the partial change in z with respect to x (ceteris paribus: the amount z would have changed if x changed by the amount it did, but all other inputs had not changed). First term = magnitude, second term = polarity (per Richardson, 1995).

### 3.4 Link Score for Flows to Stocks (Equation 2)

For stock `s = integral(i - o)`:

```
Inflow:   LS(i -> s) = |i / (i - o)| * (+1)
Outflow:  LS(o -> s) = |o / (i - o)| * (-1)
```

### 3.5 Path Score

An important attribute of link scores is that they can be **multiplied together** to give the effect along a path between an input and an output. This product is called the **path score**. Path scores are used to deal with hidden paths, such as those arising from macros (discussed below).

### 3.6 Loop Score and Relative Loop Score

- Loop score = product of all link scores around a closed loop.
- Magnitude represents the contribution of a loop to changes in all model variables at a point in time (footnote: assuming all variables share the same feedback loops; for models with disconnected feedback loops, variables are partitioned into sets sharing feedback loops, and scores are computed on each set).
- Loop scores can be very large in magnitude, so **relative loop scores** (normalized so absolute values sum to 100%) are typically used.

---

## 4. Implementation Challenges and Solutions

### 4.1 Macros (DELAY, SMOOTH, etc.)

**The problem:** Macros like DELAY3 or SMOOTH incorporate complex hidden internal structure. From the practitioner's perspective, there is a direct link between "input" and "output using macro." But the full set of relationships underlying the macro reveals a much less direct path.

**Example -- DELAY3 macro (Figure 1):** The DELAY3 macro expands into 3 stocks, 4 flows, and multiple causal pathways. What appears to be two simple direct links on the diagram (input -> output, delay time -> output) is actually **seven distinct causal pathways** if we include influences to flows both directly and through upstream stocks. Additionally, the structure itself contains **three feedback loops** internal to the macro, which the paper notes generally do not produce behavior by themselves.

**The challenge in detail:**
- Multiple causal pathways exist through the macro with differing strengths and potentially even polarities.
- There may be feedback loops **within** the macro equations themselves.
- Expanding macros to show all internal variables would be confusing to practitioners, generate meaningless variable names, and undermine one of the main reasons for using macros (preventing clutter).

**The solution -- composite link score:** A simple heuristic applied at each calculation interval:
1. Compute the path score for every pathway through the macro.
2. If there is only one pathway through the macro, the composite link score equals that path score (identical to the fully expanded case).
3. If there are multiple pathways, **choose the path score with the largest magnitude** (positive or negative).

**Rationale:** This maintains the integrity of loop scores computed through macros:
- Single path: loop score is exactly what it would have been if the macro had been expanded.
- Multiple paths: loop score reflects the biggest (most important) of all the loops involving the macro.

**The link score for anything going into a macro is thus a composite.** As a consequence, the structure represented by a link through a macro is **not necessarily fixed** throughout the simulation run -- the dominant pathway can change over time.

**Alternative approaches considered and rejected:**
1. Post-processing all path scores and picking the best one -- advantage of invariant macro structure, but would change loop scores relative to fully expanded case.
2. Predefined pathways for link score computation -- rejected for similar reasons.
3. Expanding all macros to expose internal variables to the user -- rejected because it would be confusing, generate meaningless names, and undermine the purpose of macros.

**Behavioral note about composite link scores:** They preserve overall loop scoring but may look different from expanded versions. For example, if a DELAY3 macro receives a step input (and nothing else), the composite link score will always be 0. This is because the input changes only at a single point in time when the output is not yet changing, and once the output starts changing, the input is no longer changing. Since the reported link score is the product of internal link scores (one of which is always 0), the composite is 0. This makes macros somewhat inscrutable when they are not actually part of any feedback structure -- which is correct behavior.

**Loop trimming:** Loops involving internal macro variables need to be trimmed (collapsed) before being reported. Similarly, loops internal to the macro (such as the first-order drains in DELAY3) are dropped altogether and not reported.

**Implementation also supports a PATHSCORE builtin function** that can be used in the model to compute the raw loop score during simulation (without post-processing into relative values).

### 4.2 Discrete Variables and Stateful Functions

**The problem:** Stella includes discrete elements (Conveyors, Queues, Ovens) and builtin functions like PREVIOUS that retain state. Unlike macros, there is no rigorous way to expand these into structures amenable to complete link score computation, because:
- Internal structures cannot practically be exposed (a conveyor can have thousands of individual elements waiting to be used at a later time).
- Following paths through such elements is not feasible.

**The solution -- perfect mixing approximation:** Treat the instantaneous response to changing inputs **as if it were the eventual response** (analogous to the perfect mixing assumption in traditional SD models). For a normal stock with proportional outflow, a change in stock value causes the outflow to change immediately. For a conveyor, the change is not immediate but eventual, with the same basic character. Treating it as instantaneous may cause some distortion in the **time profile** of loop dominance but gets polarity and magnitude correct.

**In practice this works well**, as demonstrated by the discrete workforce training model example later in the paper.

### 4.3 Too Many Loops

**The problem:** Feedback is pervasive, and some models have more feedback loops than can be dealt with in reasonable time by software, let alone by a practitioner. The number of potential loops is typically **more than linearly proportional** to the number of stocks in a model (Kampmann, 2012) -- it can get very large very fast. Many published models have too many loops to practically enumerate.

**Static analysis limitations:** The independent loopset and shortest independent loopset (used by EEA) are static analysis techniques that do not allow the loops under consideration to change over the course of a simulation. This is exactly what is required for dominance shifts and can therefore **miss important feedback** (Guneralp, 2006; Huang et al., 2012).

**The solution -- the "strongest path" algorithm** (described in Eberlein and Schoenberg, 2020): A heuristic that finds the loops that matter **at each point in time**, with the resulting set of loops used for further analysis. Unlike static ILS/SILS:
- The set of identified loops **can change** based on the parameterization of a simulation run.
- Just as the most important loops change with different parameters, so will the loops actually identified for large models.

**The LOOPSCORE builtin function:** Because the strongest path algorithm may not always identify a particular loop of interest, a LOOPSCORE builtin was devised. It allows the practitioner to specify any arbitrary feedback loop and compute its score over time, guaranteeing the loop will be reported regardless of the input space. This also enables **cross-run comparisons** of how active a loop is under different scenarios. Loop scores via LOOPSCORE are reported as relative values and are only available at the end of the simulation. Raw loop scores can be computed during simulation using the PATHSCORE builtin.

### 4.4 Visualization: Simplified Causal Loop Diagrams

The paper describes a significant advance in LTM visualization compared to the earlier LoopX tool (Schoenberg, 2019). The goal is to fulfill the call from Sterman (2000) for software that can perform "Automated identification of dominant loops and feedback structure," "Linking behavior to generative structure," and "Visualization of model behavior."

#### 4.4.1 The LoopX Tool (Prior Work)

LoopX introduced two key concepts for CLD simplification:

1. **Link inclusion threshold:** Filters variables in the simplified visualization based on the **variation in magnitude of the relative link score** across the entire simulation period. Only variables with an inbound link whose relative link score varies by at least the threshold are kept.

2. **Loop inclusion threshold:** Keeps the stocks (and optionally flows) in every loop that explains, on average, at least the specified percent of model behavior. Since all loop scores are presented as relative values with magnitude adding to 100, this is straightforward.

**Problem with the LoopX approach:** The implementation first selected variables to include, then found all pathways connecting those variables, then detected loops in the simplified diagram. This had computational shortcomings (loop detection is not always easy) and conceptual shortcomings (difficult to relate simplified loops to original loops, and unimportant loops were likely to appear).

#### 4.4.2 Improved Implementation

The new implementation uses the same metrics for selecting variables but **uses the original loops** to create the simplified diagram. A **two-way mapping** is maintained from full-model loops to simplified-diagram loops (though more than one full loop may map to the same simplified loop). This enables:

- Computing what fraction of total model behavior the simplified CLD explains.
- Attaching scores to the simplified CLD relative to the original model.
- Ensuring that the important connections are included.

A small amount of **loop closure** is performed to improve layout, which can add some aesthetically pleasing but unimportant loops.

#### 4.4.3 Composite Relative Loop Score for Simplified CLDs

Because each simplified CLD loop may represent multiple full-model loops, each simplified loop gets a **composite relative loop score**: the sum of relative loop scores from all full loops that reduce to that simplified loop. Since relative loop scores are normalized and each full loop maps to exactly one simplified loop, the **sum of composite relative loop scores** for all simplified loops represents the **fraction of full model behavior explained by the simplified CLD**. Numbers closer to 100 mean the simplified CLD explains more behavior; closer to 0 means less is represented.

#### 4.4.4 Disconnected Simplified CLDs

A surprising but valid outcome: the simplification process can produce **disconnected** simplified CLDs. Example (Figure 2): a model with two strong minor feedback loops tied together by a weak major feedback loop. When thresholds are set high, the weak major loop disappears, leaving two unconnected minor loops as the primary drivers of behavior. This is a faithful representation of the underlying feedback structure.

### 4.5 Link Thickness and Polarity Markings in Simplified CLDs

**The problem:** A simplified link may represent several different causal pathways with different strengths and potentially different polarities.

**The solution:** Use the causal pathway with the **largest path score magnitude averaged across time**. The reasoning: if a variable is included because it has a strong link, and a loop is included because it is strong, then the representation should reflect that strongest pathway's average strength. This produces satisfying results for links representing pathways of the same polarity.

**Mixed-polarity links (the confidence metric -- Equation 3):** When a simplified link over-abstracts pathways of both reinforcing and balancing polarities, the displayed polarity may be misleading.

```
confidence = |r - |b|| / (r + |b|)
```

Where:
- `r` = sum of the single highest magnitude instantaneous reinforcing pathway scores across the entire simulation
- `b` = sum of the single highest magnitude instantaneous balancing pathway scores across the entire simulation

**Interpretation:**
- Confidence = 1 when only one polarity is present (either r or b is 0).
- Confidence approaches 0 when both polarities contribute equally.
- A confidence value of **0.99 or lower** triggers the link to be displayed in **gray** (representing mixed/unknown polarity), making it abundantly clear the simplified CLD is over-simplified at that point.

**Figure 3 demonstrates this:** A simplified CLD of Forrester's (1968) market growth model shows a gray link indicating indeterminate polarity. Adjusting the link inclusion threshold down by one one-hundredth of a percent expands that over-simplified link into its key constituent pathways, resolving the ambiguity.

### 4.6 Loops with Unknown Polarity

**The problem:** Some models contain links (and therefore loops) that **change polarity during simulation**. For example, in the yeast alcohol model (Schoenberg et al., 2019), a link from yeast concentration to growth is first positive, then negative. The paper notes these links are rare (typically in equations compounding multiple effects), but they occur often enough to require handling. Any loop containing such a link has expressed both positive and negative polarities at different points in time, making it technically impossible to classify the loop's polarity across the full simulation.

**The solution -- a loop polarity classification scheme:**

| Label | Meaning |
|-------|---------|
| Rx | Reinforcing (index x) |
| Bx | Balancing (index x) |
| Rux | Unknown polarity, predominantly reinforcing |
| Bux | Unknown polarity, predominantly balancing |
| Ux | Unknown polarity |

The Ru and Bu designations are assigned when the **confidence value for loop polarity is above 0.99** (calculated using Equation 3 applied at the loop level). This cutoff allows a well-reasoned factual interpretation of full and simplified CLDs where the polarity-changing nature of links is not important over the course of the simulation.

### 4.7 An Additional Option to Simplify CLDs

**The problem:** In the original definition of link and loop inclusion thresholds, anytime a stock is kept, so are its flows, regardless of the relative link scores of those flows. In large models with many stocks and long feedback loops, this produces extra flows that add nothing to understanding.

**The solution:** A third simplification parameter -- a **boolean** controlling whether flows are automatically kept when a stock is kept. This gives the user control over flow inclusion in the simplified CLD.

---

## 5. Demonstration: Pedagogy (Population Models)

### 5.1 Simple Population -- Births Only (Figure 4)

The simplest population model: one stock (Population), one flow (births), one parameter (birth rate). One reinforcing loop (R1) that accounts for 100% of behavior. Students can play with the growth rate to develop intuition for exponential growth.

### 5.2 Population with Births and Deaths (Figure 5)

Adding deaths with an average lifetime of 20 produces slower exponential growth with two loops:
- R1 (reinforcing): 67% contribution
- B1 (balancing): 35% contribution (note: magnitudes sum to more than 100% in instantaneous terms because opposing effects inflate both)

**Pedagogical insight:** If students set "average lifetime" to exactly 10, nothing changes and no loops are reported. This is a **learning moment**: the lack of reported loops is an artifact of LTM (it cannot analyze equilibrium) but also reflects that two opposing loops are perfectly balanced -- either cosmic coincidence or meaningful. This naturally leads to introducing **carrying capacity**.

### 5.3 Population with Carrying Capacity (Figure 7)

Adding carrying capacity (effect of crowding on deaths) creates three loops:
- R1: 50% (reinforcing)
- B1: -36.17% (balancing, from death rate)
- B2: -13.83% (balancing, from carrying capacity effect)

This dramatically shows the difference between a fragile equilibrium and one resulting from **shifting loop dominance**. B1 (the capacity constraint loop) is at first inactive but then becomes the bigger of the two balancing loops. The question of whether B1 is changing behavior or merely making B2 strong enough to balance R1 makes for good class discussion.

**Key pedagogical point:** LTM helps even with obvious models because "simply obvious" means different things to novices versus experienced modelers. The extra quantitative cues make discussion faster, more informative, and better remembered. Highlighting a balancing loop on the stock-and-flow diagram (e.g., the outflow "deaths" loop B1 in Figure 6, which does not have an arrowhead back to the stock) helps students see loops that are structurally present but visually hidden.

---

## 6. Demonstration: Understanding Economic Cycles (Mass, 1975)

### 6.1 Model Characteristics

Mass' 1975 Economic Cycles model is a practitioner-developed model with a high level of complexity:
- **163 variables**
- **17 stocks**
- **494 feedback loops** (plus 4 additional two-variable stock/flow balancing loops, each in their own cycle partition, which do not affect model behavior)

### 6.2 Simplified CLD (Figure 8)

A machine-generated simplified CLD with:
- Link inclusion threshold: over 100%
- Loop inclusion threshold: 2.4%
- Flows not automatically kept with stocks

Produces **9 simplified feedback loops** representing the combined effects of **21 full feedback loops**. These 9 simplified loops explain **59.7%** of the behavior across the entire simulation.

Accounting for the remaining 40.3%:
- **31.2%** comes from 469 relatively unimportant loops (each individually producing less than 2% of cumulative behavior).
- **8.9%** comes from 4 remaining loops not shown. These consist of **two sets of paired feedback loops** (one balancing and one reinforcing in each pair) which perfectly cancel each other at all time points, making all 4 irrelevant to observed behavior.

### 6.3 Simplified Loop Details (Table 1)

| Loop | Total Contribution | Full Loops Aggregated | Links Included |
|------|-------------------|-----------------------|----------------|
| B1   | 38.12%            | 1                     | Vacancies -> labor -> Inventory -> Backlog |
| B2   | 6.60%             | 1                     | labor -> Inventory -> Backlog |
| B3   | 5.11%             | 3                     | Vacancies -> labor -> Avg Prod Rate -> Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Inventory |
| R1   | 5.11%             | 3                     | Vacancies -> labor -> Avg Prod Rate -> Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Backlog |
| B4   | 4.15%             | 1                     | Vacancies -> labor -> Inventory -> Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Backlog |
| B5   | 0.37%             | 1                     | Vacancies -> labor -> Inventory |
| B6   | 0.13%             | 1                     | Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Backlog -> labor -> Inventory |
| R2   | 0.10%             | 3                     | Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Backlog -> labor -> Avg Prod Rate |
| U1   | 0.02%             | 7                     | Avg Unit Cost of Prod -> Avg Price -> Smoothed Avg Price -> Perc Rate of Inc in Price -> Inventory |

Note: B5, B6, R2, and U1 are **artifact loops** brought forth by the specific combination of selected feedback loops (loop closure for layout improvement). They were not directly selected.

### 6.4 Loop Dominance Analysis (Figures 9 and 10)

**Analysis period:** Time 9 to 12.5 (one complete cycle of Inventory from trough to trough), chosen because it represents one complete wave.

**Key indicator stocks:** Backlog, Inventory, Labor, Vacancies (Figure 10) show dampened oscillation across the full simulation (time 0-20) and within the analysis period.

**The loop dominance pattern repeats twice** in the analysis period (beginning at time 9 and again at time 10.6). The first instance corresponds to the growth of labor, the second to the decline.

**Dominance progression within each half-cycle:**

1. **B1 starts nearly completely dominant.** B1 describes the major drivers of hiring labor based on vacancies created by backlog driven directly by inventory changes.

2. **R1 and B3 become active as B1 wanes.** These are both long-delay loops related to hiring labor through changes in vacancies. R1 and B3 are a pair that **perfectly destructively interfere** -- they cancel each other out because their only difference is the specific route through which perceived rate of increase in price affects vacancies. R1 (reinforcing) represents the effect of perceived price increase on backlog (which reinforces vacancies); B3 (balancing) represents the same price perception effect on inventory (which balances vacancies). The two pathways have the same magnitude contribution but opposite signs.

3. **B4 becomes important.** B4, like B1, describes changes in labor due to vacancies, but does so through a **long delay** from the perception of price (due to inventory changes affecting unit costs). After B4's delayed price adjustment effect plays out, the direct hiring process (B1) becomes dominant again.

4. **B2 becomes active**, representing the direct effect of backlog changes on labor through termination (not yet dominant at this stage).

5. **B2 becomes the dominant loop**, driving changes in labor through termination at the inflection points in either the growth or decline of labor.

6. **B2 yields back to B1**, starting the cycle of shifting feedback loop dominance anew.

**Every other progression through this cycle changes** whether labor is growing or shrinking (since B1 is an oscillatory loop). This progression continues over the entire simulation period, even as oscillations dampen.

**Significance:** This cogent explanation of a 163-variable, 494-loop model demonstrates the toolset's power to dramatically simplify model understanding while providing objective clarity on the causes of behavior in complex systems.

---

## 7. Demonstration: Discrete System (Workforce Training Model)

### 7.1 Model Structure (Figure 11, Table 2)

A simple workforce training model using:
- A **conveyor** (pipeline delay) modeling the training process: apprentices enter, train for a specified time, then emerge as workers.
- A **non-negative stock** for Workers (intentionally used to limit the number of employees leaving the system).

**Model equations:**

| Equation | Units |
|----------|-------|
| Apprentices = CONVEYOR(hiring - finishing training, training time) | People |
| Workers = NONNEGATIVE(finishing training - leaving) | People |
| hiring = adjustment + leaving | People/Time |
| leaving = 100 + STEP(50, 5) | People/Time |
| finishing training = f(Apprentices) | People/Time |
| adjustment = (target workers - workers) / time to adjust | People/Time |
| training time = 5 | Time |
| target workers = 500 | People |
| Initial Apprentices = 5 * hiring | People |
| Initial Workers = target workers | People |
| time to adjust = 5 or 2 (two cases) | Time |

### 7.2 Two Parameterizations Analyzed

Two cases are analyzed, identical except for "time to adjust": Case 1 uses 5, Case 2 uses 2.

### 7.3 Results: Simplified CLDs (Figure 12)

The simplified CLDs for the two parameterizations reveal **qualitatively different feedback structures**:

**Case 1 (time to adjust = 5):** The simplified CLD shows **two balancing loops**:
- One involving apprentices -> finishing training -> workers -> adjustment -> hiring (the main balancing chain)
- Shows the hidden feedback loop within the conveyor between Apprentices and finishing training (the conveyor directly affects its own output)

**Case 2 (time to adjust = 2):** The non-negative stock becomes active and constrains the outflow "leaving." The simplified CLD shows **four loops**:
- The same two balancing loops from Case 1
- An **additional balancing loop** (between leaving and workers)
- An **additional reinforcing loop** (across the entirety of the main chain, adjusting hiring without passing through the adjustment variable)

### 7.4 Key Insights

1. **LTM identifies hidden feedback loops** in discrete structures. The conveyor's internal feedback (output depends on its own contents) is correctly surfaced.

2. **The feedback complexity of the model changes with its parameterization.** Case 1 has two balancing loops; Case 2 has two balancing plus one additional balancing plus one reinforcing loop. This structural change arises because the non-negative constraint on Workers becomes active under different parameters, and "leaving" now changes (and therefore appears in the CLD as a concept unto itself).

3. **LTM is fully capable of performing analyses on discrete systems** without losing any of its ability to quickly generate insight. The perfect mixing approximation for conveyors works well in practice.

---

## 8. Conclusions

The paper concludes that the Stella family of products now gives all system dynamics practitioners automated tools to quickly understand the origins of their model's behavior. The paper has demonstrated:

1. **Animated stock-and-flow diagrams** where connectors and flows change size and color based on link scores.
2. **Animated automatically generated causal loop diagrams** showing dominant structure.
3. **Automated simplification of CLD structure** on large and dynamically complex models.
4. **Applicability to discrete feedback systems** (conveyors, non-negative stocks).

### 8.1 Remaining Limitations

Only two limitations are identified:
1. **Inability to report on unobserved behavioral modes** -- LTM can only analyze behavior that actually occurs during simulation.
2. **Inability to report on loop dominance during equilibrium** -- when nothing changes, all scores are zero.

The authors argue neither limitation should prevent acceptance and usage by the field.

### 8.2 Broader Applicability

The work is designed for the system dynamics community but is **directly applicable to other fields** such as data science, machine learning, and automated intelligence. Future research aims to broaden the appeal of structural dominance analysis across disciplines.

---

## 9. Key Figures Summary

### Figure 1
Structure of the DELAY3 macro showing the complex set of pathways between arguments (input, delay time) and the output. Shows 3 stocks, 4 flows, and the much more indirect true path structure. Demonstrates that 2 apparent links on a diagram represent 7 distinct causal pathways.

### Figure 2
Three views of a weakly coupled system: (left) stock-and-flow diagram, (middle) full CLD showing all feedback relationships, (right) simplified CLD showing only the most important feedback -- which produces two disconnected subsystems because the weak coupling loop is filtered out.

### Figure 3
Simplified CLD of Forrester's (1968) market growth model. Left panel shows an over-simplified version with a gray link indicating indeterminate polarity (confidence < 0.99). Right panel shows the result of lowering the link inclusion threshold slightly, expanding the gray link into its constituent pathways.

### Figure 4
Simple population model with births only. Shows stock-and-flow diagram, exponential growth, and LTM panel reporting R1 with 100% contribution.

### Figure 5
Population model with births and deaths (average lifetime = 20). Shows R1 at +66.67% (current), B1 at -33.33% (current). Still exponential growth but slower.

### Figure 6
Highlighting the negative loop involving "deaths" on the SFD. Shows how the outflow loop (which lacks a visible arrowhead back to the stock) is harder for students to recognize.

### Figure 7
Population model with carrying capacity. Three loops: R1 (+50%), B1 (-36.17%), B2 (-13.83%). Shows shifting loop dominance as the capacity constraint loop becomes active.

### Figure 8
Machine-generated simplified CLD of Mass' 1975 Economic Cycles model (163 variables, 17 stocks, 494 feedback loops). 9 simplified loops explaining 59.7% of total behavior. Key variables: Vacancies, labor, Inventory, Backlog, Avg Prod Rate, Avg Unit Cost of Prod, Avg Price, Smoothed Avg Price, Perc Rate of Inc in Price.

### Figure 9
Composite relative loop scores over time (period 9-12.5) for the Economic Cycles model. Shows the oscillating dominance pattern with B1 dominant, transitioning through R1/B3 interference, B4's delayed effect, and B2's termination-driven dominance at inflection points.

### Figure 10
Key indicator stocks (Backlog, Inventory, Labor, Vacancies) for the Economic Cycles model. Left panel: full simulation period showing dampened oscillation. Right panel: analysis period (9-12.5) showing one complete cycle.

### Figure 11
Stock-and-flow diagram of the workforce training model with discrete elements: conveyor for Apprentices, non-negative stock for Workers, flows for hiring, finishing training, and leaving.

### Figure 12
Two simplified CLDs for the workforce training model under different parameterizations. Left (time to adjust = 5): two balancing loops. Right (time to adjust = 2): four loops including additional feedback from the active non-negative constraint. Demonstrates how parameterization changes the feedback complexity of discrete systems.

---

## 10. Concepts and Enhancements Covered in This Paper

| Concept | Definition |
|---------|-----------|
| **Path score** | Product of link scores along a path between two variables; equivalent to the link score that would exist if the path were a single direct link |
| **Composite link score** | Link score for macros: the path score of the dominant (largest magnitude) pathway through the macro at each time step |
| **LOOPSCORE builtin** | Model equation function allowing practitioners to specify an arbitrary loop and compute its relative loop score across the simulation |
| **PATHSCORE builtin** | Model equation function computing raw (non-relative) loop/path scores during simulation |
| **Link inclusion threshold** | Simplification metric introduced in LoopX: minimum variation in relative link score magnitude across the simulation for a variable to be included in a simplified CLD |
| **Loop inclusion threshold** | Simplification metric introduced in LoopX: minimum average percentage of behavior a loop must explain to have its stocks (and optionally flows) included in a simplified CLD |
| **Polarity confidence** (Equation 3) | `\|r - \|b\|\| / (r + \|b\|)` -- measures how consistently a simplified link has a single polarity; values of 0.99 or lower trigger gray (mixed polarity) display |
| **Composite relative loop score** | For simplified CLD loops: sum of relative loop scores from all full-model loops that reduce to that simplified loop |
| **Loop polarity labels** | Rx (reinforcing), Bx (balancing), Rux (unknown, predominantly reinforcing), Bux (unknown, predominantly balancing), Ux (unknown) |
| **Flow inclusion toggle** | Boolean controlling whether flows are automatically kept when a stock is included in a simplified CLD |
| **Strongest path algorithm** | Heuristic (Eberlein and Schoenberg, 2020) that dynamically identifies the most important loops at each time point, allowing the set of analyzed loops to change during simulation |
| **Perfect mixing approximation** | Treatment of discrete elements (conveyors, etc.) as if their eventual response to input changes were instantaneous, preserving polarity and magnitude while potentially distorting timing |

---

## 11. Implementation Reference

The method is implemented in the **Stella family of products** (Professional, Architect, and online tools), starting with Version 2.0. It is activated by a simple checkbox. The paper frames this implementation as a refinement and expansion of prior LoopX-style visualization concepts for production use on practitioner-built models of arbitrary complexity.

Key implementation requirements:
1. Ability to re-evaluate equations with ceteris paribus inputs (for link score computation).
2. Ability to enumerate and track all pathways through macros.
3. Strongest path algorithm for identifying important loops at each time step.
4. Two-way mapping between full-model loops and simplified-diagram loops.
5. Graph simplification and layout algorithms for automated CLD generation.
