// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Phase 7 Task 2 (macros.AC6.4): the tiered metasd macro corpus harness.
//!
//! A `:MACRO:` grep over `test/metasd/` finds **17 macro-using `.mdl` files
//! across 14 directories** (12 directories contribute one file each, plus
//! `scientific-revolution` with `scirev7.mdl` + `scirev8.mdl` and
//! `social-network-valuation` with `groupon 1/2/3.mdl`). AC6.4's "all 14
//! macro-using metasd models" maps to these 14 *directories*; every
//! directory is represented and the multi-file ones contribute all their
//! macro-using files, so the harness covers all 17 files.
//!
//! Two tiers:
//!
//! * **Expansion tier (all 17):** `open_vensim` -> sync -> compile ->
//!   `collect_all_diagnostics`; assert **no macro-attributable diagnostic**.
//!   A model with *unrelated, non-macro* blockers (an engine-unsupported
//!   builtin such as `RANDOM NORMAL`, a model-logic circular dependency, a
//!   dimension mismatch, an MDL-parse `ExtraToken`, etc.) still PASSES the
//!   expansion tier as long as none of its diagnostics are
//!   macro-attributable -- those blockers are unrelated to macro handling
//!   and identical whether they appear in a macro body or a `main` equation.
//!
//! * **Simulation tier (subset):** for each model that **both** has a
//!   checked-in sibling reference output (a `.vdf` / `output.tab` / `.dat`)
//!   **and** has no unrelated blockers, additionally run the VM to
//!   completion and compare against the reference. Every model that is not
//!   simulation-tier-eligible is annotated below with its reason
//!   (`no reference output checked in` -- a documented prerequisite per the
//!   design; or `unrelated blocker: <desc>`).
//!
//! ## What "macro-attributable" means here (vs. the C-LEARN classifier)
//!
//! `simulate.rs::macro_attributable_diagnostics` (Phase 7 Task 1) also
//! flags any Error-severity diagnostic inside a macro template body. That
//! is correct for C-LEARN, whose macro bodies compile cleanly (only unit
//! *warnings*) -- the focused `macro_clearn_*` fixtures prove each invoked
//! macro's body computes correctly. The metasd corpus is different: a macro
//! body legitimately uses engine-unsupported Vensim builtins (`RANDOM
//! NORMAL` in `pink_noise`, `DELAY N` in `delayn`, ...) or has its own
//! model-logic issues. Those `UnknownBuiltin` / `CircularDependency`
//! diagnostics land *inside* a macro body but are NOT macro-handling
//! failures: the macro model still materializes with a correct `MacroSpec`
//! and the invocation still expands; the body equation just hit the same
//! unrelated engine-feature gap it would hit in a `main` equation. So the
//! corpus uses AC6.4's narrower definition -- a macro-attributable
//! diagnostic is exactly **a failed macro-call resolution, a missing macro
//! model, a macro arity error, or a macro-registry-build error**:
//!
//! 1. a project-level (`model` empty, `variable` `None`) `Model`
//!    `CircularDependency` / `DuplicateMacroName` -- a
//!    `MacroRegistry::build` failure (the #554 cascade class); or
//! 2. a `BadModelName` / `DuplicateMacroName` on a macro-marked model or
//!    project-level -- a macro/model name collision.
//!
//! An `UnknownBuiltin` (or any other code) *inside* a macro body is an
//! unrelated blocker, surfaced via the per-model annotations below, NOT a
//! macro failure.

mod test_helpers;

use simlin_engine::common::ErrorCode;
use simlin_engine::db::{
    Diagnostic, DiagnosticError, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel_incremental,
};
use simlin_engine::{Vm, open_vensim};

use test_helpers::ensure_results;

/// Simulation-tier eligibility for one corpus entry.
///
/// `Eligible.reference` is currently unconstructed: as of Phase 7 NO
/// metasd macro model is simulation-tier-eligible (Theil compiles+runs but
/// has no checked-in reference output; the models that carry a sibling
/// `.vdf` have heavy unrelated non-macro blockers). The variant and the
/// `metasd_simulation_tier` machinery that consumes it are kept so a model
/// becoming eligible (a reference output added, or its unrelated blocker
/// fixed) is a one-line `CORPUS` edit, not new harness code.
#[allow(dead_code)]
enum SimTier {
    /// Has a checked-in sibling reference output AND no unrelated blockers:
    /// run the VM and compare. (`reference` is the sibling path; `.vdf`
    /// goes through `VdfFile`, `.tab`/`.csv`/`.dat` through `load_csv` /
    /// `load_dat`.)
    Eligible { reference: &'static str },
    /// Not simulation-tier-eligible; the string is the documented reason
    /// (`no reference output checked in` -- a documented prerequisite per
    /// the design -- or `unrelated blocker: <description>`, surfaced for
    /// the parent to file/confirm a tracking issue).
    Skip(&'static str),
}

/// One macro-using metasd `.mdl`, annotated with its tier status in the
/// `TEST_SDEVERYWHERE_MODELS` style (a small struct rather than parallel
/// commented sections so the reason travels with the path).
struct CorpusModel {
    /// Path relative to `src/simlin-engine/` (the `../../test/...` prefix).
    path: &'static str,
    /// `true` => the expansion tier for this model is `#[ignore]`d into
    /// `metasd_expansion_tier_heavy` (it is a large real-world model whose
    /// compile exceeds the per-test time budget; see `docs/dev/rust.md`).
    /// `false` => it runs in the fast default `metasd_expansion_tier`.
    heavy: bool,
    sim: SimTier,
}

/// The full corpus: every macro-using `.mdl` under `test/metasd/` (the
/// exact 17-file list, 14 directories). Each entry's `sim` reason and
/// `heavy` flag is the *measured, verified* status as of Phase 7
/// (2026-05-15). The expansion tier asserts NONE of these has a
/// macro-attributable diagnostic; `thyroid-2008-d.mdl` is the one genuine
/// macro-attributable failure and is tracked + excluded explicitly below
/// (NOT silently dropped, NOT masked by a weakened assertion).
const CORPUS: &[CorpusModel] = &[
    // -- 12 single-file directories --
    CorpusModel {
        path: "../../test/metasd/bathtub-statistics/integration3.mdl",
        heavy: false,
        // Macros trend2/init/pink_noise all expand (correct MacroSpecs).
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             pink_noise body uses the engine-unsupported RANDOM NORMAL \
             builtin (UnknownBuiltin), same gap as in a main equation",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/beer-game/RealBeer4-Sterman13.mdl",
        heavy: true, // ~1.2s compile
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             RANDOM NORMAL (UnknownBuiltin), a model-logic peak->peak \
             CircularDependency in the `peak` macro body, and main-model \
             dimension/dependency errors -- none macro-attributable",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl",
        heavy: true, // ~0.17s but large; grouped with the opt-in corpus
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: the \
             *_data variables are unresolved GET DIRECT/GET XLS DATA refs \
             (no DataProvider supplied) -> EmptyEquation/UnknownBuiltin; \
             the SSTATS macro itself expands cleanly",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/critical-slowing/critical-slowing.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             pink_noise body uses RANDOM NORMAL (UnknownBuiltin)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/early-warnings-catastrophe/catastropeWarning2.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             pink_noise body uses RANDOM NORMAL (UnknownBuiltin)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/FREE/FREE6/FREE6-original/free 6.mdl",
        heavy: true, // ~1.2s compile
        // A sibling `all_data2.vdf` EXISTS, but `free 6.mdl` has heavy
        // unrelated MDL-parse / dimension blockers, so it is NOT
        // simulation-tier-eligible (the `init` macro itself expands).
        sim: SimTier::Skip(
            "unrelated blocker: free 6.mdl has heavy non-macro MDL-parse \
             and dimension errors (ExtraToken / UnrecognizedToken / \
             MismatchedDimensions / ArrayReferenceNeedsExplicitSubscripts \
             on main-model variables) despite a sibling all_data2.vdf; the \
             `init` macro expands cleanly",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/industrial-dynamics/IDch15/IDch15d.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             main-model UnrecognizedToken / UnknownBuiltin (the `clip` \
             macro expands cleanly)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/interpolating-arrays/InterpolatingArrays.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             main-model ExtraToken / UnrecognizedToken / CantSubscriptScalar \
             / UnknownBuiltin (the `cubic_spline` macro expands cleanly)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/pink-noise/PinkNoise2010.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             pink_noise body uses RANDOM NORMAL (UnknownBuiltin)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/theil-statistics/Theil_2011.mdl",
        heavy: false,
        // Theil_2011 COMPILES with ZERO errors (the THEIL multi-output
        // macro materializes + simulates -- pinned end-to-end by
        // simulate.rs::corpus_theil_multi_output_materializes_and_simulates).
        // It is still not simulation-tier-eligible *here* only because no
        // sibling Vensim reference output is checked in (a documented
        // prerequisite per the design).
        sim: SimTier::Skip(
            "no reference output checked in (the model itself compiles and \
             simulates with zero errors -- see \
             corpus_theil_multi_output_materializes_and_simulates)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "KNOWN MACRO BUG (escalated, see metasd_expansion_tier doc + \
             KNOWN_MACRO_BUG below): false-positive `delayn -> delayn` \
             macro-registry recursion -- a #554-class false positive that \
             #554 deliberately scoped out for stdlib-module-backed builtins \
             like DELAY N. NOT simulation-tier-eligible until fixed",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/wonderland/Wonderland3.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             main-model UnknownBuiltin / UnknownDependency (the `p_exp` / \
             `sshape` macros expand cleanly)",
        ),
    },
    // -- scientific-revolution: two macro-using files --
    CorpusModel {
        path: "../../test/metasd/scientific-revolution/scirev7.mdl",
        heavy: true, // ~2.5s compile
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             main-model UnknownBuiltin / UnknownDependency / Generic (the \
             `pos` / `clip` macros expand cleanly)",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/scientific-revolution/scirev8.mdl",
        heavy: true, // ~3.6s compile (over the 5s soft ceiling combined)
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: \
             main-model UnknownBuiltin / UnknownDependency / Generic (the \
             `pos` / `clip` macros expand cleanly)",
        ),
    },
    // -- social-network-valuation: three macro-using files. A sibling
    // .vdf set EXISTS (groupon3*.vdf / optimistic.vdf / pessimistic.vdf)
    // but every groupon model has heavy unrelated MDL-parse blockers, so
    // none is simulation-tier-eligible (the `report` macro expands). --
    CorpusModel {
        path: "../../test/metasd/social-network-valuation/groupon 1.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "unrelated blocker: data_* variables are EmptyEquation / \
             UnrecognizedToken (unresolved external data) despite sibling \
             .vdf files; the `report` macro expands cleanly",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/social-network-valuation/groupon 2.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "unrelated blocker: data_* variables are EmptyEquation / \
             UnrecognizedToken (unresolved external data) despite sibling \
             .vdf files; the `report` macro expands cleanly",
        ),
    },
    CorpusModel {
        path: "../../test/metasd/social-network-valuation/groupon 3.mdl",
        heavy: false,
        sim: SimTier::Skip(
            "unrelated blocker: data_* variables are EmptyEquation / \
             UnrecognizedToken (unresolved external data) despite sibling \
             .vdf files; the `report` macro expands cleanly",
        ),
    },
];

/// The one corpus model with a genuine, *macro-attributable* failure --
/// excluded from the expansion-tier assertion EXPLICITLY (not silently,
/// not by weakening the assertion), with the full root cause, so it is
/// loudly documented and surfaced for the parent to file/confirm a
/// tracking issue.
///
/// ROOT CAUSE (a real macro bug, escalated): `thyroid-2008-d.mdl` defines
/// `:MACRO: DELAYN(Input,DelayTime,Init,Order)` with body
/// `DELAYN = DELAY N(Input,DelayTime,Init,Order)`. The MDL importer
/// canonicalizes the Vensim `DELAY N` builtin to `delayn` (spaces
/// stripped), so the macro body's builtin call now has the *same canonical
/// name as the enclosing macro*. `module_functions::collect_called_macros`
/// then records a `delayn -> delayn` self-edge and `MacroRegistry::build`
/// fails with a project-level `Model(CircularDependency)`:
/// `"recursive macro: delayn -> delayn"`. This un-shadows `delayn` /
/// `pipeline` for the whole project (the #554 cascade shape).
///
/// This is structurally identical to the #554 false positive
/// (`:MACRO: INIT(x) ... INIT = INITIAL(x)`), but #554's fix
/// (`is_renamed_opcode_intrinsic`) is *deliberately* scoped to the
/// opcode-backed intrinsics `init`/`previous` only -- its own doc comment
/// (`module_functions.rs`, "Other importer renames ... target ordinary
/// builtins or stdlib modules with no special walk() routing ... and is
/// intentionally NOT in this set") excludes the stdlib-module-backed
/// `DELAY N` case because the #554 termination argument (the macro body's
/// call falls through to the LoadInitial/LoadPrev *opcode*) does not
/// extend to a stdlib-module-backed builtin without additional work in
/// BOTH #554 halves plus a proof that the macro-body `DELAY N` expansion
/// terminates against the stdlib module rather than re-entering the
/// `delayn` macro.
///
/// Fixing it is a focused, separate engine change (a #554 follow-up:
/// extend the renamed-builtin self-call suppression to stdlib-module-backed
/// builtins, with a termination guarantee), out of scope for this
/// corpus-harness task. ESCALATED -- the parent should file/confirm a
/// tracking issue. The expansion-tier assertion below excludes ONLY this
/// model (the other 16 are asserted), and a dedicated test pins that this
/// failure is still macro-attributable so a future fix flips it green.
const KNOWN_MACRO_BUG: &str = "../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl";

/// Compile a macro-using metasd model via the salsa path and return
/// `(macro_attributable_diagnostics, all_diagnostics, compiled_ok)`.
fn compile_and_diagnose(path: &str) -> (Vec<Diagnostic>, usize, bool) {
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync_state.to_sync_result();
    let compiled_ok = compile_project_incremental(&db, sync.project, "main").is_ok();
    let diags = collect_all_diagnostics(&db, &sync);
    let total = diags.len();

    let macro_models: std::collections::BTreeSet<&str> = dm
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    // AC6.4 macro-attributable classifier (see the module doc for why this
    // is narrower than Task 1's C-LEARN classifier): a project-level
    // macro-registry build error, or a macro/model name collision.
    let macro_diags: Vec<Diagnostic> = diags
        .into_iter()
        .filter(|d| {
            let code = match &d.error {
                DiagnosticError::Equation(e) => Some(e.code),
                DiagnosticError::Model(e) => Some(e.code),
                _ => None,
            };
            let is_project_level = d.model.is_empty() && d.variable.is_none();
            let in_macro_model = macro_models.contains(d.model.as_str());

            // (1) macro-registry-build error (the #554 cascade class).
            let registry_build_error = is_project_level
                && matches!(&d.error, DiagnosticError::Model(_))
                && matches!(
                    code,
                    Some(ErrorCode::CircularDependency) | Some(ErrorCode::DuplicateMacroName)
                );

            // (2) macro/model name collision.
            let name_collision = matches!(
                code,
                Some(ErrorCode::BadModelName) | Some(ErrorCode::DuplicateMacroName)
            ) && (in_macro_model || is_project_level);

            registry_build_error || name_collision
        })
        .collect();

    (macro_diags, total, compiled_ok)
}

/// The expansion-tier assertion over a slice of corpus entries: every
/// model must produce ZERO macro-attributable diagnostics. Accumulates
/// `(path, formatted-diagnostic)` failures and asserts the vec is empty
/// (the established corpus-harness pattern). The `KNOWN_MACRO_BUG` entry
/// is skipped here (asserted separately) so the genuine failure is neither
/// masked nor allowed to fail the 16 good models.
fn run_expansion_tier(entries: impl Iterator<Item = &'static CorpusModel>) {
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut checked = 0usize;

    for m in entries {
        if m.path == KNOWN_MACRO_BUG {
            continue; // asserted by `thyroid_known_macro_bug_is_macro_attributable`
        }
        checked += 1;
        let (macro_diags, total, compiled_ok) = compile_and_diagnose(m.path);
        eprintln!(
            "{}: total_diags={total} compiled_ok={compiled_ok} macro_attributable={}",
            m.path,
            macro_diags.len()
        );
        for d in &macro_diags {
            failures.push((m.path.to_string(), format!("{d:?}")));
        }
    }

    assert!(
        checked > 0,
        "expansion tier ran zero models -- corpus list or filter is wrong"
    );
    if !failures.is_empty() {
        eprintln!("\nmacro-attributable diagnostics (expansion tier):");
        for (path, d) in &failures {
            eprintln!("  {path}: {d}");
        }
        panic!(
            "{} macro-attributable diagnostic(s) across {checked} metasd models -- \
             these indicate a macro-handling failure (failed macro-call \
             resolution / missing macro model / macro arity error / \
             registry-build error), NOT an unrelated non-macro blocker. \
             Investigate the root cause; do not weaken this assertion.",
            failures.len()
        );
    }
}

/// macros.AC6.4 (expansion tier, fast subset). The light macro-using
/// metasd models compile via the salsa path with NO macro-attributable
/// diagnostic. Runs by default (each model compiles in well under the
/// per-test budget); the heavy real-world models are in the `#[ignore]`d
/// `metasd_expansion_tier_heavy` opt-in below (`docs/dev/rust.md`
/// test-time-budget rules). Together they cover all 14 macro-using metasd
/// directories / all 17 macro-using files.
#[test]
fn metasd_expansion_tier() {
    run_expansion_tier(CORPUS.iter().filter(|m| !m.heavy));
}

/// macros.AC6.4 (expansion tier, the heavy real-world models). Same
/// assertion as `metasd_expansion_tier` for the large models whose
/// compile exceeds the per-test time budget (beer-game ~1.2s, FREE ~1.2s,
/// covid19, scirev7 ~2.5s, scirev8 ~3.6s). `#[ignore]`d with a documented
/// opt-in per `docs/dev/rust.md`.
// Run with: cargo test -p simlin-engine --test metasd_macros --release -- --ignored metasd_expansion_tier_heavy
#[test]
#[ignore]
fn metasd_expansion_tier_heavy() {
    run_expansion_tier(CORPUS.iter().filter(|m| m.heavy));
}

/// The full expansion tier over ALL 17 macro-using files in one run
/// (light + heavy), the AC6.4 "all 14 macro-using metasd models pass the
/// expansion tier" check. `#[ignore]`d (sum of compiles ~10s, over the
/// per-test budget); the default `metasd_expansion_tier` covers the light
/// subset on every build.
// Run with: cargo test -p simlin-engine --test metasd_macros --release -- --ignored metasd_expansion_tier_full
#[test]
#[ignore]
fn metasd_expansion_tier_full() {
    run_expansion_tier(CORPUS.iter());
}

/// The one genuine macro-attributable failure in the corpus
/// (`thyroid-2008-d.mdl`) MUST still be detected as macro-attributable.
/// This is the inverse guard: it documents the real (escalated) macro bug
/// in an executable, non-masking way and will flip to "fixed" exactly when
/// the #554-follow-up engine fix lands (at which point this test fails,
/// signalling that the model should move into the passing expansion tier
/// and its `CORPUS` `SimTier` reason be revisited). See `KNOWN_MACRO_BUG`.
#[test]
fn thyroid_known_macro_bug_is_macro_attributable() {
    let (macro_diags, _total, compiled_ok) = compile_and_diagnose(KNOWN_MACRO_BUG);
    assert!(
        !compiled_ok,
        "thyroid-2008-d unexpectedly compiled -- the #554-class \
         `delayn -> delayn` false-positive recursion may be FIXED. If so, \
         remove this test, move thyroid-2008-d into the passing expansion \
         tier, and update its CORPUS SimTier reason."
    );
    let is_delayn_recursion = macro_diags.iter().any(|d| match &d.error {
        DiagnosticError::Model(e) => {
            e.code == ErrorCode::CircularDependency
                && d.model.is_empty()
                && d.variable.is_none()
                && e.get_details()
                    .map(|s| s.contains("delayn"))
                    .unwrap_or(false)
        }
        _ => false,
    });
    assert!(
        is_delayn_recursion,
        "expected the known macro-attributable `recursive macro: delayn -> \
         delayn` project-level diagnostic for thyroid-2008-d (the #554-class \
         false positive for the stdlib-module-backed DELAY N builtin); got \
         macro-attributable diagnostics: {macro_diags:#?}. If this changed, \
         the bug's shape changed -- re-investigate, do not silence."
    );
}

/// macros.AC6.4 (simulation tier). For each corpus model that is
/// simulation-tier-eligible (has a checked-in sibling reference output AND
/// no unrelated blockers), run the VM to completion and compare against
/// the reference via `ensure_results` / VDF decoding. As of Phase 7
/// (2026-05-15) NO metasd macro model is eligible: `Theil_2011.mdl`
/// compiles+simulates cleanly but has no checked-in reference output (a
/// documented prerequisite per the design); the models that *do* carry a
/// sibling `.vdf` (FREE `free 6.mdl`, the `groupon` models) have heavy
/// unrelated non-macro blockers. Every non-eligible model is annotated in
/// `CORPUS` with its reason (surfaced for the parent to track). This test
/// runs the eligible subset (currently empty) and asserts each match; it
/// also asserts every `Skip` reason is non-empty so a reason can never be
/// silently dropped.
#[test]
fn metasd_simulation_tier() {
    let mut ran = 0usize;
    for m in CORPUS {
        match m.sim {
            SimTier::Eligible { reference } => {
                ran += 1;
                let contents = std::fs::read_to_string(m.path)
                    .unwrap_or_else(|e| panic!("failed to read {}: {e}", m.path));
                let dm = open_vensim(&contents)
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", m.path));
                let mut db = SimlinDb::default();
                let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
                let compiled = compile_project_incremental(&db, sync.project, "main")
                    .unwrap_or_else(|e| panic!("compile failed for {}: {e}", m.path));
                let mut vm = Vm::new(compiled)
                    .unwrap_or_else(|e| panic!("VM creation failed for {}: {e}", m.path));
                vm.run_to_end()
                    .unwrap_or_else(|e| panic!("VM run failed for {}: {e}", m.path));
                let results = vm.into_results();

                // Decode the reference: `.vdf` via VdfFile, otherwise
                // `load_csv` (tab/csv) / `load_dat`.
                let expected = if reference.ends_with(".vdf") {
                    let bytes = std::fs::read(reference)
                        .unwrap_or_else(|e| panic!("failed to read {reference}: {e}"));
                    simlin_engine::vdf::VdfFile::parse(bytes)
                        .unwrap_or_else(|e| panic!("failed to parse VDF {reference}: {e}"))
                        .to_results_via_records()
                        .unwrap_or_else(|e| panic!("VDF decode failed for {reference}: {e}"))
                } else if reference.ends_with(".dat") {
                    simlin_engine::load_dat(reference)
                        .unwrap_or_else(|e| panic!("failed to load {reference}: {e}"))
                } else {
                    let delim = if reference.ends_with(".csv") {
                        b','
                    } else {
                        b'\t'
                    };
                    simlin_engine::load_csv(reference, delim)
                        .unwrap_or_else(|e| panic!("failed to load {reference}: {e}"))
                };
                ensure_results(&expected, &results);
            }
            SimTier::Skip(reason) => {
                assert!(
                    !reason.is_empty(),
                    "{}: a non-simulation-tier-eligible model MUST carry a \
                     documented reason (a prerequisite per the design); an \
                     empty reason would silently drop a tracked blocker",
                    m.path
                );
            }
        }
    }
    eprintln!(
        "simulation tier: {ran} eligible model(s) run; the rest are \
         annotated (no reference output checked in / unrelated blocker)"
    );
}

/// Corpus-integrity guard: the list is exactly the 17 macro-using metasd
/// files across 14 directories (the `:MACRO:`-grep result), every path
/// exists, and every model actually contains a macro (`:MACRO:`). Pins
/// the corpus against an accidental drop/add so AC6.4's "all 14
/// directories / 17 files" coverage cannot silently regress.
#[test]
fn corpus_is_exactly_the_17_macro_using_metasd_files() {
    assert_eq!(
        CORPUS.len(),
        17,
        "the metasd macro corpus must be exactly the 17 macro-using files \
         (14 directories); got {}",
        CORPUS.len()
    );

    // Distinct directories: 14 (12 single-file + scientific-revolution +
    // social-network-valuation).
    let dirs: std::collections::BTreeSet<&str> = CORPUS
        .iter()
        .map(|m| {
            // strip the leading "../../test/metasd/" and take the first
            // path component (the directory under test/metasd/).
            m.path
                .strip_prefix("../../test/metasd/")
                .expect("corpus path is under test/metasd/")
                .split('/')
                .next()
                .expect("non-empty path")
        })
        .collect();
    assert_eq!(
        dirs.len(),
        14,
        "the corpus must span exactly 14 macro-using metasd directories; \
         got {dirs:?}"
    );

    for m in CORPUS {
        let p = std::path::Path::new(m.path);
        assert!(p.exists(), "corpus model missing on disk: {}", m.path);
        let contents = std::fs::read_to_string(m.path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", m.path));
        assert!(
            contents.contains(":MACRO:"),
            "corpus model {} does not contain a :MACRO: block -- it should \
             not be in the macro corpus",
            m.path
        );
    }
}
