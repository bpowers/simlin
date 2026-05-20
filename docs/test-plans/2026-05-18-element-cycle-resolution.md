# Test Plan — Element-Level Cycle Resolution (C-LEARN hero model)

Human verification plan for the `2026-05-18-element-cycle-resolution`
implementation plan. Companion to the automated AC→test map in that plan's
`test-requirements.md`.

## Coverage summary

Automated coverage validation: **PASS** — all **32** acceptance criteria
(`element-cycle-resolution.AC1.1` … `AC8.3`) are covered by a real test that
exists and asserts the criterion's behavior (test bodies read + suite run: 0
failures across 3565 lib + 91 `simulate` + 23 `compiler_vector` tests; the
runtime-class C-LEARN tests compile cleanly and are correctly `#[test] #[ignore]`).

This plan covers what automated assertions cannot: (a) the **runtime-class**
`#[ignore]`d C-LEARN tests (too heavy for the 3-minute default cap — run
explicitly), and (b) the human-judgement / whole-suite-property items
(HV-1…HV-5 and the AC6.3 `y`/`p` exclusion disposition).

## Prerequisites

- Working tree on branch `clearn-hero-model`; run `./scripts/dev-init.sh` once per session.
- Default automated suite green: `cargo test -p simlin-engine --features file_io`
  (the runtime-class C-LEARN tests report `ignored`).
- Fixtures present: `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (~53k lines),
  `test/xmutil_test_models/Ref.vdf` (~1.86 MB), and
  `test/sdeverywhere/models/{ref,interleaved,init_recurrence,helper_recurrence,vector,vector_simple}/`.
- A release toolchain (the C-LEARN runtime-class tests run `--release`).

## Runtime-class: C-LEARN structural gate — AC7.1, AC7.2, AC7.3, AC7.5

`#[ignore]`d (C-LEARN is ~53k lines; full compile+run+VDF-compare exceeds the
3-min cap). Run explicitly and confirm green.

1. `cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`
   → passes; stderr prints `C-LEARN structural gate: <N> core series matched (Ref.vdf ∩ results) across <S> steps`, N > 0; no `CircularDependency`, no panic, no entirely-NaN matched series.
2. Re-run under a **debug** build with backtraces: `RUST_BACKTRACE=1 cargo test -p simlin-engine --features file_io -- --ignored compiles_and_runs_clearn_structural --nocapture`
   → either passes (no post-gate panic — confirms #363's converted-`Err` path end-to-end), or a hard, root-caused failure **with backtrace** (no `catch_unwind` masking). Record which.
3. `cargo test -p simlin-engine --features xmutil --release -- --ignored clearn_ltm_discovery --nocapture`
   → `clearn_ltm_discovery_compiles` passes — C-LEARN compiles `Ok` with LTM discovery enabled.

## Runtime-class: C-LEARN numeric finalization — AC8.1

1. `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn --nocapture`
   → passes — C-LEARN matches `Ref.vdf` within the **unchanged** 1% tolerance (`VDF_RTOL = 0.01`) on every non-excluded variable; matched floor enforced after exclusion.
2. `cargo test -p simlin-engine --features file_io --release -- --ignored clearn_residual_exactness --nocapture`
   → passes — the live failing-base set EXACTLY equals `EXPECTED_VDF_RESIDUAL`. If it fails, the message names bases that **grew** (a regression → track under #590/#591) or **shrank** (an engine fix → prune the exclusion).

## Human verification

### HV-1 — default suite stays under the 3-minute cap (AC8.3; cross-phase "Done When")
A whole-suite wall-clock property no single test asserts.
- `time cargo test -p simlin-engine --features file_io` (and optionally `time cargo test --workspace`): confirm wall-clock < 180s with the C-LEARN runtime-class tests reported `ignored`.
- Confirm `ensure_vdf_results_rejects_vacuous_comparisons` (AC8.2) ran in the **default** set (fast/synthetic) and `simulates_clearn` did **not**.
- Confirm the pre-commit hook completed on the final Phase 6/7 commits (runs `cargo test` under the cap; never `--no-verify`).

### HV-2 — AC2.4 / AC3.1 fixture authenticity
The new `.dat`s have no external Vensim ground truth (hand-computed); an assertion proves engine-vs-`.dat` agreement, not the hand computation.
- `init_recurrence.{mdl,dat}`: re-derive by hand (`cs=[1,3,5], ecs=[2,4,6]`) and confirm it equals the committed `.dat`. Confirm the automated test's structural assertion is the genuine init-only multi-member SCC (`ResolvedScc{phase:Initial}`, members `{cs,ecs}`, `has_cycle==false`), not incidental.
- `helper_recurrence.{mdl,dat}`: re-derive (`ecc=[1,2,4]`). Confirm a `$⁚`-prefixed synthetic helper (no `SourceVariable`) is genuinely in the resolved init SCC parented to `ecc` (the parent-`implicit_vars` sourcing path is exercised, not bypassed).

### HV-3 — AC8.1 "general fix vs model-specific hack" judgement
Qualitative; an assertion can't enforce it, and the tolerance/guards must not be weakened to force a pass.
- Review every source change under Phase 7 (esp. the per-element graphical-function element→dimension-index mapping fix #589 and the comparator). Confirm each is a **general** engine fix carrying its own fast model-agnostic unit test, not gated on C-LEARN-specific identifiers/shapes.
- Verify `VDF_RTOL` is still literally `0.01` and the AC8.2 guard thresholds (`MIN_MATCHED_FRACTION`, `MIN_MATCHED_ABSOLUTE`, `MAX_NAN_SKIPPED_FRACTION`) were not relaxed.
- Confirm the residual that resisted a general fix is filed via `track-issue` (#590/#591), not silenced — `EXPECTED_VDF_RESIDUAL` is a documented carve-out (matched-after-exclusion ≈9.5× the floor), not a tolerance loosening.

### HV-4 — #363 re-verification disposition (AC7.5 / AC7.2)
Whether a post-gate panic reproduces is unknown until run; the response includes a GitHub-issue side effect the tests don't assert.
- After the debug run (Structural gate step 2): if a panic reproduced and was converted to `Err`, confirm `previous_of_non_var_inside_subscript_index_is_err_not_panic` (`src/compiler/codegen.rs`) reproduces the converted condition and the pipeline returns a clean diagnostic. If no panic reproduced, confirm GitHub #363 has a re-verification comment (and was **not** closed unless you directed).
- Mechanical: `rg catch_unwind src/simlin-engine/tests` — the only hits should be comments + the AC8.2 synthetic guard test (a legitimate test-of-a-panicking-assertion); no production-masking `catch_unwind` remains.

### HV-5 — spike findings adequacy
Phase 2 Task 1 / Phase 5 Task 1 are `Verifies: none` spikes; Phase 2 Task 1 carried a HARD GATE (a non-representable init combined fragment must STOP-and-surface, not silently degrade).
- Read `phase_02_spike_findings.md`: confirm it states a **representable** init mechanism (injection point, synthetic ident, how `member_base + elem` offsets stay correct), and that the AC2.4 init path is implemented (exercised green by `init_recurrence_mdl_multi_member_init_scc_simulates`).
- Read `phase_05_spike_findings.md`: confirm it states the exact base+stride computation and reproduces the genuine `vector.dat`/`vector_simple.dat` `f`/`g` values by hand.

### AC6.3 — `vector.xmile` `y`/`p` exclusion disposition
The dedicated `simulates_vector_xmile_genuine` test carves out `y` (#578) and `p` (#576).
- Confirm `y[DimA]=VECTOR ELM MAP(x[three],(DimA-1))` is a scalar-source/expression-offset *compile* gap (#578), not the base/full-source numeric behavior this work fixed; and `p` is a 2-D VSO fixture-data issue (#576), a different builtin. Confirm both have open GitHub issues, and that `c`/`f`/`g` (the AC6 variables) remain hard genuine-Vensim gates vs `vector.dat` (optionally re-derive `c=[11,12,12]`, `f=[1,5,6]`, `g=[1,4,5,2,3,6]`).

## Tracked residual (not gate failures)

C-LEARN matches `Ref.vdf` within 1% on ~96.3% of cells; the ~3.7% residual is
explicitly excluded from `simulates_clearn` (via `EXPECTED_VDF_RESIDUAL`) and
tracked for future work:
- **#590** — data/graph-lookup variables importing as `0+0` (the dominant cluster).
- **#591** — SAMPLE UNTIL / INIT-`:NA:` / numeric tail / NaN-vs-`:NA:` clusters.

The exactness of this carve-out is guarded by the `clearn_residual_exactness`
test (above): it fails if the residual grows or shrinks, so the exclusion can
never silently drift.
