# C-LEARN Residual — Phase 4: Residual attribution and gate tightening

**Goal:** Every remaining divergent C-LEARN base (after Phases 1-3) is honestly classified and either fixed or documented with a sourced reason; `EXPECTED_VDF_RESIDUAL` shrinks to exactly the genuine remainder, and both `--ignored` C-LEARN gates pass.

**Architecture:** This phase is measurement-driven. It re-runs the C-LEARN-vs-`Ref.vdf` comparison after the Phase 1-3 engine fixes, classifies each still-divergent base into a precise category, fixes the tractable reference-side (VDF-reader decode) bugs and any remaining engine-genuine divergences, and then tightens the carve-out (`EXPECTED_VDF_RESIDUAL`) to exactly the live remainder — enforced by the `clearn_residual_exactness` guard. The comparator's tolerances and floors are NOT loosened; the carve-out remains a documented, exact exclusion.

**Tech Stack:** Rust (`simlin-engine` crate): the VDF reader (`vdf.rs`, `vdf/record_results.rs`), the comparator + gate (`tests/simulate.rs`), and `gh` for issue updates.

**Scope:** Phase 4 of 5 from `docs/design-plans/2026-05-19-clearn-residual.md`. **Depends on Phases 1-3** (must re-measure after the engine fixes).

**Codebase verified:** 2026-05-20 (branch `clearn-residual`, off `main`@`2ed93950`). Both `--ignored` gates pass today with exactly 32 residual bases (verified by a live run).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### clearn-residual.AC4: The residual is honestly attributed and the gate is tightened
- **clearn-residual.AC4.1 Success:** After Phases 1-3, `EXPECTED_VDF_RESIDUAL` is reduced to the proven remainder and `simulates_clearn` passes (`--ignored`).
- **clearn-residual.AC4.2 Success:** `clearn_residual_exactness` passes — the live failing set equals the reduced `EXPECTED_VDF_RESIDUAL` exactly (no `grew` / no `shrank`).
- **clearn-residual.AC4.3 Success:** Every still-excluded base carries a one-line sourced reason (engine-genuine-tracked / VDF-decode-artifact / benign-near-zero / NaN-vs-`:NA:` / boundary), and #590 / #591 are updated to reflect the final disposition.
- **clearn-residual.AC4.4 Failure/guard:** The comparator's 1% tolerance, per-series floor, and matched-variable floor are unchanged for every non-excluded variable (the carve-out remains a documented exclusion, never a tolerance loosening; the gate cannot pass vacuously).
- **clearn-residual.AC4.5 Edge:** A NaN-vs-`:NA:` series (e.g. `slr_inches_from_2000`) is handled by the documented NaN-skip mechanism and does not enter the failure set.

---

## Verified ground truth (read before starting)

Confirmed by investigation on 2026-05-20 (line numbers accurate; both gates ran green live).

- **`EXPECTED_VDF_RESIDUAL`**: `src/simlin-engine/tests/simulate.rs:1563-1622`, exactly **32 base strings** today, grouped by GitHub-issue cluster comments (#590: 17 bases; #591-c1: 8 bases incl. `historical_gdp`; #591-c2: 7 bases incl. `diffusion_flux`, `co2eq_gap_closing_percentage`; #591-c3: 0 bases). The five design category labels (engine-genuine-tracked / VDF-decode-artifact / benign-near-zero / NaN-vs-`:NA:` / boundary) do NOT exist in code yet — **this phase creates that taxonomy.**
- **`simulates_clearn`**: `tests/simulate.rs:1652-1661` (`#[test] #[ignore]`); calls `run_clearn_vs_vdf` + `ensure_vdf_results_excluding(.., EXPECTED_VDF_RESIDUAL)`. Run: `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored simulates_clearn`.
- **`clearn_residual_exactness`**: `tests/simulate.rs:1720-1766` (`#[test] #[ignore]`). Computes the live failing set (no exclusion), collapses to base names via `vdf_ident_base_name`, and asserts `grew.is_empty() && shrank.is_empty()` vs `EXPECTED_VDF_RESIDUAL`. This is the lockstep guard: when a Phase 1-3 fix reconciles a base, it appears in `shrank` and this test fails until the base is pruned.
- **Comparator (do NOT loosen — AC4.4)**: `classify_vdf_ident` (`:236-304`), `ensure_vdf_results_excluding` (`:349-454`). Constants: `VDF_RTOL = 0.01` (`:164`, the 1% tol), `K_ATOL = 1e-4` (`:176`, per-series abs floor `atol = K_ATOL*peak`), `MIN_MATCHED_FRACTION = 0.10` (`:144`), `MIN_MATCHED_ABSOLUTE = 10` (`:150`), `MAX_NAN_SKIPPED_FRACTION = 0.10` (`:160`). The matched floor is checked AFTER exclusion (so the gate can't pass vacuously). These five constants and the floor logic must remain unchanged.
- **NaN-skip (AC4.5)**: `classify_vdf_ident:264-268` skips any cell where either side is NaN (counts `nan_skipped`, never `failures`). This is why `slr_inches_from_2000` (Vensim literal NaN vs Simlin `:NA:` sentinel) is NOT in the list (it appears only in comments at `:1616-1621`). `float.rs::NA = -6.490371073168535e32 = -2^109` (`float.rs:24`), finite, distinct from NaN.
- **VDF-reader decode-bug candidate sites** (`src/simlin-engine/src/vdf/record_results.rs`, `vdf.rs`): (a) smallest-`start` name-collision pick (`record_results.rs:532-537`); (b) post-canonicalization HashMap overwrite (`record_results.rs:627-632` + `vdf.rs:1973-1974`); (c) owner-vs-descriptor `f[10]`-highest fallback for `Ref.vdf`'s abbreviated `rs_*` descriptors (`record_results.rs:346-354`). The `is_lookupish_name` doc (`vdf.rs:95-97`) explicitly names `Ref.vdf` as the one corpus fixture where the lexical descriptor test fails — corroborating the `rs_*` cluster as a likely reference-side artifact. There are existing non-ignored VDF unit tests (`tests/vdf_alias_decoder.rs`, `vdf/signatures.rs` tests) to extend.
- **`run_clearn_vs_vdf`**: `tests/simulate.rs:1669-1697` (loads `../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` + `Ref.vdf`; uses `open_vensim` null provider). `vdf_ident_base_name` strips a trailing `[elem,…]` subscript.
- **Issues**: #590 (data/graph-lookup `0+0` zeroing) and #591 (residual divergence) are OPEN. Code references at `tests/simulate.rs` lines 185, 341-342, 408, 1554, 1564/1585/1600/1616/1645/1658/1714/1762.

---

<!-- START_TASK_1 -->
### Task 1: Re-measure after Phases 1-3 and classify every remaining divergent base

**Verifies:** none directly — produces the classification that AC4.3 records and that drives Tasks 2-4.

**Files:**
- Investigate: `tests/simulate.rs` (`clearn_residual_exactness` already prints the live `grew`/`shrank`; optionally add temporary instrumentation to `run_clearn_vs_vdf`/`classify_vdf_ident` to dump, per still-failing base, the failing cells, magnitudes, NaN-skip counts, and `:NA:`-reconciled counts). Revert instrumentation before committing.
- Output: a classification table recorded in the Phase 4 commit body and as the restructured `EXPECTED_VDF_RESIDUAL` comments (Task 4).

**Implementation:**
Run `clearn_residual_exactness` (`--ignored`, release). It will FAIL with a `shrank` list (the bases Phases 1-3 reconciled) — that is expected. Capture the live failing set. For EACH still-failing base, assign exactly one category with sourced evidence:
- **engine-genuine** — the engine's value is wrong on a meaningful magnitude (a real bug to fix in Task 3).
- **VDF-decode-artifact** — the engine value is correct but the `Ref.vdf` reference column is mis-decoded (trace to one of the three candidate sites; e.g. the abbreviated `rs_*` descriptors). Fix in Task 2 if tractable.
- **benign-near-zero** — divergence only on near-zero magnitudes (cross-simulator noise), e.g. `diffusion_flux`, `co2eq_gap_closing_percentage`.
- **NaN-vs-`:NA:`** — representation mismatch, NaN-skipped by the comparator (e.g. `slr_inches_from_2000`); should NOT be in the failure set at all.
- **boundary** — matches except at a `:NA:`-arithmetic boundary cell, e.g. `historical_gdp`.

Use the systematic-debugging skill; root-cause-confirm each classification (this codebase has a history of wrong guesses). Avoid assuming the design's pre-classification — verify each against the live data.

**Verification:**
A complete classification table: every live-failing base has exactly one category and a one-line sourced reason (no "unknown"). Temporary instrumentation reverted (`git status` clean of probe code).

**Commit:** none (investigation; the table lands in Task 4's comments and commit body).
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Fix tractable VDF-reader decode bugs (contingent)

**Verifies:** clearn-residual.AC4.1, clearn-residual.AC4.2 (by removing reference-side artifacts from the failure set)

**Entry criteria:** Task 1 classified one or more bases as VDF-decode-artifact AND the root cause is tractable at one of the three candidate sites. If no decode artifact is tractable, document why (precise attribution) and SKIP to Task 4 (those bases stay excluded with a `VDF-decode-artifact` reason).

**Files (if performed):**
- `src/simlin-engine/src/vdf/record_results.rs` (candidate sites at `:346-354`, `:518-538`, `:622-632`) and/or `src/simlin-engine/src/vdf.rs` (`:1967-2005` build_results; `is_lookupish_name` `:85-101`).
- Test: `src/simlin-engine/tests/vdf_alias_decoder.rs` or `src/simlin-engine/src/vdf/signatures.rs` `#[cfg(test)]` — a non-ignored unit test reproducing the mis-decode on a small synthetic record set (NOT keyed on C-LEARN names).

**Implementation (if performed):** Fix the identified decode bug generally (e.g. correct the owner-vs-descriptor disambiguation so `Ref.vdf`'s abbreviated `rs_*` descriptors are not misclassified, or the name-collision/canonicalization-overwrite that drops a column). Follow the format spec `docs/design/vdf.md`. Add a focused regression test at the reader level that pins the corrected decode on a minimal synthetic input, independent of C-LEARN.

**Verification (if performed):**
Run: `cargo test -p simlin-engine vdf` (the non-ignored VDF reader tests, including the new one).
Expected: green; the artifact base(s) now decode correctly (will be removed from the live failure set, observed in Task 4).

**If skipped:** record the precise attribution for each decode-artifact base (which candidate site, why intractable now) for Task 4's comment + a tracked follow-up issue.

**Commit (if performed):** `engine: fix VDF reader decode of <described columns>`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Fix any remaining engine-genuine divergences (contingent)

**Verifies:** clearn-residual.AC4.1, clearn-residual.AC4.2

**Entry criteria:** Task 1 classified one or more bases as engine-genuine (the engine value is wrong on a meaningful magnitude) AND the cause is in scope (not NA-arithmetic, which is confirmed correct and out of scope). If none, SKIP to Task 4.

**Files (if performed):** determined by root-cause (engine compiler/VM/importer). Add a general regression fixture independent of C-LEARN.

**Implementation (if performed):** Use the systematic-debugging skill. Reproduce on a minimal general fixture, fix the root cause generally, add the fixture as a regression test. Do NOT touch NA-arithmetic (`float.rs::NA` and `NA+NA == -2^110` are correct and explicitly out of scope). Do NOT key on any C-LEARN name.

**Verification (if performed):**
Run: the new fixture test + `cargo test -p simlin-engine`.
Expected: green; the base reconciles in the `--ignored` gate (observed in Task 4).

**If skipped:** record why no engine-genuine divergence remains (or why an identified one is out of scope, e.g. tracked as separate tech debt).

**Commit (if performed):** `engine: <general fix for the identified divergence>`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Tighten the gate to the proven remainder and attribute every base

**Verifies:** clearn-residual.AC4.1, clearn-residual.AC4.2, clearn-residual.AC4.3, clearn-residual.AC4.4, clearn-residual.AC4.5

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:1563-1622` (`EXPECTED_VDF_RESIDUAL`) — prune to exactly the live remainder; restructure the comments into the five-category taxonomy with a one-line sourced reason per base.
- Modify (only if the guard logic/messages need it): `tests/simulate.rs:1720-1766` (`clearn_residual_exactness`) — keep the grew/shrank assertion; update only the doc/messages, not the comparator.
- Add: a non-ignored guard test asserting AC4.4 invariants (see below) if one does not already exist.
- Update: GitHub issues #590 / #591 via `gh`.

**Implementation:**
1. Run `clearn_residual_exactness` (`--ignored`) to get the exact live failing set after Tasks 1-3. Set `EXPECTED_VDF_RESIDUAL` to that set exactly (prune the reconciled bases).
2. Reorganize the list comments under the five category headings, each base carrying a one-line sourced reason (AC4.3). Bases reconciled by Phases 1-3 / Tasks 2-3 are removed entirely (not re-labeled).
3. Confirm `slr_inches_from_2000`-style NaN-vs-`:NA:` series are NOT in the list (NaN-skipped) — AC4.5. Add a small non-ignored unit test of `classify_vdf_ident` proving a synthetic NaN-vs-`:NA:` series produces `nan_skipped` cells and zero `failures` (so it never enters the failure set), independent of C-LEARN.
4. AC4.4 guard: do not change `VDF_RTOL`, `K_ATOL`, `MIN_MATCHED_FRACTION`, `MIN_MATCHED_ABSOLUTE`, `MAX_NAN_SKIPPED_FRACTION`, or the floor logic. Add (or confirm) a non-ignored unit test that pins these constants and that a matched-variable count below the floor still panics (the gate cannot pass vacuously).
5. Update #590 / #591 with the final disposition of each base (which were fixed in which phase; which remain and why, by category). Use `gh issue comment`/`gh issue close` as appropriate; if `gh` is unavailable in the execution environment, write the intended issue update text into the commit body and flag it for the human to post.

**Testing:**
- AC4.1: `simulates_clearn` (`--ignored`) passes with the reduced list.
- AC4.2: `clearn_residual_exactness` (`--ignored`) passes (no `grew`, no `shrank`).
- AC4.3: every remaining base has a one-line sourced category reason; issues updated.
- AC4.4: the comparator-constants/floor guard test passes; a diff shows no tolerance/floor change.
- AC4.5: the NaN-vs-`:NA:` `classify_vdf_ident` unit test passes.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored simulates_clearn clearn_residual_exactness`
Expected: both pass.
Run: `cargo test -p simlin-engine` (default suite, including the new AC4.4/AC4.5 guard tests)
Expected: green.

**Commit:** `engine: tighten C-LEARN VDF residual carve-out to the proven remainder`
<!-- END_TASK_4 -->

---

## Phase completion criteria

- `simulates_clearn` and `clearn_residual_exactness` both pass (`--ignored`, release) with the reduced `EXPECTED_VDF_RESIDUAL`.
- Every still-excluded base carries a one-line sourced reason under one of the five categories (no "unknown"); #590 / #591 updated/closed with the final disposition.
- The comparator's tolerance/floors are demonstrably unchanged (AC4.4 guard test green); the NaN-vs-`:NA:` skip is tested (AC4.5).
- `cargo test -p simlin-engine` (default, non-ignored) is green, including the new VDF reader and guard tests.

## No special-casing (hard constraint)

The carve-out is the ONLY place C-LEARN base names appear (it is the documented measurement exclusion, which this phase shrinks — never a tolerance loosening). All engine/reader fixes (Tasks 2-3) ship with general regression tests on synthetic inputs, independent of C-LEARN names/paths. No fix branches on a C-LEARN variable name or the residual list.
