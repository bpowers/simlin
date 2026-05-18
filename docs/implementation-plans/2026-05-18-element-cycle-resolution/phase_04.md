# Element-Level Cycle Resolution — Phase 4 Implementation Plan

**Goal:** `VECTOR SORT ORDER` returns genuine-Vensim **0-based** permutation
indices (not 1-based); every fixture that encoded the 1-based bug is corrected
to genuine Vensim.

**Architecture:** A one-token VM fix (`i + 1` → `i`) plus comment correction,
then rewrite every Simlin-authored fixture/test that baked in the 1-based
output. This is a **purely numeric concern, independent of cycle resolution**
(`array_producing ∩ C-LEARN's cycle = ∅`), but it lies on C-LEARN's numeric
path (C-LEARN's target-sorting uses `VECTOR SORT ORDER`), so it is sequenced
before the C-LEARN numeric finalization.

**Tech Stack:** Rust (`simlin-engine`) `vm.rs` opcode, `array_tests.rs`,
`tests/compiler_vector.rs`, `.dat` fixture.

**Scope:** Phase 4 of 7. **Independent of Phases 1-3.**

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`). Genuine
0-based semantics confirmed by Ventana Systems' official Vensim documentation
(VECTOR SORT ORDER: "The returned vector contains the zero based index number
for the elements in sorted order") **and** by real Vensim DSS 7.3.4 reference
output `test/test-models/tests/vector_order/output.tab` (`SORT ORDER[*]` range
`0..n-1`, contains `0` — impossible for a 1-based permutation).

---

## Design deviations (verified — these override the design doc)

1. **The design's `array_tests.rs` module references are scrambled.** The
   design lists "`vector_builtin_promotes_active_dim_ref_* ~4145`",
   "`mod dimension_dependent_scalar_arg_tests ~4217`", "`mod
   arrayed_except_hoisting_tests ~3984`". Verified reality:
   - `vector_builtin_promotes_active_dim_ref_*` ARE at ~4145 but inside
     `mod flag_split_tests` (declared `array_tests.rs:4086`).
   - `mod dimension_dependent_scalar_arg_tests` is at `array_tests.rs:4217`
     (correct).
   - `mod arrayed_except_hoisting_tests` is at `array_tests.rs:3589` and has
     **no** VSO assertion (it is VEM-only — Phase 5). The design's intended
     target at ~3984 is actually `mod different_builtin_override_tests`
     (declared `array_tests.rs:3976`), which asserts 1-based VSO at lines
     `4010`/`4032`. Use `different_builtin_override_tests`; **drop**
     `arrayed_except_hoisting_tests` from Phase 4 scope.
2. **`tests/compiler_vector.rs` is MISSING from the design's Phase 4
   component list** and has **five** VSO tests asserting 1-based output that
   will break under the 0-based fix. They MUST be corrected in Phase 4.
3. **`Opcode::Rank` (vm.rs ~2407-2450) is correctly 1-based** (genuine Vensim
   `RANK` is 1-based, confirmed by the same `output.tab`) — it is a distinct
   opcode; **do NOT touch it**.
4. **Ties:** Ventana docs are silent on `VECTOR SORT ORDER` tie-breaking
   (sibling `VECTOR RANK` doc says ties are "arbitrary"). The current VM uses
   Rust's stable `sort_by` (`vm.rs:2390-2398`), which is deterministic and
   byte-stable — **keep stable sort** (satisfies the design's determinism
   requirement; "consistent with genuine Vensim" = not contradicting any
   documented behavior, since Vensim leaves ties unspecified).
5. **`vector_simple` is MDL-only** (`vector_simple.mdl` + `vector_simple.dat`;
   no `.xmile`). `vector_simple.dat` is ASCII text (tab-separated
   `time<TAB>value`, rows for t=0 and t=1) — the `Read` tool's binary
   heuristic misfires on `.dat`; use `cat -A`/`od -c` to view, edit as text.
   It is consumed by `simulates_vector_simple_mdl` (`tests/simulate.rs:767-770`,
   NOT `#[ignore]`, NOT in any `TEST_MODELS` list) via `ensure_results`
   (absolute eps `2e-3`); it currently **passes** only because the `.dat` was
   hand-authored to the buggy 1-based values, so the VM fix and the `.dat`
   rewrite must land together.

---

## Acceptance Criteria Coverage

### element-cycle-resolution.AC5: VECTOR SORT ORDER genuine Vensim semantics
- **element-cycle-resolution.AC5.1 Success:** `VECTOR SORT ORDER([2100,2010,2020], ascending)` yields the 0-based permutation `[1,2,0]` (matching genuine Vensim).
- **element-cycle-resolution.AC5.2 Success:** Descending direction yields the correct 0-based permutation; ties preserve stable order consistent with genuine Vensim.
- **element-cycle-resolution.AC5.3 Edge:** The Simlin-authored fixtures that encoded 1-based output (`array_tests.rs` cases, `vector_simple.dat` `l`/`m`) are corrected to genuine Vensim and pass.

---

## Testing conventions

TDD: correct the test's expected literals (the test IS the spec for the new
behavior) — write the corrected expectation, watch it fail against the
still-buggy VM, then apply the VM one-token fix and watch it pass. Unit tests:
`array_tests.rs` (declared `#[cfg(test)] mod array_tests;` in `lib.rs`) and
`tests/compiler_vector.rs` (`--features file_io` if required by that test
file). End-to-end: `tests/simulate.rs::simulates_vector_simple_mdl`. Verify
via `git commit` (pre-commit 180s cap, never `--no-verify`). All Phase 4 tests
are tiny — no `#[ignore]`.

---

<!-- START_TASK_1 -->
### Task 1: VM — `VECTOR SORT ORDER` emits 0-based indices

**Verifies:** element-cycle-resolution.AC5.1, element-cycle-resolution.AC5.2

**Files:**
- Modify: `src/simlin-engine/src/vm.rs:2386` (`indexed.push((val, i + 1));` → `indexed.push((val, i));`)
- Modify: `src/simlin-engine/src/vm.rs:2373` (comment `// Collect (value, 1-based-index) pairs` → 0-based)
- Modify: `src/simlin-engine/src/vm.rs:2358-2361` (replace the "intentional asymmetry" / "matches Vensim's VECTOR SORT ORDER semantics" comment with the correct 0-based semantics + citation)

**Implementation:**
- Change the single token at `vm.rs:2386`: `i + 1` → `i`. The loop variable
  `i` (`vm.rs:2377`, `for i in 0..size`) is the 0-based element ordinal; the
  result write at `vm.rs:2400-2402` (`temp_storage[temp_off + i] = orig_idx as f64`)
  is a flat positional write unaffected by the value change.
- Direction handling (`vm.rs:2390-2398`: `direction == 1` ascending else
  descending; Rust stable `sort_by`) is **already correct** — do not change.
- Rewrite the `vm.rs:2358-2361` comment to state the genuine semantics: VECTOR
  SORT ORDER returns a 0-based permutation (position `i` holds the source
  index of the `i`-th element in sorted order); `direction > 0` ascending,
  otherwise descending; ties are stable (deterministic; Vensim leaves ties
  unspecified). Cite the genuine-Vensim ground truth:
  `test/test-models/tests/vector_order/output.tab` (real Vensim DSS 7.3.4,
  `SORT ORDER[*]` range `0..n-1`) and Ventana's official VECTOR SORT ORDER
  reference ("zero based index number ... in sorted order"). Note that prior
  design docs `docs/design-plans/2026-02-27-vm-vector-ops.md` and
  `2026-03-10-close-array-gaps.md:253` encoded the now-disproven 1-based
  assumption and are superseded.

**Testing:**
This task's behavior is verified by Tasks 2-4 (they correct the expected
literals to 0-based and must pass once this token changes). No new VM unit
test is added here beyond what Tasks 2-4 already assert (the AC5.1/AC5.2
spec is exactly those expectations); adding a redundant VM-internal test
would test wiring, not behavior.

**Verification:**
Run: `cargo build -p simlin-engine` — compiles.
(Tasks 2-4 run the assertions; do not commit a known-red suite — sequence so
Task 1's token change and Tasks 2-4's expectation corrections are committed
together if pre-commit would otherwise be red. Practically: implement Tasks
1-4, then a single `git commit`.)
**Commit:** (folded into Task 4's commit — see note)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Correct `array_tests.rs` VSO expectations

**Verifies:** element-cycle-resolution.AC5.3, element-cycle-resolution.AC5.1, element-cycle-resolution.AC5.2

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` — the following VSO-asserting cases (all currently 1-based):
  - `mod flag_split_tests` (decl 4086): `vector_builtin_promotes_active_dim_ref_monolithic` (line `4155`: `&[2.0, 3.0, 1.0]`) and `vector_builtin_promotes_active_dim_ref_vm` (line `4166`: `&[2.0, 3.0, 1.0]`). Model `vals[DimA]=[30,10,20]`, `VECTOR SORT ORDER(vals,1)` (asc). Genuine 0-based ⇒ `&[1.0, 2.0, 0.0]`.
  - `mod different_builtin_override_tests` (decl 3976): `different_builtin_override_monolithic` (line `4010`: `(vals[2] - 3.0).abs() < 1e-9`) and `different_builtin_override_vm` (line `4032`: same). `source[D]=[10,20,30]`, override elem "3" = `vector_sort_order(source[*],1)`. Genuine 0-based VSO of `[10,20,30]` asc = `[0,1,2]`, slot 2 ⇒ `2.0`. (Leave `vals[0]`/`vals[1]` — those are VEM defaults, Phase 5.)
  - `mod dimension_dependent_scalar_arg_tests` (decl 4217): `assert_vso_dim_dep_results` (lines `4239`/`4244`/`4249`: `vals[0]==2.0`,`vals[1]==3.0`,`vals[2]==2.0`) → genuine `[1.0, 2.0, 1.0]`; `vso_nested_direction_varies_by_dimension_vm` (lines `4284`/`4289`/`4294`: `12.0`/`13.0`/`12.0`, model `10+vso`) → `[11.0,12.0,11.0]`; `vso_except_direction_varies_by_dimension_vm` (lines `4322`/`4335`: `vals[0]==2.0`,`vals[2]==2.0`; `vals[1]==999.0` override unchanged) → `vals[0]=1.0,vals[2]=1.0`. Update the explanatory comments (4224-4228, 4320, 4332-4333) to 0-based.

**Implementation:**
Recompute each expected literal as the genuine 0-based permutation per the
verified per-case derivations above (ascending: position `i` = source index
of the `i`-th smallest; descending: of the `i`-th largest; `dir > 0`
ascending, else descending). Update accompanying comments that narrate
"1-based"/"rank" to the 0-based gather semantics.

**Testing:** These corrected cases ARE the AC5.* spec; they must fail against
the pre-Task-1 VM and pass with the Task-1 token fix.

**Verification:** Run: `cargo test -p simlin-engine vector_builtin_promotes_active_dim_ref different_builtin_override dimension_dependent_scalar_arg` — pass (with Task 1 applied).
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Correct `tests/compiler_vector.rs` VSO expectations

**Verifies:** element-cycle-resolution.AC5.3

**Files:**
- Modify: `src/simlin-engine/tests/compiler_vector.rs`:
  - `vector_sort_order_a2a_produces_correct_results` (line `27`: `&[2.0, 3.0, 1.0]`) → `&[1.0, 2.0, 0.0]`
  - `vector_sort_order_a2a_produces_correct_values_monolithic` (line `40`: `&[2.0, 3.0, 1.0]`) → `&[1.0, 2.0, 0.0]` (update comment line ~33)
  - `vector_sort_order_a2a_produces_correct_values_vm` (line `50`: `&[2.0, 3.0, 1.0]`) → `&[1.0, 2.0, 0.0]`
  - `nested_vector_sort_order_inside_sum_in_array_context_monolithic` (line `202`: `&[6.0, 6.0, 6.0]`) → `&[3.0, 3.0, 3.0]` (update comment line ~196: `SUM([1,2,0])=3`)
  - `nested_vector_sort_order_inside_sum_in_array_context_vm` (line `212`: `&[6.0, 6.0, 6.0]`) → `&[3.0, 3.0, 3.0]`

**Implementation:**
All five use `vals[D]=[30,10,20]`, `VECTOR SORT ORDER(vals,1)` (asc) ⇒ genuine
0-based `[1,2,0]`; the two `SUM` cases sum that to `3.0`. Apply the literal +
comment corrections.

**Testing:** Corrected expectations are the spec; fail pre-Task-1, pass after.

**Verification:** Run: `cargo test -p simlin-engine --features file_io --test compiler_vector` — pass (with Task 1).
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Correct `vector_simple.dat` `l`/`m` columns + commit Phase 4

**Verifies:** element-cycle-resolution.AC5.3, element-cycle-resolution.AC5.1, element-cycle-resolution.AC5.2

**Files:**
- Modify: `test/sdeverywhere/models/vector_simple/vector_simple.dat` — the `l` and `m` rows (both the t=0 and t=1 rows)

**Implementation:**
Model: `DimA: A1,A2,A3`; `h[DimA]=2100,2010,2020`;
`l[DimA]=VECTOR SORT ORDER(h[DimA],ASCENDING)` (`ASCENDING==1`);
`m[DimA]=VECTOR SORT ORDER(h[DimA],0)` (descending). Genuine 0-based:
- `l` (ascending of `[2100,2010,2020]`): sorted `2010(idx1),2020(idx2),2100(idx0)`
  ⇒ `l[a1]=1, l[a2]=2, l[a3]=0` (currently `2,3,1`).
- `m` (descending): `2100(idx0),2020(idx2),2010(idx1)` ⇒
  `m[a1]=0, m[a2]=2, m[a3]=1` (currently `1,3,2`).
Rewrite **both** the t=0 and t=1 rows for `l[a1..a3]` and `m[a1..a3]`
identically (constant over time). Preserve the exact `.dat` tab/line format
(edit as text via `cat -A`/an editor; do not reformat other columns).

Then commit Phase 4 as one unit (Tasks 1-4 together — the VM token change and
all expectation/fixture corrections must land in the same commit so
pre-commit's `cargo test` is green):

**Verification:**
Run: `cargo test -p simlin-engine --features file_io simulates_vector_simple_mdl` — passes against corrected `.dat`.
Run: `git commit -m "engine: VECTOR SORT ORDER genuine Vensim 0-based indices (AC5)"`
— pre-commit runs fmt/clippy/`cargo test` (180s cap); all Phase 4 tests
(`array_tests` VSO cases, `compiler_vector` VSO tests, `simulates_vector_simple_mdl`)
green.

**Commit:** `engine: VECTOR SORT ORDER genuine Vensim 0-based indices (AC5)`
<!-- END_TASK_4 -->

---

## Phase 4 Done When

- `VECTOR SORT ORDER` emits 0-based permutation indices; `[2100,2010,2020]`
  ascending ⇒ `[1,2,0]`, descending correct, ties stable (Task 1 — AC5.1,
  AC5.2).
- Every Simlin-authored fixture that encoded 1-based output is corrected to
  genuine Vensim and passes: `array_tests.rs` VSO cases,
  `tests/compiler_vector.rs` VSO tests, `vector_simple.dat` `l`/`m` (Tasks
  2-4 — AC5.3).
- `Opcode::Rank` untouched (genuine Vensim RANK is 1-based).
- Full engine suite green under the 3-minute `cargo test` pre-commit cap.
