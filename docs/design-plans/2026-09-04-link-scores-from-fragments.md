# Link scores from compiled fragments

## Summary

Loops That Matter (LTM) explains a simulation by scoring every causal link at
every timestep: how much of a variable's change this step is attributable to
one particular input. Each score is a small program the engine adds to the
model alongside the modeler's own variables. Today those programs are built as
equation text. For a link `x -> z` the generator prints `z`'s equation back
out with every read except `x` wrapped in `PREVIOUS(..)`, prints a guard
around it, parses the result, and pushes that parse through the entire
compiler a second time -- once per link, 6,155 times on C-LEARN. The numbers
that come out are right. The construction is what costs: every compiler tier
of every link is retained (a 222 MiB database against 29 MiB without LTM),
and because all of the generated equations live in one whole-model value,
editing any variable invalidates all of them, so one edit recompiles thousands
of link programs.

This plan builds each link score's program from the target's already-compiled
fragment instead. A fragment is one variable's equation lowered to symbolic
bytecode, the same artifact the plain simulation assembles, and the "hold
everything else at the previous step" partial a link score needs is a
mechanical rewrite of that opcode stream: reads outside the source's live slot
range become previous-step reads, the final assignment is redirected to the
score's own slot, and every other opcode is copied. It is a pure function over
bytecode -- no printing, no parsing, no re-lowering, and no synthesized helper
variables. Because the rewrite is keyed per link on the target and its live
range, the whole-model list becomes metadata (which links exist, their names
and dimensions) while each link's program lives in its own memo, so an edit
invalidates only the links that touch the edited variable. The families that
are not partials (flow-to-stock, black-box, aggregate, module composite and
loop scores) become typed builders over slot reads on the same footing, which
is what lets the text generators be deleted rather than merely bypassed. Two
run-time consequences follow from having a single place that emits a score:
the 37-opcode guard scaffold collapses to one `LinkScore` opcode reading
per-variable deltas computed once per step, and, on native and separably, the
LTM program moves out of the integration loop into a post-pass over saved
states. Six phases, each gated on bit-identical score series against the text
generators, which are held as a retained oracle until Phase 4 deletes them.

## Definition of Done

Each item states what is true of the tree when this plan has landed, with the
command or test that shows it.

1. **A link score's program is derived from the target's compiled fragment,
   never from generated equation text.** The ceteris-paribus partial that
   every auxiliary-to-auxiliary and stock-to-flow link score is built on is a
   rewrite over the target's symbolic fragment (`compiler::symbolic::SymbolicByteCode`),
   and the guard around it is emitted by a typed builder. No link, loop,
   aggregate or composite score is printed as equation text and parsed back:
   `rg "link_score_guard_form|LtmArm::new|Expr0::new\(" src/simlin-engine/src/db/ltm src/simlin-engine/src/ltm_augment*.rs`
   is empty, and `LtmSyntheticVar` carries no `LtmEquation`.
2. **Parity.** For every model in the LTM corpus (`test/test-models/**/*ltm*`,
   `test/sdeverywhere/models/**` with loops, and C-LEARN v77 as an
   `--ignored` release test) the synthesized link-score, loop-score and
   relative-loop-score series are bit-identical to the text generators'
   (`tests/integration/ltm_synthesis_parity.rs`, which holds the generators
   as its oracle until Phase 4 deletes them, and the golden `.dat`/`.tab`
   outputs after).
3. **Incremental by construction.** Under LTM, a structure-preserving edit to
   one variable re-executes that variable's fragment and the link scores whose
   target or source it is, and nothing else; a rename re-derives only the
   links it touches. Counted over every tracked query with
   `db::exec_probe::ProbedDb` (`db::ltm_synthesis_tests`), the way
   `fragment_char_tests` counts today; on C-LEARN the LTM literal edit is
   under 50 M instructions (7.4 G today) and the structural edit under 500 M
   (9.6 G today).
4. **Nothing whole-model holds a per-link program.** `model_ltm_variables`
   returns names, dimensions, flags, loop partitions and diagnostics; the
   per-link memo owns its fragment. `CLEARN_LTM=1 CLEARN_RESIDENCY=1
   cargo run --release --example clearn_profile` reports the database under
   40 MiB after an LTM compile (222 MiB today) and no `Expr0` family among the
   LTM residency rows.
5. **One opcode per link score at run time.** The 37-opcode guard scaffold is
   one `LinkScore` opcode over per-variable delta slots computed once per
   step, in both backends (`vm.rs` and `wasmgen`); the C-LEARN LTM flow
   program is under 250 K opcodes per step (855 K today) and
   `tests/integration/simulate_ltm_wasm.rs` still agrees with the VM.
6. **Cold LTM compile and run.** `CLEARN_LTM=1 CLEARN_PROFILE=compile
   CLEARN_COMPILE_ITERS=2 perf stat -e instructions` is within 2x of the
   plain compile (10x today), and the LTM run within 3x of the plain run
   (13x today), recorded in the ledger at the end of this document.

## Acceptance Criteria

Proposed; to be validated in review before an implementation plan is written
from them.

### link-scores-from-fragments.AC1: A link score's program is a rewrite of the target's fragment
- **link-scores-from-fragments.AC1.1 Success:** For a scalar auxiliary-to-auxiliary link, `freeze_reads` over the target's flows-phase fragment with the source's slot as the live range yields a program whose resolved bytecode, run in the VM, produces the same score series bit for bit as the text path's program on every model of the LTM corpus.
- **link-scores-from-fragments.AC1.2 Success:** The same holds for a stock-to-flow link (the source is a stock, read at the current step).
- **link-scores-from-fragments.AC1.3 Success:** A `SymLoadPrev` already present in the target's fragment is left unchanged by the rewrite, and a target that reads `TIME` scores identically under both paths.
- **link-scores-from-fragments.AC1.4 Success:** A target that reads a sub-model variable (`m·x`) scores identically under both paths; the frozen read is a previous-step read of the module block's slot.
- **link-scores-from-fragments.AC1.5 Success:** After Phase 4, `rg "link_score_guard_form|LtmArm|LtmEquation" src/simlin-engine/src` is empty and `LtmSyntheticVar` carries a `kind`, not an equation.

### link-scores-from-fragments.AC2: Parity on every shape the generators emit
- **link-scores-from-fragments.AC2.1 Success:** Every model under `test/test-models/**/*ltm*` and every `sdeverywhere` model with a detected loop produces bit-identical link-score, loop-score and relative-loop-score series before and after each phase (`tests/integration/ltm_synthesis_parity.rs` while the generators exist, the golden outputs after).
- **link-scores-from-fragments.AC2.2 Success:** C-LEARN v77 under LTM produces bit-identical series (an `--ignored` release test, run and recorded in the ledger per phase).
- **link-scores-from-fragments.AC2.3 Failure:** A target whose fragment does not compile yields no link score and one `Warning` naming the target and the source, as the `PartialEquationError` path does today; no score reads a constant 0 without a diagnostic.
- **link-scores-from-fragments.AC2.4 Success:** Every arrayed reference shape (`FixedIndex`, `Wildcard`, `DynamicIndex`, `PerElement`, mapped and iterated axes) is a live range, and the arrayed corpus (`arrayed_population_ltm`, `cross_element_ltm`, `cross_agg_ltm`, `tests/integration/ltm_array_agg.rs`) is bit-identical.
- **link-scores-from-fragments.AC2.5 Failure:** An edge the generators decline today (a dynamic-pinned slice, a repeated-dimension target, an unfreezable partial) is declined with the same `Warning` under the new path; a shape that cannot be expressed as a live range is a loud decline, never a silent score.

### link-scores-from-fragments.AC3: Incremental by construction
- **link-scores-from-fragments.AC3.1 Success:** Under the overlay, editing one variable's literal re-executes that variable's fragment and the link-score memos whose target or source it is, and no other `link_score_fragment` or `compile_var_fragment` body (`db::exec_probe::ProbedDb` counts, on a model with two independent loops).
- **link-scores-from-fragments.AC3.2 Success:** Renaming an unreferenced constant re-executes no `link_score_fragment` body; C-LEARN's LTM literal edit is under 50 M instructions and its structural edit under 500 M.

### link-scores-from-fragments.AC4: Nothing whole-model holds a per-link program
- **link-scores-from-fragments.AC4.1 Success:** `model_ltm_variables` holds no `Expr0` and no equation text; `CLEARN_LTM=1 CLEARN_RESIDENCY=1` reports the database under 40 MiB after an LTM compile.
- **link-scores-from-fragments.AC4.2 Success:** `model_ltm_implicit_var_info` and layout section 3b are gone, and the results-offset map of every corpus model is unchanged (every synthetic variable keeps its name and its slot; `ltm_post` and the FFI are untouched).

### link-scores-from-fragments.AC5: One opcode per link score at run time
- **link-scores-from-fragments.AC5.1 Success:** The C-LEARN LTM flow program is under 250 K opcodes per step and its series are unchanged.
- **link-scores-from-fragments.AC5.2 Success:** The VM and the wasm backend agree slab for slab on every LTM corpus model with the `LinkScore` opcode (`simulate_ltm_wasm.rs`).
- **link-scores-from-fragments.AC5.3 Edge:** The first-step and zero-delta guards produce exactly the 0 the scaffold produced, including when `Δz` is 0 while `Δx` is not.

### link-scores-from-fragments.AC6: Post-pass (native)
- **link-scores-from-fragments.AC6.1 Success:** The post-pass over saved states produces series identical to the in-loop form, on the LTM corpus and C-LEARN, with `save_step > dt` still scoring every dt.
- **link-scores-from-fragments.AC6.2 Success:** The C-LEARN LTM run on native is within 3x of the plain run; the wasm bundle keeps the in-loop form and its series are unchanged.

## Glossary

- **LTM (Loops That Matter)**: the feedback-loop dominance method Simlin
  implements (`docs/design/ltm--loops-that-matter.md`). It attributes each
  variable's change at each timestep to its individual inputs, then combines
  those attributions into per-loop scores.
- **Link score**: the per-timestep number for one causal edge `x -> z`: the
  share of `z`'s change this step attributable to `x`, signed by the direction
  of the relationship.
- **Loop score / relative loop score**: the product of the link scores around
  one feedback loop, and that product normalized against the other loops in
  its cycle partition. Relative scores are computed after the run
  (`ltm_post`), not in the program this plan changes.
- **Ceteris-paribus partial**: the target's equation evaluated with only the
  source advanced to the current step and every other read held at the
  previous step. It is the numerator of a link score, and the thing this plan
  synthesizes by rewriting bytecode instead of printing text.
- **Live range**: the new key of the rewrite. The source's slots that stay
  current under the freeze -- a whole variable block for a bare reference, a
  single element slot for an indexed one. Every reference shape becomes a
  choice of live range.
- **Reference shape (`RefShape`)**: how a target's equation reads its source
  (`Bare`, `FixedIndex`, `Wildcard`, `DynamicIndex`, `PerElement`, mapped and
  iterated axes). Classified once per variable in `db/ltm_ir.rs`; it decides
  which occurrences of the source are live.
- **Aggregate node**: the stand-in for a reducer subexpression (`SUM(pop)` and
  friends, `ltm_agg.rs`), named `$⁚ltm⁚agg⁚{n}`, so a per-element source can be
  scored through a reduction that collapses its axis.
- **Module instance / composite / pathway**: a sub-model instantiated inside a
  parent (its variables addressed `m·x`). A link into a module gets a
  composite score, the product of link scores along the strongest internal
  pathway from the input port to the output, rather than a partial.
- **Black-box unit transfer**: the fallback score used when no partial and no
  composite can be taken through a module: a signed unit (`+1`/`-1`) rather
  than a computed share.
- **PREVIOUS**: the XMILE builtin reading a variable's value at the previous
  step. It lowers to `Opcode::LoadPrev` / `ViewStorage::Prev`, so the rewrite
  emits previous-step reads directly and never needs the builtin.
- **SAFEDIV**: division that yields an explicit given value when the
  denominator is zero, instead of a NaN or an infinity.
- **Fragment**: one variable's one phase (initial / flow / stock) compiled to
  layout-independent symbolic bytecode. `compile_var_fragment` is the memo
  that produces it and the input this plan's rewrite consumes.
- **Symbolic bytecode**: opcodes whose variable operands are names
  (`SymVarRef`), not addresses; `resolve_module` turns them into slots at
  assembly. A rewritten fragment is therefore position-independent like any
  other and needs no new resolution machinery.
- **Slot**: an index into the flat per-step data buffer a model's variables
  live in. `db/layout.rs` assigns them; LTM synthetic variables occupy their
  own section of the layout.
- **Delta slot**: a slot reserved per LTM-relevant variable holding
  `v_t - v_{t-1}`, written once per step so that every link touching that
  variable reads the difference instead of recomputing it.
- **`Expr0`**: the first AST tier, straight from the parser and before builtin
  resolution. It is the tier the text path re-enters the compiler at, and the
  bulk of the memory this plan frees.
- **Overlay (`LtmOverlay`)**: whether a compile is of the model alone (`Off`)
  or the model plus its LTM synthetic variables (`On`). Layouts and fragments
  are memoized separately per overlay so the two do not evict each other.
- **salsa**: the incremental-computation framework the engine's compile
  pipeline is built on. The document's "tracked query", "memo",
  "invalidates" and "re-executes" vocabulary is salsa's.
- **Memo**: one tracked query's cached value for one key. Re-executing a
  query body is the unit of work the incrementality criteria count; "the
  per-link memo owns its fragment" means each link's program is cached
  independently of every other link's.
- **Firewall query**: a deliberately narrow query (`model_variable_by_name`)
  interposed so a consumer depends on exactly what it looks up rather than on
  a whole-model value. Coarse keys are what make an unrelated edit invalidate
  everything; this plan replaces one such key.
- **`ProbedDb` (execution-count test)**: a test database that logs every
  tracked-query body entry (`db/exec_probe.rs`), so a test can assert which
  queries an edit re-executed rather than only that the result is right.
- **Post-pass**: running the LTM program after the simulation finishes, over
  retained per-dt states, instead of inside each integration step. A link
  score reads only current and previous state, so the pass is parallel across
  steps. Native only; the wasm bundle keeps the in-loop form.
- **Slab**: the flat results buffer holding every saved step's values.
  "Agree slab for slab" means the two backends produce byte-equal buffers.
- **`wasmgen`**: the second backend, lowering the same symbolic bytecode to a
  WebAssembly module. Every new opcode needs a row there as well as in the VM.
- **SCC (strongly connected component)**: a group of variables that must be
  solved together because they refer to each other within a step. The
  machinery that cuts a fragment per element for that path
  (`segment_member_by_element`) is reused here for per-element attribution.
- **Cycle partition**: a group of stocks connected by feedback paths. Relative
  loop scores are normalized within one partition, so partitions decide which
  loops are compared against which.
- **Discovery mode**: the LTM mode that emits link scores for every causal
  edge and finds dominant loops after the run, used for models too large for
  exhaustive circuit enumeration. Exhaustive mode scores only edges in
  detected loops.
- **Loud decline**: refusing to emit a score, with a diagnostic naming what
  was refused, rather than emitting a plausible-looking zero. The alternative
  failure mode -- a score that silently reads a constant 0 -- is the one the
  acceptance criteria rule out.
- **C-LEARN**: the hero model used for every performance figure here: ~53k MDL
  lines, 934 datamodel variables, 5,726 root slots, 1,000 Euler timesteps, and
  6,155 LTM links (`docs/design/engine-performance.md`).
- **Retired instructions**: the `perf stat` instruction channel. Deterministic
  enough across builds to be the unit every ledger row and every compile-cost
  criterion is stated in; wall time is not.

## Architecture

### The score, and what it costs today

An LTM link score for the edge `x -> z` is

    LS(x -> z) = |Δ_x z / Δz| · sign(Δ_x z / Δx),   Δ_x z = z(x_t, w_{t-1}) - z_{t-1}

where `z(x_t, w_{t-1})` is the target's equation evaluated with the live source
at the current step and every other read at the previous step (the
ceteris-paribus partial), guarded to 0 at `INITIAL_TIME` and whenever `Δz` or
`Δx` is 0 (`ltm_augment::link_score_guard_form_with_numerator`).

Today `db/ltm/link_scores.rs::shaped_link_score` builds that per link by
PRINTING the target's equation with every non-source reference wrapped in
`PREVIOUS(..)` (`ltm_augment::wrap_non_matching_in_previous`), printing the
guard around it, parsing the text (`LtmArm::new`), and then lowering the parse
through every compiler tier (`db/ltm/compile.rs::compile_ltm_fragment_at`).
Each of C-LEARN's 6,155 links pays the whole compiler once more, at about four
times the cost of the user variable it derives from, and retains every tier:
117 MiB of `Expr0` trees and text, 48 MiB of fragments, in a database of
222 MiB, against 29 MiB plain. Because `model_ltm_variables` is one
whole-model value holding every equation, an edit to any target changes it
and 3,015 link fragments recompile; a rename re-does 96% of a cold LTM
compile. At run time the guard is 37 opcodes of scaffold around a partial
that is usually a handful, 855 K flow opcodes per step, and every link that
shares a variable recomputes that variable's delta
(`docs/design/engine-performance.md`, C7).

### Synthesis: the partial is a rewrite over the target's fragment

The target's flows-phase symbolic fragment (`VarFragmentResult` from
`db::compile_var_fragment`, the same memo assembly reads) already IS the
target's equation, lowered. The ceteris-paribus freeze is a rewrite over that
opcode stream, keyed by the LIVE SLOT RANGE (the source's slots that stay
current):

| opcode in the target's fragment | outside the live range becomes | inside it |
|---|---|---|
| `LoadVar { var }` | `SymLoadPrev { var }` (the previous-step snapshot, what `PREVIOUS()` reads) | unchanged |
| `PushStaticView` with base `SymStaticViewBase::Var(v)` | base `PrevVar(v)` (`ViewStorage::Prev` exists) | unchanged |
| `PushVarViewDirect { var, .. }` | a previous-storage twin of the direct view (new symbolic row, resolves to `ViewStorage::Prev`) | unchanged |
| an original `SymLoadPrev`, `SymLoadInitial`, `LoadGlobalVar` | unchanged: a lagged read is not lagged twice, a snapshot is a snapshot, and the implicit globals follow the text generators' rule (settled by the parity test in Phase 1) | unchanged |
| `AssignCurr { var: target }` | `AssignCurr { var: <the link score's partial slot> }` | -- |
| everything else | copied | copied |

Freezing READS is the freeze of the expression: a pure expression over
previous-step values is the previous-step value of that expression, so the
`PREVIOUS(<subexpression>)` helper slots the text path synthesizes for
non-atomic wrapped subtrees (`model_ltm_implicit_var_info`, 738 helpers on
C-LEARN, layout section 3b) are not needed -- every frozen read is a direct
`LoadPrev` of a variable slot. The rewrite is a pure function
`freeze_reads(&SymbolicByteCode, &LiveRange) -> SymbolicByteCode` in a new
module `db/ltm/synthesize.rs`, with no text, no parse and no lowering.

Per-element attribution is a choice of live range. A `Bare` source is its
whole slot block; a `FixedIndex`/`PerElement` occurrence is one element slot;
an arrayed target's per-element link score is the rewrite of that element's
segment of the fragment (`assemble::segment_member_by_element` already cuts
a fragment per element for the SCC path). The reference-shape classification
that decides which occurrences of the source are live
(`db::analysis::RefShape`, `ltm_ir`) is unchanged; only what it drives
changes: a slot range instead of a text rewrite.

The guard is emitted by a typed builder over the partial's result: the
comparisons, the `SAFEDIV`, the `SIGN`. In Phase 2 it is the same opcode
sequence the text path lowers to, so parity is bit-for-bit; Phase 5 replaces
it with one `LinkScore` opcode.

### Keying: nothing whole-model between an equation and a link's program

    link_score_fragment(db, target: SourceVariable, live: LiveRange, model, project) -> Arc<VarFragmentResult>

reads `compile_var_fragment(target)` and the live range. `model_ltm_variables`
keeps deciding WHICH links exist (the causal edges, the loops, the
partitions) and becomes metadata: `LtmSyntheticVar { name, dimensions,
kind: LinkScore { target, live } | LoopScore { .. } | .. }` with no equation.
Assembly and diagnostics iterate that list and call the per-link query; an
edit to one target invalidates the fragments keyed on it and the link scores
that read it, and no other. This is what closes the incrementality cliff and
frees the trees: there is no tree.

### The families that are formulas, not partials

Not every synthetic variable is a ceteris-paribus partial. Each remaining
family is a fixed formula over reads of other slots and is emitted by a
typed builder with no text:

| family | today | this plan |
|---|---|---|
| flow -> stock (second-order structural formula, `generate_flow_to_stock_equation`) | text | builder over `LoadVar`/`SymLoadPrev` of the flow and stock |
| black-box unit transfer (`black_box_unit_transfer_equation`) | text | builder |
| aggregate nodes (`$⁚ltm⁚agg⁚n`, `AggNode::reducer_expr0`) | typed `Expr0`, then parsed tiers | the reducer's own compiled fragment, live range per feeder |
| module composites (`m·$⁚ltm⁚composite⁚port`) | text over pathway products | builder over the pathway link scores' slots |
| loop scores (products of link scores) | text | builder |

### Run time: one opcode per link, deltas shared

`LinkScore { partial, target, source }` reads three slots -- the partial's
result, the target's, the source's -- and the per-variable deltas
`Δv = v_t - v_{t-1}` are computed once per step into delta slots the layout
reserves per LTM-relevant variable (section 3c), so 21 K links over a few
thousand variables evaluate 21 K partials, 21 K score opcodes and a few
thousand deltas instead of 855 K opcodes. Both backends implement the
opcode; `symbolic_opcode_table!` gains the row, `bytecode.rs` the concrete
twin, `wasmgen/lower.rs` the lowering.

### Post-pass (native)

A link score reads only current and previous state. With every-dt states
saved (41 MB for C-LEARN), the LTM program is a post-pass over saved states
that is parallel across steps; the wasm bundle keeps the in-loop form. This
is Phase 6 and is separable from the rest.

## Existing Patterns

- **Per-variable memos with firewalls.** `compile_var_fragment` is keyed on
  the variable and its module inputs and reads shapes through per-name
  firewall queries (`model_variable_by_name`, `model_implicit_var_by_name`),
  so a fragment depends on exactly what it looks up
  (`docs/design-plans/2026-08-25-compiler-unification.md`, "One fragment
  compiler"). The per-link query follows that shape; today's
  `shaped_link_score` is keyed per link too, but its input is the whole-model
  equation list, which is the coarseness this plan removes.
- **Symbolic bytecode with late resolution.** `SymbolicOpcode` names
  variables (`SymVarRef`) and is resolved against the layout at assembly
  (`resolve_module`), so a rewritten fragment is position-independent like
  any other and needs no new resolution machinery. The operand-kind table
  (`symbolic_opcode_table!`) is where the two new rows (the previous-storage
  direct view, `LinkScore`) are declared, and `renumber_opcode`/
  `resolve_opcode` derive from it.
- **Previous-step storage is a first-class read.** `Opcode::LoadPrev`,
  `ViewStorage::Prev` and `SymStaticViewBase::PrevVar` are what `PREVIOUS()`
  lowers to; the rewrite emits them directly.
- **Typed generation already exists for one family.** `AggNode::reducer_expr0`
  builds an `Expr0` and `LtmArm::from_typed` carries it; this plan goes one
  tier lower (opcodes) and applies it to every family.
- **Execution-count tests.** `db::exec_probe::ProbedDb` and
  `fragment_char_tests` count every tracked query's executions over an edit;
  the incrementality criteria are stated in those terms.
- **Parity against a retained oracle.** `tests/integration/simulate_ltm_wasm.rs`
  checks the wasm backend against the VM slab-for-slab; the synthesis parity
  test uses the same shape with the text generators as the oracle.

Divergence: the text generators and `LtmArm`/`LtmEquation` are deleted in
Phase 4, so `db/ltm_char_tests.rs`'s equation-text goldens become goldens over
the synthesized opcode streams (the characterization the goldens exist for is
the program, not its spelling).

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: The rewrite
**Goal:** `freeze_reads` over a symbolic fragment, proven equal to the text
path on scalar targets.

**Components:**
- `db/ltm/synthesize.rs` -- `LiveRange` (a variable and a slot range within
  it), `freeze_reads(&SymbolicByteCode, &LiveRange) -> SymbolicByteCode`, the
  guard builder emitting today's opcode sequence.
- `compiler/symbolic.rs` -- the previous-storage direct-view row in
  `symbolic_opcode_table!`, resolving to `ViewStorage::Prev`.
- `tests/integration/ltm_synthesis_parity.rs` -- for a scalar
  auxiliary-to-auxiliary and a stock-to-flow link on the LTM corpus: the
  text path's fragment and the synthesized one, resolved and run, agree
  bit-for-bit on the score series; the implicit-global rule (`TIME`) is
  settled here.

**Dependencies:** none.

**Done when:** the parity test passes on every scalar link of the corpus;
`link-scores-from-fragments.AC1.1` to `AC1.4`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Scalar link scores through the per-link query
**Goal:** production scalar link scores come from `link_score_fragment`; the
whole-model equation list no longer carries them.

**Components:**
- `db/ltm/link_scores.rs` -- `link_score_fragment` keyed on
  `(target, live, model, project)`; `shaped_link_score` returns the live
  range and the score's metadata instead of an equation for the scalar
  `Bare` shape.
- `db/ltm/mod.rs` -- `LtmSyntheticVar::kind`, the metadata form for scalar
  links; `model_ltm_variables` stops holding their equations.
- `db/assemble.rs`, `db/ltm/compile.rs` -- `collect_fragments` and
  `model_ltm_fragment_diagnostics` take the fragment from the per-link
  query; a partial that does not lower reports against the target as the
  fragment path does today.
- `db/ltm_synthesis_tests.rs` -- execution counts over a literal edit and a
  rename under LTM (`exec_probe`).

**Dependencies:** Phase 1.

**Done when:** the LTM corpus and `simulate_ltm` are green; an unrelated
edit recompiles no scalar link score; `AC2.1` to `AC2.3`, `AC3.1`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Arrayed and per-element attribution
**Goal:** every reference shape (`FixedIndex`, `Wildcard`, `DynamicIndex`,
`PerElement`, mapped and iterated axes) is a live range over the target's
fragment or one of its per-element segments.

**Components:**
- `db/ltm/synthesize.rs` -- live ranges from `RefShape` and the occurrence IR
  (`ltm_ir`), per-element segments via `segment_member_by_element`.
- `db/ltm/link_scores.rs` -- the arrayed emitters (`emit_per_shape_link_scores`,
  the `PerElement` row pinning, the array-freeze helpers) produce live
  ranges; `ltm_augment_array_freeze.rs`, `ltm_augment_index.rs`,
  `ltm_augment_post_transform.rs` stop producing text for these shapes.
- `tests/integration/ltm_synthesis_parity.rs` -- every arrayed fixture
  (`arrayed_population_ltm`, `cross_element_ltm`, `cross_agg_ltm`,
  `ltm_array_agg`), plus C-LEARN ignored/release.

**Dependencies:** Phase 2.

**Done when:** parity on every arrayed shape; `AC2.4`, `AC2.5`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: The formula families, and no text anywhere
**Goal:** flow-to-stock, black-box, aggregate, composite and loop scores are
typed builders; the text generators, `LtmArm`, `LtmEquation` and
`model_ltm_implicit_var_info` are gone.

**Components:**
- `db/ltm/synthesize.rs` -- builders for each family, over slot reads.
- `db/ltm/link_scores.rs`, `db/ltm/loops.rs`, `db/ltm/mod.rs` -- every
  emitter produces metadata plus a builder call.
- `ltm_augment.rs` and its `ltm_augment_*.rs` siblings -- deleted, with
  their tests moved to the synthesized-program goldens.
- `db/layout.rs` -- section 3b (LTM implicit helpers) removed.
- `db/ltm_char_tests.rs` -- goldens over opcode streams.
- `docs/design/ltm--loops-that-matter.md` -- the derivation described as a
  rewrite.

**Dependencies:** Phase 3.

**Done when:** DoD 1 and 4 hold; `AC1.5`, `AC3.2`, `AC4.1`, `AC4.2`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: The `LinkScore` opcode and shared deltas
**Goal:** one opcode per link score at run time, deltas once per variable
per step, in both backends.

**Components:**
- `compiler/symbolic.rs`, `bytecode.rs` -- `LinkScore` row and twin;
  `SymDelta`/delta slots in the layout (section 3c) and the per-step delta
  program.
- `vm.rs` -- evaluation, including the initial-step and zero-delta guards
  the scaffold encoded.
- `wasmgen/lower.rs`, `wasmgen/module.rs` -- lowering and the delta region.
- `tests/integration/simulate_ltm.rs`, `simulate_ltm_wasm.rs` -- series
  unchanged, both backends agree; `CompiledSimulation::bytecode_profile` on
  C-LEARN recorded in the ledger.

**Dependencies:** Phase 4 (the builder is the one place the guard is
emitted).

**Done when:** DoD 5; `AC5.1` to `AC5.3`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: The post-pass over saved states (native)
**Goal:** the LTM program runs after the simulation, over every-dt saved
states, parallel across steps.

**Components:**
- `vm.rs` -- every-dt state retention when LTM is assembled, the post-pass
  driver over a step range, a rayon fan-out on native.
- `libsimlin/src/simulation.rs` -- the pass runs inside `run_to_end` so the
  results a caller reads are unchanged.
- `tests/integration/simulate_ltm.rs` -- series identical to the in-loop
  form; a run with `save_step > dt` still scores every dt.

**Dependencies:** Phase 5.

**Done when:** DoD 6's run figure; `AC6.1`, `AC6.2`. Separable: Phases 1 to
5 stand without it.
<!-- END_PHASE_6 -->

## Additional Considerations

**Lag alignment.** The text generators leave an original `PREVIOUS(..)` alone
and never lag it twice; the rewrite must leave an original `SymLoadPrev`
alone for the same reason, and the parity test is what proves the rule is
the same one. The implicit globals (`time`, `dt`) are read through
`LoadGlobalVar`; whether the generators freeze `TIME` inside a partial is
settled by running both paths, not by reading the generators.

**Where a partial cannot be taken.** Today a target whose equation the
generator cannot handle raises `PartialEquationError` and a `Warning`, and
the link is skipped (GH #311). A fragment always exists for a variable that
compiles, so the corresponding condition is a target that does not compile;
the diagnostic stays a warning naming the target and the link is skipped, as
now. A target that is itself a module instance takes the composite path, not
a partial.

**Cross-module reads.** `m·x` in a target's fragment is a `LoadVar` of the
module block's slot with an element offset (`compiler::context::resolve`);
freezing it is a `SymLoadPrev` of the same reference, so a sub-model
variable read at the previous step needs nothing new.

**Diagnostics text.** `LtmArm::text` exists for the characterization dump
and the partial-equation warning. After Phase 4 the dump is an opcode
listing (`SymbolicByteCode` prints), and the warning names the target and
the source rather than quoting an equation.

**What this plan does not change.** Which links, loops and partitions exist
(`model_causal_edges`, `model_loop_circuits`, discovery mode, the pinned
loops), the names and slots of the synthetic variables (so
`flattened_offsets`, the FFI and `ltm_post` are untouched), and the
relative-loop-score post-processing.

**Ledger.** Recorded per phase, as `docs/design-plans/2026-08-25-compiler-unification.md` does:
C-LEARN v77 LTM compile (`CLEARN_LTM=1 CLEARN_PROFILE=compile CLEARN_COMPILE_ITERS=2`,
retired instructions), database residency after the compile
(`CLEARN_RESIDENCY=1`), flow opcodes per step (`bytecode_profile`), and the
run (`run_to_end`, retired instructions). Baseline at `e021aa0a`: 48.6 G
(two extra compiles), 222 MiB, 855,713 opcodes, 1,291 ms wall.
