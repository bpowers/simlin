# Element-Level Cycle Resolution — Phase 7 Implementation Plan

**Goal:** `simulates_clearn` is a real, passing test (no longer a permanently-
skipped stub): C-LEARN matches `test/xmutil_test_models/Ref.vdf` within the
existing 1% cross-simulator tolerance; `ensure_vdf_results` is hardened so a
near-empty or all-NaN comparison can no longer vacuously pass.

**Architecture:** Add two guards to `ensure_vdf_results` (a minimum
matched-variable floor; a NaN guard that fails on an entirely-NaN core series
or an excessive NaN-skipped fraction), proven by a dedicated synthetic-input
guard test. Then make `simulates_clearn` genuinely pass (rewrite its stale
comment; bounded numeric debugging of the C-LEARN vs `Ref.vdf` tail within the
1% tolerance; any residual mismatch that resists a *general* fix is filed via
`track-issue`, not hacked around). `simulates_clearn` stays `#[ignore]`d
(runtime-class) and is run explicitly.

**Tech Stack:** Rust (`simlin-engine`) integration tests; VDF parsing
(`vdf.rs`); the 1% cross-simulator tolerance.

**Scope:** Phase 7 of 7. **Depends on Phase 6** (the structural gate must hold
before numeric finalization).

**Codebase verified:** 2026-05-18 (branch `clearn-hero-model`).

---

## Design contradiction resolved (from the design's own AC8.3)

The design's Goal/DoD-2/AC8.1 say `simulates_clearn` is "un-`#[ignore]`d",
but AC8.3 and the Phase-7 "Done when" say it "runs explicitly by runtime
class, not in the capped default set." These conflict literally: a test with
no `#[ignore]` runs in every pre-commit/CI `cargo test`, which would blow the
3-minute cap (C-LEARN parse alone is ~4-5s release / far longer debug; full
compile+run+VDF-compare is much more). The repo has exactly **one** mechanism
for runtime-class exclusion: `#[test] #[ignore]` + a documented
`// Run with: cargo test --release -- --ignored <name>` opt-in
(`docs/dev/rust.md:38-47`; precedents `test_clearn_equivalence`
`docs/dev/commands.md:80-82`, `clearn_ltm_discovery_*`). There is no
feature-gate / separate-binary alternative.

**Resolution (per the design's own explicit AC8.3, no user question needed):**
`simulates_clearn` **keeps `#[test] #[ignore]`** and its
`// Run with: cargo test --release -- --ignored simulates_clearn` comment.
"Un-`#[ignore]`d" in the Goal/AC8.1 is interpreted as **"un-stub it"**: it
transitions from a permanently-skipped placeholder (currently it would fail —
C-LEARN does not compile) to a **real test that compiles, runs, and passes
when invoked explicitly via `--ignored`**. Do NOT literally delete
`#[ignore]` (that would break the pre-commit cap and contradict AC8.3).

---

## Design deviations (verified — these override the design doc)

1. **Line numbers:** `ensure_vdf_results` is `tests/simulate.rs:135-190`
   (doc 135-139, `fn` 140; not "~140-190"). `simulates_clearn`'s stale
   comment is `tests/simulate.rs:912-946` (not "~923-946"); `#[test]` 947,
   `#[ignore]` 948, `fn` 949; the `// Run with:` line is `946` (ABOVE the
   attributes, not ~957-958). Internal anchors are accurate:
   `assert_eq!(step_count)` 141, skip-if-absent 150-152, skip-NaN 161-163,
   `max_val` 165, literal `0.01` 173, `failures += 1` 174, `panic!` 188.
2. **Real (not theoretical) vacuous-pass vectors:** (a) `matched == 0` — if
   almost no `Ref.vdf` ident matches a sim ident, the per-step loop never
   runs, `failures` stays 0, pass; (b) all-NaN matched columns — every cell
   NaN-skipped (`simulate.rs:161-163`), `failures` 0, pass. And
   `to_results_via_records`/`build_results` **provably** produce all-NaN
   columns for unrecovered OT spans (`vdf.rs:1970` inits the buffer to
   `f64::NAN`), so the NaN guard is load-bearing, not hypothetical.
3. **`ensure_vdf_results` is a module-scope free fn in a flat integration
   crate** (no `mod tests` anywhere in `tests/simulate.rs`). A new sibling
   `#[test]` calls it directly with **zero visibility change** (exactly as
   `simulates_clearn:976` does). `Results`/`Specs`/`Method` are all `pub` +
   re-exported (`lib.rs:117`; note `results::Specs` is re-exported as
   `simlin_engine::SimSpecs`, a different type than `datamodel::SimSpecs`).
   Synthetic-`Results` construction templates: `results.rs:276-300`,
   `vdf.rs:1990-2004`. `simulate.rs` currently imports only `Results`
   (line 15) — the guard test must add `Specs`/`Method` imports.
4. **No `tech-debt.md` / GitHub-issue tracking exists** for the
   `ensure_vdf_results` vacuous-pass risk or a "C-LEARN numeric tail." Phase
   7's `track-issue` step for any residual numeric mismatch is required and
   currently uncovered.
5. **Stale comment blockers** (`tests/simulate.rs:912-946`) listed:
   `CircularDependency` on `main.previous_emissions_intensity_vs_refyr`;
   `MismatchedDimensions` on four vars; `UnknownDependency` on
   `emissions_with_cumulative_constraints`; `DoesNotExist` on
   `"goal_1.5_for_temperature"`; + non-fatal unit warnings. Per the design,
   the dim/unknown/doesnotexist items are already cleared on this branch; only
   the (false) `CircularDependency` remained (resolved by Phases 1-2). The
   comment must be rewritten accurately (AC8.3).

---

## Acceptance Criteria Coverage

### element-cycle-resolution.AC8: C-LEARN numeric finalization
- **element-cycle-resolution.AC8.1 Success:** `simulates_clearn` is un-`#[ignore]`d and passes — C-LEARN matches `test/xmutil_test_models/Ref.vdf` within the existing 1% cross-simulator tolerance. *(Interpreted per the resolved contradiction: un-stubbed; `#[ignore]` stays; passes when run explicitly via `--ignored`.)*
- **element-cycle-resolution.AC8.2 Failure:** `ensure_vdf_results` fails (does not vacuously pass) when fewer than a minimum number of variables match, or when a core series is entirely NaN / the NaN-skipped fraction exceeds the guard threshold (covered by a dedicated guard test with synthetic inputs).
- **element-cycle-resolution.AC8.3 Edge:** The `simulates_clearn` stale comment is replaced with an accurate description; the engine default test suite stays green under the 3-minute cap (`simulates_clearn` runs explicitly by runtime class, not in the capped default set).

---

## Testing conventions

`ensure_vdf_results` and the new guard test live in the flat
`tests/simulate.rs` crate (free `#[test]` fns; `--features file_io`). The
guard test fabricates `Results` literals (templates: `results.rs:276-300`,
`vdf.rs:1990-2004`) and runs in the default capped suite (fast, synthetic).
`simulates_clearn` stays `#[test] #[ignore]` — run explicitly with
`cargo test -p simlin-engine --features file_io --release -- --ignored
simulates_clearn --nocapture`. Verify via `git commit` (pre-commit, 180s cap,
never `--no-verify`); the heavy `simulates_clearn` is excluded from the cap by
`#[ignore]`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Dedicated guard test for `ensure_vdf_results` (RED first)

**Verifies:** element-cycle-resolution.AC8.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` — add a new free `#[test]` (e.g. `ensure_vdf_results_rejects_vacuous_comparisons`); add `Specs`/`Method` to the imports (currently only `Results` is imported, `simulate.rs:15`).

**Implementation:**
Write the guard test FIRST (TDD RED — it must fail against the current
unguarded `ensure_vdf_results`). It constructs synthetic `Results` literals
(template: `results.rs:276-300`) and asserts `ensure_vdf_results` **panics**
(does not vacuously pass) in each vacuous scenario. Use
`std::panic::catch_unwind` *inside this synthetic guard test only* (this is a
legitimate test-of-a-panicking-assertion, distinct from the production
`catch_unwind` retired in Phase 6) to assert the panic occurs:
- **Below-floor:** `vdf_expected` with many idents, `results` sharing only 0
  or 1 matching ident ⇒ `matched` below the minimum floor ⇒ must panic.
- **Entirely-NaN core series:** `vdf_expected` and `results` share several
  idents but one matched series is `f64::NAN` at every step (mirroring the
  `build_results` all-NaN-unrecovered-span case) ⇒ must panic.
- **Excessive NaN-skipped fraction:** matched series with a NaN-skipped
  fraction above the threshold ⇒ must panic.
- **Positive control:** a well-formed comparison with enough matches and
  finite values ⇒ does NOT panic (guards don't false-positive).
- **Comparator: `:NA:`-sentinel ≡ Vensim 0:** a matched series where the SIM
  value is `crate::float::NA` (`-2^109`) and the VDF value is `0` ⇒ does NOT
  panic (reconciled as `:NA:`).
- **Comparator: spurious `:NA:` is a real mismatch:** SIM = `crate::float::NA`
  but VDF value is genuinely non-zero ⇒ MUST panic (not silently passed).
- **Comparator: near-zero robustness:** VDF `0` vs SIM tiny jitter (within the
  per-series abs floor) ⇒ does NOT panic; VDF `0` vs SIM a meaningfully-large
  value ⇒ MUST panic (the abs floor must not swallow a genuine divergence).

**Testing:** This test IS the AC8.2 verification. RED now (current
`ensure_vdf_results` vacuously passes the first three); GREEN after Task 2.

**Verification:** Run: `cargo test -p simlin-engine --features file_io ensure_vdf_results_rejects_vacuous_comparisons` — fails RED before Task 2.
**Commit:** (fold with Task 2 — do not commit a RED suite)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Harden `ensure_vdf_results` (matched floor + NaN guard)

**Verifies:** element-cycle-resolution.AC8.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:135-190` (`ensure_vdf_results`)

**Implementation:**
- **Matched-variable floor:** after the `for ident in vdf_expected.offsets.keys()`
  loop closes (after `simulate.rs:182`, before the success `eprintln!` at
  184), assert `matched >= MIN_MATCHED` — `panic!` with a clear message if
  not. Choose `MIN_MATCHED` as a named const justified for C-LEARN (it has
  hundreds of `Ref.vdf` variables; pick a floor that is comfortably below the
  true matched count but far above 0/1 — e.g. derived as a fraction of
  `vdf_expected.offsets.len()`, or a fixed conservative integer; document the
  rationale in a comment, citing that `simulates_wrld3_03` already expects
  `offsets.len() > 200` for a comparably broad model).
- **NaN guard:** add a per-matched-variable NaN-skip counter at the
  `simulate.rs:161-163` skip site (track, per ident, total compared vs
  NaN-skipped). After the loop: `panic!` if any matched **core** series was
  entirely NaN, and/or if the global NaN-skipped fraction exceeds a
  documented threshold. Keep the existing finite-value 1%-tolerance
  comparison (`simulate.rs:165-180`) and the final
  `failures > 0 ⇒ panic!` (`simulate.rs:186-189`) intact — the guards are
  *additional* failure conditions, never relaxations.
- **`:NA:`-sentinel reconciliation (user-directed comparator):** before the
  tolerance check, if the SIM value `a` is the Vensim `:NA:` sentinel
  (`crate::float::NA` = `-2^109`, matched via `approx_eq`/exact bits), the cell is
  `:NA:` in Simlin; Vensim renders `:NA:` as `0` in the VDF, so: if the VDF value
  `e` is ≈0 (within the near-zero floor below) ⇒ MATCH (count it reconciled, do
  not fail); if `e` is genuinely non-zero ⇒ a REAL mismatch (Simlin spuriously
  `:NA:` where Vensim has a value — must be caught, never silently passed). Do NOT
  map `:NA:`→0 on the engine/output side (the engine keeps the sentinel); only the
  comparator interprets it. (After Task 10's `:NA:` fix, `:NA:` cells are finite
  `-2^109`, never NaN, so they reach the comparator, not the NaN guard.)
- **Near-zero-robust tolerance (fixes the literal-0 relative-error breakdown):**
  replace the pure per-cell relative error (`|e-a|/max(|e|,|a|,1e-10)`, which is
  ~100% whenever `e` is a literal 0 and `a` is any small jitter) with the standard
  `isclose`-style combined criterion: a cell matches if
  `|e - a| <= atol + rtol·max(|e|,|a|)`, with `rtol = 0.01` (the existing
  cross-simulator 1%) and a PER-SERIES absolute floor `atol = k · max_step|e[ident]|`
  (the series' peak magnitude). Choose `k` empirically (start ~1e-4) so near-zero
  jitter passes while a genuine >1% divergence on a meaningful value STILL fails
  (validate against C-LEARN's re-measure — the genuine residual MUST stay flagged;
  do NOT loosen `k`/the tolerance to force a pass). This is a principled CORRECTION
  of the comparison at zero, not a relaxation for meaningful values.
- Update `ensure_vdf_results`'s doc comment (locate by name — the structural-gate
  test shifted the old `:135-190` line numbers) to state the full contract: the
  matched-variable floor, the NaN guard, the `:NA:`-sentinel↔0 reconciliation, and
  the near-zero-robust abs+rel tolerance.

**Testing:** Task 1's guard test now passes (GREEN); the positive control
still does not panic. Existing callers `simulates_wrld3_03`
(`tests/simulate.rs:873-910`) and `simulates_clearn` must still behave
correctly (the floor/threshold must be set so a *correct* C-LEARN/WRLD3
comparison passes — Task 4 validates C-LEARN; run `simulates_wrld3_03` to
confirm no false guard trip).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io ensure_vdf_results_rejects_vacuous_comparisons simulates_wrld3_03` — pass.
Run: `git commit -m "engine: harden ensure_vdf_results against vacuous pass (AC8.2)"`
— pre-commit green.
**Commit:** `engine: harden ensure_vdf_results against vacuous pass (AC8.2)`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_3 -->
### Task 3: Un-stub `simulates_clearn` + rewrite the stale comment

**Verifies:** element-cycle-resolution.AC8.3, element-cycle-resolution.AC8.1

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:912-977` (`simulates_clearn` and its comment block)

**Implementation:**
- **Keep `#[test] #[ignore]`** (`simulate.rs:947-948`) and the
  `// Run with: cargo test --release -- --ignored simulates_clearn` comment
  (`simulate.rs:946`) — runtime-class per the resolved contradiction (AC8.3).
- Replace the stale comment block (`simulate.rs:912-946`) with an accurate
  description: C-LEARN's macro work and the formerly-listed
  `MismatchedDimensions`/`UnknownDependency`/`DoesNotExist` blockers are
  cleared on this branch; the previously-fatal `CircularDependency` was the
  false whole-variable cycle-gate verdict, now resolved by element-level
  cycle resolution (Phases 1-2); this test now performs the full end-to-end
  numeric comparison vs `Ref.vdf` within the 1% cross-simulator tolerance and
  is `#[ignore]`d only for runtime class (run explicitly via `--ignored`).
- The test body (`simulate.rs:949-977`) already does the right thing
  (`open_vensim` → `compile_vm` → `Vm::new` → `run_to_end` → parse `Ref.vdf`
  → `ensure_vdf_results`); keep it. It now exercises the hardened
  `ensure_vdf_results` (Task 2) so it cannot vacuously pass.

**Testing:** Running `simulates_clearn` explicitly must now reach
`ensure_vdf_results` (no compile panic — Phases 1-6) and is the AC8.1 target;
Task 4 drives it to green within tolerance.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn --nocapture`
— compiles, runs, reaches `ensure_vdf_results` (pass/fail depends on Task 4).
Run the default suite via `git commit` — green under the 3-min cap
(`simulates_clearn` excluded by `#[ignore]`; AC8.3).
**Commit:** `engine: un-stub simulates_clearn; rewrite stale comment (AC8.3)`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Bounded numeric finalization (C-LEARN vs Ref.vdf within 1%)

**Verifies:** element-cycle-resolution.AC8.1

**Files:**
- (Conditional) Modify: whichever engine source files a *general* root cause
  points to (no model-specific hacks). Possibly none if C-LEARN already
  matches once Phases 1-6 + hardened comparison are in.

**Implementation:**
Run `simulates_clearn` explicitly and inspect the `ensure_vdf_results` output
(`matched`, `max_rel_error`, `max_rel_ident`, the up-to-5 printed failures).
Within the **existing 1% cross-simulator tolerance** (literal `0.01`,
`simulate.rs:173` — do not loosen it):
- If it passes: done — AC8.1 met.
- If it fails: do **bounded** numeric debugging from the
  largest-relative-error variable backward to a root cause. Fixes must be
  **general engine fixes with no model-specific hacks** (CLAUDE.md hard rule;
  `workflow.md` "no special casing"). Add a focused unit test for any
  general bug found (so coverage doesn't depend on the heavy `#[ignore]`d
  test). Any residual mismatch that resists a *general* fix is **filed/triaged
  via the `track-issue` agent** (Task tool, `subagent_type: "track-issue"`),
  NOT hacked around and NOT silenced by loosening tolerance or the new
  guards. (Note: `tech-debt.md` currently has no entry for a C-LEARN numeric
  tail — `track-issue` will create one if needed.)
- The Phase 6 structural gate already locked compile+run+not-all-NaN, so a
  long numeric tail here does not strand the structural deliverable; Phases
  1-3 remain independently shippable.

**Testing:** `simulates_clearn` passes within 1% when run explicitly (AC8.1).
Any general fix carries its own fast unit test.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn --nocapture` — passes (`VDF comparison` reports `matched` ≥ floor, 0 tolerance violations, no NaN-guard trip).
Run: `git commit -m "engine: C-LEARN matches Ref.vdf within 1% (AC8.1)"` — pre-commit green (default suite under cap).
**Commit:** `engine: C-LEARN matches Ref.vdf within 1% (AC8.1)`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: map per-element graphical functions by element name, not Arrayed-Vec position (dominant C-LEARN numeric divergence)

**Verifies:** element-cycle-resolution.AC8.1 (the dominant numeric-divergence root
cause); directly verified by a new focused unit test + a re-measure of C-LEARN
vs `Ref.vdf`.

**Execution order:** done FIRST in Phase 7 (before the Tasks 1/2 comparator), per
the user's "fix the GF-mapping bug first, then re-measure" decision. After it
lands, RE-MEASURE C-LEARN vs `Ref.vdf` (the diagnostic harness) to confirm the
cascade collapses and see the true residual, and checkpoint — then proceed with
Tasks 1/2 (the `:NA:`-aware + near-zero-robust comparator), Task 3 (un-stub), and
Task 4 (final 1% match).

**Why added (Phase 7 diagnostic — the bounded-numeric-finalization root cause):**
a measured C-LEARN-vs-`Ref.vdf` categorization found ~71% of matched cells already
match-or-comparator-reconcilable (58.8% within 1%; 12.5% `:NA:`-sentinel; 0.1%
near-zero), and the 28% "genuine divergence" (1,314 idents) is a CASCADE from a
single bug: per-element graphical functions are consumed by their position in the
`Equation::Arrayed` `elems` Vec (in SORTED element order) instead of by
**element-name → dimension-index**. For a dimension whose DECLARED element order
≠ sorted order (C-LEARN's `COP`: `OECD US, OECD EU, G77 China, G77 India,
Remaining Developed, Remaining Developing A, COP Developing B`), every arrayed GF
feeds the WRONG element's table to each dimension slot. Concrete evidence:
`un_population_high[oecd_us]` simulates to `0.0738609` (COP Developing B's table,
`elems[0]`) instead of `0.0235797` (OECD US's table, which genuine Vensim
produces); this poisons `population → gdp → per-capita/emissions/HFC/CO2eq` and
everything downstream. The datamodel content is CORRECT (each `elems[i]` carries
the right `(ElementName, GF)`); the bug is at lowering/codegen, which must key
each per-element GF by its `ElementName`'s dimension index, not by `elems` Vec
position. NOT caught earlier because most test fixtures declare elements in sorted
order (position == dim-index).

**Files:**
- Modify: the per-element-GF lowering/codegen mapping — likely
  `src/simlin-engine/src/compiler/` (`subscript.rs` / `codegen.rs` / `context.rs`)
  and/or where `Equation::Arrayed`'s per-element `GraphicalFunction` list
  (`datamodel.rs:~195`, `Vec<(ElementName, ...)>`) is lowered into the per-element
  table layout. (Root-cause-confirm the exact site.)
- Add: a focused `#[cfg(test)]` test with a deliberately NON-SORTED declared
  element order + a per-element GF, asserting each element evaluates ITS OWN table.

**Implementation — root-cause-confirm FIRST (sensitive lowering, broad blast
radius — VERIFY before touching), then fix (general):**
0. **Establish the test baseline FIRST (per user direction — broad blast radius):**
   AUDIT the existing unit coverage of the per-element-GF / arrayed-GF
   lowering+evaluation surface (`array_tests`, `compiler_vector`, the
   graphical-function tests, the `benches/`). Identify GAPS, then ADD baseline
   tests BEFORE the fix so it cannot silently regress: (a) **correctness pins** —
   sorted-order arrayed GFs (must stay byte-identical; the fix is identity there),
   per-element GF evaluation by element, and the cross-product of declared-order ×
   element; (b) **performance**: the fix MUST map elements at LOWERING/compile time
   (once), NOT via a per-step element-name lookup in the VM hot loop — confirm the
   mapping site is compile-time, and ensure GF evaluation is not on a hot path that
   the fix would slow (the `benches/` GF-eval coverage; add/extend a focused
   compile or per-element-GF micro-benchmark if a regression-detecting test is
   missing). The committed baseline tests are then the fix's soundness pins.
1. **Confirm** in the lowering/codegen code exactly where a `Variable::Var`'s
   per-element `tables` (the arrayed GF) are assigned to dimension element slots.
   Confirm the bug: the i-th `elems` entry's GF is placed at dimension-index `i`
   (positional) rather than at the dimension index of its `ElementName`. Build the
   RED fixture (non-sorted declared order) and confirm it reproduces.
2. **Fix:** key each per-element GF by `ElementName → dimension index` (the
   declared dimension order), so element `e`'s slot gets element `e`'s table
   regardless of the `elems` Vec ordering — done at LOWERING time (a one-time
   compile-time reorder of the per-element table layout), so the VM hot path is
   unchanged (no runtime name lookup, no per-step cost). General fix for ALL
   arrayed GFs; no model-specific hack.

**Loud-safe / no-regression:** this touches every arrayed GF in every model — the
mandatory soundness pins are paramount.

**Testing (TDD, mandatory):**
- RED-first: the non-sorted-declared-order arrayed-GF fixture — RED (wrong table)
  before, GREEN after.
- **MANDATORY soundness pins (must stay GREEN unchanged):** the FULL array/GF test
  surface — `array_tests`, `compiler_vector`, all graphical-function tests,
  `incremental_compilation_covers_all_models` (AC2.6, 22-model corpus), the
  recurrence/cycle gates, and the full engine lib + integration suites (`simulate`,
  `simulate_ltm`). A sorted-order GF model MUST stay byte-identical (the fix is
  identity for sorted order); if any sorted-order case changes, STOP and report.
  If a non-sorted-order case's numeric output changes, VERIFY it is a CORRECTION
  toward genuine Vensim, not a regression.

**Verification:**
Run the fixture + the full soundness-pin set. Then re-measure C-LEARN vs `Ref.vdf`
and report the new bucket histogram (how much of the 28% divergence collapsed; the
true remaining residual). `git commit` (NEVER `--no-verify`).
**Commit:** `engine: map per-element graphical functions by element name, not Arrayed-Vec position (AC8.1)`
<!-- END_TASK_5 -->

---

## Phase 7 Done When

- `ensure_vdf_results` fails (does not vacuously pass) on below-floor matches,
  an entirely-NaN core series, or an excessive NaN-skipped fraction, proven by
  the dedicated synthetic-input guard test (Tasks 1, 2 — AC8.2).
- `simulates_clearn` is un-stubbed (real, passing when run explicitly), its
  stale comment replaced with an accurate description, `#[ignore]`d for
  runtime class so the default suite stays under the 3-minute cap (Task 3 —
  AC8.3).
- C-LEARN matches `Ref.vdf` within the existing 1% cross-simulator tolerance;
  any residual general-fix-resistant mismatch is tracked via `track-issue`,
  not hacked (Task 4 — AC8.1).
- Full default engine suite green under the 3-minute `cargo test` pre-commit
  cap.
<!-- END_PHASE_7 -->
