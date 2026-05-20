# C-LEARN Residual — Test Requirements

Maps every acceptance criterion in
[`docs/design-plans/2026-05-19-clearn-residual.md`](/docs/design-plans/2026-05-19-clearn-residual.md)
to its verification: an automated test (with phase/task, type, file path, and run
command) or human verification (with justification + approach). Used by the
test-analyst during execution to validate coverage.

## Conventions

- **Default suite** = `cargo test -p simlin-engine` (the non-ignored, capped suite
  the pre-commit hook runs). New per-phase generality fixtures live here and must
  stay tiny (root `CLAUDE.md` 3-minute cap).
- **`file_io`-gated** = the test is compiled only under `--features file_io`
  (file-fixture path, CSV providers, the C-LEARN gate).
- **`#[ignore]` runtime class** = `#[test] #[ignore]`, NOT part of the default
  suite; run explicitly via `-- --ignored`, release build (C-LEARN is ~53k lines,
  ~5s just to parse). The two C-LEARN gates are the only such tests here.
- **C-LEARN gate run command** (AC4.1/AC4.2; informational observation in
  Phases 1-3):
  `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored simulates_clearn clearn_residual_exactness`
- All engine-fix generality fixtures are C-LEARN-independent by design constraint
  (no fix may key on a C-LEARN name/path/residual-list); the only C-LEARN contact
  is read-only measurement in the two gates.

---

## clearn-residual.AC1 — Lookup-only variables produce Vensim-matching saved series (Phase 1)

| AC | Criterion (abridged) | Verification | Phase/Task | Type | File / run |
|----|----------------------|--------------|------------|------|------------|
| **AC1.1** | Scalar lookup-only var produces the Vensim-determined series, not constant `gf(0)` | **Automated** (rule correctness). The Vensim *determination* itself is a documented spike (P1 T1) — a human/process artifact (written conclusion + chosen `index_expr`); the resulting rule's correctness is automated by the `scalar_lookup_only_*` fixture. | P1 T1 (spike, determines rule) → P1 T3 (RED) / T4 (GREEN) | unit (in-crate) | `src/simlin-engine/src/lookup_only_tests.rs` (fn names contain `lookup_only`); `cargo test -p simlin-engine --lib lookup_only` |
| **AC1.2** | Arrayed (`Equation::Arrayed`) lookup-only produces correct per-element series, not literal `0` | **Automated** | P1 T3/T4 | unit | `src/simlin-engine/src/lookup_only_tests.rs`; `cargo test -p simlin-engine --lib lookup_only` |
| **AC1.3** | Apply-to-all lookup-only produces correct series for every element | **Automated** | P1 T3/T4 | unit | `src/simlin-engine/src/lookup_only_tests.rs`; same command |
| **AC1.4** | Applied lookup `var(idx)`→`LOOKUP(var,idx)` still correct (no regression) | **Automated** — applied-lookup fixture in T3 + the pure detection helper (T2) keeping WITH-LOOKUP on `gf(input)` | P1 T2 (helper unit tests) + T3/T4 (applied fixture) | unit | `src/simlin-engine/src/lookup_only_tests.rs` (applied case) + `src/simlin-engine/src/compiler/mod.rs` `#[cfg(test)]` (`is_lookup_only`); `cargo test -p simlin-engine --lib lookup_only is_lookup_only` |
| **AC1.5** | Non-alphabetical declared element order maps each element to its own table | **Automated** — arrayed fixture uses a non-alphabetical order (e.g. `Dim=[Z,A,M]`) with element-identifying tables | P1 T3/T4 | unit | `src/simlin-engine/src/lookup_only_tests.rs` (`arrayed_lookup_only_non_sorted_order`); same command |
| **AC1.6** | Out-of-range scalar `Lookup` clamp/NaN behavior unchanged by the fix | **Automated** — (a) existing per-element-GF / lookup tests stay green; (b) a clamp assertion (added in T4 if not already covered) | P1 T4 | unit | `src/simlin-engine/src/per_element_gf_tests.rs` (no regression) + clamp assertion; `cargo test -p simlin-engine --lib per_element_gf` and default suite |

Notes: `TestProject` cannot attach graphical functions, so fixtures build
`datamodel::Project` directly (mirror `per_element_gf_tests.rs::arrayed_gf_project`/`ramp_gf`).
None of these are `file_io`-gated or ignored. Pruning `EXPECTED_VDF_RESIDUAL` for
#590 bases happens in Phase 4, not here (the `--ignored` exactness guard will report
`shrank` after this phase — expected).

---

## clearn-residual.AC2 — User macros sharing a builtin name resolve via the macro path (Phase 2)

| AC | Criterion (abridged) | Verification | Phase/Task | Type | File / run |
|----|----------------------|--------------|------------|------|------------|
| **AC2.1** | `:MACRO: RAMP FROM TO(...)` with `islinear=0` runs the exponential branch, not a linear ramp | **Automated** | P2 T1 (RED) → T3 (GREEN) | unit (in-crate inline-MDL harness, no `file_io`) | `src/simlin-engine/src/macro_expansion_tests.rs` (new `#[test]` near `ac5_4_macro_shadows_ramp_from_to_builtin`); `cargo test -p simlin-engine --lib <new_test>` |
| **AC2.2** | Same call with `islinear=1` runs the linear branch (no regression) | **Automated** — new test's `y_lin` assertion *and* the preserved file fixture | P2 T1 + T3 | unit + integration (`file_io`) | `macro_expansion_tests.rs` (`y_lin`) + `src/simlin-engine/tests/simulate.rs::simulates_macro_clearn_ramp_from_to_mdl`; `cargo test -p simlin-engine --features file_io --test simulate simulates_macro_clearn_ramp_from_to_mdl` |
| **AC2.3** | After import `RAMP FROM TO(...)` survives as a resolvable call (not a pre-linearized `RAMP(...)` string) | **Automated** — inverted formatter unit test | P2 T2 (RED, inverts) → T3 (GREEN) | unit | `src/simlin-engine/src/mdl/xmile_compat.rs` (`test_ramp_from_to_survives_as_macro_call`, formerly `test_ramp_from_to_transforms_args`); `cargo test -p simlin-engine --lib test_ramp_from_to` |
| **AC2.4** | Nonpositive endpoints force the macro's `linear` selector | **Automated** — `y_force` assertion in the T1 test | P2 T1 → T3 | unit | `src/simlin-engine/src/macro_expansion_tests.rs` (`y_force`); same command as AC2.1 |
| **AC2.5** | No `xmile_compat.rs` formatter special-case rewrites a model-defined macro name ahead of macro resolution (incl. audited `SSHAPE`) | **Mostly automated** — guard tests prove macro-shadows precedence; the *audit conclusion* itself (which formatter arms carry the latent hazard) is a human-reviewed doc comment | P2 T4 | unit (existing guards) + doc-comment audit (human) | `src/simlin-engine/src/macro_expansion_tests.rs` (`ac5_4_macro_shadows_ramp_from_to_builtin`, `ac5_4_macro_shadows_sshape_builtin`); `cargo test -p simlin-engine --lib ac5_4_macro_shadows`. Audit prose in `xmile_compat.rs` above `format_call_ctx`. |

Notes: the existing `simulates_macro_clearn_ramp_from_to_mdl` does NOT discriminate
bug from fix (its `islinear=1`/positive-endpoint series is identical either way);
the discriminating proof is the new T1 exponential-branch test. SSHAPE was confirmed
*not* a formatter-rewrite hazard (name-preserving via `format_function_name`) — T4
records this rather than guarding it. `im_3_*`/`relative_emissions_*` reconciliation
is observed via the `--ignored` gate but pruned in Phase 4.

---

## clearn-residual.AC3 — User-macro `INITIAL` recurrences produce correct multi-step values (Phase 3)

| AC | Criterion (abridged) | Verification | Phase/Task | Type | File / run |
|----|----------------------|--------------|------------|------|------------|
| **AC3.1** | `:MACRO: INIT(x)=INITIAL(x)` + scalar capture holds constant across all saved steps | **Automated** | P3 T1 (RED) → T4 (GREEN) | integration (`file_io`, inline-MDL via `run_inline_mdl`) | `src/simlin-engine/tests/simulate.rs` (new inline `#[test]` near `simulates_macro_independent_invocation_state`); `cargo test -p simlin-engine --features file_io --test simulate <scalar_test>` |
| **AC3.2** | Element-wise `INITIAL` recurrence via passthrough macro correct at t0 and every step (no drop to `0`, no spurious `:NA:`) | **Automated** — primarily via T4 collapse; contingent T5 only if a non-passthrough module remains divergent | P3 T1 (RED) → T4 (GREEN); P3 T5 (contingent) | integration (`file_io`, file fixture) | fixture `test/test-models/tests/macro_init_recurrence/` (= `helper_recurrence.mdl` + prepended `:MACRO: INIT`); test in `src/simlin-engine/tests/simulate.rs` via `simulate_mdl_path`; `cargo test -p simlin-engine --features file_io --test simulate <element_test>` |
| **AC3.3** | Bare `INITIAL(expr)` (no user macro) still compiles to `LoadInitial` and behaves as today | **Automated** — preserved reference fixture | P3 T4 (no-regression) | integration (`file_io`) | `src/simlin-engine/tests/simulate.rs::helper_recurrence_mdl_synthetic_helper_in_scc_simulates`; `cargo test -p simlin-engine --features file_io --test simulate helper_recurrence` |
| **AC3.4** | Passthrough collapse fires only for genuine `out=BUILTIN(param)`; a name-sharing non-passthrough macro still expands as a macro | **Automated** — pure classifier unit tests + registry-level wiring tests | P3 T2 (classifier unit) + T3 (registry wiring) | unit | `src/simlin-engine/src/module_functions.rs` `#[cfg(test)]` (`classify_passthrough` positives/negatives; registry `passthrough==Some/None`); `cargo test -p simlin-engine --lib classify_passthrough` and `cargo test -p simlin-engine --lib macro_registry passthrough` |
| **AC3.5** | `SAMPLE UNTIL` fed by a corrected `INITIAL` samples until the correct time with no dedicated `SAMPLE UNTIL` change | **Automated, but indirectly** — resolves downstream of AC3.2; the C-LEARN `SAMPLE UNTIL`-fed bases reconcile in the Phase 4 `--ignored` gate (no dedicated `SAMPLE UNTIL` test/fixture is created). Phase 3 only *observes* movement (informational). | P3 T4 (observe) → P4 T4 (`simulates_clearn`/exactness confirm) | integration, `#[ignore]` runtime class | `src/simlin-engine/tests/simulate.rs` (`simulates_clearn`, `clearn_residual_exactness`); C-LEARN gate run command |

Notes: `ensure_results` skips implicit `$⁚` module vars, so fixtures assert the
user-facing series directly. NO NA-arithmetic change is made (out of scope). Task 5
is contingent — if performed it adds a minimal non-passthrough INITIAL-macro-module
regression fixture (default suite); if skipped, the skip is recorded with `--ignored`
evidence (a human/process artifact, not a test).

---

## clearn-residual.AC4 — Residual honestly attributed and gate tightened (Phase 4)

| AC | Criterion (abridged) | Verification | Phase/Task | Type | File / run |
|----|----------------------|--------------|------------|------|------------|
| **AC4.1** | After P1-3, `EXPECTED_VDF_RESIDUAL` reduced to the proven remainder and `simulates_clearn` passes | **Automated** | P4 T4 (depends on T1 classify, contingent T2/T3 fixes) | integration, **`file_io` + `#[ignore]` runtime class, release** | `src/simlin-engine/tests/simulate.rs::simulates_clearn`; `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored simulates_clearn` |
| **AC4.2** | `clearn_residual_exactness` passes — live failing set equals `EXPECTED_VDF_RESIDUAL` exactly (no `grew`/`shrank`) | **Automated** | P4 T4 | integration, **`file_io` + `#[ignore]` runtime class, release** | `src/simlin-engine/tests/simulate.rs::clearn_residual_exactness`; `... -- --ignored clearn_residual_exactness` |
| **AC4.3** | Every still-excluded base carries a sourced one-line reason (5-category taxonomy); #590/#591 updated | **Partly automated, partly human.** Automated: the exactness guard (AC4.2) proves the list is exactly the live remainder. Human: the prose category reasons in `EXPECTED_VDF_RESIDUAL` comments and the GitHub #590/#591 issue updates (`gh issue comment`/`close`) are reviewed, not asserted by a test. | P4 T1 (classify, human) + T4 (comments + `gh`) | guard (automated) + doc/issue review (human) | guard: `clearn_residual_exactness` (above). Reasons: `src/simlin-engine/tests/simulate.rs:~1563-1622` comments. Issues: #590, #591 (`gh`). |
| **AC4.4** | Comparator 1% tol / per-series floor / matched-variable floor unchanged; gate cannot pass vacuously | **Automated** — a non-ignored guard test pinning the five constants (`VDF_RTOL`, `K_ATOL`, `MIN_MATCHED_FRACTION`, `MIN_MATCHED_ABSOLUTE`, `MAX_NAN_SKIPPED_FRACTION`) + a sub-floor-panics assertion | P4 T4 (item 4) | unit (default suite) | `src/simlin-engine/tests/simulate.rs` (guard test pinning constants + below-floor panic); `cargo test -p simlin-engine` |
| **AC4.5** | NaN-vs-`:NA:` series (e.g. `slr_inches_from_2000`) handled by NaN-skip; never enters the failure set | **Automated** — `classify_vdf_ident` unit test on a synthetic NaN-vs-`:NA:` series (asserts `nan_skipped` cells, zero `failures`), C-LEARN-independent | P4 T4 (item 3) | unit (default suite) | `src/simlin-engine/tests/simulate.rs` (`classify_vdf_ident` synthetic-series unit test); `cargo test -p simlin-engine` |

Notes: Task 1 (classification) and the contingent Task 2 (VDF-reader decode fix) /
Task 3 (engine-genuine fix) feed AC4.1/AC4.2 by removing reference-side artifacts and
real divergences from the live set; if performed, each ships a non-ignored synthetic
regression test (`cargo test -p simlin-engine vdf` for T2). The two C-LEARN gates are
intentionally NOT in the default `cargo test`.

---

## clearn-residual.AC5 — Native targets resolve external-data functions (Phase 5 — cuttable)

> **Cuttability:** Phase 5 is forward-looking and independent of C-LEARN residual
> closure (C-LEARN needs no external data). If the phase is cut, AC5.1–AC5.4 are
> not delivered and are not gating on the residual work.

| AC | Criterion (abridged) | Verification | Phase/Task | Type | File / run |
|----|----------------------|--------------|------------|------|------------|
| **AC5.1** | Native (`file_io`) build resolves `GET DIRECT DATA`/`LOOKUPS`/`CONSTANTS` from a companion file relative to the model | **Automated** | P5 T1 (RED, extract `open_vensim_model`) → T2 (GREEN, wire provider) | integration (`file_io`) | `src/simlin-cli/tests/external_data.rs` (calls extracted `open_vensim_model`); fixture `test/sdeverywhere/models/directconst/` (primary) or tiny new `test/test-models/` CSV fixture; `cargo test -p simlin-cli external_data` |
| **AC5.2** | Resolved external data drives simulation (downstream value reflects the data, not a zeroed series) | **Automated** — same test asserts the CSV-derived downstream value | P5 T1 → T2 | integration (`file_io`) | `src/simlin-cli/tests/external_data.rs`; `cargo test -p simlin-cli external_data` |
| **AC5.3** | Missing/unreadable data file produces a clear diagnostic, not a silent `0+0` zeroing | **Automated** — missing-CSV test asserts a clear file-level diagnostic via the `FilesystemDataProvider` error path | P5 T2 | integration (`file_io`) | `src/simlin-cli/tests/external_data.rs` (missing-file case); `cargo test -p simlin-cli` |
| **AC5.4** | WASM/libsimlin remain on the null provider (unchanged); a tracked follow-up issue captures their data-supply API | **Partly automated, partly human/process.** Automated: WASM/libsimlin default-open paths untouched (diff/build confirm). Human/process: filing the tracked follow-up issue via the `track-issue` agent. | P5 T3 | build/diff confirm (automated) + issue filing (human) | `cargo test -p simlin-engine` + WASM build per repo conventions; follow-up issue filed via `track-issue` agent |

---

## clearn-residual.AC6 — Cross-cutting correctness and hygiene

| AC | Criterion (abridged) | Verification | Type | File / run |
|----|----------------------|--------------|------|------------|
| **AC6.1** | No change keys on a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list | **Human verification** (grep-based review of the change set). Optionally scriptable as a grep check, but judgment is required (the `EXPECTED_VDF_RESIDUAL` carve-out legitimately contains base names; the reviewer must confirm it is the *only* C-LEARN contact and no code *branches* on those names). | grep review (human) | `git diff main...HEAD` reviewed for C-LEARN names/paths/residual-list branches; carve-out is the sole permitted contact |
| **AC6.2** | Each engine fix (Phases 1-3) ships ≥1 generality-proving test independent of C-LEARN | **Automated** — satisfied by the per-phase fixtures | unit/integration | P1: `lookup_only_tests.rs` + `is_lookup_only`; P2: `macro_expansion_tests.rs` exponential-branch + inverted formatter test; P3: `classify_passthrough` + scalar/element INIT fixtures. All run in the default suite (P3 element fixture under `file_io`). |
| **AC6.3** | `cargo test --workspace` green within the 3-min cap AND the pre-commit hook passes | **Automated** — enforced by the pre-commit hook / CI (Rust fmt/clippy/tests, TS lint/types, WASM build, TS tests, Python bindings) | suite / hook | pre-commit hook (`scripts/pre-commit`); `cargo test --workspace`. (The two C-LEARN `#[ignore]` gates are excluded from this capped run by design.) |
| **AC6.4** | New end-to-end C-LEARN tests stay `#[ignore]` (runtime class) and run via `--ignored` | **Human/structural check** — confirm `simulates_clearn` / `clearn_residual_exactness` retain `#[test] #[ignore]` and are absent from the default suite; optionally grep for the attribute. Not a behavioral assertion. | structural review (human) | `src/simlin-engine/tests/simulate.rs` (`#[ignore]` retained on both gates); confirm they don't run under bare `cargo test` |

---

## Automated coverage summary

All **29** sub-criteria (AC1: 6, AC2: 5, AC3: 5, AC4: 5, AC5: 4, AC6: 4) bucketed
exactly once:

- **Automated, default suite (`cargo test -p simlin-engine`) — 18:** AC1.1, AC1.2,
  AC1.3, AC1.4, AC1.5, AC1.6, AC2.1, AC2.3, AC2.4, AC3.1, AC3.2, AC3.3, AC3.4, AC4.4,
  AC4.5, AC6.2, AC6.3, plus AC5.1, AC5.2, AC5.3 *if Phase 5 ships* (Phase 5 is
  cuttable; AC2.2 also has a `file_io` integration leg). These run unignored under
  the pre-commit hook / CI cap. (AC3.2's element fixture and AC2.2's regression
  fixture require `--features file_io`; they are still part of the unignored suite.)
- **Automated, `#[ignore]` runtime class (release, `--ignored`) — 2:** AC4.1
  (`simulates_clearn`), AC4.2 (`clearn_residual_exactness`). Run only via
  `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored ...`;
  intentionally NOT in the default `cargo test`/pre-commit run (per AC6.3, AC6.4).
- **Hybrid (automated test/guard + a human/process component) — 4:** AC2.5 (shadow
  guards automated; audit conclusion is a reviewed doc comment), AC3.5 (resolves
  downstream — confirmed by the automated `--ignored` gate, no dedicated test),
  AC4.3 (exactness guard automated; the per-base category prose and #590/#591 issue
  updates are human-reviewed), AC5.4 (WASM/libsimlin-unchanged is build/diff-confirmable;
  filing the follow-up issue via the `track-issue` agent is process).
- **Primarily human/process — 2:** AC6.1 (grep/review the change set for C-LEARN
  special-casing), AC6.4 (`#[ignore]` retention / not-in-default-suite structural
  check).

Process artifacts feeding the automated rules (not themselves ACs): AC1.1's
Vensim-semantics *determination* (Phase 1 Task 1 spike) is a written conclusion whose
rule is then validated by the `lookup_only` fixtures; Phase 3 Task 5's skip rationale
and Phase 4 Task 1's classification table are human-recorded inputs to automated
guards. Phase 5's three contingent/automated ACs (AC5.1–AC5.3) drop if the cuttable
phase is not shipped.

## Human verification checklist

1. **AC1.1 (spike conclusion)** — Confirm Phase 1 Task 1's empirical determination of
   the standalone-lookup saved-value rule is recorded (evidence + chosen `index_expr`)
   as the `lookup_only_tests.rs` module doc comment, and that temporary instrumentation
   was reverted. (Rule correctness is then automated.)
2. **AC2.5 (formatter audit)** — Read the doc comment above `format_call_ctx`'s match:
   it must record which restructuring arms carry the macro-shadowing hazard and note
   that `SSHAPE` is name-preserving (no guard needed). Confirm no new formatter arm
   rewrites a name a model could define as a macro.
3. **AC3.5 (downstream)** — Confirm no dedicated `SAMPLE UNTIL` code/test was added and
   that the `SAMPLE UNTIL`-fed bases reconcile via the Phase 4 `--ignored` gate.
4. **AC3 Task 5 (contingent)** — Confirm Task 5 was either performed (with a minimal
   non-passthrough INITIAL-macro regression fixture green) or explicitly skipped with
   recorded `--ignored` evidence that the call-site collapse sufficed.
5. **AC4.3 (attribution + issues)** — Confirm every still-excluded base in
   `EXPECTED_VDF_RESIDUAL` has a one-line sourced reason under one of the five
   categories (no "unknown"), and that #590/#591 were updated/closed with the final
   per-base disposition.
6. **AC5.4 (WASM/libsimlin + follow-up)** — Confirm the WASM/libsimlin default-open
   paths are untouched by the Phase 5 diff and that the data-supply follow-up issue
   was filed via the `track-issue` agent. (Skip if Phase 5 is cut.)
7. **AC6.1 (no special-casing)** — Grep/review the full change set: no code branches on
   a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list; the
   `EXPECTED_VDF_RESIDUAL` carve-out is the only place C-LEARN base names appear.
8. **AC6.4 (`#[ignore]` retention)** — Confirm `simulates_clearn` and
   `clearn_residual_exactness` retain `#[test] #[ignore]` and do not run under bare
   `cargo test`.
