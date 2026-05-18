# Element-Level Cycle Resolution — Phase 5 Implementation Plan

**Goal:** `VECTOR ELM MAP` implements **genuine Vensim** semantics: result
element `i` = `source[base_i + offset[i]]` resolved over the **full source
array** (not a flattened sliced view), where `base_i` is the position
established by the first argument's element reference; an offset landing
outside the source variable's full storage range yields `:NA:` (NaN) — **no
modulo / no wraparound**. The genuine-Vensim `vector.xmile` is un-excluded as
a regression gate.

**Architecture:** Two-part VM correction: (a) add the per-element base derived
from the first-argument element reference, and (b) resolve the **full
underlying source dimension/strides** instead of the flattened sliced
`source_view`. Keep the existing out-of-range → NaN behavior (it matches
genuine Vensim's `:NA:`). Then correct every Simlin-authored fixture/test that
encoded the missing-base / sliced-view bug, and un-exclude the genuine-Vensim
`vector.xmile`.

**Tech Stack:** Rust (`simlin-engine`) `vm.rs`, `bytecode.rs`,
`array_tests.rs`, `tests/compiler_vector.rs`, `.dat`/`.xmile` fixtures,
`tests/simulate.rs`.

**Scope:** Phase 5 of 7. **Depends on Phase 4** (shared fixture file
`vector_simple.dat`; sequence after Phase 4 to avoid churn).

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`). Genuine
semantics confirmed by Ventana Systems' official Vensim VECTOR ELM MAP
reference and by real-Vensim ground-truth `.dat` files.

---

## CRITICAL design correction (user-approved 2026-05-18)

The design doc (AC6.1, DoD, Glossary, Phase 5) prescribes `rem_euclid`
**modulo wraparound** (`source[(i + offset[i]) mod n]`) as "genuine Vensim
semantics." **This is factually wrong.** Ventana Systems' official VECTOR ELM
MAP reference states verbatim: *"If you try to use an offset that would take
your mapping outside the range of the variable an error message will be
issued and :NA: will be returned."* No modulo, no wraparound. Neither
genuine-Vensim ground-truth file (`vector.dat`, `vector_simple.dat`)
exercises any wrap — every real offset lands in-range, so the numeric
regression gates pass identically either way.

**User decision (approved):** implement **genuine Vensim** — per-element base
+ full source dimension + out-of-range → NaN (`:NA:`), **NO `rem_euclid`, NO
modulo**. This **overrides** the design's AC6.1 wording. The corrected AC6.1
is stated below. Do **not** use `vm.rs:102`'s `Op2::Mod`/`rem_euclid` for
ELM MAP. A consequence: the existing `out_of_bounds_*` / `negative_offset_*`
unit tests' **NaN premise is correct genuine Vensim** and is *preserved* (the
Phase-5 investigation's "rewrite the OOB tests around wrap" analysis was made
under the now-rejected `rem_euclid` assumption — disregard it; OOB→NaN
stays).

---

## Design deviations (verified — these override the design doc)

1. **No modulo (see CRITICAL correction above).**
2. **The bug is the missing base + sliced view, not the OOB behavior.** The
   current `vm.rs:2304-2356` computes `source_values[round(offset[i])]` over a
   **flattened sliced** `source_view` (materialized at `vm.rs:2311-2329`),
   with `idx<0 || NaN || >=len ⇒ NaN` (`vm.rs:2348-2352`). The OOB→NaN part is
   **genuine Vensim and stays**. The defects to fix: (a) no per-element base
   from the arg-1 element reference; (b) it indexes a flattened *slice* (e.g.
   for `d[DimA,B1]` only the 3-element B1 column), so it can never reach the
   `B2` column — genuine Vensim resolves over the **full** `d` array with
   correct strides (last subscript varies fastest).
3. **For a 1-D `source[<first element>]` arg, `base = 0`**, so
   `result[i] = source[offset[i]]` — which is what the current code already
   does. Therefore several Simlin-authored 1-D unit tests
   (`mod vector_elm_map_tests`, `tests/compiler_vector.rs` VEM tests, the 7
   hoisting modules) **may already be correct** under genuine semantics.
   **Each affected expectation must be recomputed per-case under the verified
   genuine rule; do not blanket-rewrite.** The genuine-Vensim `.dat` files are
   the authoritative ground truth.
4. **`array_tests.rs` corrections are under-scoped in the design.** Besides
   `mod vector_elm_map_tests` (`array_tests.rs:3474-3587`), the 7 hoisting
   modules at `array_tests.rs:3589-4039`
   (`arrayed_except_hoisting_tests`@3589, `first_element_override_hoisting_tests`@3645,
   `mixed_element_hoisting_tests`@3710, `nested_hoisting_first_override_tests`@3778,
   `nested_override_different_wrapping_tests`@3844,
   `toplevel_default_nested_override_tests`@3910,
   `different_builtin_override_tests`@3976) hard-code
   `vector_elm_map(source=[10,20,30], offsets=[2,0,1])` results and must each
   be re-derived under genuine semantics. `tests/compiler_vector.rs` VEM
   tests likewise. (The design's Architecture §'s "arrayed_except_hoisting_tests
   ~3984" line ref is stale: that module is at 3589.)
5. **`mod arrayed_except_hoisting_tests` is at `array_tests.rs:3589`** (not
   ~3984). `mod vector_elm_map_tests` is `3474-3587`. `vm.rs` ELM MAP handler
   is `2301-2356` (comment 2301-2303). `vector.xmile` exclusion is the
   commented entry at `tests/simulate.rs:656` (block 651-657); iterator
   `simulates_arrayed_models_correctly` at `tests/simulate.rs:671-677`.
6. **`vector_simple` is MDL-only** (no `.xmile`); `vector_simple.dat` has
   **no `c` column** (don't expect to edit one). `.dat` files are ASCII —
   edit as text (`cat -A`/`od -c`; `Read` misfires on `.dat`).

---

## Acceptance Criteria Coverage (AC6.1 corrected per user decision)

### element-cycle-resolution.AC6: VECTOR ELM MAP genuine Vensim semantics
- **element-cycle-resolution.AC6.1 Success (CORRECTED):** Result element `i` equals `source[base_i + offset[i]]` resolved over the **full source array** (last subscript varies fastest), where `base_i` is the position established by the first-argument element reference; an offset landing outside the source variable's full storage range yields `:NA:`/NaN (no modulo, no wraparound). *(This supersedes the design's `(i + offset[i]) mod n` / `rem_euclid` wording per the user-approved correction.)*
- **element-cycle-resolution.AC6.2 Success:** A cross-dimension source (`d[DimA,B1]` with offset reaching the `B2` column) resolves against the full source dimension, matching genuine Vensim `vector.dat`.
- **element-cycle-resolution.AC6.3 Success:** `test/sdeverywhere/models/vector/vector.xmile` is un-excluded and simulates to genuine-Vensim `vector.dat`.
- **element-cycle-resolution.AC6.4 Edge (CORRECTED):** Corrected `vector_elm_map_tests` and `vector_simple.dat` `f`/`g` columns pass against genuine Vensim values; out-of-range still yields NaN (genuine Vensim `:NA:`) — the existing OOB/negative→NaN cases are recomputed with the base added, not redesigned around wrap.

---

## Testing conventions

Same as prior phases. `array_tests.rs` + `tests/compiler_vector.rs` unit
tests; end-to-end `tests/simulate.rs` (`--features file_io`). `vector.xmile`
goes through 3 `ensure_results` paths (VM, protobuf round-trip, XMILE
round-trip) at Vensim relative tolerance. Verify via `git commit` (pre-commit
180s cap, never `--no-verify`). All Phase 5 fixtures tiny — no `#[ignore]`.

---

<!-- START_TASK_1 -->
### Task 1: SPIKE — full-source-dimension base + stride resolution

**Verifies:** none (de-risking spike; gates Task 2)

**Files:**
- Read only: `src/simlin-engine/src/vm.rs:2301-2356` (ELM MAP handler), `vm.rs:2664-2684` (`read_view_element`), `src/simlin-engine/src/bytecode.rs:281-309` (`flat_offset`), `bytecode.rs:160-181` (`RuntimeView`), `bytecode.rs:311-345` (`apply_single_subscript`).
- Write: `docs/implementation-plans/2026-05-18-element-cycle-resolution/phase_05_spike_findings.md`.

**Implementation:**
Determine, precisely, how to obtain at the ELM MAP opcode: (a) the
**per-result-element base** = the flat position established by the
first-argument element reference (for `source[<first>]` base=0; for
`d[DimA,B1]` base for result element `a` = flat position of `d[a,B1]` in full
`d`); (b) the **full source array** strides so `offset[i]` steps the correct
(innermost-fastest) stride and can cross into a second dimension. The current
code collapses `source_view` into a flat `source_values` vec
(`vm.rs:2311-2329`) and loses the full-array stride. Identify whether the
compiler must push a full-array view (vs the sliced `source_view`) or whether
`RuntimeView.base_off`/`strides`/`offset` already encode enough to compute
`curr[full_base + base_i + offset[i]*innermost_stride]` via
`read_view_element`/`flat_offset`. Decide the exact change to
`vm.rs:2311-2353` (and any compiler-side view-construction change). Record the
chosen approach and the hand-derivation for `vector_simple`'s `f`/`g` and
`vector.xmile`'s `c`/`f`/`g`/`y` confirming it reproduces the genuine `.dat`.

**Verification:** Findings doc states the exact base + stride computation and
reproduces the genuine `vector.dat` values by hand. Reviewed before Task 2.

**Commit:** `doc: phase 5 VECTOR ELM MAP base+stride resolution spike`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: VM — genuine Vensim VECTOR ELM MAP

**Verifies:** element-cycle-resolution.AC6.1, element-cycle-resolution.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/vm.rs:2301-2303` (comment → correct genuine semantics + citation), `vm.rs:2311-2329` (resolve full source dimension, not flattened sliced view), `vm.rs:2337-2353` (per-element base; keep OOB→NaN; **no modulo**)
- Modify (if the spike requires): the compiler-side view construction that pushes the ELM MAP source argument (so the full-array view/strides reach the opcode)

**Implementation:**
Apply the Task 1 spike's chosen approach:
- Compute `result[i] = source[base_i + offset_round_i]` over the **full
  source array** using its real strides (last subscript fastest), where
  `base_i` is the first-argument element reference's flat position for result
  element `i`.
- Keep the out-of-range guard producing `f64::NAN` (genuine Vensim `:NA:`) —
  i.e. when `base_i + offset_i` falls outside `[0, full_source_len)`, or
  `offset` is NaN. **Do NOT add `rem_euclid`/modulo** (user-approved
  correction; `vm.rs:102`'s `Op2::Mod` is unrelated and not used here).
- Replace the `vm.rs:2301-2303` comment (currently "0-based offset indexing
  ... matches Vensim's VECTOR ELM MAP semantics" — factually describes the
  buggy behavior) with the genuine rule + citation: Ventana's official VECTOR
  ELM MAP reference (offset 0-based, base from arg-1 element ref, out-of-range
  ⇒ `:NA:`, full-array resolution, last subscript fastest) and the
  genuine-Vensim ground truth `test/sdeverywhere/models/vector/vector.dat`.

**Testing:**
Behavior is proven by Tasks 3-6 (corrected unit expectations + the
un-excluded `vector.xmile`/`vector_simple.dat` gates). Add a focused
`array_tests.rs` cross-dimension unit case (the AC6.2 shape:
`f[DimA,DimB]=VECTOR ELM MAP(d[DimA,B1], a[DimA])`) asserting the genuine
values `f[A1]=1, f[A2]=5, f[A3]=6` (offset reaches the `B2` column) — this is
the new behavior not previously covered by any unit test.

**Verification:** Run: `cargo test -p simlin-engine vector_elm_map` (with
Tasks 3-6 applied) — pass.
**Commit:** (fold Phase 5 VM + fixture corrections into one green commit — see Task 6)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Re-derive `mod vector_elm_map_tests` under genuine semantics

**Verifies:** element-cycle-resolution.AC6.4

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs:3474-3587` (`mod vector_elm_map_tests`: helper `make_oob_project` 3479-3485 and the 6 tests `in_bounds_*`/`out_of_bounds_*`/`negative_offset_*` 3487-3586)

**Implementation:**
For **each** test, recompute the expected value under genuine semantics
`result[i] = source[base_i + round(offset[i])]` over the full source array,
out-of-range ⇒ NaN, **no modulo**. Determine `base_i` from each fixture's
actual first-argument element reference (read the fixture model; for a 1-D
`source[<first>]` arg, `base_i = 0` ⇒ `result[i] = source[offset[i]]`, which
may mean the current expectation is **already correct** and must be left
unchanged). The `out_of_bounds_*`/`negative_offset_*` tests keep their NaN
assertions where `base_i + offset_i` is genuinely out of `[0, len)` (genuine
Vensim `:NA:`); only values that change under the added base are edited.
Document the per-case derivation in the test comments.

**Testing:** The recomputed expectations ARE the AC6.4 spec; they must pass
with Task 2 applied and fail against the pre-Task-2 VM where the base/full-dim
behavior differs.

**Verification:** Run: `cargo test -p simlin-engine vector_elm_map_tests` (with Task 2) — pass.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Re-derive the 7 hoisting modules + `compiler_vector.rs` VEM tests

**Verifies:** element-cycle-resolution.AC6.4

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs:3589-4039` — `mod arrayed_except_hoisting_tests`@3589, `first_element_override_hoisting_tests`@3645, `mixed_element_hoisting_tests`@3710, `nested_hoisting_first_override_tests`@3778, `nested_override_different_wrapping_tests`@3844, `toplevel_default_nested_override_tests`@3910, `different_builtin_override_tests`@3976 (only the `vector_elm_map` default/override expectations; the VSO parts of `different_builtin_override_tests` were handled in Phase 4)
- Modify: `src/simlin-engine/tests/compiler_vector.rs` (the `vector_elm_map_*` tests; the VSO ones were Phase 4)

**Implementation:**
Each hoisting module uses `vector_elm_map(source=[10,20,30], offsets=[2,0,1])`
(or `10 + vector_elm_map(...)`). Re-derive each pinned element under genuine
semantics from the fixture's actual arg-1 element reference and full source
array (out-of-range ⇒ NaN, no modulo). Recompute every affected literal +
narrating comment per-case (some may be unchanged if arg-1 is the first
element and the offset stays in range). Do not blanket-substitute a single
expected vector — derive per fixture.

**Testing:** Recomputed expectations are the spec; pass with Task 2.

**Verification:** Run: `cargo test -p simlin-engine vector_elm_map hoisting` and `cargo test -p simlin-engine --features file_io --test compiler_vector` — pass (with Task 2).
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Correct `vector_simple.dat` `f`/`g` columns

**Verifies:** element-cycle-resolution.AC6.4, element-cycle-resolution.AC6.2

**Files:**
- Modify: `test/sdeverywhere/models/vector_simple/vector_simple.dat` — the `f` and `g` rows (both t=0 and t=1)

**Implementation:**
Model: `DimA:A1,A2,A3`, `DimB:B1,B2`; `a[DimA]=0,1,1`;
`d` = `d[A1,B1]=1,d[A2,B1]=2,d[A3,B1]=3,d[A1,B2]=4,d[A2,B2]=5,d[A3,B2]=6`;
`e[A1,B1]=0,e[A2,B1]=1,e[A3,B1]=0,e[A1,B2]=1,e[A2,B2]=0,e[A3,B2]=1`;
`f[DimA,DimB]=VECTOR ELM MAP(d[DimA,B1], a[DimA])`;
`g[DimA,DimB]=VECTOR ELM MAP(d[DimA,B1], e[DimA,DimB])`. Genuine values
(authoritative — independently confirmed by real-Vensim `vector.dat`):
- `f` = `[A1]=1, [A2]=5, [A3]=6` (broadcast across DimB).
- `g` = `g[A1,B1]=1, g[A1,B2]=4, g[A2,B1]=5, g[A2,B2]=2, g[A3,B1]=3, g[A3,B2]=6`.
Rewrite both the t=0 and t=1 rows for the `f[...]`/`g[...]` entries
identically (constant over time). Preserve exact `.dat` tab/line format; do
not touch other columns (the `l`/`m` VSO columns were Phase 4).

**Testing:** `simulates_vector_simple_mdl` (`tests/simulate.rs:767-770`) IS
the gate — it must pass against corrected `.dat` with Task 2.

**Verification:** Run: `cargo test -p simlin-engine --features file_io simulates_vector_simple_mdl` — pass (with Task 2).
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Un-exclude `vector.xmile` as the regression gate + commit Phase 5

**Verifies:** element-cycle-resolution.AC6.3, element-cycle-resolution.AC6.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:656` (uncomment `"test/sdeverywhere/models/vector/vector.xmile",` in `TEST_SDEVERYWHERE_MODELS`); update the now-stale exclusion comment block at `tests/simulate.rs:651-657`

**Implementation:**
Uncomment the `vector.xmile` entry so `simulates_arrayed_models_correctly`
(`tests/simulate.rs:671-677`) picks it up and compares against the
real-Vensim `test/sdeverywhere/models/vector/vector.dat`
(`c[A1]=11,c[A2]=12,c[A3]=12`; `f`/`g` as in Task 5; `y[A1]=3,y[A2]=4,y[A3]=5`
— the `y=VECTOR ELM MAP(x[three],(DimA-1))` cross-dim scalar-source case)
through all three `ensure_results` paths (VM, protobuf round-trip, XMILE
round-trip). Replace the stale exclusion comment (which says "several
variables still fail ... y[DimA] ... fails in VM incremental path") with a
note that ELM MAP now matches genuine Vensim. If `vector.xmile` exercises a
`VECTOR SELECT` cross-dimension pattern that is explicitly **out of scope**
(design "Out of scope: VECTOR SELECT cross-dimension patterns beyond what
C-LEARN / `vector.xmile` exercise"), and that specific variable still cannot
match, narrow the gate to the ELM-MAP variables rather than weakening the
whole comparison — but prefer full inclusion; only narrow with a tracked
issue (`track-issue`) if a non-ELM-MAP variable genuinely resists a general
fix.

Commit Phase 5 as one green unit (Tasks 2-6 together — the VM change and all
fixture/test corrections must land in the same commit so pre-commit's
`cargo test` is green):

**Verification:**
Run: `cargo test -p simlin-engine --features file_io simulates_arrayed_models_correctly` — `vector.xmile` passes against `vector.dat`.
Run: `git commit -m "engine: VECTOR ELM MAP genuine Vensim base+full-source (AC6)"`
— pre-commit fmt/clippy/`cargo test` (180s cap) green.
**Commit:** `engine: VECTOR ELM MAP genuine Vensim base+full-source (AC6)`
<!-- END_TASK_6 -->

---

## Phase 5 Done When

- `VECTOR ELM MAP` = `source[base_i + offset[i]]` over the full source array,
  out-of-range ⇒ NaN (genuine Vensim `:NA:`), no modulo (Tasks 1, 2 — AC6.1,
  AC6.2).
- `vector.xmile` is un-excluded and simulates to genuine-Vensim `vector.dat`
  through all comparison paths (Task 6 — AC6.3).
- `vector_elm_map_tests`, the 7 hoisting modules, `compiler_vector.rs` VEM
  tests, and `vector_simple.dat` `f`/`g` are corrected to genuine Vensim and
  pass; OOB/negative still NaN (Tasks 3-5 — AC6.4).
- Full engine suite green under the 3-minute `cargo test` pre-commit cap.
