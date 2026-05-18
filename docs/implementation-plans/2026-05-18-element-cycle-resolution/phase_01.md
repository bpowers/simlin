# Element-Level Cycle Resolution — Phase 1 Implementation Plan

**Goal:** A single-variable self-recurrence SCC (a dt self-loop whose induced
element graph is acyclic, e.g. `ecc[t2]=ecc[t1]+1`) is resolved and simulates;
genuine cycles stay rejected.

**Architecture:** Keep the fast whole-variable dt cycle gate's acyclic happy
path unchanged. Only when `model_dependency_graph_impl` finds a back-edge,
identify the offending SCC over the existing `dt_walk_successors` relation,
refine *just that SCC* into an exact `(member, element-offset)` graph built
from the engine's own production-lowered per-element `Expr::AssignCurr`
expressions, and run the (newly promoted) iterative Tarjan primitive on it. If
element-acyclic and every member is element-sourceable, emit a per-element
topological order and do **not** set `has_cycle`; otherwise keep the
conservative `CircularDependency` (loud-safe fallback). The single-variable
case needs no combined fragment — it already lowers correctly within its one
existing fragment via declared-order `SubscriptIterator` (Phase 2 adds the
multi-variable combined fragment).

**Tech Stack:** Rust (`simlin-engine` crate), salsa incremental framework,
existing `compiler::Expr` lowering, `ltm::indexed` Tarjan.

**Scope:** Phase 1 of 7 from `docs/design-plans/2026-05-18-element-cycle-resolution.md`.

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`), via
codebase-investigator. Key verified facts and design deviations are folded
into the tasks below.

---

## Design deviations (verified — apply these, they override the design doc)

1. **Two `#[cfg(test)]` gates, not one.** `scc_components` is gated in *two*
   places: the `fn` at `src/simlin-engine/src/ltm/indexed.rs:207` **and** the
   re-export `#[cfg(test)] pub(crate) use indexed::scc_components;` at
   `src/simlin-engine/src/ltm/mod.rs:58-59`. The production caller uses the
   re-export path `crate::ltm::scc_components`. Both gates must be removed.
2. **Phase 1 needs a new production lowering accessor.** The design says reuse
   the `var_noninitial_lowered_exprs` bridge, but that function is
   `#[cfg(test)]`-gated (`db_dep_graph.rs:435`, `#[cfg(test)]` attr at line
   434) and *panics* when a variable has no `SourceVariable` /
   `LoweredVarFragment::Fatal` / `Var::new` errors. Production code cannot
   panic. Phase 1 introduces a **new production `pub(crate)` accessor** that
   returns `Option<Vec<Expr>>` (`None` ⇒ "not element-sourceable" ⇒ loud-safe
   fallback to `CircularDependency`). The existing `#[cfg(test)]`
   `var_noninitial_lowered_exprs` / `array_producing_vars` panic wrapper is
   left **unchanged** (preserves AC3.3; Phase 3 extends the production
   accessor with parent-`implicit_vars` sourcing).
3. **`self_recurrence/` has no expected `.dat`.** AC1.2 is verified by
   asserting the series **in-test** using the existing `element_series`
   helper. The helper is **defined at `tests/simulate.rs:1042`**; the call
   idiom to copy is in `previous_self_reference_still_resolves`
   (`tests/simulate.rs:1289-1291`). No `.dat` is added.
4. **`self_recurrence_self_token_resolves_to_real_name`
   (`tests/simulate.rs:1097-1146`) is a 3-assertion #559 regression guard**,
   not a pure `CircularDependency` test. Phase 1 inverts assertions (1)
   `compile_err` and (3) `has_circular` (model now compiles & simulates) while
   **preserving** assertion (2) "no `self` token leaks as
   `UnknownDependency`/`DoesNotExist`" (the #559 guard must survive).
5. **`genuine_cycles_still_rejected`'s `x[dimA]=x[dimA]+1` assertion is
   deliberately loose** (`tests/simulate.rs:1207-1218` accepts
   `CircularDependency | UnknownDependency | DoesNotExist`) and a nearby
   comment block (`tests/simulate.rs:1186-1191` and inline rationale
   `~1214-1217`) explicitly mandates the loose form ("Do NOT pin
   UnknownDependency ..."). AC4.2 requires `CircularDependency` specifically —
   tighten the assertion **and** rewrite that comment in the same commit
   (CLAUDE.md comment-freshness hard rule).
6. **`compute_transitive`/`compute_inner` are a closure + nested fn** inside
   `model_dependency_graph_impl` (`db.rs:1149-1255`/`1155-1244`), not
   module-level functions. `ModelDepGraphResult` is at `db.rs:1129-1137`
   exactly; `assemble_module` at `db.rs:4287-4293` exactly.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### element-cycle-resolution.AC1: Single-variable self-recurrence resolves
- **element-cycle-resolution.AC1.1 Success:** `test/sdeverywhere/models/self_recurrence/self_recurrence.mdl` compiles via the incremental path with no `CircularDependency`.
- **element-cycle-resolution.AC1.2 Success:** It simulates to the well-founded series `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` over its steps.
- **element-cycle-resolution.AC1.3 Success:** `scc_components` is callable from production code (`pub(crate)`, not `#[cfg(test)]`) and the whole-variable happy path (acyclic models) is unaffected.
- **element-cycle-resolution.AC1.4 Edge:** The emitted per-element run order is byte-identical across repeated compiles of the same model.
- **element-cycle-resolution.AC1.5 Failure:** A single-variable SCC whose induced element graph is genuinely cyclic (`x[dimA]=x[dimA]+1`) is NOT resolved — it still reports `CircularDependency`.

### element-cycle-resolution.AC4: Genuine cycles still rejected (hard rule)
- **element-cycle-resolution.AC4.1 Failure:** `a=b+1; b=a+1` reports `CircularDependency` (scalar 2-cycle).
- **element-cycle-resolution.AC4.2 Failure:** `x[dimA]=x[dimA]+1` reports `CircularDependency` (genuine same-element self-cycle).
- **element-cycle-resolution.AC4.3 Success:** `genuine_cycles_still_rejected` passes unchanged; the `dt_cycle_sccs_engine_consistent` harness passes against the new invariant (element-acyclic ⇒ resolved/no diagnostic; element-cyclic ⇒ `CircularDependency`).

---

## Testing conventions (apply to every task)

- **TDD is mandatory** (root `CLAUDE.md` CRITICAL rule, 95%+ coverage). Write
  the failing test first, watch it fail, then implement.
- **Outside-in for Phase 1:** Subcomponent B authors the `self_recurrence`
  end-to-end acceptance test FIRST (Task 3), observes it RED (fails with
  `CircularDependency`), then the engine wiring (Tasks 4-6) drives it GREEN.
  The acceptance test plus the wiring are committed as **one green commit** at
  the end of Task 6 — the project's documented "fold into one green commit"
  pattern (`docs/dev/rust.md`; never `--no-verify`; the pre-commit hook runs
  the full `cargo test`, so a RED test cannot be committed mid-fold). The
  user's memory rule **bars** stash/checkout/reset to toggle RED/GREEN — do
  not use it; use the fold.
- Unit tests for `db_dep_graph.rs` machinery go in its sibling test file
  `src/simlin-engine/src/db_dep_graph_tests.rs` (declared
  `#[cfg(test)] #[path = "db_dep_graph_tests.rs"] mod db_dep_graph_tests;` at
  `db_dep_graph.rs:498-500`; keeps `db.rs`/`db_dep_graph.rs` under the
  per-file line cap). New `ResolvedScc`/`SccPhase` unit tests go here.
- End-to-end model-fixture tests go in `src/simlin-engine/tests/simulate.rs`
  (requires `--features file_io`). Use `TestProject` (from
  `src/test_common.rs`) for synthetic models: `.assert_compiles_incremental()`,
  `.assert_vm_result(name, &[f64])`. The MDL fixture harness is
  `simulate_mdl_path(path)` (`tests/simulate.rs:286-305`).
- **Verify via the pre-commit hook**: run `git commit`; `scripts/pre-commit`
  runs `cargo fmt --check`, `cargo clippy --all-targets --all-features -D
  warnings`, and `cargo test` under a 180s cap. NEVER `--no-verify`. On hook
  failure, fix and make a NEW commit (do not `--amend`). Per-test budget ~2s
  debug (5s ceiling). `self_recurrence.mdl` is tiny — no `#[ignore]` needed.
- Determinism discipline already exists: `scc_components` sorts members
  lexicographically and components by smallest member
  (`ltm/indexed.rs:220-227`); `dt_walk_successors` returns BTreeSet-sorted
  order. Reuse it; do not introduce nondeterministic ordering.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Promote `scc_components` to a production `pub(crate)` primitive

**Verifies:** element-cycle-resolution.AC1.3 (partial — promotion half)

**Files:**
- Modify: `src/simlin-engine/src/ltm/indexed.rs:207` (remove `#[cfg(test)]` on the `fn`)
- Modify: `src/simlin-engine/src/ltm/mod.rs:58-59` (remove `#[cfg(test)]` on the `pub(crate) use indexed::scc_components;` re-export)
- Modify: `src/simlin-engine/src/ltm/indexed.rs:189-206` and `src/simlin-engine/src/ltm/mod.rs:53-57` (update the two "promote ... when a production consumer is added" doc comments to state that the production consumer now exists: `db_dep_graph.rs` element-cycle refinement)

**Implementation:**
- Remove the `#[cfg(test)]` attribute immediately above
  `pub(crate) fn scc_components(edges: &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>) -> Vec<Vec<Ident<Canonical>>>` at `ltm/indexed.rs:207`.
- Remove the `#[cfg(test)]` attribute above the `pub(crate) use indexed::scc_components;` re-export at `ltm/mod.rs:58`.
- Rewrite both doc comments: the function's behavior contract (sorted/byte-stable: each component sorted by canonical name, outer `Vec` sorted by smallest member; a self-loop is a *size-1* component so self-loop callers must detect self-loops from adjacency directly) is unchanged and must be preserved verbatim in substance; only the `#[cfg(test)]`-rationale sentence ("promote ... when a production consumer is added") changes to record that the production consumer is the `db_dep_graph.rs` element-cycle refinement added in this phase.
- Do not change the algorithm or its signature.

**Testing:**
Existing `scc_components` unit tests must still pass with the gate removed. No
new test for the promotion itself (it is a visibility change; AC1.3's
"callable from production" is proven by Task 6 wiring + the Task 3 end-to-end
test). A `#[cfg(test)]`-only call would not prove production callability.

**Verification:**
Run: `cargo build -p simlin-engine` — compiles.
Run: `git commit -m "engine: promote scc_components to production pub(crate) primitive"` — pre-commit green.

**Commit:** `engine: promote scc_components to production pub(crate) primitive`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add `ResolvedScc` / `SccPhase` types and `resolved_sccs` payload

**Verifies:** element-cycle-resolution.AC1.1 (scaffolding for the resolution payload)

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (near `ModelDepGraphResult` at `db.rs:1129-1137`): add `SccPhase` enum, `ResolvedScc` struct, and a `resolved_sccs: Vec<ResolvedScc>` field on `ModelDepGraphResult`.

**Implementation:**
Add the new types adjacent to `ModelDepGraphResult`. They MUST derive the
same trait set `ModelDepGraphResult` already derives
(`Clone, Debug, PartialEq, Eq, salsa::Update`) because `ModelDepGraphResult`
is a salsa return value:

```rust
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum SccPhase {
    Dt,
    Initial,
}

/// A recurrence SCC whose induced element graph the cycle gate proved
/// acyclic. `members` is byte-stable (BTreeSet); `element_order` is the
/// per-element topological evaluation order `(member, element-offset)`.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ResolvedScc {
    pub members: std::collections::BTreeSet<Ident<Canonical>>,
    pub element_order: Vec<(Ident<Canonical>, usize)>,
    pub phase: SccPhase,
}
```

Add `pub resolved_sccs: Vec<ResolvedScc>,` to `ModelDepGraphResult`. Every
construction site of `ModelDepGraphResult` in `db.rs` (including the
early-return / error paths inside `model_dependency_graph_impl`) must
initialize the new field to `Vec::new()` — the compiler will enumerate the
sites; do not use `..Default::default()` (the struct has no `Default`).
`Ident<Canonical>` (defined `common.rs:1216`, derives `Ord` + `salsa::Update`)
makes the `BTreeSet`/`Vec` field types well-formed.

**Testing:**
A `db_dep_graph_tests.rs` unit test constructs a `ResolvedScc` and round-trips
`ModelDepGraphResult` equality (proves `salsa::Update`/`Eq` derive correctly
and the field is wired). Keep it minimal (don't unit-test what the compiler
verifies) — its value is proving the struct participates in salsa equality so
cache invalidation is correct.

**Verification:**
Run: `cargo build -p simlin-engine` — compiles; all `ModelDepGraphResult`
construction sites updated.
Run: `git commit -m "engine: add ResolvedScc/SccPhase and resolved_sccs payload"`.

**Commit:** `engine: add ResolvedScc/SccPhase and resolved_sccs payload`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-6) -->

> **Outside-in TDD fold.** Task 3 authors the `self_recurrence` end-to-end
> acceptance test and observes it RED (fails with `CircularDependency`).
> Tasks 4-5 build the engine machinery with their own RED-first unit tests.
> Task 6 wires the verdict in, turning the Task 3 acceptance test GREEN.
> Because the pre-commit hook runs the full `cargo test` and a RED test
> cannot be committed, **the working-tree changes from Tasks 3-6 are committed
> together as ONE green commit at the end of Task 6** (the project's
> documented fold pattern; stash/reset is barred by the user's memory rule).
> Run tests locally (`cargo test -p simlin-engine --features file_io ...`)
> between tasks to observe RED→GREEN without committing.

<!-- START_TASK_3 -->
### Task 3: Author the `self_recurrence` end-to-end acceptance test (observe RED)

**Verifies:** element-cycle-resolution.AC1.1, element-cycle-resolution.AC1.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` — rewrite `self_recurrence_self_token_resolves_to_real_name` (`tests/simulate.rs:1097-1146`).

**Implementation (write the test first; do NOT commit yet — folded into Task 6):**
`test/sdeverywhere/models/self_recurrence/self_recurrence.mdl` is
`ecc[t1]=1; ecc[tNext]=ecc[tPrev]+1` over subrange `t1..t3`, FINAL TIME=1,
TIME STEP=1 ⇒ 2 saved steps. Rewrite the test so it asserts the **target
post-resolution behavior**:
- The model **compiles** via the incremental path with **no
  `CircularDependency`** (invert the current assertions 1 and 3). Use the same
  `open_vensim` → `compile_vm` path the sibling tests use; assert the compile
  `Result` is `Ok` (no `NotSimulatable`, no `CircularDependency` diagnostic).
- **Preserve** assertion (2): no `self` token leaks as
  `UnknownDependency`/`DoesNotExist` (the #559 regression guard must survive).
  Rename to reflect both intents, e.g.
  `self_recurrence_resolves_and_no_self_token_leak`.
- It simulates to `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3`. Since `self_recurrence/`
  has **no `.dat`**, assert the series in-test with the `element_series`
  helper (defined `tests/simulate.rs:1042`; copy the call idiom from
  `previous_self_reference_still_resolves`, `tests/simulate.rs:1289-1291`):
  read `ecc[t1]`, `ecc[t2]`, `ecc[t3]` and assert `[1.0, 2.0, 3.0]` (constant
  across both saved steps — the recurrence is over the subrange, not time).

**Testing (RED gate):**
Run the rewritten test now, before Tasks 4-6: it MUST fail with
`CircularDependency` (the whole-variable gate still rejects the self-loop).
Record this RED observation (it is the outside-in acceptance proof). Do not
commit (the fold commits at Task 6 once GREEN).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io self_recurrence` — observe RED (fails: `CircularDependency`/`NotSimulatable`).

**Commit:** (none — folded into Task 6's single green commit)
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Production per-element lowered-exprs accessor

**Verifies:** element-cycle-resolution.AC1.1 (enables sourcing the element relation), element-cycle-resolution.AC1.3 (production-callable, happy path unaffected)

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` (add a new production `pub(crate)` fn; leave the existing `#[cfg(test)]` `var_noninitial_lowered_exprs` at lines 434-496 and `array_producing_vars` at 383-403 **unchanged**)

**Implementation:**
Add a production accessor returning the engine's own production-lowered
per-element `Vec<Expr>` for a variable+phase, or `None` when it cannot be
sourced (loud-safe — caller falls back to `CircularDependency`). Mirror
exactly how the `#[cfg(test)]` `var_noninitial_lowered_exprs` constructs the
caller-owned lowering context (`db_dep_graph.rs:454-477`) and extracts
`per_phase_lowered`, but return `Option` instead of panicking:

```rust
/// The engine's own production-lowered per-element `Vec<Expr>` for
/// `var_name` in the requested phase, or `None` when it cannot be element-
/// sourced (no `SourceVariable`, `LoweredVarFragment::Fatal`, or the phase's
/// `Var::new` errored). `None` is the loud-safe signal: the element-cycle
/// refinement keeps the conservative `CircularDependency` rather than emit a
/// wrong run order. Sourced via `crate::db_var_fragment::lower_var_fragment`
/// — the exact per-variable lowering the production caller
/// `crate::db::compile_var_fragment` runs — never a re-derivation. Phase 3
/// extends the no-`SourceVariable` arm with parent-`implicit_vars` sourcing;
/// Phase 1 only needs the real-`SourceVariable` happy path.
pub(crate) fn var_phase_lowered_exprs_prod(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
    phase: crate::db::SccPhase,
) -> Option<Vec<crate::compiler::Expr>> { /* ... */ }
```

Contract:
- Look up the `SourceVariable` via `model.variables(db)` (same as
  `var_noninitial_lowered_exprs:443-444`). If absent ⇒ return `None` (Phase 1
  does not parent-source; Phase 3 does). Do **not** panic.
- Build the caller-owned context byte-identically to
  `var_noninitial_lowered_exprs:454-477`, call
  `crate::db_var_fragment::lower_var_fragment(...)`.
- On `LoweredVarFragment::Fatal` ⇒ `None`. On `Lowered { per_phase_lowered, .. }`,
  select `per_phase_lowered.noninitial` for `SccPhase::Dt`, `.initial` for
  `SccPhase::Initial`; `Ok(v)` ⇒ `Some(v.ast)`, `Err(_)` ⇒ `None`.
- Reuses the salsa-cached `lower_var_fragment` bridge verbatim (Existing
  Patterns: "single shared relation, never re-derive").

**Testing (RED-first unit):**
`db_dep_graph_tests.rs`: write the test first — a `TestProject` with a simple
arrayed real-`SourceVariable` ⇒ `var_phase_lowered_exprs_prod(.., SccPhase::Dt)`
returns `Some` with one `Expr::AssignCurr` slot per element (declared order);
a name absent from `model.variables` ⇒ `None` (no panic). Observe RED (fn
doesn't exist), implement, observe GREEN.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io var_phase_lowered_exprs_prod` — RED then GREEN locally (commit folded into Task 6).
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Per-element dt relation builder + element-acyclicity verdict

**Verifies:** element-cycle-resolution.AC1.1, element-cycle-resolution.AC1.5, element-cycle-resolution.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` (add the SCC identification + per-element relation builder + verdict; all `pub(crate)`)

**Implementation:**

*Step A — SCC identification over the existing dt relation.* Add a
`pub(crate)` fn that builds the whole-variable dt adjacency exactly as
`dt_cycle_sccs` does (`db_dep_graph.rs:259-293`): edges from
`dt_walk_successors` (`db_dep_graph.rs:83-103`); multi-variable SCCs via the
now-promoted `crate::ltm::scc_components` filtered to `len() >= 2`; self-loops
detected directly from adjacency (`dt_cycle_sccs:274-276` — Tarjan reports a
self-loop as a size-1 component). Phase 1 only needs the self-loop /
single-variable case to resolve; multi-variable SCCs are detected but routed
to `CircularDependency` (Phase 2 resolves them).

*Step B — per-element dt graph (the refinement).* For a given SCC, build a
graph whose nodes are `(member, element_offset)`:
- For each member, get production dt lowered exprs via
  `var_phase_lowered_exprs_prod(db, model, project, member, SccPhase::Dt)`
  (Task 4). Any `None` ⇒ SCC **not element-sourceable** ⇒ verdict =
  unresolved (keep `CircularDependency`), loud-safe.
- Each arrayed member's lowered `Vec<Expr>` is a flat sequence segmented per
  element as `[pre-exprs..., Expr::AssignCurr(member_base + i, rhs_i)]`
  (`Expr::AssignCurr(usize, Box<Expr>)` is `compiler/expr.rs:85`; per-element
  expansion is `compiler/mod.rs:1651-1683` Arrayed / `:1721-1736` A2A,
  declared dimension order via `SubscriptIterator`). The first `AssignCurr`
  operand is the absolute current-value slot.
- Build a reverse map `slot_offset -> (member, element_index)` for all SCC
  members (each member occupies `member_base .. member_base + element_count`
  contiguous slots; `member_base` = offset of its first `AssignCurr`,
  `element_index` = slot − base, declared element order = `SubscriptIterator`
  enumeration order).
- For each member element `(M, e)`, take the RHS of
  `AssignCurr(member_base_M + e, rhs)` and collect every current-value slot
  the RHS reads (traverse the `Expr` tree; read
  `src/simlin-engine/src/compiler/expr.rs` to enumerate the current-value-read
  variants and reuse any existing offset-collecting traversal, e.g. the
  visitor `exprs_contain_array_producing_builtin` uses). For each read slot
  mapping to an in-SCC node `(M', e')`, add edge `(M', e') -> (M, e)`.
- dt-phase semantics are **inherited, not re-implemented**: the lowered exprs
  come from the same `lower_var_fragment` the production pipeline uses, so
  PREVIOUS/lagged reads (already stripped from `dt_deps` by `build_var_info`'s
  `dt_previous_referenced_vars.retain`, `db_dep_graph.rs:158/187`) and dt
  stock-breaking are inherited. Do **not** re-add lagged edges (this is why
  it is *not* the LTM `model_element_causal_edges` graph).

*Step C — verdict.* Run promoted `crate::ltm::scc_components` on the
SCC-induced element graph (nodes = byte-stably-encoded `(member, element)`).
Element-acyclic (no element multi-SCC, no element self-loop) **and** every
member element-sourceable ⇒ build `ResolvedScc { members, element_order,
phase: SccPhase::Dt }` where `element_order` is the deterministic topological
order (sorted tie-break by `(member canonical name, element index)`).
Otherwise ⇒ unresolved. Genuine cycles (`a=b+1;b=a+1` multi-var element
2-cycle; `x[dimA]=x[dimA]+1` single-var element self-loop) stay element-cyclic
⇒ rejected **by construction**.

**Testing (RED-first unit):**
`db_dep_graph_tests.rs` (write tests first, observe RED, implement, GREEN):
- forward recurrence (`ecc[t1]=1; ecc[t2]=ecc[t1]+1; ecc[t3]=ecc[t2]+1`) ⇒
  `ResolvedScc` `element_order = [(ecc,0),(ecc,1),(ecc,2)]`, `phase == Dt`.
- `x[dimA]=x[dimA]+1` ⇒ element self-loop ⇒ unresolved (AC1.5/AC4.2).
- `a=b+1; b=a+1` ⇒ element 2-cycle ⇒ unresolved (AC4.2).
- a member returning `None` from the accessor ⇒ unresolved, no panic.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io db_dep_graph_tests` — RED then GREEN locally (commit folded into Task 6).
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Consume the verdict in `model_dependency_graph_impl` (fold GREEN)

**Verifies:** element-cycle-resolution.AC1.1, element-cycle-resolution.AC1.2, element-cycle-resolution.AC4.1, element-cycle-resolution.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/db.rs` `model_dependency_graph_impl` (`db.rs:1139-1436`); the dt `has_cycle` accumulation block is `db.rs:1257-1272`.

**Implementation:**
When `compute_transitive(false)` (dt) reports a back-edge (today this
unconditionally sets `has_cycle = true` + accumulates `CircularDependency` at
`db.rs:1258-1272`):
- Identify the offending dt SCC(s) via the Task 5 identification over the
  same `build_var_info` universe `model_dependency_graph_impl` already builds.
- For each SCC, run the Task 5 refinement + verdict.
- Single-variable (self-loop) SCC **resolved**: push its `ResolvedScc` into
  `ModelDepGraphResult.resolved_sccs`, **exclude that member from the
  `CircularDependency` accumulation** (do not set `has_cycle` for it). The
  member stays in the normal per-variable runlist (single-variable
  self-recurrence already lowers correctly via declared-order
  `SubscriptIterator`; no runlist change, no combined fragment in Phase 1).
- Any SCC unresolved (multi-variable in Phase 1, element-cyclic, or not
  element-sourceable) ⇒ keep `has_cycle = true` + accumulate
  `CircularDependency` (loud-safe). Multi-variable resolution is Phase 2.
- Preserve byte-stability: iterate SCCs/members in the existing
  sorted/`BTreeSet` order so `resolved_sccs` and the runlist are
  deterministic.
- The init `compute_transitive(true)` block (`db.rs:1273-1287`) is **not**
  changed in Phase 1 (init resolution is Phase 2). The acyclic happy path is
  completely unchanged — `resolved_sccs` stays empty, zero extra work.

**Testing:**
- Write/observe the `db_dep_graph_tests.rs` integration assertions RED-first:
  single-variable self-recurrence `TestProject` ⇒ `has_cycle == false`, one
  `ResolvedScc`; `a=b+1;b=a+1` and `x[dimA]=x[dimA]+1` ⇒ `has_cycle == true`,
  empty `resolved_sccs`; an unrelated acyclic model ⇒ empty `resolved_sccs`,
  `has_cycle == false` (AC1.3 happy path unaffected).
- The **Task 3 end-to-end acceptance test now passes GREEN** (AC1.1/AC1.2):
  `self_recurrence.mdl` compiles with no `CircularDependency` and simulates to
  `[1,2,3]`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io db_dep_graph_tests self_recurrence` — all GREEN (Task 3 acceptance test + Task 4-6 unit tests).
Run: `git commit -m "engine: resolve single-variable self-recurrence SCCs in dt cycle gate"`
— **one green commit** folding Tasks 3-6 (acceptance test + accessor + builder
+ verdict). Pre-commit runs the full `cargo test` (180s cap) and must be
green.

**Commit:** `engine: resolve single-variable self-recurrence SCCs in dt cycle gate`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Genuine cycles still rejected (tighten assertion + refresh comment)

**Verifies:** element-cycle-resolution.AC4.1, element-cycle-resolution.AC4.2, element-cycle-resolution.AC4.3

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` `genuine_cycles_still_rejected` (`tests/simulate.rs:1152-1219`), including the rationale comment block at `tests/simulate.rs:1186-1191` and the inline rationale `~1214-1217`.

**Implementation:**
- `a=b+1; b=a+1` (`tests/simulate.rs:1159-1167`): keep — must still report
  `CircularDependency` (scalar 2-cycle / multi-var SCC, unresolved in Phase 1
  and a genuine element 2-cycle). Assertion unchanged.
- `x[dimA]=x[dimA]+1` (`tests/simulate.rs:1192-1200`): the current assertion
  (`tests/simulate.rs:1207-1218`) accepts
  `CircularDependency | UnknownDependency | DoesNotExist`. **Tighten** to
  require `CircularDependency` specifically (AC4.2): every element reads
  itself ⇒ element self-loop ⇒ element-cyclic ⇒ `CircularDependency`. Remove
  the `UnknownDependency`/`DoesNotExist` acceptance for this case.
- **Comment-freshness (CLAUDE.md hard rule):** the comment block at
  `tests/simulate.rs:1186-1191` (and the inline rationale at `~1214-1217`)
  currently *mandates* the loose form ("Do NOT pin UnknownDependency ...",
  "the self-reference fix flips it unknown->circular -- either is
  acceptable"). After tightening, that comment contradicts the code. Rewrite
  it **in the same commit** to state the new invariant: element-level cycle
  resolution resolves the `self` token to the real name, so
  `x[dimA]=x[dimA]+1` produces a genuine element self-loop and MUST report
  `CircularDependency` specifically (no longer an `UnknownDependency` leak).
  Keep the "no unknown/doesnotexist leak" guard where it still guards a real
  name-resolution regression.

**Testing:**
`genuine_cycles_still_rejected` is the AC4.1/AC4.2 verification and must stay
green with the tightened assertion (CLAUDE.md hard rule). AC4.3's harness half
is Task 8. The rewritten comment and the assertion must be internally
consistent (no contradiction remains).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io genuine_cycles_still_rejected` — passes.
Run: `git commit -m "engine: tighten genuine same-element self-cycle to CircularDependency (AC4.2)"`.

**Commit:** `engine: tighten genuine same-element self-cycle to CircularDependency (AC4.2)`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Re-point the cycle-consistency harness to the new invariant

**Verifies:** element-cycle-resolution.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph.rs` `dt_cycle_sccs_consistency_violation` (`db_dep_graph.rs:311-329`, `#[cfg(test)]`) and `dt_cycle_sccs_engine_consistent` (`db_dep_graph.rs:342-363`, `#[cfg(test)]`), and their doc comments (the init-acyclic-premise note around `db_dep_graph.rs:303-305`).

**Implementation:**
The current invariant "instrumentation reports some cycle ⇔ engine raised
`CircularDependency`" (`dt_cycle_sccs_consistency_violation:315-318`) is now
false by design (a single-variable self-recurrence is an instrumented
self-loop but the engine no longer raises `CircularDependency`). Re-point to:
> An instrumented SCC whose induced element graph is acyclic and
> element-sourceable ⇒ the engine does **not** raise `CircularDependency`
> (it is in `resolved_sccs`); an instrumented SCC that is element-cyclic or
> not element-sourceable ⇒ the engine **does** raise `CircularDependency`.

Implement by consulting the Task 5 verdict / resulting
`ModelDepGraphResult.resolved_sccs`+`has_cycle`: for each instrumented SCC,
the engine must raise `CircularDependency` **iff** that SCC is not in
`resolved_sccs`. Update the doc comment that assumes "the init-phase relation
is acyclic by construction" — note it as a **Phase-1 scoping statement** to be
generalized in Phase 2 (Phase 2 introduces init-cyclic-but-element-acyclic
fixtures); do not assert it as a permanent invariant.

**Testing:**
Extend the `db_dep_graph_tests.rs` consistency cases against the new
invariant: single-variable self-recurrence ⇒ instrumented self-loop present
**and** no `CircularDependency` (resolved); `a=b+1;b=a+1` ⇒ instrumented +
`CircularDependency`. Existing genuine-cycle cases stay green.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io dt_cycle_sccs` — pass.
Run: `git commit -m "engine: re-point dt cycle consistency harness to element invariant"`.

**Commit:** `engine: re-point dt cycle consistency harness to element invariant`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Byte-stable per-element run order

**Verifies:** element-cycle-resolution.AC1.4, element-cycle-resolution.AC1.3 (happy path unaffected)

**Files:**
- Modify: `src/simlin-engine/src/db_dep_graph_tests.rs` (add a determinism test alongside the existing `dt_cycle_sccs_is_byte_stable_across_runs` at `db_dep_graph_tests.rs:204-222`).

**Implementation:**
No existing test asserts the *emitted per-element run order* is
byte-identical across repeated compiles — a new obligation. The determinism
*discipline* already exists (sorted Tarjan `ltm/indexed.rs:220-227`;
BTreeSet-sorted `dt_walk_successors`); this proves the new
`ResolvedScc.element_order` inherits it.

**Testing (RED-first):**
Add a unit test: build the single-variable self-recurrence `TestProject`,
compute `model_dependency_graph` (or invoke the Task 5 builder) **twice** on
fresh databases, assert the two `resolved_sccs` (and their `element_order`
vectors and `members` sets) are exactly equal — modeled on
`dt_cycle_sccs_is_byte_stable_across_runs:204-222`. Also assert an acyclic
control model yields empty `resolved_sccs` both times (AC1.3 happy path).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io byte_stable` (or the new
test's name) — passes.
Run: `git commit -m "engine: assert byte-stable per-element run order (AC1.4)"`
— pre-commit runs `cargo test` under the 180s cap; must stay green.

**Commit:** `engine: assert byte-stable per-element run order (AC1.4)`
<!-- END_TASK_9 -->

---

## Phase 1 Done When

- `self_recurrence.mdl` compiles via the incremental path with no
  `CircularDependency` and simulates to `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3`
  (Tasks 3, 6 — AC1.1, AC1.2).
- `scc_components` is `pub(crate)` production-callable; the acyclic
  whole-variable happy path is unaffected (Tasks 1, 6, 9 — AC1.3).
- The per-element run order is byte-identical across repeated compiles
  (Task 9 — AC1.4).
- `x[dimA]=x[dimA]+1` and `a=b+1;b=a+1` still report `CircularDependency`;
  `genuine_cycles_still_rejected` green with a consistent refreshed comment;
  the consistency harness passes against the new invariant (Tasks 5, 7, 8 —
  AC1.5, AC4.1, AC4.2, AC4.3).
- Full engine suite green under the 3-minute `cargo test` pre-commit cap.
