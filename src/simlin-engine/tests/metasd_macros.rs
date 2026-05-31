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
/// (2026-05-15). The expansion tier asserts NONE of these -- all 17 -- has a
/// macro-attributable diagnostic. (Historical note: `thyroid-2008-d.mdl` was
/// once excluded for a #554-class false-positive `delayn -> delayn`
/// macro-registry recursion; that bug is FIXED -- see the #554 follow-up,
/// `module_functions::is_renamed_stdlib_module_builtin` -- and thyroid is now
/// in the asserted set with the other 16. No model is excluded; the
/// assertion is not weakened.)
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
        // The #554-class false-positive `delayn -> delayn` macro-registry
        // recursion is FIXED (the #554 follow-up extended the shared
        // renamed-builtin self-edge suppression to the stdlib-module-backed
        // set -- `module_functions::is_renamed_stdlib_module_builtin`). The
        // `DELAYN`/`PIPELINE` macros now expand with NO macro-attributable
        // diagnostic, so this model is in the asserted expansion tier.
        sim: SimTier::Skip(
            "no reference output checked in; also unrelated blocker: the \
             DELAYN/PIPELINE macro bodies use DELAY N / DELAY MATERIAL with a \
             non-constant (macro-port) order, an orthogonal pre-existing \
             stdlib limitation surfacing as a non-macro-attributable in-body \
             UnknownBuiltin -- same gap as in a main equation. The macro \
             handling itself is correct (no #554-class cascade); pinned by \
             macro_expansion_tests::issue_554_followup_thyroid_shape_*",
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

// Historical note (no longer an open bug): `thyroid-2008-d.mdl`
// (`:MACRO: DELAYN(...) ... DELAYN = DELAY N(...)`) once produced a genuine
// macro-attributable failure -- a #554-class false-positive
// `recursive macro: delayn -> delayn` project-level `CircularDependency`.
// The MDL importer rewrites the body's Vensim `DELAY N(...)` to the
// single-token XMILE `DELAYN(...)`, colliding with the enclosing macro's
// canonical name; `module_functions::collect_called_macros` then recorded a
// false `delayn -> delayn` self-edge that failed the whole
// `MacroRegistry::build` and un-shadowed every other macro (`PIPELINE`) --
// the #554 cascade, but for a *stdlib-module-backed* builtin that #554's
// `is_renamed_opcode_intrinsic` was deliberately scoped to exclude.
//
// FIXED by the #554 follow-up: `module_functions::is_renamed_stdlib_module_builtin`
// (delegating to `builtins::is_stdlib_module_function`) extends the SHARED
// self-edge suppression predicate (`is_renamed_builtin_macro_collision`,
// used by BOTH `collect_called_macros` and `builtins_visitor`'s
// macro-shadows-everything precedence) to the stdlib-module-backed renamed
// builtins. Termination: the skipped self-call falls through to
// `rewrite_alias_module_call`/`stdlib_descriptor` and resolves to a DISTINCT
// `stdlib⁚delay1`/... module (the U+205A prefix can never name a user
// model), not back into the macro. So thyroid now expands with NO
// macro-attributable diagnostic and is in the asserted expansion tier with
// the other 16 (no model is excluded). The residual in-body `UnknownBuiltin`
// (DELAY N needs a constant order; thyroid passes a macro-port order) is an
// orthogonal, pre-existing stdlib limitation -- the same gap a `main`
// equation hits -- NOT macro-attributable (the harness's narrower AC6.4
// definition: registry-build error or macro/model name collision).
// `thyroid_produces_no_macro_attributable_diagnostics` below is the positive
// regression guard.

/// The narrower AC6.4 macro-attributable classifier (see the module doc
/// for *why* it is narrower than Task 1's C-LEARN classifier in
/// `simulate.rs::macro_attributable_diagnostics`): the metasd corpus's
/// macro bodies legitimately hit unrelated engine gaps (`RANDOM NORMAL`,
/// `DELAY N` with a non-constant order, ...) that land *inside* a macro
/// model but are NOT macro-handling failures, so this classifier flags
/// **only** the two unambiguous macro-handling failures:
///
/// 1. a project-level (`model` empty, `variable` `None`) `Model`
///    `CircularDependency`/`DuplicateMacroName` -- a
///    `MacroRegistry::build` failure (the #554 cascade class); or
/// 2. a `BadModelName`/`DuplicateMacroName` on a macro-marked model or
///    project-level -- a macro/model name collision.
///
/// `macro_models` is the set of macro-marked model names (`macro_spec` is
/// `Some`). Extracted from `compile_and_diagnose` (it was an inline
/// closure) so it is independently pinnable: `compile_and_diagnose` and
/// the non-vacuity pin
/// (`narrower_classifier_flags_registry_error_but_not_in_body_unknown_builtin`)
/// drive the *same* code, mirroring `simulate.rs`'s named-function +
/// pin-test structure. Behavior is byte-identical to the former closure.
fn narrower_macro_attributable_diagnostics(
    macro_models: &std::collections::BTreeSet<&str>,
    diags: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    diags
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
        .collect()
}

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
    let diags = collect_all_diagnostics(&db, sync.project);
    let total = diags.len();

    let macro_models: std::collections::BTreeSet<&str> = dm
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    let macro_diags = narrower_macro_attributable_diagnostics(&macro_models, diags);

    (macro_diags, total, compiled_ok)
}

/// The expansion-tier assertion over a slice of corpus entries: every
/// model must produce ZERO macro-attributable diagnostics. Accumulates
/// `(path, formatted-diagnostic)` failures and asserts the vec is empty
/// (the established corpus-harness pattern). ALL corpus entries are
/// asserted -- no model is excluded (the former `thyroid-2008-d.mdl`
/// carve-out was retired when the #554 follow-up fixed its false-positive
/// `delayn -> delayn` recursion; see the historical note above).
fn run_expansion_tier(entries: impl Iterator<Item = &'static CorpusModel>) {
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut checked = 0usize;

    for m in entries {
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

/// Positive regression guard (inverted premise -- the bug is FIXED):
/// `thyroid-2008-d.mdl` MUST now produce ZERO macro-attributable
/// diagnostics. Pre-fix this model was the one genuine macro-attributable
/// failure in the corpus -- a #554-class false-positive
/// `recursive macro: delayn -> delayn` project-level `CircularDependency`
/// from the importer's `DELAY N -> DELAYN` rewrite colliding with the
/// enclosing macro's name (see the historical note above). The #554
/// follow-up (`module_functions::is_renamed_stdlib_module_builtin`)
/// suppresses that false self-edge, so thyroid is now in the asserted
/// expansion tier; this test additionally pins thyroid *specifically* (so a
/// regression of the follow-up is caught here with a focused message, not
/// only in the bulk `metasd_expansion_tier`). It deliberately does NOT
/// assert `compiled_ok`: the macro handling is correct, but the body's
/// `DELAY N(...,Order)` with the order a macro *port* still hits the
/// orthogonal, pre-existing stdlib "order must be a compile-time constant"
/// limitation -- a non-macro-attributable in-body `UnknownBuiltin` (the same
/// gap a `main` equation hits), tracked separately.
#[test]
fn thyroid_produces_no_macro_attributable_diagnostics() {
    const THYROID: &str = "../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl";
    let (macro_diags, _total, _compiled_ok) = compile_and_diagnose(THYROID);
    assert!(
        macro_diags.is_empty(),
        "thyroid-2008-d must produce ZERO macro-attributable diagnostics \
         after the #554 follow-up (the false-positive `delayn -> delayn` \
         macro-registry recursion is fixed). A non-empty set means the \
         follow-up regressed -- re-investigate \
         `module_functions::is_renamed_stdlib_module_builtin` / the shared \
         `is_renamed_builtin_macro_collision` predicate; do not silence. \
         Got: {macro_diags:#?}"
    );
}

/// Compile a tiny inline `.mdl` through the SAME real salsa diagnostic
/// path `compile_and_diagnose` uses, returning the macro-marked model
/// names and every diagnostic. Used by the non-vacuity pin so it
/// exercises *real* diagnostics (a genuine registry-build error / a
/// genuine in-body `UnknownBuiltin`), not synthetic `Diagnostic` structs.
fn macro_models_and_diags(source: &str) -> (Vec<String>, Vec<Diagnostic>) {
    let dm = open_vensim(source).expect("inline macro mdl parses");
    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync_state.to_sync_result();
    // Drive the compile so the registry-build / macro-resolution
    // diagnostics are accumulated (same as `compile_and_diagnose`).
    let _ = compile_project_incremental(&db, sync.project, "main");
    let diags = collect_all_diagnostics(&db, sync.project);
    let macro_models: Vec<String> = dm
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.clone())
        .collect();
    (macro_models, diags)
}

/// Non-vacuity pin for the *narrower* AC6.4 classifier
/// (`narrower_macro_attributable_diagnostics`). Unlike `simulate.rs`'s
/// classifier, this narrower copy had no independent pin --
/// `thyroid_produces_no_macro_attributable_diagnostics` only exercises
/// the registry-error-ABSENT path, so the classifier could silently
/// degrade (stop flagging a real registry-build error, or start flagging
/// the metasd in-body engine-gap diagnostics it must tolerate) with no
/// test failing. This asserts BOTH directions against REAL diagnostics
/// driven through the same salsa path the corpus harness uses:
///
/// * **(a) flagged:** a directly-recursive macro
///   (`RECUR = RECUR(a,b) + 1`) makes `MacroRegistry::build` reject the
///   set, emitting a project-level `Model` `CircularDependency` (the #554
///   cascade class). The narrower classifier MUST flag it -- if it did
///   not (a "flag nothing" mutation), the corpus harness would silently
///   stop catching the very failure class it exists to catch.
/// * **(b) NOT flagged:** a macro whose *body* calls an
///   engine-unimplemented builtin (`GAP = RANDOM NORMAL(-6, 6, 0, 1,
///   seed)`, the literal metasd `pink_noise` shape) yields an in-body
///   `UnknownBuiltin` on the macro-marked model -- an *unrelated* engine
///   gap, identical whether it appears in a macro body or a `main`
///   equation. The narrower classifier MUST NOT flag it -- if it did (a
///   "flag everything" mutation, or a widening to Task 1's body-error
///   rule), every metasd model with such a gap would false-positive and
///   the expansion tier's "no model excluded" guarantee would collapse.
///
/// Genuinely non-vacuous: mutating the classifier body to
/// `registry_build_error || name_collision || true` makes (b) fail;
/// mutating it to `false` makes (a) fail (verified by the author and
/// reverted). The two directions also pin the classifier's *narrowness*
/// (it must NOT adopt Task 1's macro-body-Error rule -- that is the
/// documented metasd/C-LEARN difference).
#[test]
fn narrower_classifier_flags_registry_error_but_not_in_body_unknown_builtin() {
    use simlin_engine::db::DiagnosticSeverity;

    const CONTROL_TAIL: &str = "\nINITIAL TIME = 0 ~~|\n\
         FINAL TIME = 1 ~~|\n\
         SAVEPER = 1 ~~|\n\
         TIME STEP = 1 ~~|\n";

    // --- (a) genuine project-level macro-registry-build error ---
    let recursive = format!(
        "{{UTF-8}}\n\
         :MACRO: RECUR(a, b)\n\
         RECUR = RECUR(a, b) + 1\n\t~\ta\n\t~\tdirectly recursive\n\t~\t|\n\
         :END OF MACRO:\n\
         y= RECUR(3, 4) ~~|\n{CONTROL_TAIL}"
    );
    let (macro_models_a, diags_a) = macro_models_and_diags(&recursive);
    let set_a: std::collections::BTreeSet<&str> =
        macro_models_a.iter().map(String::as_str).collect();

    // Sanity (the pin's premise, asserted so a pipeline change that stops
    // emitting the registry error surfaces here, not as a silent vacuous
    // pass): a REAL project-level `Model` `CircularDependency` exists.
    assert!(
        diags_a.iter().any(|d| {
            d.model.is_empty()
                && d.variable.is_none()
                && d.severity == DiagnosticSeverity::Error
                && matches!(
                    &d.error,
                    DiagnosticError::Model(e) if e.code == ErrorCode::CircularDependency
                )
        }),
        "premise: a directly-recursive macro must emit a project-level \
         Model CircularDependency (the registry-build failure); got: \
         {diags_a:#?}"
    );

    let flagged_a = narrower_macro_attributable_diagnostics(&set_a, diags_a);
    assert!(
        !flagged_a.is_empty(),
        "(a) the narrower classifier MUST flag a project-level \
         macro-registry-build error (recursive macro -> \
         CircularDependency, the #554 cascade class). An empty result \
         means the classifier degraded to 'flag nothing' -- the corpus \
         harness would silently stop catching macro-registry failures. \
         Got: {flagged_a:#?}"
    );

    // --- (b) in-body engine gap (the metasd RANDOM NORMAL shape) ---
    // The macro body calls `RANDOM NORMAL`, the *literal* metasd
    // `pink_noise` shape: recognized by the MDL parser but
    // engine-unimplemented, so it surfaces as a non-project-level in-body
    // diagnostic -- an unrelated engine gap, NOT a macro-handling failure
    // (identical whether it appears in a macro body or a `main` equation).
    let in_body_gap = format!(
        "{{UTF-8}}\n\
         :MACRO: GAP(seed)\n\
         GAP = RANDOM NORMAL(-6, 6, 0, 1, seed)\n\t~\tdmnl\n\t~\tbody hits the RANDOM NORMAL engine gap\n\t~\t|\n\
         :END OF MACRO:\n\
         z= GAP(1) ~~|\n{CONTROL_TAIL}"
    );
    let (macro_models_b, diags_b) = macro_models_and_diags(&in_body_gap);
    let set_b: std::collections::BTreeSet<&str> =
        macro_models_b.iter().map(String::as_str).collect();
    assert!(
        !set_b.is_empty(),
        "premise: `GAP` must import as a macro-marked model"
    );

    // Premise: a REAL in-body `UnknownBuiltin` exists ON the macro-marked
    // model `gap` (NOT project-level) -- the *literal* documented metasd
    // shape ("an `UnknownBuiltin` inside a macro body is an unrelated
    // blocker, NOT a macro failure"; module doc lines 58-60). Asserting
    // the exact shape (not just "some Error") so a pipeline change that
    // stopped producing it surfaces here, not as a vacuous pass.
    let has_in_body_unknown_builtin = diags_b.iter().any(|d| {
        d.severity == DiagnosticSeverity::Error
            && !(d.model.is_empty() && d.variable.is_none())
            && set_b.contains(d.model.as_str())
            && matches!(
                &d.error,
                DiagnosticError::Equation(e) if e.code == ErrorCode::UnknownBuiltin
            )
    });
    assert!(
        has_in_body_unknown_builtin,
        "premise: the macro body's RANDOM NORMAL call must produce a \
         non-project-level UnknownBuiltin ON the macro-marked model (the \
         literal metasd RANDOM-NORMAL-in-a-macro-body shape); got: \
         {diags_b:#?}"
    );

    let flagged_b = narrower_macro_attributable_diagnostics(&set_b, diags_b);
    assert!(
        flagged_b.is_empty(),
        "(b) the narrower classifier MUST NOT flag an in-body \
         UnknownBuiltin (an unrelated engine gap identical in a macro \
         body or a `main` equation -- the documented reason this \
         classifier is narrower than Task 1's). A non-empty result means \
         the classifier degraded to 'flag everything' (or wrongly adopted \
         Task 1's macro-body-Error rule); every metasd model with such a \
         gap would false-positive and the expansion tier's no-model-\
         excluded guarantee would collapse. Got: {flagged_b:#?}"
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
