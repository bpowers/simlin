# Phase 6 — Documentation and Tech-Debt Closure

**Goal:** Update `docs/design/ltm--loops-that-matter.md` to describe the per-reference walker. Update `src/simlin-engine/CLAUDE.md` to drop `ElementDependencyKind` references. Mark tech-debt items #20 and #26 RESOLVED with commit hashes. Update tech-debt #25 with a measurement reference. Update `docs/test-plans/2026-04-04-ltm-arrays.md` AC8.5 to reference the new model.

**Architecture:** Documentation-only.

**Tech Stack:** Markdown.

**Codebase verified:** 2026-04-25 (Phase 6 codebase-investigator confirmed: design doc section at lines 525–548; CLAUDE.md ref at line 44; tech-debt items #20 (181–188), #25 (222–228), #26 (230–236); RESOLVED format from items #23 and #34; test plan reference at `docs/test-plans/2026-04-04-ltm-arrays.md:58`; `docs/README.md` already indexes the design doc).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC6: Documentation reflects the new design
- **ltm-per-ref-elem-graph.AC6.1 Design doc updated:** `docs/design/ltm--loops-that-matter.md` "Element-Level Causal Graph" section is rewritten to describe the per-reference walker.
- **ltm-per-ref-elem-graph.AC6.2 Engine CLAUDE.md updated:** References to `ElementDependencyKind` in `src/simlin-engine/CLAUDE.md` are removed or updated.
- **ltm-per-ref-elem-graph.AC6.3 Tech-debt items closed:** `docs/tech-debt.md` items **#20** and **#26** are marked RESOLVED with the resolving commit hash. Item **#25** is updated to reflect the SCC-pressure measurement.

---

## Implementation Tasks

<!-- START_TASK_1 -->
### Task 1: Rewrite "Element-Level Causal Graph" section in design doc

**Verifies:** AC6.1

**Files:**
- Modify: `docs/design/ltm--loops-that-matter.md:525-548`

**Implementation:**
Rewrite the section to describe per-reference walking. The new content should:

1. Open with a sentence explaining that LTM extends to arrayed variables by operating on an element-level causal graph derived per-AST-reference.
2. Replace the table (lines 532–538) with a per-reference table showing `RefShape × source_dims × target_dims → edges emitted`. Use the table from `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` "Edge emission per reference site" section.
3. Explain that the walker visits each `Expr2::Var` / `Expr2::Subscript` reference in a target's AST, classifies its access shape (`Bare`, `FixedIndex(elements)`, `Wildcard`, or `DynamicIndex`), and emits element edges per occurrence. Edge sets from multiple references are unioned.
4. Note the rule for fixed-index multi-dim subscripts that contain a wildcard alongside a literal: conservative classification as `Wildcard`, with a TODO pointer to a future tech-debt item if real models exercise the partial-fixed pattern.
5. Note that structural flow→stock edges short-circuit AST classification with `Bare`.
6. Reference Tech-Debt #20, #26, and #25 by their resolution and the implementation plan
   `docs/implementation-plans/2026-04-25-ltm-per-ref-elem-graph/`.

Replace the existing content at lines 525–548 with the new section. The "SameElement applies when..." paragraph (lines 540-548 in the old form) becomes obsolete; replace with a paragraph describing per-reference shape classification.

Suggested text:
```markdown
### Element-Level Causal Graph

`model_element_causal_edges` (salsa tracked, `db_analysis.rs`) builds the
element-level graph by walking each target variable's `Expr2` AST and
emitting one or more element edges per reference site. A reference site is
one occurrence of an `Expr2::Var` or `Expr2::Subscript` node naming a
source variable.

Each reference is classified by access shape:

| Source dims | Target dims | RefShape         | Edges emitted                                |
|-------------|-------------|------------------|----------------------------------------------|
| scalar      | scalar      | Bare             | `from -> to`                                 |
| scalar      | arrayed     | Bare             | `from -> to[d]` for each target element d    |
| arrayed     | scalar      | Bare             | `from[d] -> to` for each source element d    |
| arrayed     | arrayed     | Bare             | `from[d] -> to[d]` per shared element        |
| arrayed     | any         | Wildcard         | `from[d] -> to[e]` full cross-product        |
| arrayed     | scalar      | FixedIndex(elem) | `from[elem] -> to` (one edge)                |
| arrayed     | arrayed     | FixedIndex(elem) | `from[elem] -> to[d]` for each target element d |
| arrayed     | any         | DynamicIndex     | conservative full cross-product              |

`Bare` covers both bare `Expr2::Var` references (scalar dep or A2A
same-element). `FixedIndex` carries the resolved element subscripts from a
literal-index `Subscript` node. `Wildcard` covers reducer patterns
(`SUM(x[*])`); `DynamicIndex` covers any subscript with non-literal indices
(`@N`, `Range`, `StarRange`, or arbitrary `Expr`).

Edges from multiple reference sites in the same target are unioned. For
`relative_pop[R] = population / population[NYC]`, the bare numerator emits
diagonal edges `population[d] -> relative_pop[d]` and the fixed-index
denominator emits broadcast edges `population[NYC] -> relative_pop[d]` —
2N − 1 unique edges, not N². For `share[R] = pop / SUM(pop[*])`, the bare
numerator and the wildcard reducer each emit their own edge sets; the
result is the union (N diagonals plus N² cross-pairs, deduplicated).

Structural flow→stock edges (an inflow or outflow's variable name does not
appear in the stock's equation, which holds only the initial value) are
emitted as same-element diagonals without AST consultation.

Multidimensional subscripts where some indices are literal and others are
wildcards (e.g., `source[NYC, *]`) are conservatively classified as
`Wildcard`. A future refinement could honor partial-fixed semantics; the
overhead is bounded today because such patterns are uncommon in real
models.

When no variables in a model are arrayed, the element graph is identical
to the variable graph (zero overhead).
```

**Verification:**
Read the rewritten section and confirm it accurately describes the post-Phase-2/3 implementation.

**Commit:** `doc: rewrite Element-Level Causal Graph section for per-reference walker`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update `src/simlin-engine/CLAUDE.md` line 44

**Verifies:** AC6.2

**Files:**
- Modify: `src/simlin-engine/CLAUDE.md:44`

**Implementation:**
Replace the current line 44 phrase about `ElementDependencyKind` classification with a phrase describing per-reference walking. Suggested replacement (preserving surrounding context):

```
- **`src/db_analysis.rs`** - Salsa-tracked causal graph analysis: ... Element-level tracked functions: `model_element_causal_edges` (walks each target's AST and emits per-reference element edges based on access shape: Bare, FixedIndex(elements), Wildcard, or DynamicIndex), ...
```

**Verification:**
Run: `grep ElementDependencyKind src/simlin-engine/CLAUDE.md`. Expected: no matches.

**Commit:** `doc: update simlin-engine CLAUDE.md to describe per-reference walker`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Mark tech-debt #20 and #26 RESOLVED

**Verifies:** AC6.3

**Files:**
- Modify: `docs/tech-debt.md:181-188` (#20)
- Modify: `docs/tech-debt.md:230-236` (#26)

**Implementation:**
Use the same RESOLVED format as tech-debt items #23 and #34:

For **#20** (lines 181–188):
- Change `**Severity**: high` → `**Severity**: RESOLVED (2026-MM-DD)` (use actual close date)
- Prepend the description with `(**Resolved** during the per-reference element graph refactor; see commit <SHA> and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.)`
- Keep the original description as historical pointer.
- Update **Owner** if useful; otherwise leave `unassigned`.
- Update **Last reviewed** to today's date.

For **#26** (lines 230–236):
- Change `**Severity**: medium` → `**Severity**: RESOLVED (2026-MM-DD)`
- Prepend the description with `(**Resolved** alongside #20 via per-shape partial equations; see commit <SHA> and `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.)`
- Keep the original description.
- Update **Last reviewed**.

The actual SHA values come from the commits made in Phases 2/3. Use `git log --oneline | head -20` to find them.

**Verification:**
Read the modified sections and confirm the format matches #23 and #34.

**Commit:** `doc: mark tech-debt #20 and #26 RESOLVED`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update tech-debt #25 with measurement reference

**Verifies:** AC6.3

**Files:**
- Modify: `docs/tech-debt.md:222-228`

**Implementation:**
Append a paragraph or note to item #25's description noting the Phase 5 measurement results:

```
- **Note (2026-MM-DD):** Per the per-reference element graph refactor
  (docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md), the spurious
  NxN edge density that previously drove element-graph SCC bloat on
  FixedIndex-using models has been eliminated. Phase 5's measurement
  postscript records before/after SCC sizes; the auto-flip threshold
  (`MAX_LTM_SCC_NODES = 50`) was retained because WRLD3-class models
  still trip the gate from variable-level cycle structure rather than
  element-graph artifacts. The hypothesized "enumerate at the variable
  level first, then expand only the cross-element subgraph" remains a
  worthwhile follow-up for further reducing per-loop overhead, but its
  pressure is materially lower now.
```

Use the actual measurement-postscript section from Phase 5 to refine the language.

**Verification:**
Read the updated entry; confirm it reads coherently.

**Commit:** `doc: tech-debt #25 acknowledges Phase 5 measurement results`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Update test plan AC8.5

**Verifies:** AC6.1 (related — keeping references current)

**Files:**
- Modify: `docs/test-plans/2026-04-04-ltm-arrays.md:58`

**Implementation:**
The current AC8.5 likely says something like "verify the edge expansion table in the design doc matches the `ElementDependencyKind` enum." After Phase 2 the enum is gone. Replace with a reference to the per-reference walker:

> AC8.5: verify the edge expansion table in `docs/design/ltm--loops-that-matter.md` "Element-Level Causal Graph" matches the `RefShape` enum and the per-reference walker in `db_analysis.rs::collect_reference_sites`.

(Adjust to actual test plan wording.)

**Verification:**
Read the updated AC; confirm it points at the right symbols.

**Commit:** `doc: update ltm-arrays test plan AC8.5 to reference RefShape`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Pre-commit verification

**Verifies:** AC5.2

**Files:**
- None modified

**Implementation:**
Trigger the pre-commit hook (e.g., via the next commit). Documentation-only changes should pass quickly.

**Verification:**
Run: pre-commit hook on the next commit. Expected: passes within budget.

Run: `bash scripts/check-doc-links.sh` (or whichever doc-link checker exists). Expected: passes — doc cross-references are valid.

**Commit:** No new commit (verification gate).
<!-- END_TASK_6 -->

---

## Phase Done When

- All 5 documentation tasks committed; Task 6 verification gate passes.
- `docs/design/ltm--loops-that-matter.md` "Element-Level Causal Graph" section accurately describes the per-reference walker.
- `src/simlin-engine/CLAUDE.md` no longer mentions `ElementDependencyKind`.
- `docs/tech-debt.md` items #20 and #26 are marked RESOLVED with commit hashes; #25 has a measurement reference.
- `docs/test-plans/2026-04-04-ltm-arrays.md` AC8.5 references `RefShape`/per-reference walker.
- Pre-commit hook passes.
