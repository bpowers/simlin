# Element-Level Cycle Resolution — Phase 2 Implementation Plan

**Goal:** A multi-variable element-acyclic recurrence SCC evaluates in
interleaved per-element order, in **both** the dt (flows) and init (initials)
phases.

**Architecture:** Phase 1 resolves single-variable self-recurrences (they
already lower correctly within their one existing fragment). A *multi-variable*
recurrence SCC (e.g. C-LEARN's emissions/target cluster, or `ref.mdl`'s
`ce`/`ecc`) needs its members evaluated in interleaved per-element order across
variables — inexpressible with today's one-contiguous-`AssignCurr`-block-per-
variable lowering. Phase 2 adds a **combined-fragment lowering path**:
interleave the SCC members' per-element lowered `Expr::AssignCurr` slices in
the computed `element_order` into one synthetic `PerVarBytecodes`, inject it at
the SCC's runlist slot, and skip the members during per-variable fragment
collection. Variable layout offsets and the results map are unchanged (the
combined fragment writes the same `member_base + elem` slots, just reordered),
so per-variable result series stay individually addressable.

**Tech Stack:** Rust (`simlin-engine`), salsa, `compiler::Expr` lowering,
`compiler::symbolic` bytecode assembly.

**Scope:** Phase 2 of 7. **Depends on Phase 1** (Tarjan promotion, the
per-element dt relation builder + verdict, `ResolvedScc`/`SccPhase`/
`resolved_sccs`).

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`).

---

## Design deviations (verified — these override the design doc)

1. **REVISED (supersedes the original deviation #1; see GH #575).
   `Expr::AssignCurr` operands are per-variable *mini-slots*, NOT
   cross-member-comparable absolute model slots.** `var_phase_lowered_exprs_prod`
   → `lower_var_fragment` (`db_var_fragment.rs:532-877`) builds a *fresh
   per-variable mini-layout*: `mini_offset` starts at
   `crate::vm::IMPLICIT_VAR_COUNT` (=4 for root), the variable itself is
   placed first, then its own deps, then implicit vars;
   `rmap = ReverseOffsetMap::from_layout(&mini_layout)`. So every SCC member's
   own variable sits at slot 4 in *its own* layout, every member's
   `AssignCurr` write-slots collide, and a member's cross-member reads land on
   *that member's private* dep mini-slots. Consequently the original plan's
   "interleave raw `Vec<Expr>` and recompile through one shared context"
   mechanism is **unimplementable**, and Phase 1's `phase_element_order` (which
   keys an element graph on raw `AssignCurr` slots) builds **zero cross-member
   edges** for any multi-member SCC — it produces a wrong topological order
   *and* (fatally) resolves a genuine multi-variable element cycle
   (`a[i]=b[i];b[i]=a[i]`) as acyclic (unsound, violates the AC4 loud-safe
   hard rule). It is masked today only by the `members.len() != 1`
   short-circuit in `refine_scc_to_element_verdict` (`db_dep_graph.rs:1023`),
   which this phase must remove.

   **Both the multi-member verdict AND the combined-fragment interleave must
   operate at the SYMBOLIC layer.** `compile_var_fragment`'s `compile_phase`
   closure (`db.rs:~3670-3757`) already compiles a variable's phase exprs
   through its own correct mini-context and `symbolize_bytecode`s the result
   into a `PerVarBytecodes { symbolic: SymbolicByteCode, .. }` whose every
   variable reference is a layout-independent `SymVarRef { name: String,
   element_offset: usize }` (`compiler/symbolic.rs:37-42`). `SymVarRef` IS the
   cross-member-comparable identity the verdict and the interleave need.
   `concatenate_fragments`/`renumber_opcode` (`symbolic.rs:1218-1400`) already
   merge `&[&PerVarBytecodes]` with per-fragment resource renumbering;
   `resolve_module` (`symbolic.rs:826-` / the `assemble_module` call site)
   resolves `SymVarRef` → real model offsets at assembly so variable layout
   offsets and the results map are unchanged. The plan's "recompile the
   combined `Vec<Expr>`" is replaced by "compile each member independently
   (existing `compile_phase`), split each member's `SymbolicByteCode` into
   per-element segments delimited by its per-element write opcode
   (`AssignCurr`/`AssignConstCurr`/`BinOpAssignCurr`, element id =
   `SymVarRef.element_offset`), and interleave the segments in
   `ResolvedScc.element_order` with per-member resource renumbering — a
   per-element-granular generalization of `concatenate_fragments`."
2. **The runlist `Vec<String>` is salsa-owned and immutable at the
   `assemble_module` site** (`db.rs:4303-4307` reads `&ModelDepGraphResult`).
   "Remove members from the per-variable runlist" means **skip SCC members
   during fragment collection at `db.rs:4450-4486` and inject the synthetic
   combined fragment there**, not mutate the `Vec<String>`.
3. **Init phase does NOT use `concatenate_fragments`.** Flows/stocks use
   `concatenate_fragments` (`db.rs:4556`/`4567`); **initials are renumbered
   one-fragment-at-a-time into `Vec<SymbolicCompiledInitial>` keyed by each
   variable's `Ident`** (`db.rs:4583-4635`, key at `db.rs:4617-4619`). A
   combined init-phase fragment becomes ONE `SymbolicCompiledInitial` with one
   synthetic ident — **representability through `resolve_module`
   (`db.rs:4719`) is the single highest-priority verification gap** and is
   addressed by a spike task FIRST (Task 1), with a loud-safe fallback.
4. **No `init_walk_successors` exists.** The init successor relation is
   inlined in `compute_inner` at `db.rs:1206-1214`
   (`info.initial_deps` filtered to `var_info.contains_key`, **no
   stock-breaking**; the dt stock sink `db.rs:1171-1175` is `!is_initial`-
   gated). Phase 2 extracts a `pub(crate) fn init_walk_successors` in
   `db_dep_graph.rs` and refactors `compute_inner` to call it (matching the
   `db_dep_graph.rs` "single shared relation, never re-derive" pattern).
5. **No init-isolating fixture exists.** `ref.mdl`, `interleaved.mdl`,
   `self_recurrence.mdl` are all stock-free aux-only (their recurrence sits in
   *both* dt and init relations only incidentally). AC2.4 (an init-phase
   recurrence where a stock breaks the dt chain but **not** the init chain)
   needs a **new fixture** authored in this phase.
6. **No bytecode-determinism test exists.** AC2.3's byte-stable combined
   fragment is a new test obligation (relation/SCC-level determinism is
   already tested at `db_dep_graph_tests.rs:204-222`/`94-110`).
7. **`incremental_compilation_covers_all_models`
   (`tests/simulate.rs:1525-1569`) skips `.mdl`** (filters to
   `.stmx`/`.xmile`, `simulate.rs:1540`), so `ref.mdl`/`interleaved.mdl` are
   not auto-covered there; AC2.6 is about the existing 22-model
   `ALL_INCREMENTALLY_COMPILABLE_MODELS` + `TEST_MODELS` set staying green.

---

## Acceptance Criteria Coverage

### element-cycle-resolution.AC2: Multi-variable recurrence SCC resolves
- **element-cycle-resolution.AC2.1 Success:** `test/sdeverywhere/models/ref/ref.mdl` compiles and simulates to its hand-computed per-element series.
- **element-cycle-resolution.AC2.2 Success:** `test/sdeverywhere/models/interleaved/interleaved.mdl` compiles and simulates to its hand-computed values.
- **element-cycle-resolution.AC2.3 Success:** The SCC is lowered as one combined fragment; variable offsets and the results offset map are unchanged vs. a hypothetical acyclic equivalent (per-variable result series remain individually addressable).
- **element-cycle-resolution.AC2.4 Success:** Both dt-phase and init-phase recurrence SCCs are resolved (a fixture with an init-phase recurrence simulates correctly).
- **element-cycle-resolution.AC2.5 Edge:** `ref_interleaved_inter_variable_cycles_report_circular` is transitioned to assert correct simulation (not `CircularDependency`).
- **element-cycle-resolution.AC2.6 Failure:** `incremental_compilation_covers_all_models` and the existing model corpus stay green (no regression on non-recurrence models).

---

## Testing conventions

Same as Phase 1: TDD mandatory; `db_dep_graph.rs` unit tests in
`db_dep_graph_tests.rs`; `assemble_module`/bytecode unit tests in `db.rs`'s
in-module `#[cfg(test)] mod tests` or a sibling `db_*_tests.rs` per the
per-file-line-cap convention; end-to-end fixtures in `tests/simulate.rs`
(`--features file_io`). Verify via `git commit` (pre-commit, 180s cap, never
`--no-verify`). `ref.mdl`/`interleaved.mdl` are tiny — no `#[ignore]`.

---

<!-- START_TASK_1 -->
### Task 1: SPIKE — init-phase combined-fragment representability

**Verifies:** none (de-risking spike; gates Task 6's design)

**Files:**
- Read only: `src/simlin-engine/src/db.rs:4583-4635` (init `SymbolicCompiledInitial` renumbering), `db.rs:4617-4619` (per-ident keying), `db.rs:4719` (`resolve_module`), the `SymbolicCompiledInitial` definition, and the downstream init-resolution/consumption path.
- Write: `docs/implementation-plans/2026-05-18-element-cycle-resolution/phase_02_spike_findings.md` (the decided mechanism + fallback).

**Implementation:**
This is an infrastructure/investigation task (no TDD). Determine **how a
combined init-phase fragment can be represented** given that initials are
renumbered per-`Ident` into `Vec<SymbolicCompiledInitial>` and resolved
per-variable. Concretely answer:
- Is `compiled_initials` consumed strictly per-variable-ident downstream of
  `db.rs:4719`, or can a single `SymbolicCompiledInitial` carrying a synthetic
  ident write multiple members' init slots?
- Can the combined init fragment be emitted as one `SymbolicCompiledInitial`
  with a synthetic ident whose bytecode writes each member's
  `member_base + elem` init slot (the same offsets, reordered), analogous to
  the flows combined fragment? Verify `resolve_module` and the init code
  generation do not assume a 1:1 ident↔init-slot mapping.
- If representable: record the exact mechanism (where to inject, what ident,
  how offsets stay correct).
- If NOT cleanly representable: record the **loud-safe fallback** — resolve dt
  combined fragments normally, but for an init-phase recurrence SCC keep
  `has_cycle`/`CircularDependency` (conservative). NOTE: C-LEARN reports both
  a dt and an init cycle, so the fallback would leave C-LEARN blocked on
  init; the spike must therefore push to find a representable mechanism (e.g.
  emit per-member `SymbolicCompiledInitial`s whose bytecodes are individually
  correct but ordered so cross-member element dependencies are satisfied —
  feasible only if init slots are written, not accumulated; verify).
- **HARD GATE — STOP and surface, do not silently fall back.** If the spike
  concludes the init combined fragment is **not** representable by any
  mechanism, the implementation MUST STOP and surface to the user before
  proceeding past this task. Do **not** silently apply the loud-safe init
  fallback: it strands AC2.4 *and* leaves C-LEARN's init cycle blocked (Phase
  6 depends on Phase 2), so it is a plan-invalidating outcome that requires a
  human decision, not an autonomous degrade. Also file via the `track-issue`
  agent. Only proceed to Tasks 2-6 once the spike has a representable init
  mechanism (or the user has explicitly accepted a revised scope).

**Verification:**
The findings doc exists and unambiguously states the chosen init mechanism
(or the fallback + its consequence for C-LEARN). Reviewed before Task 6.

**Commit:** `doc: phase 2 init combined-fragment representability spike`
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-3) -->

<!-- START_TASK_2 -->
### Task 2: Extract `init_walk_successors` and refactor `compute_inner`

**Verifies:** element-cycle-resolution.AC2.4 (init relation foundation), element-cycle-resolution.AC2.6 (no behavior change for existing models)

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` (add `pub(crate) fn init_walk_successors`, mirroring `dt_walk_successors` at `db_dep_graph.rs:83-103`)
- Modify: `src/simlin-engine/src/db.rs:1206-1214` (`compute_inner` init branch calls the new fn)

**Implementation:**
Add `pub(crate) fn init_walk_successors<'a>(var_info: &'a HashMap<String, VarInfo>, name: &str) -> Vec<&'a str>` returning:
- `[]` if `name` absent or the var is a Module (module early-return applies to
  both phases, `db.rs:1178-1186`);
- otherwise `var_info[name].initial_deps` (`VarInfo.initial_deps`,
  `db_dep_graph.rs:48`) filtered **only** to `var_info.contains_key(dep)` —
  **NO stock filter and NO stock sink** (a stock is a valid init-relation
  node; the dt stock sink `db.rs:1171-1175` is `!is_initial`-gated). This
  exactly reproduces the current inlined init logic at `db.rs:1209-1211`.
- Return in `BTreeSet`-sorted order (deterministic, like `dt_walk_successors`).

Refactor `compute_inner`'s `db.rs:1206-1214` so the `is_initial` branch calls
`init_walk_successors(var_info, name)` and the else branch keeps
`dt_walk_successors(var_info, name)`. This is a **pure refactor** — behavior
must be byte-identical (the relation is now shared by construction, matching
the `db_dep_graph.rs` "single shared relation, never re-derive" pattern, so
later init introspection observes the engine's actual relation).

**Testing:**
`db_dep_graph_tests.rs`: a unit test asserting `init_walk_successors` returns
BTreeSet-sorted, omits modules, includes stock deps (no stock break), matches
`db.rs:1209-1211` semantics on a small fixture. Regression: the existing
corpus must be unchanged — covered operationally by the suite + Task 9's
`incremental_compilation_covers_all_models`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io init_walk_successors` and
the broader cycle tests — green (pure refactor).
**Commit:** `engine: extract init_walk_successors, share the init relation`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Init-phase per-element relation + verdict

**Verifies:** element-cycle-resolution.AC2.4

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` (init-phase SCC identification + per-element relation + verdict, reusing the Phase 1 builder parameterized by phase)
- Modify: `src/simlin-engine/src/db.rs` `model_dependency_graph_impl` init `has_cycle` block (`db.rs:1273-1287`)

**Implementation:**
Parameterize the Phase 1 per-element relation builder + verdict by `SccPhase`:
- Init SCC identification: same as dt but over `init_walk_successors`
  adjacency (Task 2). Self-loops detected directly from the init adjacency.
- Per-element init relation: for each member, source production **init**
  lowered exprs via `var_phase_lowered_exprs_prod(.., SccPhase::Initial)`
  (Phase 1 Task 3 accessor; it already selects `per_phase_lowered.initial`).
  Build `(member, element)` nodes/edges from the init `AssignCurr` RHS reads,
  exactly as the dt builder does. Init-phase semantics inherited: stocks do
  **not** break the init chain (init lowered exprs include the stock's init
  equation), `initial_previous_referenced_vars` already stripped by
  `build_var_info` (`db_dep_graph.rs:189`).
- Verdict: element-acyclic + element-sourceable ⇒ `ResolvedScc { phase: SccPhase::Initial, .. }`;
  else keep `CircularDependency` (loud-safe).
- Consume in `model_dependency_graph_impl`'s init block (`db.rs:1273-1287`)
  symmetrically to Phase 1 Task 5's dt block: a resolved init SCC is excluded
  from the init `CircularDependency` accumulation and recorded in
  `resolved_sccs` with `phase == Initial`.

**Testing:**
`db_dep_graph_tests.rs`: an init-phase recurrence `TestProject` where a stock
breaks the dt chain but the init relation has a forward element recurrence —
assert a `ResolvedScc { phase: Initial }` is produced and dt has no cycle. A
genuine init element cycle ⇒ unresolved (`CircularDependency`).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io` init-phase tests — pass.
**Commit:** `engine: init-phase per-element relation and verdict`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->

<!-- START_TASK_4 -->
### Task 4: Symbolic per-member fragment accessor + multi-member symbolic element-graph verdict (the correctness rebuild — GH #575)

**Verifies:** element-cycle-resolution.AC2.1, element-cycle-resolution.AC2.4, element-cycle-resolution.AC4 (loud-safe: genuine multi-variable element cycles stay rejected)

**Why this task exists:** Phase 1's `phase_element_order` builds the SCC
element graph from raw per-variable mini-slots and is structurally incapable
of cross-member edges (GH #575): for any multi-member SCC it produces a wrong
order *and* resolves a genuine multi-variable element cycle as acyclic
(unsound). Phase 2 must resolve multi-member SCCs, so the verdict's
element-graph construction must be rebuilt on the cross-member-comparable
**symbolic** representation BEFORE the combined fragment (Task 5) can consume
a correct `ResolvedScc`.

**Files:**
- Modify: `src/simlin-engine/src/db.rs` — add a `pub(crate)` accessor
  returning a variable's *symbolic* `PerVarBytecodes` for a phase (factor the
  existing `compile_phase` closure in `compile_var_fragment` so the SCC path
  reuses the exact production compile+symbolize, never a re-derivation).
- Modify: `src/simlin-engine/src/db_dep_graph.rs` — replace the mini-slot
  element-graph construction (`phase_element_order` / `slot_to_node` /
  `member_elements` / `element_node_key` / the `Expr`-based
  `collect_read_slots`) with a symbolic builder over `SymVarRef`; remove the
  `members.len() != 1` guard in `refine_scc_to_element_verdict`
  (`db_dep_graph.rs:1023`).

**Implementation:**
- **Accessor:** add `pub(crate) fn var_phase_symbolic_fragment_prod(db, model,
  project, var_name, phase: SccPhase) -> Option<PerVarBytecodes>`. It must
  reuse the *exact* production compile+symbolize path: factor the
  `compile_phase` closure body out of `compile_var_fragment`
  (`db.rs:~3670-3757`) into a `pub(crate)` helper taking the caller-owned
  context (`offsets`, `rmap`, `tables`, `module_refs`, `mini_offset`,
  `converted_dims`, `dim_context`, `model_name_ident`, `inputs`) + the phase
  `Vec<Expr>`, returning `Option<PerVarBytecodes>`; the accessor builds that
  context exactly as `var_phase_lowered_exprs_prod` does (mirror it
  byte-for-byte; select `per_phase_lowered.noninitial` for `Dt`, `.initial`
  for `Initial`), then calls the factored helper. `None` is the loud-safe
  signal (no `SourceVariable`, `Fatal`, `Var::new` error, or compile/
  symbolize failure) — never panic. `var_phase_lowered_exprs_prod` stays
  (Phase 3 still extends its no-`SourceVariable` arm); the symbolic accessor
  is the new SCC-graph source.
- **Symbolic element graph (replaces `phase_element_order`):** for each SCC
  member, get its symbolic `PerVarBytecodes` for the phase. Walk
  `symbolic.code`:
  - A per-element **write** is `SymbolicOpcode::AssignCurr { var } |
    AssignConstCurr { var, .. } | BinOpAssignCurr { var, .. }` where
    `var.name == member`. It defines node `(var.name.clone(),
    var.element_offset)` and terminates the current element segment.
  - The **reads** consumed by that element are the `SymVarRef`s in the
    opcodes since the previous write (inclusive of this write's own operand
    sub-expression already flattened into preceding stack ops):
    `LoadVar{var} | SymLoadPrev{var} | SymLoadInitial{var} |
    LoadSubscript{var} | PushVarView{var,..} | PushVarViewDirect{var,..}`,
    and `PushStaticView{view_id}` → resolve `static_views[view_id].base`; if
    `SymStaticViewBase::Var(v)` enumerate the exact element set the view
    addresses (mirror Phase 1 `collect_read_slots`'s `StaticSubscript`
    dims/strides/offset enumeration, but in symbolic space — exact, not an
    over-approximation, so genuinely element-acyclic models like `ref.mdl`
    still resolve). Reuse/adapt the existing
    `sym_var_refs_in_bytecode` enumeration shape (`symbolic.rs:767-782`) but
    split read-opcodes vs the write terminal and add the static-view base
    resolution.
  - For every read `SymVarRef { name, element_offset }` whose `name` is an
    SCC member, add edge `(name, element_offset) -> (write.name,
    write.element_offset)`. Over-approximation remains the loud-safe
    direction (preserve Phase 1's documented `collect_read_slots` contract:
    an extra edge only forces a conservative `CircularDependency`, never a
    wrong order; `SymLoadPrev` is included as an edge exactly as the
    `Expr`-level `Previous`-arg was — PREVIOUS-only recurrences stay
    protected upstream by `build_var_info`'s `dt_previous` strip at SCC
    *identification*, unchanged).
  - Node identity is the `(canonical-name, element_offset)` pair encoded
    byte-stably for `crate::ltm::scc_components` (keep the existing injective
    `element_node_key` U+241F scheme — it is already an opaque graph key — or
    an equivalent; `name` is a real canonical variable name here so it is
    well-formed). Element self-loop or element multi-SCC ⇒ `None`
    (unresolved, loud-safe). Acyclic ⇒ deterministic topological order over
    `(member, element)` (same sorted Kahn/tie-break discipline as Phase 1,
    so byte-stable).
- **Unify N=1 and N≥2.** `refine_scc_to_element_verdict` drops the
  `members.len() != 1` short-circuit and calls the symbolic builder for all
  SCCs; single-variable self-recurrence is just the N=1 case of the same
  builder. The `SccPhase::Dt`/`Initial` branch structure (Dt requires
  init-element-acyclicity too; Initial is init-only) is preserved exactly.
  All Phase 1 + Subcomponent A single-member regression guards
  (`self_recurrence_resolves_and_no_self_token_leak`,
  `genuine_cycles_still_rejected`, `resolve_dt_*`, `resolve_init_*`,
  `dt_cycle_sccs_*`, byte-stability) MUST stay green unchanged — they encode
  the correct single-member behavior.

**Testing (RED-first, `db_dep_graph_tests.rs`):**
- A two-member `ref.mdl`-shaped SCC (`ce`/`ecc`, `ce[tNext]=ecc[tPrev]+1;
  ecc[tNext]=ce[tNext]+1`) ⇒ `Resolved` with the correct **interleaved**
  `element_order` (ce[0],ecc[0],ce[1],ecc[1],…) — RED before the rebuild
  (today it is short-circuited `Unresolved`), GREEN after.
- A genuine multi-variable element 2-cycle (`a[i]=b[i]; b[i]=a[i]`) ⇒
  `Unresolved` (the GH #575 unsoundness fix — this is the load-bearing
  correctness assertion; it must fail RED if the symbolic builder is wrong).
- A genuine scalar 2-cycle (`a=b+1; b=a+1`) ⇒ `Unresolved`.
- Single-variable forward self-recurrence ⇒ `Resolved`, byte-identical
  `element_order` to Phase 1 (regression: N=1 unchanged).
- `interleaved.mdl`-shaped element-acyclic-through-2-cycle ⇒ `Resolved`.
- A member whose symbolic fragment is unsourceable ⇒ `Unresolved`, no panic.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --lib db_dep_graph` —
RED on the new multi-member + unsoundness tests before, GREEN after, and the
entire existing `db_dep_graph` suite still green (no single-member
regression).
**Commit:** `engine: rebuild SCC element-graph verdict on symbolic refs (multi-member, GH #575)`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Interleave members' symbolic per-element segments into one combined `PerVarBytecodes`

**Verifies:** element-cycle-resolution.AC2.1, element-cycle-resolution.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (a new `pub(crate)` helper that, given a `ResolvedScc` and its members' symbolic `PerVarBytecodes` from the Task 4 accessor, produces one combined `PerVarBytecodes`)

**Implementation:**
Each member's symbolic `PerVarBytecodes` (Task 4 accessor) is, for an arrayed
member, a sequence of per-element computations each ending in that element's
write opcode (`SymbolicOpcode::AssignCurr | AssignConstCurr | BinOpAssignCurr`
with `var.name == member`, element id = `var.element_offset`). Build one
combined `PerVarBytecodes` whose element writes follow
`ResolvedScc.element_order`:
- **Segment** each member's `symbolic.code` into per-element slices: the slice
  for element `e` is the run of opcodes up to and including the write opcode
  whose `var.element_offset == e` (strip any trailing `Ret`). Validate every
  member element in `element_order` maps to exactly one segment (a missing /
  duplicate / non-contiguous segment ⇒ loud-safe error, caller keeps
  `CircularDependency`).
- **Resources are member-scoped, not element-scoped.** Compute each member's
  resource base offsets (literals, graphical_functions, module_decls,
  static_views, temp_sizes, dim_lists) ONCE per member exactly as
  `concatenate_fragments` (`symbolic.rs:1218-1309`) computes them per
  fragment, and apply that member's offsets (via the existing
  `renumber_opcode`, `symbolic.rs:1327-1400`) to every opcode in every
  segment of that member. Merge the side-channels per member exactly as
  `concatenate_fragments` does (this is a per-element-granular generalization
  of `concatenate_fragments`; factor the shared renumber/merge logic rather
  than duplicating it).
- **Emit** the renumbered segments in `ResolvedScc.element_order` order,
  concatenated, followed by a single trailing `SymbolicOpcode::Ret`. Each
  write keeps its original `SymVarRef { name, element_offset }` (only segment
  ordering changes), so after `resolve_module` the variable layout offsets
  and the results map are unchanged (AC2.3) and per-variable series remain
  individually addressable.
- Determinism: `element_order` is byte-stable (Task 4 sorted topo);
  per-member resource offsets are assigned in `element_order`'s member
  first-encounter order; the interleave is a pure reordering ⇒ the combined
  `PerVarBytecodes` is byte-stable.

**Testing:**
`db.rs` in-module `#[cfg(test)]`: given a hand-built two-member SCC with known
member symbolic `PerVarBytecodes` and a known `element_order`, assert the
combined `PerVarBytecodes`: segments appear in `element_order`; every member
element's write `SymVarRef` is present exactly once with its original
`name`/`element_offset`; exactly one trailing `Ret`; per-member resource ids
correctly renumbered (no collision across members); side-channels merged.
Numeric correctness is the Task 7/8 end-to-end job — keep this unit focused on
structural well-formedness.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io combined_fragment` (new
test name) — pass.
**Commit:** `engine: interleave members' symbolic segments into one combined PerVarBytecodes`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Inject the combined fragment in `assemble_module` (dt + init)

**Verifies:** element-cycle-resolution.AC2.1, element-cycle-resolution.AC2.2, element-cycle-resolution.AC2.3, element-cycle-resolution.AC2.4

**Files:**
- Modify: `src/simlin-engine/src/db.rs` `assemble_module` fragment-collection
  loops for flows; the init `SymbolicCompiledInitial` path per the **Task 1
  spike mechanism** (see `phase_02_spike_findings.md`). Locate the loops by
  name/content — line numbers shifted from Phase 1/2A edits; the Task 1 spike
  re-verified current locations (flow-fragment collection ~`db.rs:4615-4633`,
  init renumber loop ~`db.rs:4739-4795`, `resolve_module` ~`db.rs:4879`).

**Implementation:**
- Read `dep_graph.resolved_sccs` (the `ResolvedScc` payload, now correctly
  populated for multi-member SCCs by the Task 4 rebuild). For each
  `ResolvedScc`, build the combined `PerVarBytecodes` (Task 5) for its phase
  via the Task 4 symbolic accessor + Task 5 interleave.
- **Flows (dt):** in the `runlist_flows` fragment-collection loop, **skip**
  every dt-`ResolvedScc` member's per-variable `flow_bytecodes` push, and at
  the position of the first SCC member encountered, push the combined
  fragment's `&PerVarBytecodes` instead. The combined `PerVarBytecodes` must
  be **owned** somewhere that outlives the `concatenate_fragments` call (the
  existing vectors hold `&` borrows into `all_fragments`); allocate it in a
  local that lives to the end of `assemble_module` and push a reference. The
  runlist `Vec<String>` itself is **not** mutated (it is salsa-owned).
- **Initials (init):** apply the Task 1 spike's **validated** mechanism (HARD
  GATE PASSED, `phase_02_spike_findings.md`): for each init-`ResolvedScc`,
  emit ONE `SymbolicCompiledInitial` carrying a synthetic ident
  (`$⁚scc⁚init⁚{n}`) whose bytecode is the Task 5 combined fragment, at the
  first init SCC member's slot in the `initial_frags` collection loop, and
  skip the members' per-ident init entries. The spike verified
  `resolve_module`/`eval_initials` consume `compiled_initials` positionally
  (ident-agnostic) and that one `SymbolicCompiledInitial` may write multiple
  members' init slots, so this needs ZERO changes to `resolve_module` /
  `compute_layout` / the renumber loop / the VM init runner. If during
  implementation the spike's mechanism is found to genuinely contradict the
  code (not mere inconvenience), STOP and surface — the loud-safe init
  fallback is NOT an autonomous path.
- `concatenate_fragments` stays agnostic and unchanged — the combined
  fragment is just another `PerVarBytecodes`, mirroring how LTM synthetic
  fragments are appended.
- Variable layout (`compute_layout`) and `resolve_module` are untouched; the
  combined fragment's writes keep their original `SymVarRef`
  name/element_offset, so `resolve_module` maps them to the same model slots
  and the results offset map is unchanged (AC2.3).

**Testing:**
End-to-end via Tasks 7/8 (the real proof). Add a focused `db.rs`
`#[cfg(test)]` assertion that, for a resolved multi-variable SCC, the assembled
module's results offset map for each member is identical to the offsets a
hypothetical acyclic equivalent would get (AC2.3 — per-variable series
individually addressable), e.g. compare member offsets to the
non-SCC-member layout.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io assemble` — pass.
**Commit:** `engine: inject combined SCC fragment in assemble_module (dt+init)`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-10) -->

<!-- START_TASK_7 -->
### Task 7: End-to-end — `ref.mdl` compiles and simulates

**Verifies:** element-cycle-resolution.AC2.1

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (add/repurpose a test for `test/sdeverywhere/models/ref/ref.mdl`; it has a sibling `ref.dat`)

**Implementation:**
`ref.mdl` is a two-variable inter-element recurrence (`ce[t1]=1;
ce[tNext]=ecc[tPrev]+1; ecc[t1]=ce[t1]+1; ecc[tNext]=ce[tNext]+1` over
subrange `t1..t3`). Its `ref.dat` encodes the hand-computed series
`ce[t1]=1, ce[t2]=3, ce[t3]=5, ecc[t1]=2, ecc[t2]=4, ecc[t3]=6` (constant
across both saved steps). Add a `#[test]` that runs `ref.mdl` through
`simulate_mdl_path` (`tests/simulate.rs:286-305`) which loads `ref.dat` via
`load_expected_results_for_mdl` and compares with `ensure_results`. (Note:
the assertion that `ref.mdl` is `CircularDependency` lives in
`ref_interleaved_inter_variable_cycles_report_circular` — Task 9 transitions
it; this Task 7 test asserts correct simulation.)

**Testing:**
The test IS AC2.1. It must fail before Tasks 4-6 (currently rejected) and pass
after (matches `ref.dat`).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io ref_mdl` (or chosen
name) — passes against `ref.dat`.
**Commit:** `engine: ref.mdl multi-variable recurrence simulates (AC2.1)`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: End-to-end — `interleaved.mdl` compiles and simulates

**Verifies:** element-cycle-resolution.AC2.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (test for `test/sdeverywhere/models/interleaved/interleaved.mdl`; sibling `interleaved.dat`)

**Implementation:**
`interleaved.mdl`: `x=1; a[A1]=x; a[A2]=y; y=a[A1]; b[DimA]=a[DimA]`
(`DimA: A1,A2`). Whole-variable `a`↔`y` is a 2-cycle, but element-wise
`x → a[A1] → y → a[A2]` is acyclic. `interleaved.dat` = all `1.0` (101 steps).
Add a `#[test]` running it via `simulate_mdl_path`, comparing against
`interleaved.dat`.

**Testing:** The test IS AC2.2 (fails before, passes after Tasks 4-6).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io interleaved` — passes.
**Commit:** `engine: interleaved.mdl element interleave simulates (AC2.2)`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Transition the inter-variable-cycle assertion + new init fixture

**Verifies:** element-cycle-resolution.AC2.4, element-cycle-resolution.AC2.5, element-cycle-resolution.AC2.6

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` `ref_interleaved_inter_variable_cycles_report_circular` (`tests/simulate.rs:1356-1387`)
- Create: `test/sdeverywhere/models/init_recurrence/init_recurrence.mdl` (+ `init_recurrence.dat` with hand-computed expected values) — a NEW init-phase-recurrence fixture
- Modify: `src/simlin-engine/tests/simulate.rs` (a `#[test]` for the new init fixture)

**Implementation:**
- `ref_interleaved_inter_variable_cycles_report_circular` currently asserts
  `CircularDependency` for both `ref.mdl` and `interleaved.mdl`
  (`tests/simulate.rs:1366-1385`). Transition it to assert **correct
  simulation** for both (or fold its intent into Tasks 7/8 and replace this
  test's body with the correct-simulation assertions). The "no
  `UnknownDependency`/`DoesNotExist` leak" intent should be preserved as a
  guard if still meaningful (AC2.5).
- Author a **new init-phase recurrence fixture** (AC2.4): a model where a
  stock breaks the dt chain (so dt is acyclic) but the **init** relation has
  a forward element recurrence across variables (so only the init element
  graph exercises the combined init fragment). Hand-compute its expected
  series into `init_recurrence.dat` (follow the `ref`/`interleaved` fixture
  convention: `.mdl` + `.dat`). Add a `#[test]` running it via
  `simulate_mdl_path` asserting it simulates correctly — this is the AC2.4
  init-phase proof. (The executor must empirically confirm the chosen MDL
  shape actually produces an init-only element SCC by inspecting
  `resolved_sccs`/the init verdict; adjust the fixture until it does.)
- **Bounded-attempt + escalation:** if, after a reasonable bounded number of
  attempts (≈4-5 distinct MDL shapes — stock-init recurrence variants,
  different subrange/builtin combinations), no shape produces a genuine
  init-only element SCC (dt acyclic via a stock break, init element
  recurrence), STOP: do not fabricate a passing assertion or weaken AC2.4.
  Surface to the user and file via the `track-issue` agent (Task tool,
  `subagent_type: "track-issue"`) that an init-isolating fixture could not be
  constructed, with the shapes tried and the observed `resolved_sccs`/verdict
  output, so the gap is explicitly tracked rather than silently dropped.

**Testing:**
AC2.5 (transitioned test green), AC2.4 (new init fixture simulates), AC2.6
(no corpus regression — see Task 10).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io ref_interleaved init_recurrence` — pass.
**Commit:** `engine: transition inter-variable-cycle test; add init-recurrence fixture (AC2.4, AC2.5)`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: Byte-stable combined fragment + corpus regression gate

**Verifies:** element-cycle-resolution.AC2.3, element-cycle-resolution.AC2.6

**Files:**
- Modify: `src/simlin-engine/src/db.rs` `#[cfg(test)]` (combined-fragment byte-stability test) or `db_dep_graph_tests.rs`
- Verify only: `src/simlin-engine/tests/simulate.rs` `incremental_compilation_covers_all_models` (`tests/simulate.rs:1525-1569`)

**Implementation:**
- Add a determinism test: compile `ref.mdl` (or the multi-var `TestProject`)
  twice on fresh databases; assert the assembled combined fragment's
  bytecode/`resolved_sccs`/`element_order` are byte-identical across runs
  (new obligation — no existing bytecode-determinism test; model on
  `dt_cycle_sccs_is_byte_stable_across_runs:204-222`).
- Confirm `incremental_compilation_covers_all_models` (the 22-model
  `ALL_INCREMENTALLY_COMPILABLE_MODELS` + `TEST_MODELS`) stays green — no
  regression on non-recurrence models (AC2.6). This is the existing corpus
  gate; do not weaken it.

**Verification:**
Run the full engine suite via `git commit` (pre-commit `cargo test` under the
180s cap): `incremental_compilation_covers_all_models` green, new
determinism test green, all of `simulates_*` green.
**Commit:** `engine: byte-stable combined fragment; corpus regression gate green (AC2.3, AC2.6)`
<!-- END_TASK_10 -->

<!-- END_SUBCOMPONENT_C -->

---

## Phase 2 Done When

- `ref.mdl` and `interleaved.mdl` compile and simulate to their hand-computed
  values (Tasks 7, 8 — AC2.1, AC2.2).
- The SCC is one combined fragment; variable offsets and the results map are
  unchanged; per-variable series individually addressable (Tasks 4-6, 10 —
  AC2.3).
- Both dt and init recurrence SCCs resolve; the new init-phase fixture
  simulates (Tasks 1-3, 9 — AC2.4).
- `ref_interleaved_inter_variable_cycles_report_circular` transitioned to
  correct simulation (Task 9 — AC2.5).
- `incremental_compilation_covers_all_models` and the existing corpus stay
  green; combined fragment byte-stable (Task 10 — AC2.6).
- Full engine suite green under the 3-minute `cargo test` pre-commit cap.
