// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the LTM (Loops That Matter) array and
//! aggregate-node machinery -- the arrayed-model cluster of findings from
//! the LTM deep review.
//!
//! ## Arrayed-vs-scalar link-score parity
//!
//! `arrayed_isolated_loop_link_scores_match_scalar` builds an arrayed
//! model whose every element is structurally identical to a scalar
//! reference model, and pins that the per-element LTM link scores *and*
//! loop score exactly reproduce the scalar model's values.
//!
//! This is a finer-grained guard than the isolated-loop `+/-1` invariant
//! pinned by `ltm_dt_invariance.rs` and
//! `simulate_ltm.rs::arrayed_isolated_loop_raw_score_is_one_per_element`:
//! a loop score is the product of its link scores, so two compensating
//! per-element link-score errors can still multiply to a correct `+1`
//! loop score. "Finding 2" of the review -- a bare-arrayed nested
//! `PREVIOUS` term in the flow-to-stock link score silently stubbing to
//! `0` -- was exactly that kind of single-link error (it collapsed the
//! *loop* score to `1/9` only because nothing compensated it). This test
//! guards the per-link contract directly, so a future per-element
//! link-score error is caught even if it happens to cancel in the
//! loop-score product.
//!
//! ## Aggregate-node loop-score sanity
//!
//! `whole_extent_sum_agg_loop_scores_are_finite_and_sustained` pins that a
//! feedback loop running *through* a whole-extent `SUM(...)` reducer --
//! which LTM hoists into a synthetic `$⁚ltm⁚agg⁚{n}` node -- is genuinely
//! scored: every loop score is finite, and an agg-routed loop carries a
//! real, non-zero value at every discoverable timestep. The
//! aggregate-node tests already in `simulate_ltm.rs` cover agg *value*
//! correctness and *link*-score finiteness for whole-extent reducers
//! (`test_agg_aux_value_matches_reducer`) and *loop*-score
//! finiteness/non-degeneracy for *sliced* (`SUM(pop[NYC,*])`) reducers
//! (`test_sliced_agg_cross_element_loop_simulates`); the whole-extent
//! reducer's loop-score path was the remaining gap. An agg-routed loop
//! score that is identically `0` past the startup guard would mean the
//! agg-half link scores failed to compile and were silently stubbed to
//! `0` -- the silent-stubbing class of bug Piece 3 of this effort
//! addressed.
//!
//! ## Bare-arrayed nested `PREVIOUS` compile limitation
//!
//! `bare_arrayed_nested_previous_fails_to_compile` is a *characterization
//! test* -- a live tripwire, not a disabled or `#[ignore]`d test. It pins
//! the current, broken behavior that a plain apply-to-all equation with a
//! *bare* (unsubscripted) arrayed name inside a *nested* `PREVIOUS` --
//! `p2bare[region] = PREVIOUS(PREVIOUS(pop))` -- fails to compile. That is
//! exactly the shape the LTM flow-to-stock link-score generator emits;
//! "Finding 2" of the review was this bug surfacing *through* the
//! generator, and Piece 2 fixed Finding 2 with a generator-side
//! workaround, but the underlying engine limitation (GH #541) is unfixed.
//! The test passes today; when GH #541 is fixed it begins to fail, which
//! forces whoever fixes it to flip it into a positive regression test.
//! `subscripted_arrayed_nested_previous_matches_scalar` is its positive
//! companion: the properly subscripted form already works, per element.

use simlin_engine::datamodel::{self, Dimension};
use simlin_engine::db::{
    LtmSyntheticVar, SimlinDb, compile_project_incremental, model_ltm_variables,
    set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::test_common::TestProject;
use simlin_engine::{Results, Vm};

/// Canonical prefixes of the two LTM score-variable families. The
/// separator is U+205A (TWO DOT PUNCTUATION), the character LTM uses to
/// namespace its synthetic variables. The internal helper fragments LTM
/// synthesizes are double-namespaced (`$⁚$⁚ltm⁚…`), so a `starts_with`
/// test on these prefixes selects only the user-facing score variables.
const LINK_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";
const LOOP_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}loop_score\u{205A}";

/// Number of saved steps at the start of a flow-to-stock link/loop score
/// series that the equation's startup guard pins to `0`: the second-order
/// stock difference in the score's denominator needs two steps of history
/// before it is defined. (Same constant relied on by `ltm_dt_invariance.rs`
/// and `arrayed_isolated_loop_raw_score_is_one_per_element`.)
const STARTUP_STEPS: usize = 2;

/// Compile `project` with LTM enabled (exhaustive mode), run it to
/// completion, and return the results alongside the LTM synthetic-variable
/// metadata (`model_ltm_variables(..).vars`). The metadata's `dimensions`
/// field is what distinguishes an A2A (per-element) score variable from a
/// scalar one -- needed to know how many contiguous slots each score
/// variable occupies in the results buffer.
fn run_ltm(project: &datamodel::Project) -> (Results, Vec<LtmSyntheticVar>) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    (vm.into_results(), ltm_vars)
}

/// The full timeseries of the value at `offset`, bounded to the
/// simulation's real `step_count`.
///
/// `Results::iter` already yields exactly `step_count` rows, so this never
/// reads into the trailing rows of the backing buffer. (The buffer can be
/// longer than `step_count * step_size`: the VM sizes it from the
/// `n_chunks` spec estimate, so `data.len() / step_size` would overcount.)
fn series_at(results: &Results, offset: usize) -> Vec<f64> {
    results.iter().map(|row| row[offset]).collect()
}

/// The results-buffer offset of the variable named exactly `name`.
fn offset_of(results: &Results, name: &str) -> usize {
    *results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == name)
        .map(|(_, off)| off)
        .unwrap_or_else(|| {
            panic!(
                "variable {name:?} not found in results; have: {:?}",
                results
                    .offsets
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

/// Sorted names of every *canonical* LTM link-score and loop-score
/// variable -- the user-facing `$⁚ltm⁚link_score⁚…` / `$⁚ltm⁚loop_score⁚…`
/// forms, excluding the internal `$⁚$⁚ltm⁚…⁚arg0` helper fragments (whose
/// count differs between an arrayed model and its scalar equivalent and
/// which carry no user-facing contract).
fn ltm_score_var_names(results: &Results) -> Vec<String> {
    let mut names: Vec<String> = results
        .offsets
        .keys()
        .map(|k| k.as_str().to_string())
        .filter(|s| s.starts_with(LINK_SCORE_PREFIX) || s.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    names.sort();
    names
}

/// Number of contiguous element slots a (possibly A2A) LTM synthetic
/// variable occupies in the results buffer: the product of its
/// dimensions' sizes, or `1` when it is scalar.
///
/// A2A LTM variables are stored as a single base-offset entry in
/// `Results::offsets`, with their per-element slots laid out contiguously
/// starting at that offset.
fn slot_count(var: &LtmSyntheticVar, dims: &[Dimension]) -> usize {
    var.dimensions
        .iter()
        .map(|dim_name| {
            dims.iter()
                .find(|d| d.name() == dim_name.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "LTM synthetic var {:?} is dimensioned over unknown dimension \
                         {dim_name:?}",
                        var.name
                    )
                })
                .len()
        })
        .product()
}

/// Locate `name` in the LTM synthetic-variable metadata.
fn ltm_var<'a>(ltm_vars: &'a [LtmSyntheticVar], name: &str) -> &'a LtmSyntheticVar {
    ltm_vars.iter().find(|v| v.name == name).unwrap_or_else(|| {
        panic!(
            "LTM score variable {name:?} is present in the results but not in \
                 model_ltm_variables(); synthetic vars: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        )
    })
}

/// Whether `project` compiles cleanly via the incremental salsa pipeline.
fn compiles(project: &datamodel::Project) -> bool {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_project_incremental(&db, sync.project, "main").is_ok()
}

/// Arrayed-vs-scalar link-score parity (LTM review, Finding 2's per-link
/// guard).
///
/// An arrayed model whose every element is structurally identical to a
/// scalar reference model must produce per-element LTM link scores *and*
/// loop scores that exactly reproduce the scalar model's values. The
/// fixture is a one-stock reinforcing loop -- `pop -> growth -> pop` with
/// `growth = pop * 0.1` -- as a scalar model and as a two-region arrayed
/// model with identical per-element dynamics.
///
/// This pins parity at *link-score* granularity, which the isolated-loop
/// `+/-1` invariant does not: the loop score is the product of its link
/// scores, so two compensating per-element link-score errors can still
/// multiply to a correct loop score. Finding 2 (bare-arrayed nested
/// `PREVIOUS` stubbing to `0`) was such a single, uncompensated per-link
/// error.
#[test]
fn arrayed_isolated_loop_link_scores_match_scalar() {
    // Two-region arrayed model: a per-element reinforcing one-stock loop
    // `pop[region] -> growth[region] -> pop[region]`, every element
    // identical (`growth = pop * 0.1`).
    let arrayed = TestProject::new("arrayed_parity")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .build_datamodel();
    // The single-element scalar equivalent -- the reference values.
    let scalar = TestProject::new("scalar_parity")
        .with_sim_time(0.0, 6.0, 1.0)
        .stock("pop", "100", &["growth"], &[], None)
        .flow("growth", "pop * 0.1", None)
        .build_datamodel();

    let (arr_results, arr_vars) = run_ltm(&arrayed);
    let (scl_results, _) = run_ltm(&scalar);

    // Both models describe the same loop topology, so they must emit the
    // same set of canonical LTM score variables.
    let scl_names = ltm_score_var_names(&scl_results);
    let arr_names = ltm_score_var_names(&arr_results);
    assert!(
        !scl_names.is_empty(),
        "the scalar reference model must produce at least one LTM score variable"
    );
    assert_eq!(
        arr_names, scl_names,
        "the arrayed and scalar models describe the same loop topology and so must produce \
         the same set of canonical LTM score variables"
    );

    // For every canonical LTM score variable, each of the arrayed model's
    // per-element slots must exactly reproduce the scalar model's value at
    // every timestep. Each element of the arrayed model is structurally
    // identical to the scalar model, so a per-element score that deviates
    // from the scalar reference is a bug in the array extension.
    //
    // The per-slot reads below require every score variable in this
    // fully-arrayed model (arrayed source AND arrayed target over the same
    // dimension) to be A2A. Asserting that makes the reads well-defined
    // and turns a regression that scalarizes a score variable into a loud
    // failure here rather than a silent read of an adjacent variable's
    // slot.
    const PARITY_TOL: f64 = 1e-9;
    for name in &scl_names {
        let scalar_series = series_at(&scl_results, offset_of(&scl_results, name));

        let var = ltm_var(&arr_vars, name);
        assert!(
            !var.dimensions.is_empty(),
            "LTM score variable {name:?} must be A2A (dimensioned) in the fully-arrayed \
             model, but model_ltm_variables() reports it as scalar"
        );
        let n_slots = slot_count(var, &arrayed.dimensions);
        let arr_base = offset_of(&arr_results, name);

        for slot in 0..n_slots {
            let arr_series = series_at(&arr_results, arr_base + slot);
            assert_eq!(
                arr_series.len(),
                scalar_series.len(),
                "{name}: arrayed and scalar score series differ in length"
            );
            for (step, (&arr_v, &scl_v)) in arr_series.iter().zip(scalar_series.iter()).enumerate()
            {
                assert!(
                    (arr_v - scl_v).abs() <= PARITY_TOL * scl_v.abs().max(1.0),
                    "{name} slot {slot} at step {step}: arrayed score {arr_v} != scalar \
                     score {scl_v}. Each element of the arrayed model is structurally \
                     identical to the scalar model; a per-element LTM score that deviates \
                     from the scalar reference means the array extension is mis-evaluating \
                     that link (LTM review Finding 2 was exactly such a per-link error)."
                );
            }
        }
    }
}

/// A minimum magnitude for a loop score at a discoverable step to count
/// as a real, non-degenerate score rather than a silent stub. The genuine
/// agg-routed loop scores in this fixture settle at ~0.25-0.5;
/// floating-point noise around a true zero is ~1e-13 and a
/// silently-stubbed score is exactly `0.0`, so `1e-6` cleanly separates
/// the two.
const MEANINGFUL_SCORE: f64 = 1e-6;

/// Aggregate-node loop-score sanity for a whole-extent reducer.
///
/// A feedback loop that runs *through* a whole-extent `SUM(pop[*])`
/// reducer must be genuinely scored -- not merely finite, but carrying a
/// real, non-zero loop score at every discoverable timestep.
/// `SUM(pop[*])` is a *sub-expression* of `growth`'s equation (not the
/// whole right-hand side), so LTM hoists it into a synthetic
/// `$⁚ltm⁚agg⁚0` aggregate node, and the only path from `pop` back to
/// `pop` is the cross-element loop
/// `pop[r] -> $⁚ltm⁚agg⁚0 -> growth[r'] -> pop[r']` through that node.
///
/// `growth` reads `pop` *only* through the reducer -- there is no bare
/// `pop[region]` factor -- so **every** feedback loop in the model is
/// agg-routed. That is what makes the non-degeneracy assertion bite: a
/// model with an additional direct per-element loop could keep a non-zero
/// loop score even if every agg-routed loop collapsed. The agg-half link
/// scores (`source -> agg`, `agg -> target`) are a known silent-stubbing
/// risk: if they failed to compile they would be stubbed to a constant
/// `0`, leaving every agg-routed loop score identically `0` -- finite,
/// but degenerate. Asserting a *sustained, non-zero* score (not just
/// `is_finite()`) is what catches that.
#[test]
fn whole_extent_sum_agg_loop_scores_are_finite_and_sustained() {
    let project = TestProject::new("whole_extent_agg")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        // Heterogeneous initial stock values, so SUM(pop[*]) is exercised
        // non-trivially and the two elements stay distinguishable.
        .array_with_ranges("pop0[region]", vec![("north", "100"), ("south", "300")])
        .array_stock("pop[region]", "pop0[region]", &["growth"], &[], None)
        // `growth` reads `pop` only through the whole-extent `SUM(pop[*])`
        // reducer (a sub-expression of the product, hence hoisted into a
        // synthetic agg) -- no bare `pop[region]` factor -- so every
        // feedback loop in the model routes through the synthetic agg.
        .array_flow("growth[region]", "SUM(pop[*]) * 0.01", None)
        .build_datamodel();

    let (results, ltm_vars) = run_ltm(&project);
    assert!(
        results.step_count > STARTUP_STEPS,
        "the fixture must simulate past the {STARTUP_STEPS}-step startup guard to have any \
         discoverable timesteps, got {} step(s)",
        results.step_count
    );

    // LTM must have hoisted the inlined `SUM(pop[*])` reducer into a
    // synthetic aggregate node.
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    assert!(
        ltm_vars.iter().any(|v| v.name == agg_name),
        "LTM must hoist the inlined SUM(pop[*]) reducer into a synthetic {agg_name} node; \
         synthetic vars: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // The synthetic agg's simulated value equals SUM(pop[*]) =
    // pop[north] + pop[south] at every timestep -- a wiring and
    // runlist-ordering sanity check (a stale read would surface as a
    // mismatch here).
    let agg = series_at(&results, offset_of(&results, agg_name));
    let pop_north = series_at(&results, offset_of(&results, "pop[north]"));
    let pop_south = series_at(&results, offset_of(&results, "pop[south]"));
    for step in 0..results.step_count {
        let expected = pop_north[step] + pop_south[step];
        assert!(
            (agg[step] - expected).abs() <= 1e-9 * expected.abs().max(1.0),
            "step {step}: {agg_name} = {}, expected SUM(pop[*]) = {expected}",
            agg[step]
        );
    }

    // Every loop score must be finite at every timestep; and -- the
    // assertion with teeth -- at least one *agg-routed* loop score must
    // carry a real, non-zero value at *every* discoverable step (index
    // >= STARTUP_STEPS, past the two-step startup guard). A genuine
    // feedback loop through the synthetic agg must produce a genuine,
    // sustained loop score; an agg-routed score that is identically `0`
    // past the startup guard means the agg-half link scores were silently
    // stubbed to a constant `0`.
    //
    // A loop score is "agg-routed" when its equation references an
    // agg-half link score -- detectable as the agg name appearing as a
    // substring of the loop-score equation. Every loop in this model
    // routes through the agg, so this set is the whole set; checking it
    // explicitly keeps the test honest if the fixture ever gains a direct
    // loop.
    let loop_score_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|s| s.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_score_names.is_empty(),
        "the agg-routed feedback model must produce at least one loop score"
    );

    let mut agg_routed_loops = 0usize;
    let mut saw_sustained_agg_loop = false;
    for name in &loop_score_names {
        let var = ltm_var(&ltm_vars, name);
        let routes_through_agg = var.equation.source_text().contains(agg_name);
        if routes_through_agg {
            agg_routed_loops += 1;
        }
        // A loop score is either scalar (cross-element / mixed loops) or
        // A2A over `region` (pure same-element loops); `slot_count` reads
        // the right width either way.
        let n_slots = slot_count(var, &project.dimensions);
        let base = offset_of(&results, name);
        for slot in 0..n_slots {
            let s = series_at(&results, base + slot);
            for (step, &v) in s.iter().enumerate() {
                assert!(
                    v.is_finite(),
                    "loop score {name} slot {slot} at step {step} is not finite: {v}"
                );
            }
            // Does this slot carry a real, non-zero score at *every*
            // discoverable step? (`step_count > STARTUP_STEPS` was
            // asserted above, so this `all` is never vacuously true.)
            let sustained = s
                .iter()
                .skip(STARTUP_STEPS)
                .all(|v| v.abs() > MEANINGFUL_SCORE);
            if routes_through_agg && sustained {
                saw_sustained_agg_loop = true;
            }
        }
    }
    assert!(
        agg_routed_loops > 0,
        "every feedback loop in this model routes through the synthetic agg, so at least one \
         loop score's equation must reference {agg_name}; found none among {loop_score_names:?}"
    );
    assert!(
        saw_sustained_agg_loop,
        "at least one agg-routed loop score must carry a real, non-zero value \
         (> {MEANINGFUL_SCORE}) at every discoverable step (index >= {STARTUP_STEPS}); an \
         agg-routed loop score that is all-zero past the startup guard means the agg-half \
         link scores were silently stubbed to a constant 0"
    );
}

/// Characterization test (a live tripwire, not a disabled test): pins the
/// current, broken behavior that a plain apply-to-all equation with a
/// *bare* (unsubscripted) arrayed name inside a *nested* `PREVIOUS` fails
/// to compile.
///
/// `p2bare[region] = PREVIOUS(PREVIOUS(pop))` -- with `pop` arrayed over
/// `region` -- is exactly the shape the LTM flow-to-stock link-score
/// generator emits. "Finding 2" of the LTM deep review was this bug
/// surfacing *through* the LTM generator: the synthetic fragment failed
/// to compile and was silently stubbed to `0`, collapsing the loop score.
/// Piece 2 fixed Finding 2 with a *generator-side workaround* -- the
/// generator now emits the subscripted form -- but the *underlying engine
/// limitation*, which a user still hits by writing the bare form
/// directly, is unfixed.
///
/// Root cause (GH #541): `make_temp_arg` in `builtins_visitor.rs`
/// synthesizes the inner-`PREVIOUS` helper as a *scalar* aux
/// (`datamodel::Equation::Scalar(...)`), so a bare arrayed name lands
/// ill-typed inside a scalar equation and the helper fragment fails to
/// compile.
///
/// FIXME(GH #541): when the engine handles a bare arrayed name in a
/// nested `PREVIOUS` (e.g. `make_temp_arg` synthesizes a correctly-shaped
/// helper), the `p2bare` model below will compile. This test then begins
/// to fail -- flip it into a positive regression test: compile and
/// simulate the model, then assert `p2bare[north]` / `p2bare[south]`
/// equal `PREVIOUS(PREVIOUS(pop[region]))` per element (the exact values
/// `subscripted_arrayed_nested_previous_matches_scalar` already pins for
/// the subscripted form).
#[test]
fn bare_arrayed_nested_previous_fails_to_compile() {
    // The failing case: a *nested* PREVIOUS over a *bare* arrayed name.
    let nested_bare = TestProject::new("nested_bare_previous")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .array_aux_direct(
            "p2bare",
            vec!["region".to_string()],
            "PREVIOUS(PREVIOUS(pop))",
            None,
        )
        .build_datamodel();
    assert!(
        !compiles(&nested_bare),
        "GH #541 appears to be FIXED: `p2bare[region] = PREVIOUS(PREVIOUS(pop))` (a bare \
         arrayed name inside a nested PREVIOUS) now compiles. Flip this characterization \
         test into a positive regression test -- compile, simulate, and assert p2bare's \
         per-element values match PREVIOUS(PREVIOUS(pop[region])) -- and update GH #541."
    );

    // The limitation is specifically the *nesting* of the bare name: a
    // *single* (non-nested) bare arrayed `PREVIOUS(pop)` compiles fine, so
    // GH #541 is not a blanket "bare arrayed name in PREVIOUS" failure.
    let single_bare = TestProject::new("single_bare_previous")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .array_aux_direct("p1bare", vec!["region".to_string()], "PREVIOUS(pop)", None)
        .build_datamodel();
    assert!(
        compiles(&single_bare),
        "a single (non-nested) bare arrayed PREVIOUS (`p1bare[region] = PREVIOUS(pop)`) \
         must still compile -- the GH #541 limitation pinned here is specifically the \
         *nesting* of a bare arrayed name, not bare arrayed names in PREVIOUS in general"
    );
}

/// The positive companion to `bare_arrayed_nested_previous_fails_to_compile`:
/// the properly *subscripted* form of a nested `PREVIOUS` over an arrayed
/// variable compiles and is correct per element.
///
/// `p1[region] = PREVIOUS(pop[region])` and
/// `p2[region] = PREVIOUS(PREVIOUS(pop[region]))` -- the subscripted
/// equivalents of the bare forms Piece 2's generator-side workaround now
/// emits -- must produce, per element, exactly the values their scalar
/// equivalents (`sp1 = PREVIOUS(scalar_pop)`,
/// `sp2 = PREVIOUS(PREVIOUS(scalar_pop))`) produce. `pop[region]` and
/// `scalar_pop` share identical dynamics, so each arrayed slot must track
/// the scalar series step for step.
///
/// This is a plain (non-LTM) simulation: the limitation and its
/// generator-side workaround both live in the equation-compilation layer,
/// below LTM.
#[test]
fn subscripted_arrayed_nested_previous_matches_scalar() {
    let project = TestProject::new("subscripted_nested_previous")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .array_aux_direct(
            "p1",
            vec!["region".to_string()],
            "PREVIOUS(pop[region])",
            None,
        )
        .array_aux_direct(
            "p2",
            vec!["region".to_string()],
            "PREVIOUS(PREVIOUS(pop[region]))",
            None,
        )
        .stock("scalar_pop", "100", &["sgrowth"], &[], None)
        .flow("sgrowth", "scalar_pop * 0.1", None)
        .scalar_aux("sp1", "PREVIOUS(scalar_pop)")
        .scalar_aux("sp2", "PREVIOUS(PREVIOUS(scalar_pop))")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the subscripted nested-PREVIOUS model must compile");
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("plain simulation should run to completion");
    let results = vm.into_results();

    // Each arrayed slot of the subscripted PREVIOUS chains must equal its
    // scalar equivalent step for step: `pop[region]` and `scalar_pop` have
    // identical dynamics, so a correct per-element nested PREVIOUS tracks
    // the scalar reference exactly. A deviation means the arrayed nested
    // PREVIOUS is mis-evaluated (e.g. broadcasting one slot, or routing
    // through a mis-shaped helper).
    const PARITY_TOL: f64 = 1e-9;
    for (arrayed, scalar) in [("p1", "sp1"), ("p2", "sp2")] {
        let scalar_series = series_at(&results, offset_of(&results, scalar));
        for region in ["north", "south"] {
            let arrayed_series = series_at(
                &results,
                offset_of(&results, &format!("{arrayed}[{region}]")),
            );
            assert_eq!(
                arrayed_series.len(),
                scalar_series.len(),
                "{arrayed}[{region}] and {scalar} series differ in length"
            );
            for (step, (&a, &s)) in arrayed_series.iter().zip(scalar_series.iter()).enumerate() {
                assert!(
                    (a - s).abs() <= PARITY_TOL * s.abs().max(1.0),
                    "{arrayed}[{region}] at step {step} = {a}, but scalar {scalar} = {s}; a \
                     subscripted nested PREVIOUS over an arrayed variable must evaluate \
                     per-element, matching the scalar equivalent"
                );
            }
        }
    }
}
