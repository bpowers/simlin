# Arrayed / Cross-Element LTM Hardening Design

Date: 2026-05-11
Resolves: GH **#520** (unify the reference-site walkers behind one classification IR -- the structural root), **#487** (rel_loop_score cross-pollutes independent A2A loops), **#511** (subscripted-A2A-reference link-score partial fails to compile), **#483** (STDDEV/RANK ceteris-paribus partials fall back to delta-ratio), **#510** (degenerate link score for disjoint-dimension arrayed->arrayed edges with per-element target equations), **#514** (sliced reducer subexpressions not hoisted into aggregate nodes -- the last `:wildcard` path), **#515** (`MAX_AGG_PETALS=8` drops cross-element-through-aggregate loops; per-subset not per-ordering enumeration), **#502** (per-element graphical functions lose static link polarity), **#492** (GF strict-monotonicity epsilon flags numeric-import noise as Unknown).
Related: GH **#273** (LTM array support umbrella), **#488** (LTM epic), `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` (the cross-element aggregate-node work this builds on, merged as PR #519), `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` (the per-reference element-graph work that introduced `RefShape`).

## Summary

The LTM ("Loops That Matter") subsystem rewrites a system-dynamics model into an instrumented copy: it adds synthetic variables that compute, at each simulated timestep, how much every causal link contributed to its target's change (the *link score*) and how strongly each feedback loop drove behavior (the *loop score*, later normalized into a *relative loop score*). For arrayed (subscripted) variables this happens on an element-level causal graph so loops are found and scored per element. The scalar machinery follows the published LTM literature; the arrayed extension is Simlin-specific, has no published oracle, and is where the structural fragility and a cluster of silent-correctness bugs live.

This plan treats nine GitHub issues as one body of work. The strategy is **unify first, then layer the fixes on top of the unified representation**. Today three independent AST walkers separately decide a reference's access shape and whether it routes through an aggregate node -- one feeding the element graph, one deciding reducer hoisting, one emitting link scores -- plus a byte-identical filter duplicated across two files and a reducer-recognition set restated five times; "the element graph and the link scores agree" is a tested coincidence rather than a structural guarantee. **#520 is the anchor:** it introduces one salsa-tracked classification IR (`ClassifiedSite` per causal edge: access shape, per-element key, aggregate-node routing) that becomes the *only* place those decisions are made, and consolidates the reducer table into one definition. With that invariant made structural, the remaining eight issues become small, well-scoped changes against a single representation: partition-correct relative-loop-score normalization for independent apply-to-all loops (#487), two arrayed-reference shapes the old code could not represent (#511, #510), hoisting *sliced* reducer subexpressions into aggregate nodes and retiring the last conservative fallback path (#514), a budgeted-and-truncation-flagged recovery of cross-element-through-aggregate loops that also enumerates distinct cyclic orderings (#515), analytic ceteris-paribus partials for STDDEV/RANK instead of a delta-ratio fallback (#483), and static link polarity for per-element graphical functions plus a noise-tolerant monotonicity check (#502, #492). Phase 1 is verified byte-unchanged on existing golden data; every later phase's golden-data diff is investigated and documented. See "## Definition of Done" below for each issue's concrete deliverable.

## Definition of Done

One comprehensive design plan, with **#520 (the unified reference-site classification IR) as the foundational phase**, then the eight other issues layered on top -- ordered by dependency, with the stated priority (#487, then #511) as the tiebreaker among issues that are independent of #520.

**Primary deliverable:** The arrayed / cross-element LTM subsystem is structurally hardened and its known silent-correctness bugs are fixed. Concretely:

- **#520 (foundation):** A single salsa-tracked `ClassifiedSite`-style IR is the *only* place a reference site's access shape (`Bare`/`FixedIndex`/`Wildcard`/`DynamicIndex`) and aggregate-node routing are decided. Both `model_element_causal_edges` (`src/simlin-engine/src/db_analysis.rs`) and `model_ltm_variables` (`src/simlin-engine/src/db_ltm.rs`) consume it; neither re-walks the AST for shape/routing. `builtin_is_array_reducer` is removed (or reduced to a thin re-export of `enumerate_agg_nodes`'s recognition predicate). Behavior-preserving: reducer-bearing golden LTM data is byte-unchanged.
- **#487:** `partition_for_loop` resolves a real partition for pure-A2A loops; `compute_rel_loop_scores*` normalizes each disconnected A2A feedback subsystem against its own partition (not a shared fictitious bucket), so multi-A2A-loop models get correct relative loop scores.
- **#511:** An A2A equation referencing an arrayed dependency by the iterated dimension (`growth[D1,D2] = row_sum[D1] * c`) compiles with LTM enabled and gets a meaningful link score (the iterated-dimension subscript is recognized as a same-element/Bare reference) -- or, if genuinely unscoreable, a clear compile-time diagnostic instead of the internal "PREVIOUS requires a variable reference after helper rewriting" assertion.
- **#483:** STDDEV and RANK ceteris-paribus partials compute true element-by-element analytic partials (STDDEV via the unrolled variance formula holding the source element live; RANK by comparison against PREVIOUS of all other elements), not the delta-ratio fallback -- so e.g. under uniform scaling of all elements STDDEV's per-element link scores are ~0.
- **#510:** A disjoint-dimension arrayed->arrayed edge with per-element target equations gets a correct per-(source-elem, target-elem) link score -- or, if not meaningfully scoreable, a clear compile-time diagnostic -- not a silent scalarized stand-in.
- **#514:** Sliced reducer subexpressions (`SUM(pop[NYC,*])` used inside a larger expression) are hoisted into aggregate nodes whose descriptor carries the read slice's pinned elements / result axes; the element-graph reroute and per-element reducer link scores cover only the read slice. The last `:wildcard` link-score path for reducers is retired.
- **#515:** A reducer in a feedback loop over a >8-element dimension yields a budgeted/truncated set of cross-element-through-aggregate loops with a truncation flag (mirroring the element-graph auto-flip backstop) rather than zero; and the recovery enumerates distinct cyclic orderings (not just subsets) for k>=3 petals, within the loop budget.
- **#502:** Per-element graphical functions (`LOOKUP(curve[Region], dose)`) get static link polarity -- the Lookup arm of `analyze_link_polarity` handles `Expr2::Subscript` (FixedIndex resolves the element's table; Bare-A2A aggregates per-element table monotonicity; Wildcard/DynamicIndex stays conservatively `Unknown`).
- **#492:** `analyze_graphical_function_polarity` uses a y-range-relative epsilon (or counted-violations check) so lookup tables that are monotone modulo round-trip import noise retain their polarity; genuinely non-monotone tables still return `Unknown`.

**Success criteria:** All work follows TDD with new/updated tests; `cargo test --workspace` passes within the 3-minute cap and the pre-commit hook passes. #520 is verified byte-unchanged on reducer-bearing/scalar/A2A golden data; the bug-fix phases will change golden data for the affected model shapes -- every diff is investigated and documented. Docs updated (`src/simlin-engine/CLAUDE.md`, `docs/design/ltm--loops-that-matter.md`, the user-facing `docs/reference/ltm--loops-that-matter.md` with new behavior and any residual limitations, the relevant design-plan postscripts). GH issues #520/#487/#511/#483/#510/#514/#515/#502/#492 closed referencing implementing commits; epic #488's checklist ticked; `docs/tech-debt.md` items updated.

**Explicitly out of scope:** Already-resolved issues (#516, #517, #503, #480, #482, #448, #308 -- closed-as-completed, #519 merged) unless review surfaces a regression; and other open LTM epic items not in this list (#506, #497, #486, #466, #495, #507, #484, #481, #468, #464, #313, #311, #310, #309, #282, #505, #504).

## Acceptance Criteria

### ltm-arrays-hardening.AC1: #520 -- unified classification IR (behavior-preserving)

- **ltm-arrays-hardening.AC1.1 Success:** `model_ltm_reference_sites` is a salsa-tracked function returning, per `(from, to)` causal edge, a `Vec<ClassifiedSite>` (shape + target_element + routing); `model_element_causal_edges` and `model_ltm_variables` both read it and neither contains its own `Expr2` AST walk for reference shape nor its own `routed_aggs` filter -- the inline `route_through_agg = !routed_aggs.is_empty() && site.in_reducer` decision and the byte-identical `aggs_in_var(to).filter(...)` filter exist in exactly one place (the IR builder). Verified by the IR-driven `model_element_causal_edges` / `model_ltm_variables` passing the existing `db_element_graph_tests` and `db_ltm_*_tests` suites.
- **ltm-arrays-hardening.AC1.2 Success:** `builtin_is_array_reducer` no longer exists; the array-reducer set and its `Linear`/`Nonlinear`/`Constant` + `is_monotone` classification are defined in exactly one place (`reducer_kind` in `ltm_agg.rs`); `agg_reducer_is_monotone`, `ltm_augment::classify_reducer`, and `ltm_augment::is_array_reducer_name` are thin readers of it. Verified by a unit test exercising each `BuiltinFn` reducer variant (SUM, 1-arg MEAN, 2-arg MEAN, 1-arg MIN/MAX, 2-arg MIN/MAX, STDDEV, RANK, SIZE) through `reducer_kind`.
- **ltm-arrays-hardening.AC1.3 Success (behavior preserved):** every reducer-bearing, scalar, and pure-A2A golden LTM fixture (`logistic_growth_ltm`, `cross_element_ltm`, the WRLD3 LTM smoke, the partial-reduce model, and any others) produces byte-identical results before and after Phase 1; `cargo test --workspace` passes within the 3-minute cap.
- **ltm-arrays-hardening.AC1.4 Edge:** a reducer reference over a `StarRange` (`x[*..*]`) extent is classified consistently by the element graph and the link-score emitter (routed through the agg, with no separate Bare-named link score) -- the latent `RefShape`-vs-`expr_is_full_extent` disagreement is gone. Verified by a unit test (no current golden fixture exercises this).
- **ltm-arrays-hardening.AC1.5 Edge:** a `SIZE` reducer reference, and a reducer over a scalar source, classify as `Direct` (never `ThroughAgg`) -- `enumerate_agg_nodes` mints no agg for either, and the IR's routing reflects that.

### ltm-arrays-hardening.AC2: #487 -- partition-correct A2A loop normalization

- **ltm-arrays-hardening.AC2.1 Success:** a model with two disconnected A2A feedback subsystems over *different* dimensions (e.g. `pop[Region] -> births[Region] -> pop[Region]` and `widgets[Product] -> production[Product] -> widgets[Product]`) -- each loop's relative loop score normalizes against the sum of absolute loop scores within its own cycle partition, not a shared bucket; the two loops do not cross-normalize. Verified by a hand calculation matching each per-loop relative score and differing from the pre-fix pooled value.
- **ltm-arrays-hardening.AC2.2 Success:** an A2A loop over an N-element dimension has N entries in `loop_partitions` (one per slot); for a pure-A2A loop over an element-wise-uncoupled dimension those are N distinct partition indices; for an element-wise-coupled dimension they coincide; scalar and cross-element loops have exactly one entry.
- **ltm-arrays-hardening.AC2.3 Success:** `Loop::stocks` for an A2A loop is the element-level `{var}[{elem}]` list (not the variable-level name); the `Loop` struct's granularity docstring states this; `enrich_with_module_stocks`, JSON SDAI relationships, and layout feedback metadata still produce correct output for A2A loops.
- **ltm-arrays-hardening.AC2.4 Success (no regression):** a purely scalar model's loops and relative scores are unchanged; a model with a single A2A feedback subsystem has the same relative loop score it had before (the pre-fix single-`None`-loop value coincided with the correct partition-local value).
- **ltm-arrays-hardening.AC2.5 Success:** the FFI surface (`libsimlin`'s `loop_partitions` snapshot, the generated C header, `@simlin/engine`, `pysimlin`) exposes the per-slot partitions with a slot-0 convenience for callers that want a single value; a round trip through the FFI preserves them.

### ltm-arrays-hardening.AC3: #511 + #510 -- iterated-dimension subscripts; disjoint-dim arrayed->arrayed link scores

- **ltm-arrays-hardening.AC3.1 Success:** `growth[D1,D2] = row_sum[D1] * c` (with `row_sum` over `D1`, `growth` over `D1 x D2`) + LTM enabled compiles; the element graph has the same-element-on-shared-dims projection `row_sum[d1] -> growth[d1,*]` (not the full cross-product); `$\u{205A}ltm\u{205A}link_score\u{205A}row_sum\u{2192}growth` is emitted as the Bare partial (`row_sum` held live, no spurious `SUM(...)` source-ref, no `PREVIOUS`-wrapped `Subscript`) and simulates without the `"PREVIOUS requires a variable reference after helper rewriting"` error.
- **ltm-arrays-hardening.AC3.2 Success:** a feedback loop through such an edge (`... -> row_sum[D1] -> growth[D1,D2] -> ... -> row_sum[D1]`) is enumerated and its loop-score equation references `$\u{205A}ltm\u{205A}link_score\u{205A}row_sum\u{2192}growth`; the loop score matches a hand calculation at >= 1 timestep within 1e-6.
- **ltm-arrays-hardening.AC3.3 Success:** a disjoint-dim arrayed->arrayed model with per-element target equations (`target[D1,D2]` whose `<element subscript="...">` equations reference `source[D3]`, D3 disjoint from D1/D2) emits one link-score variable per distinct referenced source element (`$\u{205A}ltm\u{205A}link_score\u{205A}source[m]\u{2192}target`, ...), each an `Equation::Arrayed` over `D1 x D2` whose partial holds `source[m]` live in the slots that reference it and is the trivial-zero guard form elsewhere -- not a single scalar stand-in; `link_score_dimensions` returns `target`'s dims for this edge.
- **ltm-arrays-hardening.AC3.4 Failure:** an edge that is genuinely unscoreable this way (e.g. a `DynamicIndex` source into a disjoint-dim arrayed target) produces a clear compile-time `Warning` diagnostic naming the unscoreable edge, not a silent scalarized stand-in.
- **ltm-arrays-hardening.AC3.5 Edge:** a mapped-dimension iterated subscript (`x` over `Region`, target over `State`, a `State -> Region` mapping, `x[State]` referenced) is handled the same way a bare `x` reference into that target is handled today -- no new dimension-mapping behavior, no regression.

### ltm-arrays-hardening.AC4: #514 -- sliced-reducer hoisting; no Wildcard-cross-product path

- **ltm-arrays-hardening.AC4.1 Success:** `x[r] = ... + SUM(pop[NYC,*])` over `pop[Region,Age]` mints a synthetic agg `$\u{205A}ltm\u{205A}agg\u{205A}0` with equation `SUM(pop[NYC,*])` and `read_slice = [Pinned(nyc), Reduced]`; the element graph has `pop[nyc] -> $\u{205A}ltm\u{205A}agg\u{205A}0` (a single edge, not `pop[*] -> agg`) and `$\u{205A}ltm\u{205A}agg\u{205A}0 -> x[r]`; no `pop[d] -> x[e]` full-cross-product edges exist.
- **ltm-arrays-hardening.AC4.2 Success:** `x[D1] = ... + SUM(matrix[D1,*])` over `matrix[D1,D2]` mints an arrayed synthetic agg over D1 with `read_slice = [Iterated(D1), Reduced]` and `result_dims = [D1]`; the element graph has `matrix[d1,d2] -> agg[d1]` and `agg[d1] -> x[d1]` (the appropriate projection).
- **ltm-arrays-hardening.AC4.3 Success:** the per-element reducer link scores for a sliced agg cover only the read slice (`$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}agg` exists; the analogous link scores for `pop[la]`, `pop[chi]` do not); a cross-element feedback loop through a sliced agg is enumerated, scored from those per-slice link scores along the un-trimmed path, and the agg node is trimmed from the reported `Loop`.
- **ltm-arrays-hardening.AC4.4 Edge:** a reducer over a *dynamic* index (`SUM(pop[idx,*])`, `idx` a non-literal index) is *not* hoisted -- it stays on the conservative path; a unit test pins this narrow carve-out.
- **ltm-arrays-hardening.AC4.5 Success (no Wildcard path remains):** no model emits a `Wildcard`-shape full-cross-product element edge or a `Wildcard`-shape link-score variable; `link_score_var_name` has no `Wildcard` arm; `emit_edges_for_reference` has no `Wildcard`-cross-product arm.

### ltm-arrays-hardening.AC5: #515 -- budgeted cross-agg loop recovery; cyclic-ordering enumeration

- **ltm-arrays-hardening.AC5.1 Success:** a reducer in a feedback loop over a >8-element dimension (so >8 disjoint petals through one agg) recovers a non-empty, budgeted set of cross-element-through-agg loops (not zero, as today); the result carries a `TruncatedByBudget` flag (and a `CompilationDiagnostic` `Warning` is also emitted) so the model author knows the loop list is incomplete.
- **ltm-arrays-hardening.AC5.2 Success:** for a model with k = 3 disjoint petals through one agg, all distinct cyclic orderings are recovered as distinct directed cycles (`A->p1->A->p2->A->p3->A` and `A->p1->A->p3->A->p2->A` are both present) within the loop budget -- not just the single per-subset ordering today's `2^k`-bitmask enumeration produces.
- **ltm-arrays-hardening.AC5.3 Success (no regression):** a model with <= 8 petals and at most 2 petals per recovered loop produces the same cross-agg loops as before (the per-subset enumeration for m = 2 has exactly one cyclic ordering, which equals today's output).
- **ltm-arrays-hardening.AC5.4 Edge:** the recovered cyclic orderings of one petal subset share a loop score (the product over the same edge set); a test confirms their `loop_score` series are equal even though they are distinct loops by the directed-edge-sequence identity rule.

### ltm-arrays-hardening.AC6: #483 -- analytic STDDEV (and RANK) ceteris-paribus partials

- **ltm-arrays-hardening.AC6.1 Success:** for a model where `total = STDDEV(s[*])` feeds back into `s`, the per-element link score `$\u{205A}ltm\u{205A}link_score\u{205A}s[d]\u{2192}total` is the analytic ceteris-paribus partial (the unrolled variance formula holding `s[d]` live, the other elements at `PREVIOUS`), not `(total - PREVIOUS(total)) / (s[d] - PREVIOUS(s[d]))`.
- **ltm-arrays-hardening.AC6.2 Success:** under uniform scaling of all elements each step (all `s[i]` multiplied by the same factor), STDDEV does not change, so the per-element link scores `s[d] -> total` are ~0 at every step >= 1 -- the pre-fix delta-ratio reported spurious nonzero values.
- **ltm-arrays-hardening.AC6.3 Success:** the STDDEV link score matches a hand calculation at >= 1 timestep within 1e-6.
- **ltm-arrays-hardening.AC6.4 Edge:** RANK -- either the analytic per-element partial is implemented (a test pins it), or RANK keeps the delta-ratio with a documented in-code justification (RANK is an order statistic: non-differentiable, array-argument-only) and a test pins that explicit choice (so it is not a silent fallback).
- **ltm-arrays-hardening.AC6.5 Success (no regression):** SUM / MEAN / 1-arg MIN / 1-arg MAX per-element partials are unchanged (the existing `test_generate_*_equation` / `test_generate_reduced_*_equation` tests pass).

### ltm-arrays-hardening.AC7: #502 + #492 -- per-element graphical-function static polarity; plateau-tolerant GF monotonicity

- **ltm-arrays-hardening.AC7.1 Success:** a model with `effect[Region] = LOOKUP(curve[Region], dose[Region])` where every region's `curve` table is monotone increasing -- the link `dose[Region] -> effect[Region]` (and any loop through it) gets `Positive` static polarity (composed with the argument's polarity), not `Unknown`.
- **ltm-arrays-hardening.AC7.2 Success:** the same model where the regions' tables disagree on monotonicity (some increasing, some decreasing) -- the link polarity is `Unknown` (the per-element aggregation correctly reports disagreement).
- **ltm-arrays-hardening.AC7.3 Success:** `LOOKUP(curve[NYC], x)` (a `FixedIndex` per-element-GF reference) gets the static polarity of NYC's specific table (`tables[offset_of(nyc)]`).
- **ltm-arrays-hardening.AC7.4 Success:** a lookup table whose y-values are monotone non-decreasing modulo round-trip numeric noise (e.g. one segment dips by ~1e-7 against a unit-scale y-range) is classified `Positive`, not `Unknown` -- the y-range-relative epsilon tolerates it.
- **ltm-arrays-hardening.AC7.5 Failure / Edge:** a genuinely non-monotone table (a real direction change, not noise) still returns `Unknown`; a constant table still returns `Unknown`.

### ltm-arrays-hardening.AC8: Cross-cutting -- tests, golden data, docs, tracking

- **ltm-arrays-hardening.AC8.1:** `cargo test --workspace` passes within the 3-minute wall-clock cap and the pre-commit hook passes at the end of every phase.
- **ltm-arrays-hardening.AC8.2:** Phase 1's golden LTM data is byte-unchanged; every behavior-changing phase's golden-data diffs were investigated and the per-test reasoning recorded in the implementing commit message.
- **ltm-arrays-hardening.AC8.3:** `src/simlin-engine/CLAUDE.md`, `docs/design/ltm--loops-that-matter.md`, and the user-facing `docs/reference/ltm--loops-that-matter.md` reflect the new behavior and residual limitations (the dynamic-index-reducer carve-out, RANK's treatment, the cross-agg-recovery truncation, the iterated-dimension / mapped-dimension handling).
- **ltm-arrays-hardening.AC8.4:** GH issues #520, #487, #511, #510, #514, #515, #483, #502, #492 are closed referencing the implementing commits; epic #488's checklist is ticked for each; `docs/tech-debt.md` items #21, #27, #35 and the #520-related entries are updated.

## Glossary

- **LTM (Loops That Matter)**: A feedback-loop-dominance analysis method (Schoenberg, Davidsen & Eberlein 2020 and follow-ups) that quantifies, at each point in simulated time, how much each causal link and each feedback loop drives a model's behavior. Simlin implements it by adding synthetic "score" variables to the model; see `docs/design/ltm--loops-that-matter.md`.
- **link score**: Per-timestep, signed (roughly [-1, 1]) measure of how much a single causal link `x -> z` contributed to `z`'s change, computed ceteris paribus -- re-evaluate `z`'s equation with `x` at its current value and every other input frozen at its `PREVIOUS()` value. Materialized as a synthetic variable `$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}`.
- **loop score**: Per-timestep strength of a feedback loop, the product of the link scores around it. Materialized as `$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}`.
- **relative loop score**: A loop's score normalized against the sum of absolute loop scores within its *cycle partition*, so loops are only compared to structurally related loops. Computed post-simulation (`ltm_post.rs`), not synthesized.
- **cross-element loop**: A feedback loop through an arrayed variable that visits *different* elements at different points (`pop[nyc] -> mp[boston] -> mi[nyc] -> pop[nyc]`), as opposed to a loop that stays on one element index.
- **A2A / apply-to-all**: An arrayed equation with one formula evaluated for every element of its dimension(s) -- the `Equation::ApplyToAll` / `Ast::ApplyToAll` form. A "pure-A2A loop" like `pop[r] -> births[r] -> pop[r]` stays on a single element index `r`; it is one loop-score variable with per-element slots.
- **`Ast::Arrayed` vs `Ast::ApplyToAll` (and the matching `Equation::Arrayed` / `Equation::ApplyToAll`)**: Two representations of an arrayed equation. `ApplyToAll` is one shared formula over the dimension; `Arrayed` is the per-element form (XMILE `<element subscript="...">`) with a distinct expression per element key plus an optional default. A `ClassifiedSite`'s `target_element` is set when the reference sits in an `Ast::Arrayed` slot.
- **reducer / array reducer**: A builtin that aggregates over an array dimension to a smaller result -- `SUM`, `MEAN`, `STDDEV`, `MIN`/`MAX`, `RANK`, `SIZE`. Written inline in another variable's equation (`SUM(pop[*])`) it has no node of its own; this plan hoists it into one.
- **partial reduce / sliced reducer**: A reducer that collapses only some of a source's axes, leaving an arrayed result -- e.g. `SUM(matrix[D1,*])` reduces the second axis and yields an array over `D1`; `SUM(pop[NYC,*])` reads only the NYC row. Phase 4 (#514) hoists these into arrayed/per-slice aggregate nodes.
- **ceteris-paribus / partial change**: "All else equal." The LTM partial equation evaluates a target's formula with exactly one input varying and all others frozen at their previous values; "partial" here is the discrete analogue of a partial derivative. The **delta-ratio fallback** is the cruder stand-in `(target - PREVIOUS(target)) / (source - PREVIOUS(source))` used when an analytic per-element partial isn't implemented; #483 replaces it for STDDEV.
- **aggregate node / hoist**: This subsystem's central device -- treat a maximal inlined reducer subexpression as an implicit synthetic auxiliary (`$\u{205A}ltm\u{205A}agg\u{205A}{n}`) so causality routes `source[d] -> agg -> target`, both halves get real per-element link scores, and they compose by the chain rule. The agg node is *trimmed* from a loop when it is reported (`pop[d] -> agg -> share[e]` is shown as `pop[d] -> share[e]`), exactly as the LTM papers trim a macro's hidden internal nodes.
- **`$\u{205A}ltm\u{205A}...` synthetic-variable naming**: All LTM-generated variables use a `$` prefix (avoids collisions with user variables) and U+205A (`\u{205A}`, TWO DOT PUNCTUATION; a valid identifier character that essentially never appears in authored equations) as separator -- `$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}`, `$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}`, `$\u{205A}ltm\u{205A}agg\u{205A}{n}`. In generated equations these names are double-quoted and a `[elem]` subscript may follow.
- **`$\u{205A}ltm\u{205A}agg\u{205A}{n}` family**: The synthetic-auxiliary names minted for hoisted reducer subexpressions (`$\u{205A}ltm\u{205A}agg\u{205A}0`, `$\u{205A}ltm\u{205A}agg\u{205A}1`, ...); each carries the reducer subexpression as its equation. A variable whose entire equation is exactly one reducer call is its own aggregate node -- no synthetic minted.
- **`RefShape` (Bare / FixedIndex / Wildcard / DynamicIndex)**: Per-reference-site classification of how a source variable is accessed in a target's AST. `Bare` = a plain `Var` reference (scalar or same-element A2A); `FixedIndex` = literal subscripts like `x[NYC]` (broadcast edges); `Wildcard` = a reducer access like `x[*]`; `DynamicIndex` = a non-literal index (`@N`, ranges, arbitrary expression), handled conservatively. #520 makes this the IR's `ClassifiedSite.shape`; Phase 4 retires the `Wildcard` arms (every statically describable reducer is hoisted instead).
- **`StarRange` (`x[*..*]`)**: An explicit "whole-extent" range subscript -- semantically equivalent to a `Wildcard` reducer access. Today the old walkers disagree on it (`expr_is_full_extent` says full-extent, `classify_subscript_shape` says `DynamicIndex`); #520's single extent check resolves the disagreement (AC1.4 / risk R1).
- **`ClassifiedSite` IR / `model_ltm_reference_sites`**: The salsa-tracked classification function #520 introduces -- it walks each variable's AST once and returns, per `(from, to)` causal edge, a `Vec<ClassifiedSite>` (`shape` + `target_element` + `routing`, where `routing` is `Direct` or `ThroughAgg{agg}`). It becomes the only place a reference's access shape and aggregate-node routing are decided; `model_element_causal_edges` and `model_ltm_variables` become pure readers of it. `AggRef` indexes the aggregate-nodes result; `LtmReferenceSitesResult` is the wrapper struct.
- **`read_slice`**: A descriptor added to `AggNode` in Phase 4 -- one entry per source axis, each `Pinned(elem)` | `Iterated(dim)` | `Reduced` -- recording which rows a hoisted reducer actually reads (`SUM(pop[NYC,*])` -> `[Pinned(nyc), Reduced]`). It drives the element-graph edges and per-element link scores so they cover only the read slice, not a full cross-product. A whole-extent reducer is the all-`Reduced` case.
- **iterated-dimension subscript**: A subscript whose indices are exactly the target equation's iterated dimension names, in the matching positions -- e.g. `row_sum[D1]` referenced inside `growth[D1,D2] = ...`. #511 classifies this as `RefShape::Bare` (a same-element-on-shared-dims reference), the same projection `emit_edges_for_reference` already does for arrayed bare sources; today it misclassifies as `DynamicIndex` and the link score fails to compile.
- **mapped-dimension reference**: A reference like `x[State]` where `x` is over `Region` and a `State -> Region` dimension mapping exists. The plan handles it the same way a bare `x` reference into that target is handled today -- no new dimension-mapping behavior (AC3.5).
- **cycle partition / SCC**: A group of stocks connected by feedback -- a strongly connected component (SCC) of the stock-to-stock reachability graph. Relative loop scores are normalized within a partition (`partition_for_loop` keys a loop to its partition); module-internal and element-level stocks are folded in so partition assignment stays correct. #487 changes the normalization grouping key from per-partition to `(partition, slot)` so disconnected A2A subsystems over different dimensions stop cross-normalizing.
- **`loop_partitions`**: The cached loop-id -> cycle-partition map on the LTM-variables result, consumed post-simulation. #487 changes its value type from `Option<usize>` to `Vec<Option<usize>>` (one entry per A2A-loop slot; length 1 for scalar / cross-element loops); the FFI surface (`libsimlin`, the generated C header, `@simlin/engine`, `pysimlin`) exposes the per-slot vector with a slot-0 convenience.
- **tiered loop enumeration / fast path / slow path**: `model_loop_circuits_tiered` classifies cycles and routes pure-scalar / pure-A2A circuits down a cheap "fast path" and cross-element / mixed circuits down a "slow path" subgraph (which keeps the aggregate nodes in it for scoring). `build_loops_from_tiered` / `build_element_level_loops` materialize loops from this.
- **petal**: One agg-touching elementary circuit `agg -> ... -> agg`, rotated to start at the agg; the non-agg nodes are its *internal nodes*. Two petals are disjoint when their internal node sets don't overlap. Combining `k >= 2` disjoint petals of the same agg yields *non-elementary* loops that Johnson-style enumeration misses.
- **cross-agg loop recovery / `recover_cross_agg_loops`**: The pass that reconstructs those non-elementary cross-element-through-aggregate loops by combining disjoint petals. #515 replaces today's hard `MAX_AGG_PETALS = 8` drop with a deterministic petal priority ordering plus an overall loop-count *budget*, sets a `TruncatedByBudget` flag (and emits a `CompilationDiagnostic` `Warning`) when the budget is hit, and enumerates distinct *cyclic orderings* of each chosen petal subset (`(m-1)!/2` for m >= 3 via Heap's algorithm, mirror reversals skipped) instead of just one ordering per subset.
- **`TruncatedByBudget`**: A flag type in `ltm/types.rs` (currently defined but unused) that #515 wires into the loop-list result so a caller can tell the cross-agg recovery was incomplete -- robust against the known diagnostic-reachability limitation (#466, out of scope).
- **auto-flip / auto-flip-to-discovery gate**: The existing backstop that, when a model's SCC exceeds `MAX_LTM_SCC_NODES`, switches LTM from exhaustive enumeration to discovery mode and emits a `CompilationDiagnostic` `Warning`. #515's truncation signal mirrors this pattern rather than inventing a new mechanism.
- **discovery mode / strongest-path**: An alternative LTM mode for models too large for exhaustive loop enumeration -- link scores are emitted for *all* edges, and after simulation a heuristic strongest-path DFS over a per-timestep search graph (built by parsing link-score variable names in `ltm_finding.rs`, not by re-walking ASTs) finds the dominant loops. It is downstream of emitted link-score *names* only, so #520's refactor doesn't touch it; Phases 3/4 change which names are emitted, so the discovery parser is re-checked there.
- **graphical function (GF) / lookup table**: A piecewise-linear curve `y = f(x)` given as a table of `(x, y)` points, evaluated via `LOOKUP`. A *per-element* graphical function stores one `Table` per element (in element order), so `curve[Region]` selects per region. **Static link polarity** is the compile-time sign (`Positive`/`Negative`/`Unknown`) of a causal link; #502 makes the `LOOKUP` arm of `analyze_link_polarity` handle subscripted GF references; #492 replaces an absolute `dy` epsilon in the monotonicity check with a y-range-relative one so tables that are monotone modulo round-trip numeric-import noise keep their polarity.
- **reducer table / `reducer_kind` / `ReducerKind`**: The single consolidated classification #520 introduces in `ltm_agg.rs` -- maps each `BuiltinFn` reducer to `Linear` (SUM, 1-arg MEAN), `Nonlinear` (1-arg MIN/MAX, STDDEV, RANK), or `Constant` (SIZE; link score always 0), with an `is_monotone` predicate. It replaces `builtin_is_array_reducer` and four other restatements of the reducer set.
- **golden data / golden fixture**: Checked-in expected simulation outputs for test models; "byte-unchanged golden data" is the acceptance bar for the behavior-preserving Phase 1, and every later phase's golden diff must be investigated and explained in the implementing commit.
- **salsa (incremental compilation)**: The incremental-computation framework Simlin's compiler is built on; LTM analysis is a set of "tracked functions" (`model_ltm_variables`, `model_element_causal_edges`, `enumerate_agg_nodes`, the new `model_ltm_reference_sites`, ...) that re-run only when their inputs change and must be deterministic pure functions of `(db, model, project)`.
- **Heap's algorithm**: A standard algorithm for generating all permutations of a sequence; #515 uses it to enumerate the cyclic orderings of a petal subset (with the first petal pinned to kill rotations and mirror reversals skipped).
- **Schoenberg, Hayward & Eberlein (2023), "Improving Loops that Matter"**: The paper whose corrected relative-loop-score formula the scalar LTM core tracks. The arrayed extension in this document has no analogous published reference, which is why its correctness is established by construction and golden tests rather than against an oracle.

## Architecture

The LTM (Loops That Matter) subsystem instruments a model with synthetic auxiliary variables that compute, per simulated timestep, each causal link's contribution to its target's change (the link score) and each feedback loop's strength (the loop score, normalized post-simulation into a relative loop score). For arrayed (subscripted) variables this is done on an element-level causal graph so loops are found and scored at element granularity. The scalar core tracks Schoenberg/Hayward/Eberlein 2023; the arrayed extension is a Simlin-specific elaboration with no published oracle, and that is where the structural fragility and the open silent-correctness bugs concentrate.

This design treats nine issues as one cluster, anchored on the structural root. **#520** unifies three independent `Expr2` AST traversals -- `collect_reference_sites` (`db_analysis.rs`, feeds the element graph), `enumerate_agg_nodes` (`ltm_agg.rs`, decides reducer hoisting), and the link-score emitters in `db_ltm.rs` -- plus a byte-identical `routed_aggs` filter duplicated in two files and a reducer-recognition set restated five times, behind one salsa-tracked classification IR. The "the element graph and the link scores agree" property becomes structural rather than a tested coincidence. The eight other fixes layer on top, ordered by dependency with the stated priority (#487, then #511) as tiebreaker among issues independent of #520: **#487** (rel-loop-score partition cross-pollution), **#511** (iterated-dimension subscripts), **#510** (disjoint-dim arrayed->arrayed link scores), **#514** (sliced-reducer hoisting), **#515** (cross-agg loop recovery cap), **#483** (STDDEV/RANK ceteris-paribus partials), **#502** + **#492** (per-element graphical-function polarity, GF-monotonicity epsilon).

### Component map

Current locations from codebase investigation (`src/simlin-engine/src/` unless noted):

- `db_analysis.rs` -- `model_element_causal_edges` (element causal graph), `model_loop_circuits_tiered` / `classify_cycle` / `model_edge_shapes` (tiered loop enumeration), `model_element_cycle_partitions` (element-level SCCs), `collect_reference_sites` / `collect_in_expr` / `classify_subscript_shape` / `resolve_literal_index` / `builtin_is_array_reducer` (the walker #520 absorbs), `RefShape`, `emit_edges_for_reference`.
- `ltm_agg.rs` -- `enumerate_agg_nodes` (the hoisting decider; stays), `AggNode` / `AggNodesResult`, `reducer_source_vars` / `reducer_is_full_reduce` / `expr_is_full_extent` (the hoist predicates), `agg_reducer_is_monotone`, `synthetic_agg_name` / `is_synthetic_agg_name`.
- `db_ltm.rs` -- `model_ltm_variables` (the unified LTM-variable entry point), `emit_per_shape_link_scores` / `emit_link_scores_for_edge` / `emit_source_to_agg_link_scores` / `emit_agg_to_target_link_scores` (link-score emission; consume the IR after #520), `try_cross_dimensional_link_scores` / `try_scalar_to_arrayed_link_scores` / `link_score_dimensions` / `retarget_ltm_equation_dims` / `scalarize_ltm_equation` (#510), `build_loops_from_tiered` / `build_element_level_loops` (loop building; #487), `recover_cross_agg_loops` / `MAX_AGG_PETALS` (#515), `recover_agg_hop_polarities`.
- `ltm_augment.rs` -- `build_partial_equation_shaped` / `wrap_non_matching_in_previous` / `classify_expr0_subscript_shape` (ceteris-paribus partials; #511), `classify_reducer` / `ReducerKind` / `generate_element_to_scalar_equation` / `generate_element_to_reduced_equation` / `generate_nonlinear_partial` / `generate_linear_partial` / `build_element_reducer_link_score` (reducer partials; #483), `link_score_var_name` / `resolve_link_score_name_for_loop` / `substitute_reducers_in_expr0`, `is_array_reducer_name`, `build_arrayed_link_score_equation` / `source_ref_for_guard` / `shape_aware_source_ref` (#510).
- `ltm/polarity.rs` -- `analyze_link_polarity` (the `Lookup` arm; #502), `analyze_graphical_function_polarity` (the epsilon; #492), `analyze_expr_polarity_with_context`.
- `ltm/types.rs` -- `Loop` / `Link` / `LinkPolarity` / `LoopPolarity` / `TruncatedByBudget` (defined, currently unused).
- `ltm/partitions.rs` -- `CyclePartitions` / `partition_for_loop` (#487).
- `ltm/graph.rs` -- `CausalGraph` / `find_stocks_in_loop` / `circuit_to_links` / `enrich_with_module_stocks` / `assign_loop_ids`.
- `ltm_post.rs` -- `compute_rel_loop_scores` / `compute_rel_loop_scores_per_element` (#487 downstream).
- `ltm_finding.rs` -- `parse_link_offsets` / `expand_a2a_link_offsets` / `expand_fixed_from_a2a_link_offsets` / `discover_loops_with_graph` / `rank_and_filter` (discovery; downstream of emitted *names* only, not the walkers, so #520 does not touch it; #487's per-slot treatment applies to `rank_and_filter`).
- `db.rs` -- `LtmSyntheticVar`, `LtmVariablesResult` (the `loop_partitions` type changes in #487).
- `variable.rs` -- `Variable::Var.tables` / `build_tables` (per-element graphical functions stored one `Table` per element, in element order; #502).
- `compiler/codegen.rs` -- the `BuiltinFn::Previous` arm (the `"PREVIOUS requires a variable reference after helper rewriting"` site #511 trips today; not modified, but no longer reached).
- Cross-component (#487): `libsimlin/src/lib.rs` / `analysis.rs` / `simulation.rs`, the generated C header, `@simlin/engine` types, `pysimlin`, `src/diagram`, `layout/mod.rs` -- consumers of `loop_partitions`.

### The #520 classification IR (contract)

A new salsa-tracked function -- in a small new module (`src/simlin-engine/src/db_ltm_ir.rs`) or folded next to `enumerate_agg_nodes` in `ltm_agg.rs` -- consumes `enumerate_agg_nodes` (which stays the sole decider of "is this subexpression a hoistable maximal reducer") plus `reconstruct_model_variables`, walks each variable's `Expr2` AST once with the established canonical-sorted / left-to-right-DFS discipline, and buckets the results by `(from, to)` causal edge:

```rust
// model_ltm_reference_sites(db, model, project) -> LtmReferenceSitesResult
struct LtmReferenceSitesResult {
    // every (from-var, to-var) causal edge that has at least one AST reference:
    sites: HashMap<(Ident<Canonical>, Ident<Canonical>), Vec<ClassifiedSite>>,
}

struct ClassifiedSite {
    shape: RefShape,                            // Bare / FixedIndex(elems) / Wildcard / DynamicIndex
    target_element: Option<Ident<Canonical>>,   // per-element key when the ref is in an Ast::Arrayed slot
    routing: SiteRouting,
}

enum SiteRouting {
    Direct,                                     // element graph: per-`shape` edges via emit_edges_for_reference;
                                                // link score: the per-`shape` link score
    ThroughAgg { agg: AggRef },                 // element graph: from[..] -> agg.name + agg.name -> to[e];
                                                // link score: the agg's two halves; no per-`shape` link score here
}
// AggRef indexes into AggNodesResult.aggs (carrying name, is_synthetic, source_vars,
// result_dims; gains read_slice in Phase 4). Deduped across sites that share an agg.
```

`model_element_causal_edges` and `model_ltm_variables` become pure readers of this IR. The inline `route_through_agg = !routed_aggs.is_empty() && site.in_reducer` decision and the byte-identical `routed_aggs` filter (`agg_nodes.aggs_in_var(to).filter(|a| a.is_synthetic && a.source_vars.iter().any(|s| s == from))`) in both files are deleted -- routing is read from `site.routing`. `model_edge_shapes` becomes a projection of the IR (collect each edge's `shape` set) or is absorbed. The interface is per-edge; the computation walks each variable once and buckets by source, which is strictly cheaper than today's per-edge re-walk.

### The consolidated reducer table (contract)

One `reducer_kind(&BuiltinFn<_>) -> Option<ReducerKind>` in `ltm_agg.rs`:

```rust
enum ReducerKind {
    Linear,     // SUM, 1-arg MEAN -- algebraically simple per-element partial
    Nonlinear,  // 1-arg MIN / 1-arg MAX / STDDEV / RANK -- explicit element-by-element unroll
    Constant,   // SIZE -- output depends only on dimension cardinality; link score is always 0
}
impl ReducerKind { fn is_monotone(self) -> bool { /* Linear or 1-arg MIN/MAX */ } }
```

`reducer_source_vars` keeps its source-extraction job but defers recognition to `reducer_kind`; `builtin_is_array_reducer` is deleted (its callers read `reducer_kind`); `agg_reducer_is_monotone` becomes `reducer_kind(...).is_some_and(ReducerKind::is_monotone)`; `ltm_augment::classify_reducer` reads `reducer_kind`; `ltm_augment::is_array_reducer_name` becomes a thin name->builtin lookup keeping its arity rules. The five restatements collapse to one. `model_ltm_reference_sites` uses one extent check, resolving the latent `RefShape`-vs-`expr_is_full_extent` `StarRange` asymmetry.

### Data flow

```
model variables --> enumerate_agg_nodes (hoisting decider; gains AggNode.read_slice in #514)
                         |
                         +--> model_ltm_reference_sites  (THE classification IR: shape + target_element + routing)
                         |              |
        +----------------+              |  (the same Direct / ThroughAgg routing drives both consumers identically)
        v                               v
model_element_causal_edges        model_ltm_variables
  Direct      -> emit_edges_for_ref       Direct      -> per-shape link score (build_partial_equation_shaped, ...)
  ThroughAgg  -> from[..]->agg             ThroughAgg  -> source[d]->agg + agg->to[e] (reducer link-score machinery)
                + agg->to[e]               + agg auxes ($\u{205A}ltm\u{205A}agg\u{205A}n) + loop_score vars
        |                                  |
        v                                  v
model_loop_circuits_tiered  -->  build_loops_from_tiered / build_element_level_loops
  fast: pure scalar / pure-A2A      A2A loops: element-level Loop::stocks (#487); cross-element loops:
  slow: cross-element / mixed       per-slot subscripted link-score refs; agg nodes trimmed when reported
        (agg nodes kept in          + recover_cross_agg_loops (budgeted; cyclic-ordering enumeration; #515)
         the slow-path subgraph)    + recover_agg_hop_polarities
        |                                  |
        +------------> Loop list <----------+
                            |
              exhaustive: loop_score var values; rel-loop-scores grouped per (partition, slot) in ltm_post (#487)
              discovery:  parse_link_offsets (emitted names only) -> SearchGraph -> strongest-path -> FoundLoop
```

## Existing Patterns

This design extends established LTM patterns; it introduces no new architectural style.

- **Salsa-tracked classification functions already exist.** `enumerate_agg_nodes`, `model_edge_shapes`, `model_element_causal_edges`, `model_loop_circuits_tiered` are deterministic pure functions of `(db, model, project)`, visiting variables in canonical-sorted order with left-to-right DFS over reconstructed `Expr2` ASTs. `model_ltm_reference_sites` (#520) follows the same discipline; salsa's per-variable parse caching keeps re-walks cheap, and the IR additionally collapses today's per-edge re-walk into one per-variable walk.
- **Aggregate nodes are already the treatment for hidden internal structure.** The 2020.1 LTM paper handles `DELAY3` / `SMOOTH` by scoring loops through the macro's hidden stocks/flows and trimming them when reporting; the #519 work (`docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`, merged) applied this to array reducers via `$\u{205A}ltm\u{205A}agg\u{205A}{n}`. #514 extends `AggNode` to carry a read-slice descriptor; the routing/trimming machinery is unchanged in shape.
- **`RefShape` per-reference classification is already first-class** (`docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`). #520 makes it the IR's `ClassifiedSite.shape` and removes the parallel re-derivations. #511 classifies an iterated-dimension subscript as `Bare` -- exactly the same-element-on-shared-dims projection (`expand_same_element`) `emit_edges_for_reference` already does for arrayed `Bare` sources.
- **Element-subscripted `Link.from` / `Link.to` strings** already encode cross-dimensional and cross-element edges (`"pop[nyc]"`, `"mp[boston]"`). #487's element-level `Loop::stocks` for A2A loops uses the same `{var}[{elem}]` convention; the cross-element and mixed branches of `build_element_level_loops` already populate element-level stocks.
- **Truncation is already surfaced.** The auto-flip-to-discovery gate (`db_ltm.rs`, variable-level SCC over `MAX_LTM_SCC_NODES`) emits a `CompilationDiagnostic` `Warning`. #515 surfaces the cross-agg-recovery budget via the loop-list result (`ltm::TruncatedByBudget`, currently defined but unused) and/or the same diagnostic pattern -- it does not invent a new mechanism.
- **Per-element aggregation in `analyze_link_polarity`** -- the `Ast::Arrayed` arm already folds per-element polarities (adopt-first-concrete; disagree -> `Unknown`). #502 reuses this for the Bare-A2A per-element-graphical-function case.
- **Post-simulation relative loop scoring is already partition-grouped** -- `compute_rel_loop_scores` / `compute_rel_loop_scores_per_element` normalize within cycle partitions. #487 changes the grouping key from per-loop-partition to `(partition, slot)`.

Divergence from the literature: array reducers are not discussed in any LTM paper; treating an inlined reducer (whole-extent or sliced) as an aggregate node is a Simlin-specific extension, justified as the natural application of the published macro treatment. #515's per-cyclic-ordering enumeration produces distinct directed cycles that share a loop score (the score is a commutative product over the loop's edge set) -- consistent with #308's rule that loop identity is the directed edge sequence, not the node set.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: #520 -- unified reference-site classification IR + reducer-predicate consolidation

**Goal:** One source of truth for per-reference access shape and aggregate-node routing; the "element graph and link scores agree" invariant becomes structural; the reducer-recognition set lives in one place. Behavior-preserving.

**Components:**
- New module (`src/simlin-engine/src/db_ltm_ir.rs`, or folded next to `enumerate_agg_nodes` in `ltm_agg.rs`) -- salsa-tracked `model_ltm_reference_sites(db, model, project) -> LtmReferenceSitesResult` (the `ClassifiedSite` / `SiteRouting` / `AggRef` IR; consumes `enumerate_agg_nodes` + `reconstruct_model_variables`; walks each variable's `Expr2` AST once, buckets by `(from, to)`). `collect_reference_sites` / `collect_in_expr` / `classify_subscript_shape` / `resolve_literal_index` become internal helpers of the IR (or move into the new module).
- `db_analysis.rs` -- `model_element_causal_edges` reads the IR (`Direct` -> `emit_edges_for_reference`; `ThroughAgg` -> `from[..]->agg` + `agg->to[e]`); the inline `route_through_agg` / `routed_aggs` logic is deleted. `builtin_is_array_reducer` is deleted. `model_edge_shapes` becomes a projection of the IR (or is absorbed).
- `ltm_agg.rs` -- new `reducer_kind` / `ReducerKind` / `is_monotone` table; `reducer_source_vars` defers recognition to it; `agg_reducer_is_monotone` becomes a reader; `enumerate_agg_nodes` unchanged in behavior (still the hoisting decider; its extent check aligns with the IR's, fixing the `StarRange` asymmetry). `synthetic_agg_name` / `is_synthetic_agg_name` unchanged.
- `db_ltm.rs` -- `emit_per_shape_link_scores` / `emit_link_scores_for_edge` read the IR for shape + routing (the body that builds per-shape link scores is kept, driven by the IR rather than re-walking via `enumerate_shapes` / `collect_reference_shapes`); the byte-identical `routed_aggs` filter is deleted.
- `ltm_augment.rs` -- `classify_reducer` and `is_array_reducer_name` become thin readers of `reducer_kind`.

**Dependencies:** None (first phase).

**Done when:** ACs `ltm-arrays-hardening.AC1.*` are satisfied -- `model_ltm_reference_sites` is the only place shape + routing is decided; `model_element_causal_edges` and `model_ltm_variables` neither re-walk the AST for shape/routing nor restate the `routed_aggs` filter; `builtin_is_array_reducer` is gone and the reducer set + its monotone/Linear/Nonlinear/Constant classification live in one table; the cross-checking tests (`ref_site_*`, `*_reducer_is_not_hoisted`, `db_element_graph_tests`) are kept and pass as IR regression guards; `cargo test --workspace` passes within the cap; every reducer-bearing, scalar, and pure-A2A golden LTM fixture is byte-unchanged.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: #487 -- element-level A2A `Loop::stocks`; partition-correct per-slot relative-loop-score normalization

**Goal:** A2A loops resolve real cycle partitions (per slot), so disconnected A2A feedback subsystems stop cross-normalizing in the relative-loop-score computation.

**Components:**
- `ltm/types.rs` -- `Loop::stocks` is element-level (`{var}[{elem}]` over the loop's `dimensions`' element space) whenever the loop touches arrayed vars; the struct's granularity docstring is updated. `dimensions` unchanged.
- `db_ltm.rs` -- `build_loops_from_tiered` / `build_element_level_loops`: the fast-path A2A branch and the pure-dimension branch populate `Loop::stocks` with element-level names (the cross-element / mixed branches already do); `model_ltm_variables` builds `loop_partitions` as `HashMap<String, Vec<Option<usize>>>` (length 1 for scalar / cross-element loops, length N for an A2A loop over an N-element dim space) via the updated `partition_for_loop`.
- `ltm/partitions.rs` -- `partition_for_loop` returns one partition per slot for an A2A loop (group the loop's element-level stocks by their A2A-dim element tuple; the `debug_assert!` intra-slot-consistency check becomes meaningful) and a singleton for scalar / cross-element loops; signature / result type updated.
- `db.rs` -- `LtmVariablesResult.loop_partitions` type changes `HashMap<String, Option<usize>>` -> `HashMap<String, Vec<Option<usize>>>`.
- `ltm_post.rs` -- `compute_rel_loop_scores` / `compute_rel_loop_scores_per_element` group loops by `(partition_index, slot)`; a scalar loop broadcasts its slot-0 partition; the `None` cohort remains only for loops genuinely below the parent graph.
- `ltm_finding.rs` -- `rank_and_filter`'s `MIN_CONTRIBUTION` partition-scoped test uses the per-slot partition.
- Consumers audited for the `loop_partitions` type change and the `Loop::stocks` granularity change: `libsimlin/src/lib.rs` (`SimlinAnalysisState` snapshot), `libsimlin/src/analysis.rs`, `libsimlin/src/simulation.rs`, the generated C header + `@simlin/engine` + `pysimlin` (additive: a per-slot accessor; keep a slot-0 convenience), `src/diagram`, `layout/mod.rs`, JSON SDAI relationships, `enrich_with_module_stocks` (keeps producing element-level names for A2A loops, namespacing module-internal stocks consistently).

**Dependencies:** None (independent of #520).

**Done when:** ACs `ltm-arrays-hardening.AC2.*` pass -- a model with two disconnected A2A feedback subsystems over different dimensions has each loop's relative score normalized within its own cycle partition (not pooled); an A2A loop over an uncoupled dim resolves a per-slot partition (one per element); the `Loop` granularity docstring is updated; FFI / layout consumers compile and behave; `cargo test --workspace` green; rel-loop-score golden diffs on multi-A2A-loop models investigated and documented.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: #511 + #510 -- iterated-dimension subscripts; disjoint-dimension arrayed->arrayed link scores

**Goal:** Two arrayed-reference shapes the pre-#520 code could not represent (so they degenerate -- #511 to a hard compile error or garbage, #510 to a silent scalar stand-in) become correct, via the unified IR plus small emitter extensions.

**Components:**
- `model_ltm_reference_sites` (the #520 IR) -- a subscript whose indices are exactly the target's iterated dimension names (each in the position matching the source's dimension) classifies as `RefShape::Bare`. (Today `classify_subscript_shape` runs each index through `resolve_literal_index`, which fails on a dimension name and falls to `DynamicIndex`.)
- `ltm_augment.rs` -- `build_partial_equation_shaped` / `wrap_non_matching_in_previous` / `classify_expr0_subscript_shape` receive the target's iterated-dimension names and normalize iterated-dimension subscripts on source references to bare form in the partial-equation `Expr0` before `PREVIOUS`-wrapping (the live source is then held live; the model equation is untouched, so simulation evaluates `row_sum[d1]` correctly). `build_arrayed_link_score_equation` handles a `FixedIndex` source into an `Ast::Arrayed` target per slot (it already does via `source_ref_for_guard`; this confirms / extends it).
- `db_ltm.rs` -- `try_cross_dimensional_link_scores` stops returning `None` for arrayed targets when source elements are referenced; `link_score_dimensions` returns the target's own dims (not empty) for the disjoint-dim arrayed-target-with-per-element-equations case; one link-score variable is emitted per distinct referenced source element (`$\u{205A}ltm\u{205A}link_score\u{205A}source[m]\u{2192}target`, ...), each an `Equation::Arrayed` over the target's dims with the partial holding `source[m]` live in slots that reference it and the trivial-zero guard form elsewhere; a clear compile-time `Warning` diagnostic is emitted when the edge is genuinely unscoreable (e.g. a `DynamicIndex` source into a disjoint-dim arrayed target) instead of a silent scalarized stand-in.
- `compiler/codegen.rs` -- unchanged; the `BuiltinFn::Previous` arm no longer receives a `Subscript` argument from LTM partials.
- `tests/simulate_ltm.rs` -- `build_partial_reduce_model` uses the subscripted `row_sum[D1]` form (its "deliberately uses bare references to sidestep #511" comment removed).

**Dependencies:** Phase 1 (the IR; the iterated-dimension classification lives there).

**Done when:** ACs `ltm-arrays-hardening.AC3.*` pass -- `growth[D1,D2] = row_sum[D1] * c` + LTM compiles, the element graph has the same-element projection (not the full cross-product), and `$\u{205A}ltm\u{205A}link_score\u{205A}row_sum\u{2192}growth` is the meaningful Bare partial that simulates without the `"PREVIOUS requires a variable reference"` error; a disjoint-dim arrayed->arrayed model with per-element target equations gets per-source-element arrayed link scores (or a clear diagnostic when unscoreable), not a scalar stand-in; `cargo test --workspace` green; golden diffs documented.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: #514 -- sliced-reducer hoisting (per-slice aggregate nodes); retire the last Wildcard-cross-product path

**Goal:** Every statically-describable inlined reducer -- whole-extent or sliced -- is hoisted into an aggregate node; the conservative full-cross-product element graph and lumped link score for a sliced reducer subexpression are gone.

**Components:**
- `ltm_agg.rs` -- `AggNode` gains a `read_slice` descriptor (per source axis: `Pinned(elem)` | `Iterated(dim)` | `Reduced`); `result_dims` for a synthetic agg is the `Iterated` axes' dims (was always empty -- partial-reduce subexpressions become arrayed aggs). `walk_subexpr_for_aggs` hoists a reducer whenever every non-reduced axis is `Pinned` or `Iterated` (statically describable); a reducer over a dynamic index (`SUM(pop[idx,*])`) stays unhoisted -- a narrow, documented carve-out far smaller than today's. `reducer_is_full_reduce` / `expr_is_full_extent` become "compute the read slice; full-extent is the all-`Reduced` case".
- `model_ltm_reference_sites` / `model_element_causal_edges` -- `ThroughAgg { agg }` for a sliced agg emits `source[<pinned>,<iterated>,<reduced->representative>] -> agg[<iterated>]` and `agg[<iterated>] -> target[e]`, only the read rows (the `emit_edges_for_reference` Bare / FixedIndex projection generalizes; the agg's `result_dims` drive the fan-out). The `Wildcard`-shape full-cross-product arm of `emit_edges_for_reference` and the `Wildcard` case of `link_score_var_name` are removed (dead -- every statically-describable reducer is hoisted).
- `db_ltm.rs` -- `emit_source_to_agg_link_scores` runs the reducer-link-score machinery (`classify_reducer` / `generate_element_to_scalar_equation` / `generate_element_to_reduced_equation`) iterating only the read slice's elements; `link_score_dimensions` for a sliced agg edge returns the `Iterated` dims; the agg-aux emission (`$\u{205A}ltm\u{205A}agg\u{205A}{n}`, equation = the reducer subexpr) handles arrayed result dims. `recover_cross_agg_loops` / loop-reporting trimming unchanged in shape.
- `ltm_agg.rs` tests -- `slice_reducer_subexpression_is_not_hoisted` replaced by tests asserting it *is* hoisted with the right `read_slice`.

**Dependencies:** Phase 1 (the IR; `AggNode` is what `AggRef` points to); Phase 3 (the per-element-arrayed-target handling and the iterated-dimension classification -- a sliced agg over an iterated dim reuses both).

**Done when:** ACs `ltm-arrays-hardening.AC4.*` pass -- `x[r] = ... + SUM(pop[NYC,*])` mints a synthetic agg with `read_slice = [Pinned(nyc), Reduced]`, the element graph has `pop[nyc] -> agg` (not `pop[*] -> agg`) and `agg -> x[r]`, and the per-element link scores cover only the NYC row; `x[D1] = ... + SUM(matrix[D1,*])` mints an arrayed agg over D1; a cross-element loop through a sliced agg is scored from the per-slice link scores; no Wildcard-cross-product element edges or Wildcard-shape link-score machinery remain; `cargo test --workspace` green; golden diffs (slice-reducer fixtures: full-cross-product -> per-slice) documented.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: #515 -- budgeted/truncated cross-agg loop recovery; cyclic-ordering enumeration

**Goal:** A reducer in a feedback loop over a large dimension gets *some* cross-element-through-aggregate loops (with a truncation signal) instead of zero, and the recovery enumerates distinct cyclic orderings, not just subsets.

**Components:**
- `db_ltm.rs` -- `recover_cross_agg_loops`: the hard `petals.len() > MAX_AGG_PETALS -> continue` drop is replaced by a deterministic petal priority ordering (by fan-in / internal-node count) plus an overall loop-count budget; when the budget is hit, recovery stops and the result is flagged truncated. The `2^k`-bitmask-of-subsets enumeration is replaced by distinct-cyclic-ordering enumeration: for each chosen petal subset of size m >= 2, all cyclic orderings (Heap's algorithm over the subset, first petal fixed to kill rotations, mirror reversals skipped -- `(m-1)!/2` for m >= 3, `1` for m = 2), under the same budget. `MAX_AGG_PETALS` is repurposed / renamed / supplemented as the loop-count budget.
- `ltm/types.rs` -- `TruncatedByBudget` (currently defined but unused) wired into the loop-list result so a caller can see the recovery was incomplete (robust against the #466 diagnostic-reachability limitation, which is out of scope); a `CompilationDiagnostic` `Warning` is also emitted, mirroring the auto-flip-to-discovery pattern.

**Dependencies:** Phase 4 (interacts -- sliced aggs can also sit in feedback loops; safe before Phase 4 too, ordered after for the combined test surface).

**Done when:** ACs `ltm-arrays-hardening.AC5.*` pass -- a reducer-in-a-feedback-loop over a >8-element dimension recovers a budgeted, truncation-flagged set of cross-agg loops (not zero); a k=3-petal fixture recovers all distinct cyclic orderings within the budget; a k<=2 / <=8-petal model is unchanged from before; `cargo test --workspace` green; golden diffs documented.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: #483 -- analytic STDDEV (and RANK if cleanly expressible) ceteris-paribus partials

**Goal:** STDDEV per-element link scores are the true ceteris-paribus partial, not a delta-ratio fallback.

**Components:**
- `ltm_augment.rs` -- `generate_nonlinear_partial`'s STDDEV/RANK catch-all (currently `target_q.to_string()`, a delta-ratio) is replaced. STDDEV element `d`: `sqrt((sum_i (s_i' - m)^2) / N)` with `s_i' = s[d]` if `i == d` else `PREVIOUS(s[i])` and `m = (sum_i s_i') / N` -- built from the `all_elements` / `current_element` / `source_q` the function already receives; pure arithmetic + `sqrt`. RANK is assessed in implementation: if expressible as a per-element scalar formula it is unrolled; otherwise RANK keeps the delta-ratio with a documented justification in code (RANK is an order statistic -- non-differentiable, array-argument-only) and a test pins that explicit choice. The `_ if !is_bare` nested-reducer delta-ratio fallback in `build_element_reducer_link_score` is unchanged (out of #483's scope; and after Phase 4 the STDDEV agg->target link score is always the `is_bare = true` path).

**Dependencies:** Phase 4 (the `$\u{205A}ltm\u{205A}agg\u{205A}{n}` -> target link score is where reducer partials are generated for hoisted reducers, so the fix lands where it is needed).

**Done when:** ACs `ltm-arrays-hardening.AC6.*` pass -- under uniform scaling of all elements, STDDEV's per-element link scores are ~0 at every step >= 1 (not the spurious nonzero delta-ratio); a hand-calculated STDDEV link score matches at >= 1 timestep within 1e-6; SUM / MEAN / MIN / MAX partials are unchanged; RANK's treatment (analytic or documented delta-ratio) is pinned by a test; `cargo test --workspace` green; golden diffs documented.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: #502 + #492 -- per-element graphical-function static polarity; plateau-tolerant GF monotonicity

**Goal:** Loops through per-element graphical functions get static polarity instead of `Undetermined`; lookup tables that are monotone modulo round-trip numeric noise keep their polarity.

**Components:**
- `ltm/polarity.rs` -- `analyze_link_polarity`'s `Lookup` arm extends `match table_expr` to handle `Expr2::Subscript(name, indices, ..)`: a `FixedIndex` element resolves to an offset and analyzes `vars[name].tables[offset]`; a Bare-A2A reference (`curve[D]` inside `effect[D] = LOOKUP(curve[D], ..)`) aggregates per-element table monotonicity (reusing the `Ast::Arrayed` per-element accumulation pattern -- agree -> that polarity, disagree -> `Unknown`); `Wildcard` / `DynamicIndex` -> `Unknown`. (`build_tables` already stores one `Table` per element, in element order.)
- `ltm/polarity.rs` -- `analyze_graphical_function_polarity` replaces `const EPSILON = 1e-10` (an absolute tolerance on `dy`) with a y-range-relative one: `epsilon = (1e-6 * (y_max - y_min)).max(1e-12)`; the rest of the monotonicity logic is unchanged. (The non-uniform-x-spacing concern -- `dy` vs slope `dy/dx` -- is adjacent but out of #492's scope; noted for separate tracking.)

**Dependencies:** None (independent).

**Done when:** ACs `ltm-arrays-hardening.AC7.*` pass -- a model with a per-element graphical function has loops through it classified `r`/`b` (not `u`/`Undetermined`) when the per-element tables agree on monotonicity, and `Unknown` when they disagree; a `FixedIndex` per-element-GF reference gets the polarity of that element's table; a lookup table monotone modulo round-trip numeric noise retains its polarity; a genuinely non-monotone table and a constant table still return `Unknown`; `cargo test --workspace` green.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Cleanup, docs, and tracking

**Goal:** Remove obviated code; bring documentation current; close the issues.

**Components:**
- Remove dead code surfaced by Phases 1 / 4 (the `Wildcard`-cross-product arms, the deleted predicates' former call sites, any vestigial per-shape-suffix handling, unused legacy walker entry points).
- Remove or rewrite tests pinned to old behavior (`slice_reducer_subexpression_is_not_hoisted`, any `Wildcard`-suffix-era tests, the `build_partial_reduce_model` sidestep comment).
- `src/simlin-engine/CLAUDE.md` -- document `model_ltm_reference_sites` (the IR), the consolidated `reducer_kind` table, `AggNode.read_slice`, element-level A2A `Loop::stocks`, the `loop_partitions` type change, the cross-agg-recovery truncation flag, the retired Wildcard path.
- `docs/design/ltm--loops-that-matter.md` and the user-facing `docs/reference/ltm--loops-that-matter.md` -- new behavior plus residual limitations (the dynamic-index-reducer carve-out, RANK's treatment, the >budget cross-agg-loop truncation, the iterated-dimension / mapped-dimension handling).
- `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` measurement postscript and a note in `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` -- re-measure SCC / loop counts on reducer-bearing fixtures.
- Close GH #520 / #487 / #511 / #510 / #514 / #515 / #483 / #502 / #492 referencing the implementing commits; tick epic #488's checklist; update `docs/tech-debt.md` items #21 / #27 / #35 and the #520-related entries.

**Dependencies:** Phase 7 (all functional work complete).

**Done when:** ACs `ltm-arrays-hardening.AC8.*` are satisfied -- no dead code; `CLAUDE.md` / `docs/design/` / `docs/reference/` current; GH issues closed and epic #488 ticked; `cargo test --workspace` green within the cap; the pre-commit hook passes.
<!-- END_PHASE_8 -->

## Additional Considerations

**Error handling / degenerate inputs.** An agg node's equation is the reducer subexpression parsed normally; if it fails to lower (it should not -- it was a valid subexpression), the existing graceful-degradation path applies (link / loop scores referencing it get the fragment compiler's zero-contribution stub-dep fallback). A reducer over a scalar source is not hoisted (`reducer_source_vars` requires an arrayed source). A genuinely-unscoreable disjoint-dim arrayed->arrayed edge (#510) emits a `Warning`, not a silent scalar.

**PREVIOUS / INIT snapshots.** `$\u{205A}ltm\u{205A}agg\u{205A}{n}` is an ordinary auxiliary with no init equation -- a pure function of its source(s) -- so the existing dependency sort places it before its consumers and the `prev_values` snapshot makes `PREVIOUS(agg)` available. Sliced / arrayed aggs (#514) are no different. The #517 fix (`wrap_non_matching_in_previous` wrapping a whole reducer in `PREVIOUS` rather than recursing into it) is upstream of all of this and untouched.

**Layout / per-element allocation.** A link score whose equation is `Equation::Arrayed` (per-element-equation targets, #510) or `Equation::ApplyToAll` (A2A) must be allocated per-element by the layout / results allocator the same way an arrayed variable is; `parse_link_offsets`'s `expand_*` helpers are layout-driven (base offset + element index) and independent of which arrayed-equation variant produced the var.

**Per-element-equation target with a different reducer per element** (`x[a] = SUM(p[*]); x[b] = MEAN(p[*])`). `model_ltm_reference_sites` and `enumerate_agg_nodes` both walk `Ast::Arrayed`'s per-element map, so each slot's reference and reducer are classified independently. In scope; covered by tests.

**Discovery mode is downstream of emitted link-score *names*, not the walkers.** `ltm_finding.rs` parses `results.offsets` for `$\u{205A}ltm\u{205A}link_score\u{205A}...` names; it never calls a reference-site walker. #520's refactor does not touch it. Phases 3 / 4 change *which* link-score names are emitted (the disjoint-dim per-source-element names; retiring Wildcard-cross-product link scores in favor of agg halves), so the discovery parser and its tests are checked in those phases. #487's per-slot partition treatment applies to `rank_and_filter`'s `MIN_CONTRIBUTION` test.

**Out of scope.** Already-resolved issues (#516, #517, #503, #480, #482, #448, #308 -- closed-as-completed, #519 merged) unless review surfaces a regression; other open LTM epic items not in this cluster (#506, #497, #486, #466, #495, #507, #484, #481, #468, #464, #313, #311, #310, #309, #282, #505, #504); general refactors of `build_element_level_loops` beyond what #487 / #514 require; dimension-mapping completeness beyond treating mapped-dimension references the way bare references to mapped-dimension sources are handled today; the non-uniform-x-spacing GF-monotonicity concern (slope vs raw delta), tracked separately.

**Risks.**
- **R1** -- #520 is not 100% behavior-preserving on the `[*..*]` / `StarRange` corner: it incidentally fixes a latent walker disagreement (`expr_is_full_extent` says full-extent, `classify_subscript_shape` says `DynamicIndex`). Confirm no current test or golden fixture exercises a `StarRange` reducer reference; if one does, it is a real bug being fixed -- document it.
- **R2** -- #487's FFI / layout blast radius: the `loop_partitions: HashMap<String, Option<usize>>` -> `HashMap<String, Vec<Option<usize>>>` change ripples to `libsimlin`, the generated C header, `@simlin/engine`, `pysimlin`, `src/diagram`, `layout/mod.rs`. Treat the FFI change as additive where possible (a per-slot accessor; keep a slot-0 convenience). This is the phase most likely to need splitting if the FFI surface proves large.
- **R3** -- #515's cyclic-ordering enumeration adds distinct directed cycles that share a loop score (the score is a commutative product over the loop's edge set), bloating the loop list. Intentional per the "fix both fully" decision; flagged.
- **R4** -- RANK in #483 may not be cleanly expressible as a per-element scalar formula; the plan allows a documented delta-ratio fallback for RANK specifically, pinned by a test so the choice is explicit.
- **R5** -- Golden-data churn volume: Phases 2 / 3 / 4 / 5 / 6 / 7 each change golden data for some model shape. The discipline -- run with no updates first, investigate every diff, document the per-test reasoning in the implementing commit -- applies per phase; an unexpectedly large diff is a signal to re-examine the fix, not to bulk-accept.
- **R6** -- A per-element-equation target with per-element reducers must classify each slot independently; the IR and `enumerate_agg_nodes` already walk `Ast::Arrayed`'s map, and tests cover it.

**Implementation scoping.** Eight phases -- at the `writing-implementation-plans` limit, not over. If Phase 2 (#487) proves larger than expected because of the FFI ripple (R2), it may be split into "engine-side `Loop::stocks` + `loop_partitions` + post-sim" and "FFI / TS / pysimlin / diagram surface", pushing the total to nine and requiring a second implementation plan -- decide at implementation-planning time.
