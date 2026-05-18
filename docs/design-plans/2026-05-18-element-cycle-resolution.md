# Element-Level Cycle Resolution Design

## Summary

Simlin's compiler rejects models that contain a dependency cycle. That check
operates at **whole-variable granularity**: every element of an arrayed
variable is collapsed into a single graph node. This is too coarse for a
common, legitimate pattern — a forward recurrence such as
`x[tNext] = x[tPrev] + 1`, where each array element depends only on an
*earlier* element of the same (or another) variable. The native MDL converter
already expands these shift-mapped subranges to concrete per-element equations
(`x[t1]=1; x[t2]=x[t1]+1; x[t3]=x[t2]+1`), so the element-level graph is
well-founded and acyclic, but the whole-variable gate sees a self-loop (or a
multi-variable SCC) and falsely reports `CircularDependency`, marking the model
`NotSimulatable`. This is the single fatal compile blocker for the C-LEARN
reference model (one 22-node dt SCC plus a self-loop), and it also blocks
several smaller corpus fixtures.

The approach keeps the fast whole-variable gate's acyclic happy path
completely unchanged and only does extra work when it actually finds a
back-edge. At that point it identifies the offending SCC and *refines just
that SCC* into an exact `(variable, element-offset)` graph. The refinement is
deliberately a faithful refinement of the engine's existing dt dependency
relation (`dt_walk_successors`), built from the engine's own
already-lowered per-element expressions via the existing
`lower_var_fragment` bridge — not a parallel re-derivation — so dt-phase
semantics (PREVIOUS/lagged reads excluded, stock edges broken) are inherited
"by construction" and the acyclicity verdict is sound by construction. A
promoted iterative Tarjan primitive (already present, previously test-only)
decides element-acyclicity: acyclic and every member element-sourceable means
emit a per-element topological run order and resolve; anything else keeps the
conservative `CircularDependency` (loud-safe fallback). Genuine cycles
(`a=b+1;b=a+1`, `x[dimA]=x[dimA]+1`) stay element-cyclic and remain rejected
by construction, preserving the CLAUDE.md hard rule.

This particular shape is forced by two facts. First, the *whole-variable vs.
element-level* distinction is the crux: the cheap collapsed gate cannot
distinguish a real cycle from a well-founded recurrence, so the design layers
an exact element graph on top only for SCCs the cheap gate flags, rather than
making the primary gate element-granular (which would slow every compile).
Second, single-variable self-recurrences already lower correctly inside their
one existing fragment (declared-order iteration), but a *multi-variable*
recurrence SCC (C-LEARN's emissions/target cluster) needs its members
evaluated in interleaved per-element order across variables — inexpressible
with today's one-contiguous-`AssignCurr`-block-per-variable lowering. So the
design adds a **combined-fragment lowering path**: the SCC's members'
per-element `AssignCurr` slots are interleaved, in the computed element order,
into a single synthetic `PerVarBytecodes` injected at the SCC's runlist slot,
with variable layout offsets and the results map left unchanged
(per-variable series stay individually addressable). The mechanism serves
both the dt (flows) and init (initials) phases, since C-LEARN reports a cycle
in each; the init relation differs only in that stocks do not break the chain
during initialization. Finally, the bundled VECTOR ELM MAP / VECTOR SORT
ORDER corrections are a **separate, purely numeric concern**: they are not
part of cycle resolution (they don't intersect C-LEARN's cycle) but lie on
C-LEARN's *numeric* path, so they are fixed to genuine Vensim semantics, the
Simlin-authored fixtures that encoded the bugs are rewritten to true Vensim
values, and the genuine-Vensim `vector.xmile` is un-excluded as a regression
gate.

## Definition of Done

**Primary deliverable** — General element-level cycle resolution: when the
whole-variable dt/init cycle gate detects an SCC, expand just that SCC to an
exact per-element graph (reusing existing element causal-graph machinery + a
promoted Tarjan primitive), test element-acyclicity, and emit a per-element
run order — including a **new combined-fragment lowering path** so
multi-variable recurrence SCCs (C-LEARN's ~22-node emissions/target cluster)
evaluate in interleaved per-element order. In-SCC synthetic helpers (synthetic
INIT, PREVIOUS/SMOOTH, macro-expansion helpers with no source equation) are
sourced from their parent variable's implicit-vars; if any in-SCC node
genuinely cannot be element-sourced, a loud-safe fallback keeps the
`CircularDependency` rejection. Genuine cycles (`a=b+1;b=a+1`,
`x[dimA]=x[dimA]+1`) remain rejected (CLAUDE.md hard rule). PLUS: VECTOR ELM
MAP / VECTOR SORT ORDER fixed to genuine Vensim semantics, the Simlin-authored
fixtures that encode the bugs rewritten to genuine Vensim values, and
`test/sdeverywhere/models/vector/vector.xmile` un-excluded as a regression
gate.

**Success criteria**

1. Explicit mid-plan structural gate: C-LEARN
   (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) compiles via the
   incremental path and runs to FINAL TIME via the VM with no panic and no
   NaN in core series; #363 (incremental-compiler panic on C-LEARN)
   re-verified once the cycle gate no longer masks the deeper pipeline, and
   the residual test-only `catch_unwind`
   (`tests/ltm_discovery_large_models.rs:670`) retired.
2. `simulates_clearn` is un-`#[ignore]`d and passes (numeric match vs
   `test/xmutil_test_models/Ref.vdf` within the existing 1% cross-simulator
   tolerance via `ensure_vdf_results`), and `ensure_vdf_results` is hardened
   with match-coverage and NaN guards so a near-empty match or all-NaN
   comparison cannot vacuously pass.
3. The now-element-acyclic fixtures transition from "asserts
   `CircularDependency`" to correctness: `self_recurrence.mdl`, `ref.mdl`,
   and `interleaved.mdl` simulate to correct values.
4. All work is test-driven against the project coverage standard; the engine
   test suite stays green under the 3-minute wall-clock cap; fixes are
   general engine fixes with no model-specific hacks.

**Out of scope** — non-fatal unit-inference warnings on C-LEARN; VECTOR
SELECT cross-dimension patterns beyond what C-LEARN / `vector.xmile`
exercise; Vensim macro support (already complete).

## Acceptance Criteria

### element-cycle-resolution.AC1: Single-variable self-recurrence resolves
- **element-cycle-resolution.AC1.1 Success:** `test/sdeverywhere/models/self_recurrence/self_recurrence.mdl` compiles via the incremental path with no `CircularDependency`.
- **element-cycle-resolution.AC1.2 Success:** It simulates to the well-founded series `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` over its steps.
- **element-cycle-resolution.AC1.3 Success:** `scc_components` is callable from production code (`pub(crate)`, not `#[cfg(test)]`) and the whole-variable happy path (acyclic models) is unaffected.
- **element-cycle-resolution.AC1.4 Edge:** The emitted per-element run order is byte-identical across repeated compiles of the same model.
- **element-cycle-resolution.AC1.5 Failure:** A single-variable SCC whose induced element graph is genuinely cyclic (`x[dimA]=x[dimA]+1`) is NOT resolved — it still reports `CircularDependency`.

### element-cycle-resolution.AC2: Multi-variable recurrence SCC resolves
- **element-cycle-resolution.AC2.1 Success:** `test/sdeverywhere/models/ref/ref.mdl` compiles and simulates to its hand-computed per-element series.
- **element-cycle-resolution.AC2.2 Success:** `test/sdeverywhere/models/interleaved/interleaved.mdl` compiles and simulates to its hand-computed values.
- **element-cycle-resolution.AC2.3 Success:** The SCC is lowered as one combined fragment; variable offsets and the results offset map are unchanged vs. a hypothetical acyclic equivalent (per-variable result series remain individually addressable).
- **element-cycle-resolution.AC2.4 Success:** Both dt-phase and init-phase recurrence SCCs are resolved (a fixture with an init-phase recurrence simulates correctly).
- **element-cycle-resolution.AC2.5 Edge:** `ref_interleaved_inter_variable_cycles_report_circular` is transitioned to assert correct simulation (not `CircularDependency`).
- **element-cycle-resolution.AC2.6 Failure:** `incremental_compilation_covers_all_models` and the existing model corpus stay green (no regression on non-recurrence models).

### element-cycle-resolution.AC3: Synthetic-helper policy
- **element-cycle-resolution.AC3.1 Success:** A well-founded recurrence whose SCC includes a synthetic helper (e.g. an INIT/PREVIOUS-helper-bearing recurrence) compiles and simulates when the helper is sourceable from its parent's `implicit_vars`.
- **element-cycle-resolution.AC3.2 Failure:** An SCC with an in-cycle node that genuinely cannot be element-sourced falls back to `CircularDependency` — no panic, no silent miscompile.
- **element-cycle-resolution.AC3.3 Edge:** The `#[cfg(test)]` `array_producing_vars` accessor keeps its abort-on-no-`SourceVariable` contract (its test still passes unchanged).

### element-cycle-resolution.AC4: Genuine cycles still rejected (hard rule)
- **element-cycle-resolution.AC4.1 Failure:** `a=b+1; b=a+1` reports `CircularDependency` (scalar 2-cycle).
- **element-cycle-resolution.AC4.2 Failure:** `x[dimA]=x[dimA]+1` reports `CircularDependency` (genuine same-element self-cycle).
- **element-cycle-resolution.AC4.3 Success:** `genuine_cycles_still_rejected` passes unchanged; the `dt_cycle_sccs_engine_consistent` harness passes against the new invariant (element-acyclic ⇒ resolved/no diagnostic; element-cyclic ⇒ `CircularDependency`).

### element-cycle-resolution.AC5: VECTOR SORT ORDER genuine Vensim semantics
- **element-cycle-resolution.AC5.1 Success:** `VECTOR SORT ORDER([2100,2010,2020], ascending)` yields the 0-based permutation `[1,2,0]` (matching genuine Vensim).
- **element-cycle-resolution.AC5.2 Success:** Descending direction yields the correct 0-based permutation; ties preserve stable order consistent with genuine Vensim.
- **element-cycle-resolution.AC5.3 Edge:** The Simlin-authored fixtures that encoded 1-based output (`array_tests.rs` cases, `vector_simple.dat` `l`/`m`) are corrected to genuine Vensim and pass.

### element-cycle-resolution.AC6: VECTOR ELM MAP genuine Vensim semantics
- **element-cycle-resolution.AC6.1 Success:** Result element `i` equals `source[(i + offset[i]) mod n]` — per-element base applied, wraparound via `rem_euclid`.
- **element-cycle-resolution.AC6.2 Success:** A cross-dimension source (`d[DimA,B1]` with offset reaching the `B2` column) resolves against the full source dimension, matching genuine Vensim `vector.dat`.
- **element-cycle-resolution.AC6.3 Success:** `test/sdeverywhere/models/vector/vector.xmile` is un-excluded and simulates to genuine-Vensim `vector.dat`.
- **element-cycle-resolution.AC6.4 Edge:** Corrected `vector_elm_map_tests` and `vector_simple.dat` `f`/`g` columns pass against genuine Vensim values.

### element-cycle-resolution.AC7: C-LEARN structural gate + #363
- **element-cycle-resolution.AC7.1 Success:** C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) compiles via the incremental path with no fatal `ModelError` (no `circular_dependency`; non-fatal unit-inference warnings allowed).
- **element-cycle-resolution.AC7.2 Success:** The C-LEARN VM runs to FINAL TIME with no panic.
- **element-cycle-resolution.AC7.3 Success:** No core C-LEARN series is entirely NaN after the run.
- **element-cycle-resolution.AC7.4 Success:** The residual test-only `catch_unwind` for C-LEARN (`tests/ltm_discovery_large_models.rs:670`) is removed and its `clearn_*` test expects a clean compile result.
- **element-cycle-resolution.AC7.5 Failure:** If a post-gate panic surfaces, it is a hard test failure (root-caused), not caught/ignored.

### element-cycle-resolution.AC8: C-LEARN numeric finalization
- **element-cycle-resolution.AC8.1 Success:** `simulates_clearn` is un-`#[ignore]`d and passes — C-LEARN matches `test/xmutil_test_models/Ref.vdf` within the existing 1% cross-simulator tolerance.
- **element-cycle-resolution.AC8.2 Failure:** `ensure_vdf_results` fails (does not vacuously pass) when fewer than a minimum number of variables match, or when a core series is entirely NaN / the NaN-skipped fraction exceeds the guard threshold (covered by a dedicated guard test with synthetic inputs).
- **element-cycle-resolution.AC8.3 Edge:** The `simulates_clearn` stale comment is replaced with an accurate description; the engine default test suite stays green under the 3-minute cap (`simulates_clearn` runs explicitly by runtime class, not in the capped default set).

## Glossary

- **System dynamics (SD)**: Modeling discipline using stocks, flows, and feedback loops to simulate how complex systems evolve over time.
- **Stock / flow**: A stock accumulates over time (state); a flow is the rate that changes a stock. Stocks break dependency cycles because their current value is integrated from the prior step, not computed within the step.
- **dt phase / init phase**: The two compiled evaluation passes. The dt (flows) phase computes each timestep's rates; the init (initials) phase computes starting values. Cycle rules differ: stocks break edges in the dt phase but not the init phase.
- **SCC (strongly connected component)**: A maximal set of graph nodes each reachable from every other; a nontrivial SCC (or self-loop) is a dependency cycle.
- **Tarjan**: Tarjan's algorithm for finding SCCs in a directed graph; here the iterative variant in `ltm/indexed.rs::scc_components`, promoted from test-only to a production `pub(crate)` primitive.
- **Whole-variable cycle gate**: The existing compile-time cycle check that collapses all elements of an arrayed variable into one graph node — coarse, so it can't tell a real cycle from a well-founded element recurrence.
- **Element-level / per-element graph**: A refined graph whose nodes are `(variable, element-offset)` pairs, so a forward recurrence across array elements is visibly acyclic.
- **Forward recurrence / well-founded recurrence**: An equation where each element depends only on strictly earlier elements (e.g. `x[t2]=x[t1]+1`), so it terminates and has a defined value — legitimate, unlike a true cycle.
- **`CircularDependency` / `NotSimulatable`**: The diagnostic emitted when a cycle is found, and the resulting model state that blocks simulation.
- **`dt_walk_successors`**: The single canonical dt-phase dependency-successor relation in `db_dep_graph.rs`, consumed by both the production cycle detector and test introspection so they agree by construction.
- **`model_dependency_graph_impl` / `ModelDepGraphResult`**: The salsa query (and its result struct) that builds the model dependency graph and reports cycles; gains a `resolved_sccs` payload.
- **`ResolvedScc`**: New payload describing a resolved recurrence SCC: its member variables, the `(member, element-offset)` topological order, and which phase (`Dt`/`Initial`).
- **Runlist (`runlist_flows` / `runlist_initials`)**: The ordered `Vec<String>` of variable names giving evaluation order for the dt and init phases.
- **Fragment / `PerVarBytecodes`**: The compiled per-variable bytecode unit; `assemble_module` concatenates these in runlist order to form the runnable module.
- **`concatenate_fragments`**: The `compiler/symbolic.rs` routine that splices `PerVarBytecodes` together; stays agnostic — a combined SCC fragment is just another `PerVarBytecodes`.
- **Combined-fragment lowering**: New path that interleaves multiple SCC members' per-element slots into one synthetic fragment so cross-variable recurrences evaluate in interleaved element order.
- **`AssignCurr`**: The bytecode/expr op that assigns a computed value to a variable's current-value slot at a given offset; per-element `AssignCurr` slots are what the combined fragment interleaves.
- **`lower_var_fragment` / `var_noninitial_lowered_exprs`**: The salsa-tracked bridge in `db_var_fragment.rs`/`db_dep_graph.rs` exposing the engine's own production-lowered per-variable `Vec<Expr>`; reused verbatim to build the element relation.
- **`assemble_module`**: The `db.rs` stage that walks the runlist, lowers each variable, and concatenates fragments into the final simulatable module; gains combined-fragment injection.
- **Salsa-tracked**: Computed via the salsa incremental-computation framework, so results are cached and recomputed only when inputs change.
- **Shift-mapped subrange**: A Vensim subrange mapping that offsets array indices (e.g. `tPrev`→`tNext`); resolved to concrete per-element equations by the native MDL converter at conversion time.
- **`SubscriptIterator` / declared order**: Iteration over an arrayed variable's elements in declared dimension order; why a single-variable self-recurrence already lowers correctly within its one fragment.
- **`implicit_vars` / synthetic helper**: Engine-generated companion variables (synthetic INIT, PREVIOUS/SMOOTH, macro-expansion helpers) with synthesized equations and no user `SourceVariable`; in-SCC helpers are sourced from their parent's `ParsedVariableResult.implicit_vars`.
- **`SourceVariable`**: A graph node backed by a real user-authored variable equation (as opposed to a synthetic helper).
- **Loud-safe fallback**: Design principle of keeping the conservative `CircularDependency` rejection (never silently miscompiling) when an in-SCC node cannot be element-sourced.
- **`dt_previous_referenced_vars` / PREVIOUS-lagged edge**: The set of dependencies reached only through `PREVIOUS`/lagged reads; stripped from the dt relation, so those self-edges correctly vanish in the element graph.
- **`model_element_causal_edges`**: The LTM element-causal-edge relation; deliberately *not* reused here because it retains PREVIOUS-lagged edges (would create ~105 spurious self-loops and falsely block C-LEARN).
- **LTM**: Loops That Matter — Simlin's feedback-loop discovery/scoring analysis; the source of the existing element causal-edge graph and the promoted-from-test Tarjan primitive.
- **C-LEARN**: A large (~53k-line) Vensim climate-economy reference model used as the structural and numeric proving ground; `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`.
- **VDF (Vensim Data File)**: Vensim's proprietary binary simulation-output format; `Ref.vdf` is C-LEARN's reference output, parsed into a `Results` struct for cross-simulator comparison.
- **`ensure_vdf_results`**: Test helper comparing engine output to VDF reference within tolerance; hardened with a minimum matched-variable floor and NaN guards so a near-empty/all-NaN comparison cannot vacuously pass.
- **Cross-simulator tolerance (1%)**: The accepted numeric deviation between Simlin and Vensim on the same model, accounting for benign integration/semantic differences.
- **`VECTOR SORT ORDER`**: Vensim builtin returning the permutation that sorts a vector; corrected here to emit genuine-Vensim **0-based** indices instead of 1-based.
- **`VECTOR ELM MAP`**: Vensim builtin mapping result element `i` to `source[(i + offset[i]) mod n]`; corrected to add the per-element base and `rem_euclid` wraparound over the full source dimension.
- **`rem_euclid`**: Rust's Euclidean remainder (always-nonnegative modulo); the correct wraparound for negative offsets, consistent with the VM's existing `Op2::Mod`.
- **`catch_unwind`**: Rust panic-catching wrapper; a residual test-only one shielding the C-LEARN test is retired once the cycle gate no longer masks the deeper pipeline (issue #363).
- **Byte-stability / deterministic bytecode**: The requirement that emitted bytecode (and the per-element run order) be identical across repeated compiles; enforced via the existing sorted Tarjan tie-break discipline.
- **Incremental path / `incremental_compilation_covers_all_models`**: The salsa-incremental compile pipeline and the corpus regression test ensuring no model regresses.

## Architecture

### The problem

The native MDL converter resolves Vensim shift-mapped subranges to concrete
per-element equations at conversion time (`mdl/convert/variables.rs`
`build_element_context` + `xmile_compat.rs::resolve_subrange_element`):
`x[tNext] = x[tPrev] + 1` becomes
`Equation::Arrayed([("t1","1"),("t2","ecc[t1]+1"),("t3","ecc[t2]+1")])`. The
engine's compile-time cycle gate (`db.rs::model_dependency_graph_impl` ->
`compute_transitive`/`compute_inner`, over the relation
`db_dep_graph.rs::dt_walk_successors`) is **whole-variable**: it collapses an
arrayed variable's elements to one node, so a well-founded forward recurrence
(`t1 -> t2 -> t3`) appears as a self-loop or a multi-variable SCC and is
falsely rejected `CircularDependency` -> `NotSimulatable`.

Empirically, C-LEARN's only fatal compile blocker is exactly this: one
22-node dt SCC + a self-loop on `emissions_with_cumulative_constraints`, all
23 members real source variables, no array-producing builtins inside the
cycle, all 952 intra-SCC references `Bare`/`FixedIndex` (zero
`DynamicIndex`/`Wildcard`), induced element subgraph acyclic. The three
previously-listed blockers (`MismatchedDimensions`, `UnknownDependency`,
`DoesNotExist`) are already cleared on this branch; the `simulates_clearn`
comment that still lists them is stale.

### Element-level resolution

The whole-variable gate's happy path is **unchanged**. The change lives in
`db_dep_graph.rs` (which owns the dt relation) and `model_dependency_graph_impl`:

1. **Detect.** Promote `crate::ltm::indexed::scc_components` (iterative
   Tarjan, currently `#[cfg(test)]`; its docstring already anticipates "a
   production consumer") to unconditional `pub(crate)`. When
   `compute_transitive` reports a back-edge, identify the offending SCC(s)
   over the existing `dt_walk_successors` adjacency (dt phase) and the
   init-successor relation (init phase).

2. **Refine.** For each SCC, build a **per-element dt graph** that is the
   element-granularity refinement of `dt_walk_successors`: nodes are
   `(member, element-offset)`; edges come from the engine's *own* lowered
   per-element exprs via the `lower_var_fragment` /
   `var_noninitial_lowered_exprs` bridge already in `db_dep_graph.rs`,
   reading each member's per-element `Expr::AssignCurr` RHS for intra-SCC
   reads. dt-phase semantics are **inherited**, not re-implemented:
   PREVIOUS/lagged reads and stock-broken edges are excluded exactly as
   `build_var_info` already strips `dt_previous_referenced_vars`. This is
   why C-LEARN's `previous_*` self-edges (from `SAMPLE IF TRUE` ->
   `PREVIOUS(SELF,..)`) correctly vanish, and why this is **not** the LTM
   `model_element_causal_edges` graph (that relation includes
   PREVIOUS-lagged edges for loop discovery and would yield 105 spurious
   self-loops -> false fallback -> C-LEARN stays blocked).

3. **Verdict.** Run the promoted Tarjan on the SCC-induced element graph. If
   element-acyclic **and** every member is element-sourceable -> emit a
   per-element topological run order and mark members for combined-fragment
   lowering; do **not** set `has_cycle`. Otherwise -> keep `has_cycle` +
   `CircularDependency` (loud-safe). Genuine cycles (`a=b+1;b=a+1`,
   `x[dimA]=x[dimA]+1`) stay element-cyclic and remain rejected by
   construction (CLAUDE.md hard rule preserved).

`ModelDepGraphResult` (`db.rs:1129`) gains an SCC-resolution payload threaded
to `assemble_module`:

```rust
struct ResolvedScc {
    members: BTreeSet<Ident<Canonical>>,        // SCC variables (byte-stable)
    element_order: Vec<(Ident<Canonical>, usize)>, // (member, element-offset) topo order
    phase: SccPhase,                            // Dt | Initial
}
// ModelDepGraphResult gains: resolved_sccs: Vec<ResolvedScc>
```

### Combined-fragment lowering

`assemble_module` (`db.rs` ~4287) walks the runlist and concatenates
per-variable `PerVarBytecodes` via `compiler/symbolic.rs::concatenate_fragments`.
Each member is lowered as one contiguous `AssignCurr` block today, so
interleaved cross-variable element order is inexpressible. For a resolved
SCC: collect each member's per-element lowered `AssignCurr` slots (the
`lower_var_fragment` output, keyed by element offset), **interleave them in
`element_order`** into one synthetic `PerVarBytecodes`, inject it at the
SCC's runlist position, and remove members from the per-variable runlist.
`concatenate_fragments` and the `Vec<String>` runlist type are
**unchanged**; variable layout offsets and the results map are
**unchanged** (the combined fragment writes the same `member_base + elem`
slots, reordered). The mechanism serves **both** the dt (flows) and init
(initials) phases — C-LEARN reports both a dt and an init
`circular_dependency`.

### Synthetic-helper policy

A member with no `SourceVariable` (synthetic INIT, PREVIOUS/SMOOTH, or
macro-expansion helper) cannot be sourced through the normal
`lower_var_fragment` keyed path. The current `var_noninitial_lowered_exprs`
contract *panics* ("abort, never silent-skip") — correct for a test
accessor, unacceptable in production. The production contract becomes:
source the helper's per-element exprs from the parent variable's
`ParsedVariableResult.implicit_vars` (each is a real `datamodel::Variable`
with a synthesized equation); if any in-SCC node genuinely cannot be
element-sourced, **loud-safe fallback** to `CircularDependency`. C-LEARN's
SCC contains no such helpers, so this is general-correctness robustness, not
on C-LEARN's critical path — implemented fallback-first, then
parent-sourcing.

### VECTOR ELM MAP / VECTOR SORT ORDER

Independent of cycle resolution (`array_producing ∩ C-LEARN's cycle = ∅`)
but on the `simulates_clearn` numeric path (C-LEARN target-sorting uses
them). Two `vm.rs` corrections to genuine Vensim semantics:

- **`Opcode::VectorSortOrder`** (`vm.rs` ~2358-2405): emit **0-based**
  permutation indices, not 1-based. Direction handling is already correct.
- **`Opcode::VectorElmMap`** (`vm.rs` ~2301-2356): result element `i` reads
  `source[(i + offset[i]) mod n]` — add the missing per-element **base** and
  **wraparound** (`rem_euclid`, as `Op2::Mod` at `vm.rs:102` already uses),
  resolving the **full source dimension** rather than the flattened sliced
  view.

The Simlin-authored fixtures encode the bugs; the fix is incomplete without
correcting them: rewrite the wrong expected values in
`test/sdeverywhere/models/vector_simple/vector_simple.dat` and the affected
`src/simlin-engine/src/array_tests.rs` cases (`mod vector_elm_map_tests`
~3474, `vector_builtin_promotes_active_dim_ref_*` ~4145,
`mod dimension_dependent_scalar_arg_tests` ~4217, `mod
arrayed_except_hoisting_tests` ~3984) to **genuine Vensim** values, and
**un-exclude** `test/sdeverywhere/models/vector/vector.xmile`
(simulate.rs ~651-656) as the regression gate (its `.dat` is real Vensim
output).

## Existing Patterns

This design follows established codebase patterns; it introduces no new
architectural style.

- **"Single shared relation, never re-derive."** `db_dep_graph.rs` defines
  `dt_walk_successors`/`build_var_info` once and consumes them in *both* the
  production cycle detector and the `#[cfg(test)]` introspection accessor, so
  the accessor observes the engine's actual relation "by construction." This
  design extends that exact pattern: the per-element relation is a *faithful
  refinement* of `dt_walk_successors`, so the element-acyclicity verdict is
  sound by construction. The three contract changes to `db_dep_graph.rs`
  (promote `#[cfg(test)]` machinery to `pub(crate)`; replace
  `var_noninitial_lowered_exprs`'s panic-on-no-`SourceVariable` with
  parent-sourcing + loud-safe fallback; re-point
  `dt_cycle_sccs_engine_consistent` at the new invariant) are intended
  consequences of adding a production consumer — explicitly anticipated by
  the `scc_components` docstring. The dense rationale comments are the
  "explain why" CLAUDE.md requires and are preserved.

- **Salsa-tracked lowering bridge.** `var_noninitial_lowered_exprs` /
  `db_var_fragment.rs::lower_var_fragment` already source the engine's own
  per-variable production-lowered `Vec<Expr>` over the `build_var_info`
  universe. The element relation reuses this bridge verbatim — not a parallel
  derivation.

- **Per-fragment composition.** A combined SCC fragment is just another
  `PerVarBytecodes`; `concatenate_fragments` stays agnostic, mirroring how
  LTM synthetic fragments are appended in `assemble_module`.

- **Byte-stability discipline.** `dt_cycle_sccs` is sorted/byte-stable;
  the element run order uses the same Tarjan + sorted tie-break so bytecode
  is deterministic across runs.

- **Incorrect tests are corrected, not preserved** (CLAUDE.md): the
  Simlin-authored VECTOR fixtures that encode bugs are rewritten to genuine
  Vensim, and the genuine-Vensim `vector.xmile` is un-excluded — the same
  posture as prior corpus-fixture work.

- **Loud-safe fallback over silent miscompile.** Matches the codebase's
  pervasive preference (the `var_noninitial_lowered_exprs` abort rationale,
  the LTM unscoreable-edge warning): when element-sourcing is impossible,
  keep the conservative `CircularDependency` rather than emit a wrong run
  order.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Tarjan promotion + single-variable self-recurrence

**Goal:** A single-variable self-recurrence SCC (a dt self-loop whose
induced element graph is acyclic) is resolved and simulates; genuine cycles
still rejected. No combined fragment yet (the single-variable case already
lowers correctly within its one existing fragment via declared-order
`SubscriptIterator`).

**Components:**
- `src/simlin-engine/src/ltm/indexed.rs` — promote `scc_components` from
  `#[cfg(test)]` to unconditional `pub(crate)`.
- `src/simlin-engine/src/db_dep_graph.rs` — production SCC identification
  over `dt_walk_successors`/init relation; the per-element dt relation
  builder (element-granularity refinement, sourced via the existing
  `lower_var_fragment` bridge, inheriting PREVIOUS/stock exclusion);
  element-acyclicity verdict; re-point `dt_cycle_sccs_engine_consistent` /
  `dt_cycle_sccs_consistency_violation` to the new invariant (element-acyclic
  SCC ⇒ no `CircularDependency`; element-cyclic ⇒ still flagged).
- `src/simlin-engine/src/db.rs` — `model_dependency_graph_impl` consumes the
  verdict; `ModelDepGraphResult` gains `resolved_sccs`; single-variable
  resolved SCC excluded from the `CircularDependency` accumulation and kept
  in the normal per-variable runlist.

**Dependencies:** None (first phase).

**Done when:** Tests verify `element-cycle-resolution.AC1.*` and
`element-cycle-resolution.AC4.*`: `self_recurrence.mdl` compiles and
simulates to the correct series; `genuine_cycles_still_rejected` stays
green; the consistency harness passes against the new invariant; element
run order is byte-stable. Engine suite green under the 3-min cap.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Combined-fragment lowering (multi-variable SCC)

**Goal:** A multi-variable element-acyclic recurrence SCC evaluates in
interleaved per-element order, in both dt and init phases.

**Components:**
- `src/simlin-engine/src/db.rs` — `assemble_module`: build the synthetic
  combined `PerVarBytecodes` by interleaving members' per-element
  `AssignCurr` slots in `ResolvedScc.element_order`; inject at the SCC
  runlist slot; remove members from the per-variable runlist; apply to
  `runlist_flows` (dt) and `runlist_initials` (init).
- `src/simlin-engine/src/db_dep_graph.rs` — init-phase element relation
  (stock-non-breaking variant) so the init SCC gets its own element order.

**Dependencies:** Phase 1.

**Done when:** Tests verify `element-cycle-resolution.AC2.*`: `ref.mdl` and
`interleaved.mdl` compile and simulate to hand-computed values;
`ref_interleaved_inter_variable_cycles_report_circular` transitions from
asserting `CircularDependency` to asserting correct simulation; combined
fragment is byte-stable; existing non-recurrence models unaffected
(`incremental_compilation_covers_all_models` green).
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Synthetic-helper sourcing policy

**Goal:** An element-SCC containing a no-`SourceVariable` helper is resolved
when the helper is sourceable from its parent's implicit vars, else
loud-safe fallback.

**Components:**
- `src/simlin-engine/src/db_dep_graph.rs` — replace
  `var_noninitial_lowered_exprs`'s panic-on-no-`SourceVariable` (production
  path) with sourcing from the parent's
  `ParsedVariableResult.implicit_vars`; loud-safe fallback to
  `CircularDependency` when an in-SCC node cannot be element-sourced. The
  `#[cfg(test)]` array-producing accessor keeps its abort contract (a false
  negative there is still wrong).

**Dependencies:** Phase 2.

**Done when:** Tests verify `element-cycle-resolution.AC3.*`: a
synthetic-helper-bearing well-founded recurrence fixture simulates; an
unsourceable fixture falls back to `CircularDependency` (no panic, no silent
miscompile); `array_producing_vars` test contract unchanged.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: VECTOR SORT ORDER (genuine Vensim 0-based)

**Goal:** `VECTOR SORT ORDER` returns 0-based permutation indices; fixtures
corrected to genuine Vensim.

**Components:**
- `src/simlin-engine/src/vm.rs` — `Opcode::VectorSortOrder` 0-based result;
  replace the "intentional asymmetry" rationalization comment with the
  correct semantics + citation.
- `src/simlin-engine/src/array_tests.rs` — correct the cases that encode
  1-based output (`vector_builtin_promotes_active_dim_ref_*` ~4145,
  `mod dimension_dependent_scalar_arg_tests` ~4217,
  `mod arrayed_except_hoisting_tests` ~3984).
- `test/sdeverywhere/models/vector_simple/vector_simple.dat` — correct the
  `l`/`m` columns to genuine Vensim values.

**Dependencies:** None (independent of Phases 1-3).

**Done when:** Tests verify `element-cycle-resolution.AC5.*`: corrected
`array_tests.rs` cases pass; `simulates_vector_simple_mdl` passes against
corrected expectations.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: VECTOR ELM MAP (base + wraparound + cross-dim source)

**Goal:** `VECTOR ELM MAP` implements `source[(i + offset[i]) mod n]` over
the full source dimension; genuine-Vensim `vector.xmile` un-excluded as the
regression gate.

**Components:**
- `src/simlin-engine/src/vm.rs` — `Opcode::VectorElmMap` per-element base +
  `rem_euclid` wraparound; resolve the full source dimension, not the
  flattened sliced view (`read_view_element` / `bytecode.rs::flat_offset`
  interplay).
- `src/simlin-engine/src/array_tests.rs` — correct `mod
  vector_elm_map_tests` (~3474) and the OOB-NaN expectations.
- `test/sdeverywhere/models/vector_simple/vector_simple.dat` — correct
  `f`/`g` columns.
- `src/simlin-engine/tests/simulate.rs` — un-exclude
  `test/sdeverywhere/models/vector/vector.xmile` (~651-656) as a simulating
  model gated on its genuine-Vensim `.dat`.

**Dependencies:** Phase 4 (shared fixture file `vector_simple.dat`; sequence
to avoid churn).

**Done when:** Tests verify `element-cycle-resolution.AC6.*`: corrected
`vector_elm_map_tests` pass; `vector.xmile` simulates and matches genuine
Vensim `vector.dat`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: C-LEARN structural gate + #363 re-verify

**Goal (the explicit mid-plan value-locking checkpoint):** C-LEARN compiles
via the incremental path and runs to FINAL TIME via the VM with no panic
and no NaN in core series; #363 re-verified; residual test-only
`catch_unwind` retired.

**Components:**
- `src/simlin-engine/tests/simulate.rs` — a new (initially `#[ignore]`d if
  runtime requires, or feature-gated) test that compiles C-LEARN past the
  now-resolved cycle gate, asserts a clean `Result` (no panic), runs the VM
  to FINAL TIME, and asserts no all-NaN core series.
- `src/simlin-engine/tests/ltm_discovery_large_models.rs` — retire the
  `catch_unwind` (~670); re-point `clearn_*` to expect a clean compile
  result.

**Dependencies:** Phases 2, 3, 5 (C-LEARN needs the multi-variable SCC, the
helper policy as a safety net, and correct VECTOR ops to avoid NaN
propagation).

**Done when:** Tests verify `element-cycle-resolution.AC7.*`: C-LEARN
compiles with no fatal `ModelError` (unit-inference warnings allowed,
explicitly out of scope), runs to FINAL TIME, no panic, no all-NaN core
series; no `catch_unwind` remains in the engine test suite for C-LEARN.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Numeric finalization

**Goal:** `simulates_clearn` un-`#[ignore]`d and passing vs `Ref.vdf`;
`ensure_vdf_results` hardened.

**Components:**
- `src/simlin-engine/tests/simulate.rs` — harden `ensure_vdf_results`
  (~140-190): minimum matched-variable floor; NaN guard (fail on
  entirely-NaN core series or excessive NaN-skipped fraction). Un-`#[ignore]`
  `simulates_clearn`; update its stale comment.
- Bounded numeric debugging of the C-LEARN vs `Ref.vdf` tail within the
  existing 1% cross-simulator tolerance; any residual mismatch that resists
  a *general* fix is filed/triaged via `track-issue`, not hacked around.

**Dependencies:** Phase 6.

**Done when:** Tests verify `element-cycle-resolution.AC8.*`:
`simulates_clearn` passes (numeric match within 1%); `ensure_vdf_results`
cannot vacuously pass on near-empty/all-NaN comparison (covered by a
dedicated guard test); engine suite green under the 3-min cap (note:
`simulates_clearn` itself is release-`--ignored`-class by runtime and is
run explicitly, not in the capped default set).
<!-- END_PHASE_7 -->

## Additional Considerations

**Init-phase recurrence.** C-LEARN reports both a dt and an init
`circular_dependency`. The init-phase element relation differs from dt
(stocks do not break the chain in the initials phase — `compute_inner`'s
`info.is_stock && !is_initial` sink is dt-only). Both phases get their own
SCC-induced element graph and combined fragment; this is designed in (Phase
2) but is the subtlest part and is called out for the implementation plan.

**Numeric tail risk (Phase 7).** A 53k-line model matching Vensim within 1%
may surface latent integration/semantic differences beyond VECTOR ops. The
Phase 6 structural gate deliberately locks the compile+run value before
Phase 7, so a long numeric tail does not strand the structural deliverable.
Phases 1-3 are independently shippable engine improvements regardless of the
Phase 7 outcome.

**Determinism.** The per-element topological order must be byte-stable;
reuse the `dt_cycle_sccs` Tarjan + sorted tie-break discipline so emitted
bytecode does not vary across runs (the codebase enforces this elsewhere and
has tests for it).

**Out of scope (restated):** non-fatal unit-inference warnings on C-LEARN;
VECTOR SELECT cross-dimension patterns beyond what C-LEARN / `vector.xmile`
exercise; Vensim macro support (complete).
