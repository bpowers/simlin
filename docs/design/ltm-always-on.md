# Always-on Loops That Matter: correctness and cost constraints

The objective is to make useful feedback analysis available without asking a
modeler to select an instrumentation mode. This requires faithful scoring,
bounded interactive work, and results that distinguish an absent signal from
missing evidence. It does not require enumerating every loop before a run can
finish.

The product priorities are both fast interactive edits in the browser and
complete, trustworthy explanations for large models. Treat these as two
latencies: returning the updated simulation and completing its analysis.
Background execution can separate them, but cannot replace the second
requirement with a permanently sampled answer. Preserve resumable analysis
state for a run that the user wants to finish, and keep partial results clearly
identified until all relevant completeness conditions hold.

This is a proposed direction and a set of acceptance constraints, not a
description of implemented behavior. The [implementation guide](ltm--loops-that-matter.md)
describes the current engine. The [single-mode plan](../design-plans/2026-09-07-ltm-single-mode.md)
and [fragment-synthesis plan](../design-plans/2026-09-04-link-scores-from-fragments.md)
contain candidate implementation packages; their optional product decisions
need separate justification from consolidating score computation.

## The semantic boundary

The instantaneous score uses the finite counterfactual
`f(x_current, other_inputs_previous) - target_previous`, with the zero-change
guards in Eq. 1 of [Schoenberg et al. (2020)](../reference/papers/schoenberg2020-loops-that-matter.pdf).
The flow-to-stock score uses the change in the flow relative to the change in
the stock's net flow, following [Schoenberg et al. (2023)](../reference/papers/schoenberg2023-improving-loops-that-matter.pdf).
These are finite differences. Replacing them with derivatives or automatic
differentiation changes the method on nonlinear and discontinuous equations.

Zero-change guards must not apply an absolute numerical tolerance: changing
the units of a stock must not erase a dimensionless score. Identify the initial
evaluation by simulation phase rather than approximate equality between the
clock and the start time. Test both variable-magnitude scaling and time scaling.

A counterfactual input is a read with an element selection and a temporal
meaning, not merely a variable name or a contiguous slot range. A changing
`PREVIOUS(g)` co-input must be held at its previous *evaluated* value;
`INIT(g)` is an initial snapshot. The clock is an input too. An already-lagged
load is therefore not automatically frozen. Selected lagged sources need an
explicit temporal policy: grouping current and past occurrences under one
source name can make the current source delta disagree in sign with the
delta of the value actually read.

Counterfactual tests must also distinguish source occurrences that resolve to
the same element through different reference shapes. Refactoring an equation
into a named reducer or expanding an array into scalar variables must have a
specified effect on the causal graph. A named reducer and an inline reducer
represented as a real analysis node can share elementary-cycle semantics;
contracting that node and inventing cycles through repeated visits is a
different graph transformation, not just presentation.

Even correct finite counterfactuals need careful interpretation. For
`z = x*y`, moving `(x,y)` from `(2,3)` to `(4,5)` gives changed-first partials
6 and 4 against a total change of 14. Their shares sum to 5/7 because the
remaining interaction is not assigned to either first-change partial. An
incoming-link normalization conceals that residual by construction. Likewise,
a loop alone in its partition has relative magnitude 1 whenever active even
if exogenous forcing accounts for most of the target's motion. Relative loop
importance compares the measured feedback in that partition; it is not a
general decomposition of all observed change into additive causal percentages.

At equilibrium the published zero-change rule produces zero scores. Preserve
the distinction between structural feedback and observed active feedback;
deleting structural polarity is not a prerequisite for one runtime scoring
pipeline. A future perturbation or sensitivity view should identify itself as
a different analysis, rather than silently changing the LTM result.

## One analysis attached to one run

Use one post-simulation loop-scoring owner over recorded link evidence for
every consumer. Bind that evidence, loop identity, partitions, names,
simulation specifications and parameter overrides to the same immutable model
revision. A later edit must not relabel an old score column with a newly
numbered loop. Layout, Python, TypeScript, MCP and the CLI should consume this
same run analysis instead of simulating independently or selecting different
loop populations.

Separate the analysis into three lifetimes:

1. **Model structure:** dependency/read provenance, admitted score programs,
   causal graph, cycle partitions and stable circuit identity. Cache these
   across parameter sweeps where their dependencies are unchanged.
2. **Run evidence:** observed values needed for finite counterfactuals, compact
   link-score series and activity bitsets. Recompute after a trajectory change.
3. **Explanation:** discovered cycles, normalization, ranking, display limits
   and dominant periods. Reuse evidence when only the display request changes.

An active graph from one run is not a safe replacement for the structural
graph of another run. A parameter change can activate a previously unused
branch or reverse a link's sign.

## Completeness has several independent meanings

`enumeration_complete` describes whether search exhausted its admitted
runtime graph. It cannot establish that all model dependencies were scored,
that every dt was retained, or that every discovered loop is displayed.
Carry these facts alongside the result:

| Dimension | Required distinction |
|---|---|
| Scoring | Exact admitted partial, documented approximation, or declined edge with reason |
| Time | Every dt or the saved-step sample, with the actual sampling interval |
| Search | Complete universe, partial enumeration, or sampled candidates |
| Numerics | Finite normalized value, zero-change guard, or unresolved nonfinite computation |
| Presentation | Full report or omitted loops, with per-step unreported mass where known |
| Provenance | Model revision, parameters, integration method and run identity |

For a complete universe, let `c(t)` be the sum of absolute relative scores of
the displayed loops in a partition. Omitted mass is `1-c(t)` at an active
finite step. Reporting that mass keeps a short explanation honest without
requiring the user to inspect every loop. If the universe is incomplete,
normalizing its candidates only produces shares *within that candidate set*;
the true missing mass is unknown.

A named dominant set must reach the paper's 50% threshold against the stated
denominator. Keeping the strongest individual loop at each step is insufficient:
shares `+0.4, -0.3, -0.3` have a balancing majority, while the report
`+0.4, -0.3` does not establish a dominant set. A gap in dominant periods can
mean insufficient reported evidence, not just inactive feedback.

## Numerical stability belongs in the shared scorer

Raw loop products and sums of absolute scores can overflow even when every
input is finite. Saturating a denominator at `f64::MAX` is not normalized
arithmetic: two raw scores of `1e308` receive shares of about 0.556 each.
Allowing an intermediate product to underflow can also erase a loop whose
later factors would restore a representable product.

Use a signed, scaled representation for products and group sums, or an
equivalent log-magnitude representation, with explicit treatment of zero and
nonfinite input. Normalize before converting to ordinary `f64` shares.
Evaluate both numerical error and throughput before choosing the
representation. This must be one policy across retention, relative link and
loop scores, module composites, and runtime polarity. A local clamp in one
consumer cannot recover information another stage already discarded.

## Remove redundant work before changing the metric

The most direct optimization is to reuse compiled target expressions under
the validated live-read selection, with per-link memoization. The current
whole-model synthetic-equation collection causes unrelated link programs to
be revisited after an edit. A bytecode or typed-IR transformation can avoid
printing, reparsing and retaining duplicate syntax trees. Preserve read
provenance until this transformation no longer needs it. Pure arithmetic can
be reused; clock, snapshots, dynamic views and effectful operations need
explicit rules rather than an unrestricted opcode copy.

Fuse the link-score guard and share source/target deltas where profiling
justifies it. This reduces repeated scaffolding; it does not eliminate the
counterfactual target evaluation itself. Analytic fast paths for affine
expressions are possible, but their finite-difference semantics and floating
point behavior must be tested against the general path.

Separate VM working slots from saved results. Partial values, scratch space,
capture helpers and intermediate pathway products need not all become full
history columns. Retain the evidence the chosen analysis needs, with one
owner for its storage, rather than copying the complete simulation slab into
another layout for discovery.

For loop-only analysis, a scored edge can be omitted if it cannot lie on a
directed cycle. Apply this proof on the graph with the necessary module,
aggregate and temporal-state detail. A parent module outside a parent-level
cycle may still contain internal feedback. Ordinary exogenous inputs must
still execute in the simulation and be available to partials even when their
link scores are not collected. Scoring every causal link remains a separate
capability for link-oriented explanations.

A streaming pass can evaluate analysis once the relevant dt state is final,
before history is discarded. A deferred pass can parallelize independent
steps only when it retains that same evidence. With `save_step > dt`, adjacent
public result rows do not supply the previous dt state, and lagged co-inputs
can require more history still. Measure the retention cost and native/browser
behavior before selecting either approach. Avoid unnecessary analysis work
at intermediate Runge-Kutta trial states when only final-state scores are used.

## Bounded discovery

Activity-bitset pruning is exact for the scored sampled graph: a loop's
product can be nonzero only where all of its edges are active. This is a
useful extension beyond heuristic path search when enumeration completes.
It does not remove the combinatorial cost of elementary cycles on arbitrary
dense graphs.

A responsive scheduler needs a budget for graph preparation, search, scoring,
materialization and reporting, not just candidate generation. Make work
resumable and distribute it across independent SCCs so a dense early component
does not starve later ones. Keep partial normalization visibly conditional.

In the browser, perform this work outside the interaction thread and publish
results tagged with the run identity. An edit can supersede foreground demand
for an older run without attaching its unfinished explanation to the new run.
For a requested complete analysis, a short scheduling slice pauses and resumes
work; it does not discard the search frontier. A resource limit that prevents
completion must be reported as such. Exact large-model explanations need
complete denominators and polarity totals before presentation caps, together
with an expandable account of omitted loop mass.

Deleting a heuristic fallback is a separate decision from consolidating score
computation. Compare equal-budget candidate quality against partial exact
enumeration, including renamed nodes, multiple SCCs and late-important cycles.
A low recall figure for a heuristic does not prove that a traversal prefix has
better recall. Similarly, a determinant, spectral statistic, or sum of closed
walks is not automatically the sum of elementary-loop scores; any such
acceleration needs a proof of equivalence or a separately named metric.

## Acceptance evidence

Use tests that begin with production model parsing and dependency extraction.
Derive expected results independently from the equations, and enumerate the
temporal-read, shape, reducer and backend arms under test. Goldens and
VM/WASM agreement are regression evidence; two implementations can share a
wrong premise.

Maintain scalar-expansion comparisons for arrays and representation twins for
split/net flows and named/inline reducers. Verify ordinary simulated values
are unchanged by the overlay, and exercise sparse save steps, all integration
methods, inactive periods, editing after a run and parameter-dependent branch
changes. Test budget exhaustion with tiny configurable limits.

Measure cold compilation, warm edits, simulation, discovery, result extraction
and peak/retained memory separately. Native measurements do not establish
browser latency or WASM memory behavior. The full user journey must establish
that enabling analysis neither blocks ordinary simulation nor silently reports
unsupported analysis as an absence of feedback.
