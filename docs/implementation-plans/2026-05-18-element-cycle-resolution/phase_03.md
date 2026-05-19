# Element-Level Cycle Resolution — Phase 3 Implementation Plan

**Goal:** An element-SCC containing a no-`SourceVariable` synthetic helper
(synthetic INIT / PREVIOUS / SMOOTH / macro-expansion helper) is resolved when
the helper is sourceable from its parent variable's `implicit_vars`; otherwise
a loud-safe fallback keeps the `CircularDependency` rejection (no panic, no
silent miscompile).

**Architecture (REVISED for the Phase 2 GH #575 symbolic rebuild — supersedes
the original `var_phase_lowered_exprs_prod` framing):** Phase 1 introduced
`var_phase_lowered_exprs_prod` (`Option<Vec<Expr>>`) as the production
element-graph source. **Phase 2's GH #575 re-architecture replaced it**: the
SCC element graph (`symbolic_phase_element_order`, `db_dep_graph.rs`) and the
combined fragment (`combine_scc_fragment`) now consume
**`var_phase_symbolic_fragment_prod`** (`db.rs:3240`, returns
`Option<PerVarBytecodes>` — layout-independent symbolic `SymVarRef` bytecode).
`var_phase_lowered_exprs_prod` has **zero production callers** (only
`#[cfg(test)]` tests) and carries a now-stale `#[cfg_attr(not(test),
allow(dead_code))]` whose justification ("Phase 3 restores a production
consumer") is false. Phase 3 therefore targets the **symbolic** accessor:
its no-`SourceVariable` arm is `let sv = source_vars.get(var_name)?;`
(`db.rs:3253`) which already returns `None` (the loud-safe contract — Task 1
hardens/documents it *there*). Phase 3 extends *that* arm: when a graph node
has no `SourceVariable` (a synthetic helper, `$\u{205A}` prefix, absent from
`model.variables`), source its per-element **symbolic `PerVarBytecodes`** from
the **parent variable's `implicit_vars`**, mirroring the exact
`model_implicit_var_info → parent → parsed.implicit_vars[index] → parse_var →
lower_variable → compile → symbolize` chain the production function
`compile_implicit_var_fragment` (`db.rs:3759`) already implements (it already
produces a helper's symbolic `PerVarBytecodes`). Return `None` (loud-safe)
only when parent-sourcing also fails. The now-orphaned
`var_phase_lowered_exprs_prod` and its `#[cfg(test)]` tests are **removed** in
Task 1 (dead post-rebuild; the project hard rule prefers simple maintainable
code over preserving a superseded interface). The `#[cfg(test)]`
`var_noninitial_lowered_exprs` / `array_producing_vars` abort/panic contract
is a **separate** pair, left **unchanged** (a false negative there is still
wrong; its test must pass unchanged — AC3.3). C-LEARN's SCC contains no such
helpers, so this is general-correctness robustness, not on C-LEARN's critical
path; implement **fallback-first, then parent-sourcing**.

**Tech Stack:** Rust (`simlin-engine`), salsa, the existing
`model_implicit_var_info` / `parse_source_variable_with_module_context` /
`crate::model::lower_variable` production chain.

**Scope:** Phase 3 of 7. **Depends on Phase 2** (combined-fragment lowering;
the helper-bearing SCC is typically multi-node).

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`).

---

## Design deviations (verified — these override the design doc)

1. **The parent-sourcing mechanism already exists in production AND already
   produces symbolic `PerVarBytecodes`.** `model_implicit_var_info`
   (`#[salsa::tracked(returns(ref))]`) returns
   `HashMap<String, ImplicitVarMeta>` keyed by canonical implicit-var name;
   `ImplicitVarMeta` carries `parent_source_var: SourceVariable` and
   `index_in_parent: usize`. The production function
   `compile_implicit_var_fragment` (`db.rs:3759`, line numbers shifted by the
   Phase 2 rebuild — locate by name) **already** does
   `parse_source_variable_with_module_context(.., meta.parent_source_var, ..)`
   → `parsed.implicit_vars.get(meta.index_in_parent)` →
   `crate::variable::parse_var` → `crate::model::lower_variable` → compile →
   `symbolize_bytecode` → a helper `PerVarBytecodes` (it `use`s
   `PerVarBytecodes, ReverseOffsetMap, VariableLayout`). Phase 3 **mirrors
   this chain to fill `var_phase_symbolic_fragment_prod`'s no-`SourceVariable`
   arm**, returning the helper's symbolic `PerVarBytecodes` so
   `symbolic_phase_element_order` + `combine_scc_fragment` consume it exactly
   like a real member. It does NOT invent a new mechanism, and it does **NOT**
   route helpers through `lower_var_fragment` (that bridge is
   `SourceVariable`-keyed and structurally cannot lower a helper).
   `extract_implicit_var_deps` (`db_implicit_deps.rs`) is the
   dependency-extraction analog and a useful reference.
2. **Synthetic-helper naming prefix is `$` + U+205A (`$\u{205A}`)**, pattern
   `$\u{205A}{parent}\u{205A}{n}\u{205A}{func}` (with optional `\u{205A}arg{i}`
   / `\u{205A}{subscript_suffix}` tail). The design glossary's `$⸢` (U+2E22)
   shorthand is wrong — `$⸢` appears nowhere in `src/simlin-engine/src/`. Use
   `\u{205A}`. A node is a synthetic helper iff `model.variables(db).get(name)`
   is `None` AND it resolves in `model_implicit_var_info`.
3. **`var_noninitial_lowered_exprs` and `array_producing_vars` are a
   SEPARATE `#[cfg(test)]` pair from `var_phase_lowered_exprs_prod`** (locate
   by name — Phase 2 shifted `db_dep_graph.rs` substantially and moved
   `model_dependency_graph_impl` into it). They have no production callers;
   their abort/panic contract and the production parent-sourcing contract are
   cleanly separable, so AC3.3 is structurally satisfiable: **Phase 3 changes
   only the production *symbolic* accessor `var_phase_symbolic_fragment_prod`
   (`db.rs:3240`)** and *removes* the now-orphaned production-dead
   `var_phase_lowered_exprs_prod` + its `#[cfg(test)]` tests; it never touches
   the `array_producing_vars` / `var_noninitial_lowered_exprs`
   `#[cfg(test)]` panic wrapper, so
   `array_producing_vars_flags_exactly_the_two_positive_cases` must still pass
   verbatim (AC3.3).
4. **AC3.1 has no existing fixture.** No `test/sdeverywhere/models/` model
   combines a shift-mapped subrange recurrence with a PREVIOUS/INIT/SMOOTH
   helper. A new fixture must be authored, and whether the MDL converter
   actually pushes such a helper into the element SCC must be **empirically
   verified** (write the fixture, dump `dt_cycle_sccs`/`resolved_sccs` +
   `model_implicit_var_info`), not assumed.
5. **AC3.2 (genuinely unsourceable) is hard to produce organically.** An
   orphan node that is neither in `source_vars` nor resolvable via
   `model_implicit_var_info` is the canonical case. The reliable trigger is a
   `#[cfg(test)]`-only override / stub that forces the parent-sourcing lookup
   to yield `None` for a chosen in-SCC node — verify the loud-safe path
   (`CircularDependency`, no panic) is actually reached and the model is not
   rejected earlier by an unrelated diagnostic.

---

## Acceptance Criteria Coverage

### element-cycle-resolution.AC3: Synthetic-helper policy
- **element-cycle-resolution.AC3.1 Success:** A well-founded recurrence whose SCC includes a synthetic helper (e.g. an INIT/PREVIOUS-helper-bearing recurrence) compiles and simulates when the helper is sourceable from its parent's `implicit_vars`.
- **element-cycle-resolution.AC3.2 Failure:** An SCC with an in-cycle node that genuinely cannot be element-sourced falls back to `CircularDependency` — no panic, no silent miscompile.
- **element-cycle-resolution.AC3.3 Edge:** The `#[cfg(test)]` `array_producing_vars` accessor keeps its abort-on-no-`SourceVariable` contract (its test still passes unchanged).

---

## Testing conventions

Same as Phases 1-2: TDD mandatory; `db_dep_graph.rs` unit tests in
`db_dep_graph_tests.rs`; end-to-end fixtures in `tests/simulate.rs`
(`--features file_io`); verify via `git commit` (pre-commit, 180s cap, never
`--no-verify`). New fixtures are tiny — no `#[ignore]`.

---

<!-- START_TASK_1 -->
### Task 1: Loud-safe fallback first (no-`SourceVariable` ⇒ unresolved, never panic)

**Verifies:** element-cycle-resolution.AC3.2, element-cycle-resolution.AC3.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs` `var_phase_symbolic_fragment_prod` (`db.rs:3240`) — make its no-`SourceVariable` / `Fatal` / compile-or-symbolize-fail arms return `None` explicitly and document the loud-safe contract (this is the accessor `symbolic_phase_element_order` / `combine_scc_fragment` actually consume).
- Remove the now production-dead `var_phase_lowered_exprs_prod` (`db_dep_graph.rs`, currently `#[cfg_attr(not(test), allow(dead_code))]`) and its `#[cfg(test)]` tests (`var_phase_lowered_exprs_prod_*` in `db_dep_graph_tests.rs`) — Phase 2's GH #575 rebuild left it with zero production callers and its `allow(dead_code)` justification ("Phase 3 restores a production consumer") is now false; the project hard rule prefers simple maintainable code over preserving a superseded interface. Update any stale doc-comment references that mention it.
- Do **not** touch `var_noninitial_lowered_exprs` or `array_producing_vars` (the separate `#[cfg(test)]` pair — locate by name).

**Implementation:**
`var_phase_symbolic_fragment_prod` already returns `None` on no-`SourceVariable`
(`let sv = source_vars.get(var_name)?;`, `db.rs:3253`). This task hardens and
documents that as the explicit loud-safe contract before adding parent-sourcing
(Task 2): any in-SCC node that cannot be element-sourced ⇒
`symbolic_phase_element_order` returns `None` ⇒ the SCC is unresolved ⇒
`model_dependency_graph_impl` keeps `has_cycle` + accumulates
`CircularDependency` (Phase 1/2 verdict + Task 5b SCC-aware gate). No
production code path may panic (the `?` is the loud-safe signal, never a
panic/expect). Then delete the orphaned `var_phase_lowered_exprs_prod` + its
dead tests as above. Confirm the `#[cfg(test)]` `var_noninitial_lowered_exprs`
panic wrapper and `array_producing_vars` are entirely untouched (separate
functions, no production callers; their abort contract preserved verbatim).

**Testing:**
- AC3.2: `db_dep_graph_tests.rs` — a recurrence SCC where one in-cycle node is
  forced unsourceable (a `#[cfg(test)]` override/stub making the
  parent-sourcing lookup return `None`, or a synthetic orphan node): assert
  the model is rejected with `CircularDependency`, **no panic**, and no silent
  miscompile (the other members are NOT partially resolved). Drive this
  through the production symbolic path (`model_dependency_graph` /
  `symbolic_phase_element_order` → `var_phase_symbolic_fragment_prod`).
- AC3.3: run the existing `array_producing_vars_flags_exactly_the_two_positive_cases`
  (locate by name) unchanged — it must still pass (proves the separate
  `#[cfg(test)]` `array_producing_vars`/`var_noninitial_lowered_exprs` abort
  contract is intact and was not collaterally affected by removing
  `var_phase_lowered_exprs_prod`).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io array_producing_vars` and
the new loud-safe test — pass.
**Commit:** `engine: loud-safe fallback for unsourceable in-SCC nodes (AC3.2, AC3.3)`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Parent-`implicit_vars` sourcing for synthetic helpers

**Verifies:** element-cycle-resolution.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/db.rs` `var_phase_symbolic_fragment_prod` (`db.rs:3240`) — extend the no-`SourceVariable` arm with parent-`implicit_vars` sourcing that returns the helper's symbolic `PerVarBytecodes`.

**Implementation:**
When `model.variables(db).get(var_name)` is `None` (the `?` at `db.rs:3253`),
instead of returning `None`, attempt parent-sourcing, mirroring
`compile_implicit_var_fragment` (`db.rs:3759` — locate by name) which already
performs this exact chain and already produces a helper `PerVarBytecodes`:
1. `let info = model_implicit_var_info(db, model, project);`
   `let meta = info.get(&canonical(var_name))` — `None` ⇒ return `None`
   (loud-safe; genuinely unsourceable — AC3.2).
2. `module_ident_context_for_model(db, model, project, module_input_names)`
   then `parse_source_variable_with_module_context(db, meta.parent_source_var,
   ..)` — use the *same* construction `compile_implicit_var_fragment` uses
   (it has a `module_ident_context_for_model` helper; reuse it).
3. `parsed.implicit_vars.get(meta.index_in_parent)` — `None` ⇒ return `None`.
4. Parse + lower the synthesized `datamodel::Variable` via
   `crate::variable::parse_var` then `crate::model::lower_variable` (the
   non-module branch of `compile_implicit_var_fragment`; the module branch
   constructs a `Variable::Module`). On any parse/lower failure ⇒ `None`
   (loud-safe).
5. Compile + `symbolize_bytecode` the requested phase exactly as
   `compile_implicit_var_fragment` / `compile_phase_to_per_var_bytecodes`
   does, returning `Some(PerVarBytecodes)` for the helper (same `SymVarRef`
   layout-independent form every other member produces, so
   `symbolic_phase_element_order` builds the helper's element edges and
   `combine_scc_fragment` interleaves its segments exactly like a real
   member). Prefer factoring/reusing the shared step with
   `compile_implicit_var_fragment` over duplicating the chain (DRY; "single
   shared relation, never re-derive").
- This is reuse of an existing production chain, not a re-derivation. The
  `#[cfg(test)]` `array_producing_vars`/`var_noninitial_lowered_exprs`
  accessors remain untouched.

**Testing:**
`db_dep_graph_tests.rs`: a `TestProject` (or the Task 3 fixture) whose
recurrence SCC includes a synthetic helper — assert
`var_phase_symbolic_fragment_prod` returns `Some(PerVarBytecodes)` for the
helper node (sourced from the parent's `implicit_vars`) and that
`symbolic_phase_element_order` builds the SCC's element graph (helper node
present). (The end-to-end simulate proof is Task 3.) RED-first: before the
extension the helper ⇒ `None` ⇒ SCC unresolved; after ⇒ `Some` ⇒ resolvable.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io var_phase_symbolic_fragment_prod` — pass.
**Commit:** `engine: source synthetic-helper symbolic fragment from parent implicit_vars (AC3.1)`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: End-to-end — synthetic-helper-bearing recurrence simulates

**Verifies:** element-cycle-resolution.AC3.1

**Files:**
- Create: `test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl` (+ `helper_recurrence.dat` with hand-computed expected values)
- Modify: `src/simlin-engine/tests/simulate.rs` (a `#[test]` for the new fixture)

**Implementation:**
Author a minimal MDL fixture: a shift-mapped subrange recurrence (model on
`self_recurrence.mdl`'s `Target/tNext/tPrev` subrange shape) whose per-element
RHS invokes a synthetic helper so the helper enters the element SCC — e.g.

```
{UTF-8}
Target: (t1-t3)
	~	~		|
tNext: (t2-t3) -> tPrev
	~	~		|
tPrev: (t1-t2) -> tNext
	~	~		|
seed = 1 ~~|
ecc[t1] = INIT(seed) ~~|
ecc[tNext] = ecc[tPrev] + INIT(seed) ~~|
FINAL TIME  = 1
	~	Month |
INITIAL TIME  = 0
	~	Month |
SAVEPER  = TIME STEP
	~	Month [0,?] |
TIME STEP  = 1
	~	Month [0,?] |
\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
///---\\\
:L<%^E!@
1:helperrec.vdf64
9:helperrec
```

Hand-compute expected: `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` (the `INIT(seed)`
helper is `1` each element), write `helper_recurrence.dat`. **Empirically
verify** (per design deviation 4) that the converter actually pushes the
`INIT` helper (`$\u{205A}ecc\u{205A}{n}\u{205A}init`) into the element SCC —
inspect `resolved_sccs`/the dt verdict; if the chosen builtin does not
produce an in-SCC helper, iterate the fixture (try PREVIOUS/SMOOTH or a
different shape) until it does. The fixture must genuinely exercise the Task 2
parent-sourcing path (assert via a focused check that the SCC contains a
`$\u{205A}`-prefixed member).
- **Bounded-attempt + escalation:** if, after a reasonable bounded number of
  attempts (≈4-5 distinct shapes — INIT/PREVIOUS/SMOOTH × subrange variants),
  the converter does not push any synthetic helper into the element SCC, STOP:
  do not fabricate a passing test or weaken AC3.1. Surface to the user and
  file via the `track-issue` agent (Task tool,
  `subagent_type: "track-issue"`) that a synthetic-helper-in-SCC fixture
  could not be constructed (with the shapes tried and observed
  `resolved_sccs`/`model_implicit_var_info` output), so the AC3.1 happy-path
  coverage gap is explicitly tracked, not silently dropped. (The loud-safe
  fallback from Task 1 still protects correctness regardless.)

**Testing:**
A `#[test]` running `helper_recurrence.mdl` through `simulate_mdl_path`
(`tests/simulate.rs:286-305`) comparing against `helper_recurrence.dat`. Must
fail before Task 2 (helper unsourceable ⇒ `CircularDependency`) and pass
after. This is the AC3.1 happy-path proof (the design notes AC3.1 is the
*only* coverage of the parent-sourcing happy path, so it must be deliberately
constructed and verified).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io helper_recurrence` — passes.
Full suite via `git commit` (pre-commit 180s cap) — green.
**Commit:** `engine: helper-bearing recurrence simulates via parent-sourcing (AC3.1)`
<!-- END_TASK_3 -->

---

## Phase 3 Done When

- A synthetic-helper-bearing well-founded recurrence compiles and simulates
  when the helper is sourceable from its parent's `implicit_vars` (Tasks 2, 3
  — AC3.1).
- An SCC with a genuinely unsourceable in-cycle node falls back to
  `CircularDependency` — no panic, no silent miscompile (Task 1 — AC3.2).
- `array_producing_vars_flags_exactly_the_two_positive_cases` passes
  unchanged; the `#[cfg(test)]` abort contract is untouched (Task 1 — AC3.3).
- Full engine suite green under the 3-minute `cargo test` pre-commit cap.
