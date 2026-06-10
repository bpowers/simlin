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
//! ## Bare-arrayed nested `PREVIOUS` (GH #541, now fixed)
//!
//! `bare_arrayed_nested_previous_matches_subscripted` is a positive
//! regression test for GH #541: a plain apply-to-all equation with a
//! *bare* (unsubscripted) arrayed name inside a *nested* `PREVIOUS` --
//! `p2bare[region] = PREVIOUS(PREVIOUS(pop))` -- now compiles and
//! simulates, producing the same per-element values as the explicitly
//! subscripted form. `builtins_visitor`'s `make_temp_arg` synthesizes an
//! *arrayed* (`Equation::ApplyToAll`) helper aux over the active A2A
//! dimensions for the inner `PREVIOUS(pop)` and references it
//! `helper[<element>]`, so the bare arrayed name keeps its array shape
//! instead of landing ill-typed in a scalar helper. This was exactly the
//! shape the LTM flow-to-stock link-score generator emits; "Finding 2" of
//! the review was this bug surfacing *through* the generator, and Piece 2
//! worked around it generator-side, but the underlying engine limitation
//! is now fixed at the source. `subscripted_arrayed_nested_previous_matches_scalar`
//! is the companion that pins the explicitly subscripted form.

use simlin_engine::datamodel::{self, Dimension};
use simlin_engine::db::{
    DetectedLoopPolarity, DiagnosticError, DiagnosticSeverity, LtmSyntheticVar, SimlinDb,
    collect_all_diagnostics, compile_project_incremental, model_detected_loops,
    model_ltm_variables, reclassify_loops_from_results, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::open_vensim;
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

/// GH #653 (pin-dims.AC5.1): the enumerator's A2A-collapse on a
/// *per-element-equation* model -- `Equation::Arrayed` flows whose slot
/// equations reference the stock by literal element subscripts, the shape
/// the MDL importer produces -- must score EVERY element slot correctly,
/// not just the lexicographically-first one.
///
/// On this model the only emitted link scores for the `population -> births`
/// / `population -> deaths` edges are the per-element FixedIndex forms
/// (`population[nyc]→births`, `population[boston]→births`, ...), each an
/// arrayed variable whose only meaningful slot is its own element. Before
/// the slot-aware loop-score generation, the A2A-collapsed loop score was an
/// `ApplyToAll` equation referencing one arbitrary (lex-first:
/// `[boston]`) FixedIndex variable for every slot -- so the non-lex-first
/// slot (NYC, slot 0) read a frozen ceteris-paribus partial and scored 0.
///
/// The fixture couples two loops (births + deaths) on one stock with
/// heterogeneous per-element rates, so each loop's raw score is
/// rate-dependent and analytically known: for `births = pop*b`,
/// `deaths = pop*d` the birth loop scores `b/(b-d)` and the death loop
/// `-d/(b-d)` at every post-startup step.
#[test]
fn per_element_equation_a2a_loop_scores_correct_for_every_slot() {
    let project = TestProject::new("per_elem_a2a")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &["deaths"], None)
        .array_flow_with_ranges(
            "births[Region]",
            vec![
                ("NYC", "population[NYC] * 0.10"),
                ("Boston", "population[Boston] * 0.40"),
            ],
        )
        .array_flow_with_ranges(
            "deaths[Region]",
            vec![
                ("NYC", "population[NYC] * 0.03"),
                ("Boston", "population[Boston] * 0.05"),
            ],
        )
        .build_datamodel();

    let (results, ltm_vars) = run_ltm(&project);

    // Two coupled loops on the `population` stock -> a reinforcing birth
    // loop (r1) and a balancing death loop (b1), each A2A over Region.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "the births and deaths loops should each produce one loop_score variable; got {loop_names:?}"
    );

    // Expected per-slot raw loop scores (slot 0 = NYC, slot 1 = Boston,
    // following the dimension's declared element order).
    //   birth loop:  b / (b - d)  ->  NYC 0.10/0.07,  Boston 0.40/0.35
    //   death loop: -d / (b - d)  ->  NYC -0.03/0.07, Boston -0.05/0.35
    let expected: &[(&str, [f64; 2])] = &[
        ("r1", [0.10 / 0.07, 0.40 / 0.35]),
        ("b1", [-0.03 / 0.07, -0.05 / 0.35]),
    ];

    const TOL: f64 = 1e-9;
    for (loop_id, expected_slots) in expected {
        let name = format!("{LOOP_SCORE_PREFIX}{loop_id}");
        let var = ltm_var(&ltm_vars, &name);
        assert_eq!(
            var.dimensions,
            vec!["Region".to_string()],
            "loop score {name} must be dimensioned over Region"
        );
        let base = offset_of(&results, &name);
        for (slot, &expected_score) in expected_slots.iter().enumerate() {
            let series = series_at(&results, base + slot);
            // Skip the startup steps (flow-to-stock scores need two steps
            // of history) plus one for the PREVIOUS-shifted timing.
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    (v - expected_score).abs() <= TOL * expected_score.abs().max(1.0),
                    "{name} slot {slot} at step {step}: got {v}, expected {expected_score}. \
                     A wrong (or zero) value here means the A2A-collapsed loop score is not \
                     referencing this element's own per-element link scores (GH #653)."
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

/// GH #752, end-to-end: a feedback loop through a VARIABLE-BACKED partial
/// reducer -- `inflow[D1] = SUM(matrix[D1,*])` is the WHOLE RHS, so `inflow`
/// itself is the aggregate node (`is_synthetic == false`, no `$⁚ltm⁚agg⁚{n}`
/// minted) -- must compile every loop-score fragment and produce finite,
/// sustained non-zero scores.
///
/// Before the fix the element graph kept the conservative cross-product for
/// the variable-backed reducer reference (phantom `matrix[a,x] → inflow[b]`
/// edges), so the loop builder emitted phantom cross-element loops whose
/// scores referenced `"matrix[a,x]→inflow"[b]` / the bare A2A
/// `"matrix→inflow"` -- names `try_cross_dimensional_link_scores` never
/// emits (only the per-`(row, slot)` scalars `matrix[a,x]→inflow[a]` exist).
/// Every loop score through the reducer failed fragment compile (Assembly
/// Warnings) and was silently stubbed to a constant 0.
#[test]
fn variable_backed_partial_reduce_loop_scores_finite_and_sustained() {
    let project = TestProject::new("vb_partial_reduce_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux("matrix[D1,D2]", "stock[D1] * 0.1")
        // Heterogeneous initial stock values so the two D1 rows stay
        // distinguishable and the per-row reducer link scores are
        // non-degenerate.
        .array_with_ranges("stock0[D1]", vec![("a", "10"), ("b", "30")])
        .array_stock("stock[D1]", "stock0[D1]", &["inflow"], &[], None)
        // The WHOLE RHS is the partial reduce: `inflow` is the
        // variable-backed agg (result_dims = [D1]).
        .array_flow("inflow[D1]", "SUM(matrix[D1,*])", None)
        .build_datamodel();

    // Zero fragment-failure warnings: every LTM synthetic fragment for this
    // model must compile. (This was the GH #752 / #547 positive-failure
    // fixture before the fix.)
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let diags = collect_all_diagnostics(&db, sync.project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .collect();
    assert!(
        frag_failures.is_empty(),
        "a feedback loop through a variable-backed partial reducer must compile every \
         LTM fragment; got: {frag_failures:?}"
    );

    let (results, ltm_vars) = run_ltm(&project);
    assert!(
        results.step_count > STARTUP_STEPS,
        "the fixture must simulate past the {STARTUP_STEPS}-step startup guard, got {} step(s)",
        results.step_count
    );

    // No synthetic agg: the variable is the agg.
    assert!(
        !ltm_vars
            .iter()
            .any(|v| v.name.contains("\u{205A}agg\u{205A}")),
        "a whole-RHS reducer must not mint a synthetic agg; synthetic vars: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // The per-(row, slot) reducer link scores are the only matrix→inflow
    // names; the loops must reference them.
    let partial_reduce_names: Vec<String> = ltm_vars
        .iter()
        .map(|v| v.name.clone())
        .filter(|n| {
            n.starts_with(&format!("{LINK_SCORE_PREFIX}matrix[")) && n.contains("\u{2192}inflow[")
        })
        .collect();
    assert_eq!(
        partial_reduce_names.len(),
        4,
        "expected one per-(row, slot) link score per matrix element; got: {partial_reduce_names:?}"
    );

    let loop_score_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|s| s.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    // Exactly the four real per-(d1, d2) loops -- one scalar loop per matrix
    // element (`stock[d1] → matrix[d1,d2] → inflow[d1] → stock[d1]`).
    // Pre-fix this model had FIVE broken loops (four phantom cross-element
    // 6-cycles over the off-diagonal cross-product edges plus one
    // uncompilable A2A loop); a count drift here means phantom loops were
    // reintroduced (or real ones dropped), independent of whether their
    // scores happen to compile.
    assert_eq!(
        loop_score_names.len(),
        4,
        "expected exactly one loop per (d1, d2) matrix element; got: {loop_score_names:?}"
    );

    // Every loop score routes through the reducer (every feedback path runs
    // matrix → inflow), so every one must reference a per-(row, slot) link
    // score, be finite everywhere, and be sustained non-zero past the
    // startup guard.
    for name in &loop_score_names {
        let var = ltm_var(&ltm_vars, name);
        let eq = var.equation.source_text();
        assert!(
            partial_reduce_names.iter().any(|n| eq.contains(n.as_str())),
            "loop score {name} must reference a per-(row, slot) reducer link score; eq: {eq}"
        );
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
            let sustained = s
                .iter()
                .skip(STARTUP_STEPS)
                .all(|v| v.abs() > MEANINGFUL_SCORE);
            assert!(
                sustained,
                "loop score {name} slot {slot} must be sustained non-zero past the startup \
                 guard (the reinforcing loop is active every step); got: {s:?}"
            );
        }
    }
}

/// GH #533 + GH #737, end-to-end: a feedback loop whose only path back to a
/// *scalar* stock runs through a *scalar* feeder of a hoisted reducer *with a
/// scalar target*. The fixture is a scalar stock `total` grown by
/// `grow = 1 + SUM(pop[*] * scale)`, with `scale = 0.001 * total + 0.01`
/// feeding back from `total`. `SUM(pop[*] * scale)` is a sub-expression, so it
/// is hoisted into a synthetic `$⁚ltm⁚agg⁚0`.
///
/// #533's well-specified fix is the ELEMENT GRAPH: the `(scale, grow)` causal
/// edge is classified `ThroughAgg`, but `scale`/`grow` are both scalar, so
/// before the fix `model_element_causal_edges`'s both-scalar fast path emitted
/// a direct `scale → grow` element edge instead of `scale → $⁚ltm⁚agg⁚0`. That
/// element-graph fix is pinned directly by
/// `element_graph_tests::element_graph_scalar_feeder_*`.
///
/// The two follow-on gaps this scalar-target scenario originally exposed are
/// both fixed now:
///
/// 1. (FIXED, GH #738) The synthetic agg `$⁚ltm⁚agg⁚0` for
///    `SUM(pop[*] * scale)` (arrayed `pop` times scalar `scale`, reduced to a
///    scalar target) used to FAIL fragment compilation and was stubbed to a
///    constant `0` with an `Assembly` Warning: `compile_ltm_equation_fragment`
///    lowered the equation with an empty `ScopeStage0.models`, so the
///    `pop[*] * scale` Op2 never got its Expr2 `ArrayBounds` and Pass-1 temp
///    decomposition never hoisted it out of the reducer. The agg now compiles
///    and tracks the inlined reducer's value -- asserted below, and pinned in
///    detail by `scalar_target_agg_value_matches_inlined_reducer`.
///
/// 2. (FIXED, GH #737) The loop-score *builder* now routes the loop through
///    the agg: `classify_cycle` sends a cycle that traverses a
///    `ThroughAgg`-routed edge down the element-level slow path (it is no
///    longer `PureScalar`), where the post-#533 element graph's
///    `scale → $⁚ltm⁚agg⁚0 → grow` hops are traversed, so the loop score
///    composes the two agg-half link scores instead of the direct
///    `scale → grow` link. That matters because the DIRECT `scale→grow` link
///    score is still uncompilable (its ceteris-paribus partial contains
///    `PREVIOUS(pop[*])`, the GH #541-class wildcard-subscripted-arg capture,
///    tracked separately) and would be silently stubbed to 0, zeroing the
///    loop score. The agg halves avoid the lagged-wildcard shape entirely:
///    `scale → agg` freezes only the scalar feeder (`PREVIOUS(scale)`), and
///    `agg → grow` substitutes the reducer with the agg name.
#[test]
fn scalar_feeder_scalar_target_loop_compiles_and_is_well_formed() {
    let project = TestProject::new("scalar_feeder_agg_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        // Heterogeneous initial stock values so SUM(pop[*]) is exercised
        // non-trivially.
        .array_with_ranges("pop0[region]", vec![("north", "100"), ("south", "300")])
        .array_stock("pop[region]", "pop0[region]", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        // The scalar feedback variable: depends on the scalar stock `total`.
        // The `+ 0.01` keeps it strictly positive.
        .scalar_aux("scale", "0.001 * total + 0.01")
        // The scalar stock and its flow. `SUM(pop[*] * scale)` is a
        // sub-expression (the `1 +` keeps it from being a whole-RHS,
        // variable-backed agg), so it is hoisted into a synthetic agg, and
        // the `(scale, grow)` edge routes ThroughAgg.
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale)", None)
        .build_datamodel();

    // The model must compile and simulate with LTM enabled -- the baseline
    // well-formedness the #533 element-graph fix preserves (a bad element edge
    // never crashes compilation, but enumerating a loop that routes through a
    // newly-visible agg node must not break LTM assembly either).
    let (results, ltm_vars) = run_ltm(&project);
    assert!(
        results.step_count > STARTUP_STEPS,
        "the fixture must simulate past the {STARTUP_STEPS}-step startup guard, got {} step(s)",
        results.step_count
    );

    // LTM must have hoisted the inlined `SUM(pop[*] * scale)` reducer into a
    // synthetic aggregate node (the precondition for the ThroughAgg routing
    // #533 fixes at the element-graph level).
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    assert!(
        ltm_vars.iter().any(|v| v.name == agg_name),
        "LTM must hoist the inlined SUM(pop[*] * scale) reducer into a synthetic {agg_name} \
         node; synthetic vars: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // GH #738 (gap #1, fixed): the agg's own fragment compiles, so its series
    // tracks the inlined reducer -- `SUM(pop[*] * scale)` -- instead of the
    // constant 0 the silently-stubbed fragment used to read. `pop` starts at
    // 100/300 and `scale` is strictly positive, so the value is non-zero from
    // step 0.
    {
        let agg_series = series_at(&results, offset_of(&results, agg_name));
        let pop_n = series_at(&results, offset_of(&results, "pop[north]"));
        let pop_s = series_at(&results, offset_of(&results, "pop[south]"));
        let scale = series_at(&results, offset_of(&results, "scale"));
        for (step, &agg) in agg_series.iter().enumerate() {
            let expected = (pop_n[step] + pop_s[step]) * scale[step];
            assert!(
                expected.abs() > 0.0,
                "fixture defect: SUM(pop[*] * scale) must be non-zero at step {step}"
            );
            assert!(
                (agg - expected).abs() <= 1e-9 * expected.abs(),
                "step {step}: {agg_name} = {agg}, expected SUM(pop[*] * scale) = {expected}"
            );
        }
    }

    // GH #737 (gap #2, fixed): the scalar feedback loop
    // `total → scale → $⁚ltm⁚agg⁚0 → grow → total` is enumerated as exactly
    // one loop whose score composes the two agg-half link scores -- the
    // equation references the agg name on both sides -- and does NOT
    // reference the (uncompilable, stubbed-to-0) direct `scale→grow` form.
    let agg_loops: Vec<&LtmSyntheticVar> = ltm_vars
        .iter()
        .filter(|v| {
            v.name.starts_with(LOOP_SCORE_PREFIX) && v.equation.source_text().contains(agg_name)
        })
        .collect();
    assert_eq!(
        agg_loops.len(),
        1,
        "the scalar feedback loop must be enumerated as exactly one loop scored through the \
         synthetic agg; loop vars: {:?}",
        ltm_vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| (v.name.as_str(), v.equation.source_text()))
            .collect::<Vec<_>>()
    );
    let agg_loop = agg_loops[0];
    let eq = agg_loop.equation.source_text();
    assert!(
        eq.contains(&format!("scale\u{2192}{agg_name}"))
            && eq.contains(&format!("{agg_name}\u{2192}grow")),
        "the loop score must compose the two agg-half link scores \
         (scale→{agg_name} and {agg_name}→grow); got: {eq}"
    );
    assert!(
        !eq.contains("scale\u{2192}grow"),
        "the loop score must NOT reference the direct scale→grow link score \
         (uncompilable, silently stubbed to 0); got: {eq}"
    );
    // The hop polarities are recoverable (SUM is monotone in the feeder's
    // direction here, and `grow = 1 + agg` is a positive consumer), so the
    // loop must classify concretely as reinforcing -- an `r{n}` id, not the
    // degraded `u{n}` Undetermined fallback.
    assert!(
        agg_loop
            .name
            .strip_prefix(LOOP_SCORE_PREFIX)
            .is_some_and(|id| id.starts_with('r')),
        "the agg-routed loop must classify concretely (reinforcing, r-prefixed id), \
         not degrade to Undetermined; got: {}",
        agg_loop.name
    );
    // And -- the assertion with teeth -- the loop score must carry a real,
    // sustained non-zero value at every discoverable step (mirrors
    // `whole_extent_sum_agg_loop_scores_are_finite_and_sustained`): the agg
    // halves compile, so the score is no longer silently 0.
    {
        let base = offset_of(&results, &agg_loop.name);
        let s = series_at(&results, base);
        assert!(
            s.iter()
                .skip(STARTUP_STEPS)
                .all(|v| v.is_finite() && v.abs() > MEANINGFUL_SCORE),
            "the agg-routed loop score must be sustained non-zero (> {MEANINGFUL_SCORE}) past \
             the {STARTUP_STEPS}-step startup guard; series: {s:?}"
        );
    }

    // Every LTM score variable must be finite at every timestep -- no NaN/Inf
    // leaks from the agg wiring or the link scores. This is the
    // well-formedness guarantee the #533 element-graph fix preserves.
    for v in ltm_vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX) || v.name.starts_with(LINK_SCORE_PREFIX))
    {
        let n_slots = slot_count(v, &project.dimensions);
        let base = offset_of(&results, &v.name);
        for slot in 0..n_slots {
            for (step, &val) in series_at(&results, base + slot).iter().enumerate() {
                assert!(
                    val.is_finite(),
                    "LTM score {} slot {slot} at step {step} is not finite: {val}",
                    v.name
                );
            }
        }
    }
}

/// GH #738, focused regression: the synthetic agg hoisted for an inlined
/// reducer over an array *expression* with a *scalar* target --
/// `grow = 1 + SUM(pop[*] * scale)` -- compiles and its runtime series
/// equals the inlined reducer's value at every step (non-zero, since `pop`
/// starts at 100/300 and `scale` is strictly positive).
///
/// Root cause of the prior failure: `compile_ltm_equation_fragment` lowered
/// the agg's equation with an empty `ScopeStage0.models`, so `pop`'s
/// dimensions could not be resolved during Expr2 lowering, the
/// `pop[*] * scale` Op2 carried no `ArrayBounds`, and Pass-1 temp
/// decomposition (which gates on those bounds) never hoisted the array
/// expression out of the reducer -- codegen then rejected the fragment and
/// the agg silently read a constant 0. The fix threads the equation's
/// dependencies into the lowering scope, mirroring `lower_var_fragment`.
#[test]
fn scalar_target_agg_value_matches_inlined_reducer() {
    let project = TestProject::new("scalar_target_agg_value")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        // Heterogeneous initial stock values so SUM(pop[*] * scale) is
        // exercised non-trivially.
        .array_with_ranges("pop0[region]", vec![("north", "100"), ("south", "300")])
        .array_stock("pop[region]", "pop0[region]", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .scalar_aux("scale", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale)", None)
        .build_datamodel();

    let (results, ltm_vars) = run_ltm(&project);
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg = ltm_var(&ltm_vars, agg_name);
    assert!(
        agg.dimensions.is_empty(),
        "the whole-extent reducer's agg must be scalar; got dims {:?}",
        agg.dimensions
    );

    let agg_series = series_at(&results, offset_of(&results, agg_name));
    let pop_n = series_at(&results, offset_of(&results, "pop[north]"));
    let pop_s = series_at(&results, offset_of(&results, "pop[south]"));
    let scale = series_at(&results, offset_of(&results, "scale"));
    assert!(results.step_count > STARTUP_STEPS);
    for (step, &agg_val) in agg_series.iter().enumerate() {
        let expected = (pop_n[step] + pop_s[step]) * scale[step];
        assert!(
            expected.abs() > 0.0,
            "fixture defect: SUM(pop[*] * scale) must be non-zero at step {step}"
        );
        assert!(
            (agg_val - expected).abs() <= 1e-9 * expected.abs(),
            "step {step}: {agg_name} = {agg_val}, expected SUM(pop[*] * scale) = {expected}"
        );
    }
}

/// GH #738 round 2: the syntactic gate that decides whether an LTM
/// fragment's lowering needs the dependency-aware scope must cover EVERY
/// Pass-1 temp-decomposition site, not just the agg-hoistable reducer set.
/// `SIZE` is the demonstrated divergence: it is never hoisted into a
/// `$⁚ltm⁚agg⁚{n}` (its link score is constant 0), but Pass-1 decomposes
/// its argument exactly like `SUM`'s, so a fragment whose equation embeds
/// `SIZE(<array expression>)` still needs Expr2 bounds.
///
/// The reachable shape: `grow = scale * SIZE(pop[*] * 2)`. The
/// ceteris-paribus partial for `scale→grow` wraps the non-live reducer
/// subtree atomically as `PREVIOUS(SIZE(pop[*] * 2), 0)`; LTM parsing
/// captures the argument into a scalar helper aux whose equation is
/// exactly `size(pop[*] * 2)` (well-typed -- SIZE of an array is a
/// scalar -- so the GH #541 scalar-helper limitation does NOT mask it).
/// With the gate keyed on the wrong (agg-hoistable) builtin set, the
/// helper skipped the scoped re-lower, lost its bounds, failed codegen,
/// and was stubbed to constant 0 -- doubly silently, since failed
/// implicit-HELPER fragments get no `model_ltm_fragment_diagnostics`
/// Warning (that pass covers only `model_ltm_variables().vars`; the
/// assemble-time drop of helper fragments is tracked separately).
///
/// Pins the runtime values: the helper series is the element count (2.0)
/// at every step, and the `scale→grow` link score is 0 at step 0 (the
/// TIME = INITIAL_TIME guard) and exactly 1 thereafter (`scale` is the
/// only driver of `grow` and is strictly increasing).
#[test]
fn size_reducer_previous_helper_compiles_and_is_correct() {
    let project = TestProject::new("size_helper")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .scalar_aux("scale", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "scale * SIZE(pop[*] * 2)", None)
        .build_datamodel();

    let (results, _ltm_vars) = run_ltm(&project);
    assert!(results.step_count > STARTUP_STEPS);

    // The PREVIOUS-capture helper for the scale→grow partial holds
    // `size(pop[*] * 2)` == the region element count.
    let helper_offsets: Vec<(String, usize)> = results
        .offsets
        .iter()
        .filter(|(k, _)| k.as_str().contains("scale\u{2192}grow") && k.as_str().contains("arg0"))
        .map(|(k, &o)| (k.as_str().to_string(), o))
        .collect();
    assert!(
        !helper_offsets.is_empty(),
        "expected a PREVIOUS-capture helper for the scale→grow partial; offsets: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
    );
    for (name, off) in &helper_offsets {
        for (step, &v) in series_at(&results, *off).iter().enumerate() {
            assert!(
                (v - 2.0).abs() < 1e-12,
                "step {step}: helper {name} = {v}, expected SIZE(pop[*] * 2) = 2.0 \
                 (a 0 here means the helper fragment was silently stubbed)"
            );
        }
    }

    // grow = scale * 2 exactly, so the scale→grow ceteris-paribus link
    // score is 1 at every step past the initial-time guard.
    let ls = series_at(
        &results,
        offset_of(
            &results,
            "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}grow",
        ),
    );
    assert_eq!(ls[0], 0.0, "step 0 is pinned to 0 by the TIME guard");
    for (step, &v) in ls.iter().enumerate().skip(1) {
        assert!(
            (v - 1.0).abs() < 1e-9,
            "step {step}: scale\u{2192}grow link score = {v}, expected exactly 1 \
             (scale is grow's only driver)"
        );
    }
}

/// Positive regression test for GH #541 (flipped from the prior
/// characterization tripwire): a plain apply-to-all equation with a *bare*
/// (unsubscripted) arrayed name inside a *nested* `PREVIOUS` now compiles,
/// simulates, and produces per-element values identical to the explicitly
/// subscripted form.
///
/// `p2bare[region] = PREVIOUS(PREVIOUS(pop))` -- with `pop` arrayed over
/// `region` -- is exactly the shape the LTM flow-to-stock link-score
/// generator emits. "Finding 2" of the LTM deep review was this bug
/// surfacing *through* the LTM generator: the inner `PREVIOUS(pop)` was
/// captured into a *scalar* helper aux where a bare arrayed name is
/// ill-typed, so the synthetic fragment failed to compile and was silently
/// stubbed to `0`, collapsing the loop score. Piece 2 worked around it
/// generator-side; this engine-level fix removes the root cause.
///
/// Fix (GH #541): `make_temp_arg` in `builtins_visitor.rs` now synthesizes
/// an *arrayed* (`Equation::ApplyToAll`) helper aux over the active A2A
/// dimensions when the captured argument carries a bare variable reference,
/// and references it `helper[<element>]`. The bare arrayed name keeps its
/// array shape, and dimension matching (including transposed contexts) is
/// delegated to the existing apply-to-all lowering.
///
/// `pop[region]` and `scalar_pop` share identical dynamics, so each arrayed
/// slot must track `PREVIOUS(PREVIOUS(pop[region]))` -- which
/// `subscripted_arrayed_nested_previous_matches_scalar` separately pins
/// against the scalar reference.
#[test]
fn bare_arrayed_nested_previous_matches_subscripted() {
    // The previously-failing case: a *nested* PREVIOUS over a *bare* arrayed
    // name. It must now compile and simulate.
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

    // The explicitly subscripted reference form, simulated in the same model
    // so the comparison is exact per element.
    let nested_sub = TestProject::new("nested_sub_previous")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .array_aux_direct(
            "p2sub",
            vec!["region".to_string()],
            "PREVIOUS(PREVIOUS(pop[region]))",
            None,
        )
        .build_datamodel();

    let run_plain = |project: &datamodel::Project| -> Results {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("GH #541: a bare arrayed name in a nested PREVIOUS must now compile");
        let mut vm = Vm::new(compiled).expect("VM construction should succeed");
        vm.run_to_end()
            .expect("plain simulation should run to completion");
        vm.into_results()
    };

    let bare_results = run_plain(&nested_bare);
    let sub_results = run_plain(&nested_sub);

    for region in ["north", "south"] {
        let bare_series = series_at(
            &bare_results,
            offset_of(&bare_results, &format!("p2bare[{region}]")),
        );
        let sub_series = series_at(
            &sub_results,
            offset_of(&sub_results, &format!("p2sub[{region}]")),
        );
        assert_eq!(
            bare_series, sub_series,
            "GH #541: the bare form `PREVIOUS(PREVIOUS(pop))` must match the subscripted \
             form `PREVIOUS(PREVIOUS(pop[region]))` per element at {region}"
        );
    }

    // The companion invariant: a *single* (non-nested) bare arrayed
    // `PREVIOUS(pop)` always compiled; it must keep working.
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
         must still compile"
    );
}

/// The companion to `bare_arrayed_nested_previous_matches_subscripted`:
/// the properly *subscripted* form of a nested `PREVIOUS` over an arrayed
/// variable compiles and is correct per element, pinned against the scalar
/// reference (the bare-form test pins the bare-vs-subscripted equivalence).
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

/// GH #541, multi-dimensional case: a bare arrayed name in a nested
/// `PREVIOUS` inside an apply-to-all equation over *two* dimensions
/// compiles and matches the explicitly subscripted form per element.
/// `make_temp_arg`'s arrayed helper is `ApplyToAll([region, age], ...)`, so
/// the bare `pop` reference keeps its full 2-D shape.
#[test]
fn bare_arrayed_nested_previous_multidim_matches_subscripted() {
    let run_plain = |eqn: &str, name: &str| -> Results {
        let project = TestProject::new("md")
            .with_sim_time(0.0, 5.0, 1.0)
            .named_dimension("region", &["north", "south"])
            .named_dimension("age", &["young", "old"])
            .array_stock("pop[region,age]", "100", &["growth"], &[], None)
            .array_flow("growth[region,age]", "pop[region,age] * 0.1", None)
            .array_aux_direct(
                name,
                vec!["region".to_string(), "age".to_string()],
                eqn,
                None,
            )
            .build_datamodel();
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("multi-dim bare nested PREVIOUS must compile");
        let mut vm = Vm::new(compiled).expect("VM construction should succeed");
        vm.run_to_end()
            .expect("simulation should run to completion");
        vm.into_results()
    };
    let bare = run_plain("PREVIOUS(PREVIOUS(pop))", "p2bare");
    let sub = run_plain("PREVIOUS(PREVIOUS(pop[region,age]))", "p2sub");
    for region in ["north", "south"] {
        for age in ["young", "old"] {
            let b = series_at(&bare, offset_of(&bare, &format!("p2bare[{region},{age}]")));
            let s = series_at(&sub, offset_of(&sub, &format!("p2sub[{region},{age}]")));
            assert_eq!(
                b, s,
                "multi-dim bare form must match subscripted form at [{region},{age}]"
            );
        }
    }
}

/// GH #541, *transposed* context: the bare arrayed reference's declared
/// dimension order (`pop[age,region]`) differs from the enclosing
/// apply-to-all's (`[region,age]`). The arrayed helper delegates dimension
/// matching to the apply-to-all lowering, which matches by dimension *name*
/// (not position), so the bare form resolves correctly even transposed.
/// Distinct per-element initial values make a positional mis-map observable
/// (a wrong transpose would read the wrong source element).
#[test]
fn bare_arrayed_nested_previous_transposed_matches_subscripted() {
    use datamodel::{Aux, Compat, Equation, Flow, Stock, Variable};

    // pop declared over [age, region] with distinct per-element initials so a
    // transpose error is detectable: (young,north)=10, (young,south)=20,
    // (old,north)=30, (old,south)=40.
    let build = |target_eqn: &str, target_name: &str| -> datamodel::Project {
        let mut project = TestProject::new("tr")
            .with_sim_time(0.0, 5.0, 1.0)
            .named_dimension("region", &["north", "south"])
            .named_dimension("age", &["young", "old"])
            .build_datamodel();
        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .expect("main model");
        model.variables.push(Variable::Stock(Stock {
            ident: "pop".to_string(),
            equation: Equation::Arrayed(
                vec!["age".to_string(), "region".to_string()],
                vec![
                    ("young,north".to_string(), "10".to_string(), None, None),
                    ("young,south".to_string(), "20".to_string(), None, None),
                    ("old,north".to_string(), "30".to_string(), None, None),
                    ("old,south".to_string(), "40".to_string(), None, None),
                ],
                None,
                false,
            ),
            documentation: String::new(),
            units: None,
            inflows: vec!["growth".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        model.variables.push(Variable::Flow(Flow {
            ident: "growth".to_string(),
            equation: Equation::ApplyToAll(
                vec!["age".to_string(), "region".to_string()],
                "pop[age,region] * 0.1".to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        // target apply-to-all over the TRANSPOSED order [region, age].
        model.variables.push(Variable::Aux(Aux {
            ident: target_name.to_string(),
            equation: Equation::ApplyToAll(
                vec!["region".to_string(), "age".to_string()],
                target_eqn.to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        project
    };

    let run_plain = |project: &datamodel::Project| -> Results {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("transposed bare nested PREVIOUS must compile");
        let mut vm = Vm::new(compiled).expect("VM construction should succeed");
        vm.run_to_end()
            .expect("simulation should run to completion");
        vm.into_results()
    };

    let bare = run_plain(&build("PREVIOUS(PREVIOUS(pop))", "p2bare"));
    // The subscripted reference is in the SOURCE's order [age,region]; the
    // apply-to-all over [region,age] still resolves it by name.
    let sub = run_plain(&build("PREVIOUS(PREVIOUS(pop[age,region]))", "p2sub"));
    for region in ["north", "south"] {
        for age in ["young", "old"] {
            let b = series_at(&bare, offset_of(&bare, &format!("p2bare[{region},{age}]")));
            let s = series_at(&sub, offset_of(&sub, &format!("p2sub[{region},{age}]")));
            assert_eq!(
                b, s,
                "transposed bare form must match transposed subscripted form at \
                 [{region},{age}] (dimension matching is by name, not position)"
            );
        }
    }
    // Spot-check the absolute values too: p2bare[north,young] traces
    // pop[young,north] (initial 10), p2bare[south,old] traces pop[old,south]
    // (initial 40) -- a positional transpose error would swap these.
    let ny = series_at(&bare, offset_of(&bare, "p2bare[north,young]"));
    let so = series_at(&bare, offset_of(&bare, "p2bare[south,old]"));
    assert_eq!(
        ny[2], 10.0,
        "p2bare[north,young] step 2 must trace pop[young,north]=10"
    );
    assert_eq!(
        so[2], 40.0,
        "p2bare[south,old] step 2 must trace pop[old,south]=40"
    );
}

/// Regression for the C-LEARN compile break GH #541's arrayed-helper path
/// introduced (then fixed forward): an apply-to-all `INITIAL` whose argument
/// subscripts arrays by dimensions that *map* to the active dimension.
///
/// This is C-LEARN's dominant idiom -- `FF change start year[COP] =
/// INITIAL(IF THEN ELSE(..., agg[Aggregated Regions], semi[Semi Agg]))`,
/// where `Aggregated Regions` and `Semi Agg` map to `COP`. The model below is
/// the same shape in miniature: `Agg` (3 elems) maps to `COP` (3 elems) via an
/// element list. Built through the MDL importer so the dimension-mapping
/// structure is byte-identical to what C-LEARN carries (a hand-built
/// `DimensionMapping` does not reproduce the same `find_mapping_parent_of` /
/// `translate_via_mapping` resolution).
///
/// The original #541 path wrapped the whole `INITIAL` body in an
/// `Equation::ApplyToAll(["cop"], ...)` helper and SKIPPED
/// `substitute_dimension_refs`, so the foreign `[Aggregated Regions]` subscript
/// stayed un-translated and the helper fragment failed (`BadDimensionName` ->
/// silently dropped -> `NotSimulatable`). The fix keeps a subscripted argument
/// on the per-element scalar-helper path, which translates each mapped
/// subscript to a concrete element. The per-element values must be exactly the
/// mapped source values (`Agg` element `Ai` -> `COP` element `Ci`).
#[test]
fn a2a_init_mapped_dim_subscript_matches_mapped_source() {
    const MDL: &str = r#"{UTF-8}
Agg:
	A1, A2, A3
	-> (COP: COP_a, COP_b, COP_c)
	~	~	|
COP_a:
	C1
	~	~	|
COP_b:
	C2
	~	~	|
COP_c:
	C3
	~	~	|
COP:
	C1, C2, C3
	~	~	|
src agg[Agg]=
	100, 200, 300
	~	~	|
sw=
	2
	~	~	|
tgt[COP]= INITIAL(
	IF THEN ELSE(sw=3, 0, src agg[Agg]))
	~	~	|
********************************************************
	.Control
********************************************************~
		Simulation Control Parameters
	|
INITIAL TIME  = 0 ~	~	|
FINAL TIME  = 2 ~	~	|
TIME STEP  = 1 ~	~	|
SAVEPER  = 1 ~	~	|
\\\---/// Sketch information
"#;

    let project = open_vensim(MDL).expect("C-LEARN-shaped MDL must parse");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main").expect(
        "GH #541 regression: an A2A INITIAL whose arg subscripts by a mapped \
         dimension must compile (the per-element scalar helper translates the \
         mapped subscript)",
    );
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    // sw=2 -> tgt[Ci] = src agg[Ai] (Agg maps A1->C1, A2->C2, A3->C3).
    for (cop_elem, expect) in [("c1", 100.0), ("c2", 200.0), ("c3", 300.0)] {
        let series = series_at(&results, offset_of(&results, &format!("tgt[{cop_elem}]")));
        assert!(
            series.iter().all(|&v| (v - expect).abs() < 1e-9),
            "tgt[{cop_elem}] must equal the mapped source value {expect}; got {series:?}"
        );
    }
}

/// Regression for a capitalized-dimension-name variant of the GH #541
/// arrayed-helper fix: when the arrayed `PREVIOUS`/`INIT` helper IS taken
/// (a bare arrayed name, no subscript), its synthesized
/// `Equation::ApplyToAll` carries canonical dimension names (`region`), which
/// must resolve against a dimension declared with original casing (`Region`).
/// A raw `==` match rejected this as `BadDimensionName` and silently dropped
/// the helper; `variable::get_dimensions` now matches canonically.
#[test]
fn arrayed_helper_resolves_capitalized_dimension() {
    let project = TestProject::new("caps")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["North", "South"])
        .array_stock("pop[Region]", "100", &["growth"], &[], None)
        .array_flow("growth[Region]", "pop[Region] * 0.1", None)
        // Bare arrayed `pop` in a nested PREVIOUS -> the arrayed-helper path,
        // whose ApplyToAll dims are the canonical `region` but must resolve
        // against the `Region` dimension.
        .array_aux_direct(
            "p2bare",
            vec!["Region".to_string()],
            "PREVIOUS(PREVIOUS(pop))",
            None,
        )
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    compile_project_incremental(&db, sync.project, "main").expect(
        "a bare-arrayed nested PREVIOUS over a capitalized dimension must compile: \
         the arrayed helper's canonical ApplyToAll dims must resolve against the \
         capitalized dimension name",
    );
}

/// Regression for the LTM-vs-plain divergence the arrayed-helper path caused
/// on C-LEARN: a bare arrayed name *inside an array reducer*
/// (`PREVIOUS(SUM(arr))`) must use a SCALAR helper, not the arrayed-helper
/// path. The reducer collapses its arrayed argument to a scalar, so wrapping
/// `SUM(arr)` in an `Equation::ApplyToAll` would broadcast a scalar reduce
/// across the active dimensions and corrupt the value -- exactly the shape of
/// an LTM link-score numerator, which is why enabling LTM diverged from plain.
/// The non-reducer bare-name case (`PREVIOUS(PREVIOUS(arr))`) still takes the
/// arrayed path; this pins that the reducer case does not.
#[test]
fn bare_arrayed_inside_reducer_uses_scalar_helper() {
    // `agg[region] = PREVIOUS(SUM(pop))`: SUM(pop) is a whole-array scalar
    // reduce, so every element of `agg` holds the same lagged total. A
    // wrongly-arrayed helper would broadcast/mis-shape it.
    let arrayed = TestProject::new("reducer_bare")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .array_aux_direct(
            "agg",
            vec!["region".to_string()],
            "PREVIOUS(SUM(pop))",
            None,
        )
        .build_datamodel();

    // Scalar reference: `sref = PREVIOUS(SUM(pop))` computed once.
    let scalar = TestProject::new("reducer_scalar")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .scalar_aux("sref", "PREVIOUS(SUM(pop))")
        .build_datamodel();

    let run = |project: &datamodel::Project| -> Results {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("PREVIOUS(SUM(arr)) in an A2A equation must compile");
        let mut vm = Vm::new(compiled).expect("VM construction should succeed");
        vm.run_to_end()
            .expect("simulation should run to completion");
        vm.into_results()
    };
    let arr = run(&arrayed);
    let sca = run(&scalar);
    let sref = series_at(&sca, offset_of(&sca, "sref"));
    for region in ["north", "south"] {
        let agg = series_at(&arr, offset_of(&arr, &format!("agg[{region}]")));
        assert_eq!(
            agg, sref,
            "agg[{region}] = PREVIOUS(SUM(pop)) must equal the scalar PREVIOUS(SUM(pop)) \
             (the whole-array reduce is a scalar broadcast to every element)"
        );
    }
}

/// Run a plain (non-LTM) simulation of `project` to completion.
fn run_plain_sim(project: &datamodel::Project) -> Results {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("model must compile via the incremental path");
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("plain simulation should run to completion");
    vm.into_results()
}

/// Regression for the PR #668 / GH #541 per-slot helper-id collision: a
/// per-element (`Equation::Arrayed`) variable whose distinct slots each take
/// the arrayed `PREVIOUS` helper path must keep per-slot identity, not have a
/// later slot silently read an earlier slot's helper.
///
/// `x[e1] = PREVIOUS(PREVIOUS(a))`, `x[e2] = PREVIOUS(PREVIOUS(b))` with `a`,
/// `b` plain scalars of DISTINCT dynamics: both inner `PREVIOUS(scalar)` args
/// are bare non-dim references with no subscript, so the arrayed-helper branch
/// fires for each slot. The original suffix-less helper id `$⁚x⁚0⁚arg0`
/// collided across slots (each fresh per-element visitor restarts `n` at 0 and
/// shares `variable_name`), `dedup_vars_by_ident` kept the first, and `x[e2]`
/// read `a`'s helper. The fix appends the slot element suffix in the
/// per-element-equation context so the two helpers are distinct.
#[test]
fn arrayed_per_element_previous_keeps_per_slot_identity() {
    let project = TestProject::new("collision")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("reg", &["e1", "e2"])
        // a: 10,15,20,25,...   b: 100,150,200,250,... (distinct dynamics)
        .stock("a", "10", &["da"], &[], None)
        .flow("da", "5", None)
        .stock("b", "100", &["db"], &[], None)
        .flow("db", "50", None)
        .array_with_ranges_direct(
            "x",
            vec!["reg".to_string()],
            vec![
                ("e1", "PREVIOUS(PREVIOUS(a))"),
                ("e2", "PREVIOUS(PREVIOUS(b))"),
            ],
            None,
        )
        .build_datamodel();

    let results = run_plain_sim(&project);
    let a = series_at(&results, offset_of(&results, "a"));
    let b = series_at(&results, offset_of(&results, "b"));
    let x_e1 = series_at(&results, offset_of(&results, "x[e1]"));
    let x_e2 = series_at(&results, offset_of(&results, "x[e2]"));

    // `PREVIOUS(PREVIOUS(z))` at step t is z at step t-2 (0 for t<2).
    let lagged_twice = |s: &[f64]| -> Vec<f64> {
        (0..s.len())
            .map(|t| if t >= 2 { s[t - 2] } else { 0.0 })
            .collect::<Vec<f64>>()
    };
    assert_eq!(x_e1, lagged_twice(&a), "x[e1] must track a lagged twice");
    assert_eq!(
        x_e2,
        lagged_twice(&b),
        "x[e2] must track b lagged twice -- NOT a's helper (PR #668 collision)"
    );
    // Cross-check the slots genuinely differ (a vs b dynamics), so the pin
    // cannot pass vacuously if both slots collapsed to the same value.
    assert_ne!(x_e1, x_e2, "the two slots must hold distinct series");
}

/// The bare-ARRAYED variant of the per-slot-identity pin: per-element slots
/// each reference a *different* bare arrayed name in a nested `PREVIOUS`. Each
/// slot's arrayed helper must stay distinct, and each slot's value must equal
/// its explicitly-subscripted equivalent (`PREVIOUS(PREVIOUS(arr[reg]))`).
#[test]
fn arrayed_per_element_bare_arrayed_previous_distinct_helpers() {
    // pop and qty are arrayed over `reg` with distinct per-element dynamics.
    let bare = TestProject::new("bare_per_elem")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("reg", &["e1", "e2"])
        .array_stock("pop[reg]", "100", &["gp"], &[], None)
        .array_flow("gp[reg]", "pop[reg] * 0.1", None)
        .array_stock("qty[reg]", "10", &["gq"], &[], None)
        .array_flow("gq[reg]", "qty[reg] * 0.2", None)
        .array_with_ranges_direct(
            "x",
            vec!["reg".to_string()],
            // e1 references bare `pop`, e2 references bare `qty`.
            vec![
                ("e1", "PREVIOUS(PREVIOUS(pop))"),
                ("e2", "PREVIOUS(PREVIOUS(qty))"),
            ],
            None,
        )
        .build_datamodel();

    // The explicitly-subscripted equivalent, in the same per-element shape.
    let subscripted = TestProject::new("sub_per_elem")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("reg", &["e1", "e2"])
        .array_stock("pop[reg]", "100", &["gp"], &[], None)
        .array_flow("gp[reg]", "pop[reg] * 0.1", None)
        .array_stock("qty[reg]", "10", &["gq"], &[], None)
        .array_flow("gq[reg]", "qty[reg] * 0.2", None)
        .array_with_ranges_direct(
            "x",
            vec!["reg".to_string()],
            vec![
                ("e1", "PREVIOUS(PREVIOUS(pop[reg]))"),
                ("e2", "PREVIOUS(PREVIOUS(qty[reg]))"),
            ],
            None,
        )
        .build_datamodel();

    let bare_results = run_plain_sim(&bare);
    let sub_results = run_plain_sim(&subscripted);
    for elem in ["e1", "e2"] {
        let b = series_at(
            &bare_results,
            offset_of(&bare_results, &format!("x[{elem}]")),
        );
        let s = series_at(&sub_results, offset_of(&sub_results, &format!("x[{elem}]")));
        assert_eq!(
            b, s,
            "x[{elem}]: bare per-element nested PREVIOUS must match the subscripted form"
        );
    }
    // pop (init 100) and qty (init 10) differ, so the slots must differ.
    let xe1 = series_at(&bare_results, offset_of(&bare_results, "x[e1]"));
    let xe2 = series_at(&bare_results, offset_of(&bare_results, "x[e2]"));
    assert_ne!(xe1, xe2, "the two slots track pop vs qty -- must differ");
}

/// Same-body `Ast::Arrayed` slots: BOTH slots are `PREVIOUS(PREVIOUS(arr))`.
/// Here the per-element helpers genuinely *are* identical content, so the
/// values were correct even before the fix (the old suffix-less id happened to
/// collapse them). After the fix they are distinct suffixed helpers but must
/// still produce identical, correct per-element values. Pins that the fix did
/// not regress the same-body case.
#[test]
fn arrayed_per_element_same_body_previous_correct() {
    let project = TestProject::new("same_body")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("reg", &["e1", "e2"])
        .array_stock("pop[reg]", "100", &["gp"], &[], None)
        .array_flow("gp[reg]", "pop[reg] * 0.1", None)
        .array_with_ranges_direct(
            "x",
            vec!["reg".to_string()],
            vec![
                ("e1", "PREVIOUS(PREVIOUS(pop))"),
                ("e2", "PREVIOUS(PREVIOUS(pop))"),
            ],
            None,
        )
        .build_datamodel();

    let results = run_plain_sim(&project);
    let pop_e1 = series_at(&results, offset_of(&results, "pop[e1]"));
    let pop_e2 = series_at(&results, offset_of(&results, "pop[e2]"));
    let lagged_twice = |s: &[f64]| -> Vec<f64> {
        (0..s.len())
            .map(|t| if t >= 2 { s[t - 2] } else { 0.0 })
            .collect::<Vec<f64>>()
    };
    let x_e1 = series_at(&results, offset_of(&results, "x[e1]"));
    let x_e2 = series_at(&results, offset_of(&results, "x[e2]"));
    assert_eq!(
        x_e1,
        lagged_twice(&pop_e1),
        "x[e1] tracks pop[e1] lagged twice"
    );
    assert_eq!(
        x_e2,
        lagged_twice(&pop_e2),
        "x[e2] tracks pop[e2] lagged twice"
    );
}

/// The `INIT` twin of `arrayed_per_element_previous_keeps_per_slot_identity`:
/// `INIT` arguments take the same `make_temp_arg` arrayed-helper path, so a
/// per-element slot collision would corrupt initial values too. `INIT(z)` is
/// `z` at the initial step held constant, so each slot must equal its own
/// scalar's initial value.
#[test]
fn arrayed_per_element_init_keeps_per_slot_identity() {
    let project = TestProject::new("init_collision")
        .with_sim_time(0.0, 4.0, 1.0)
        // a, b distinct scalars, wrapped so INIT's arg is an expression
        // (forcing the helper path).
        .scalar_aux("a", "10")
        .scalar_aux("b", "100")
        .array_with_ranges_direct(
            "x",
            vec!["reg".to_string()],
            vec![("e1", "INIT(a + 1)"), ("e2", "INIT(b + 1)")],
            None,
        )
        .named_dimension("reg", &["e1", "e2"])
        .build_datamodel();

    let results = run_plain_sim(&project);
    let x_e1 = series_at(&results, offset_of(&results, "x[e1]"));
    let x_e2 = series_at(&results, offset_of(&results, "x[e2]"));
    assert!(
        x_e1.iter().all(|&v| v == 11.0),
        "x[e1] = INIT(a + 1) must hold 11; got {x_e1:?}"
    );
    assert!(
        x_e2.iter().all(|&v| v == 101.0),
        "x[e2] = INIT(b + 1) must hold 101 -- NOT a's helper (PR #668 collision); got {x_e2:?}"
    );
}

/// The GH #525 partially-iterated-subscript shape now compiles AND scores
/// (resolved by the GH #759 dimension-name-index fix).
///
/// The model: an arrayed stock `pop[region,age]`, a row-reducer
/// `row_sum[region] = pop[region,young] * 2` (a partially iterated
/// subscript -- `young` is fixed, `region` is iterated), and a flow
/// `growth[region,age] = row_sum[region] * 0.0001 * pop[region,age]` that
/// closes a feedback loop. The `pop[region,young]` reference still
/// classifies `DynamicIndex` (the classifier's all-iterated rule doesn't
/// cover the iterated+literal mix), but the conservative partial it
/// produces is now compilable: before the #759 fix the iterated dim name
/// `region` was PREVIOUS-wrapped inside the subscript
/// (`pop[PREVIOUS(region), young]` / `PREVIOUS(SUM(pop[PREVIOUS(region),
/// young]))`), the fragment and its capture helpers failed, and the score
/// was stubbed to 0 with a Warning (a hard `NotSimulatable` on the
/// pre-fragment-isolation path the original #525 filing hit). Now the
/// index stays verbatim, the `PREVIOUS(SUM(pop[region, young]))` guard
/// term compiles through a per-element capture helper, and the score
/// carries its true value.
///
/// `row_sum` depends on nothing but `pop`, so the conservative
/// all-references-live partial reproduces Δrow_sum exactly: the score is
/// `+1` from the first post-initial step. The residual #525 conservatism
/// -- the `DynamicIndex` cross-product element edges enumerating
/// cross-element loops that don't exist causally -- is tracked on GH #525.
#[test]
fn partially_iterated_subscript_link_score_compiles_and_scores() {
    let project = TestProject::new("ltm525_scored")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("region", &["a", "b"])
        .named_dimension("age", &["young", "old"])
        .array_stock("pop[region,age]", "100", &["growth"], &[], None)
        .array_aux_direct(
            "row_sum",
            vec!["region".to_string()],
            "pop[region,young] * 2",
            None,
        )
        .array_flow(
            "growth[region,age]",
            "row_sum[region] * 0.0001 * pop[region,age]",
            None,
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the GH #525 shape must compile with LTM enabled");

    // Every fragment (capture helpers included) compiles: no degradation
    // warnings anywhere.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the partially-iterated shape must compile every LTM fragment cleanly; got: {warnings:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // `pop` is row_sum's only dependency, so the conservative partial is
    // exact: +1 per element from the first post-initial step.
    let link_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}row_sum";
    let base = offset_of(&results, link_name);
    for slot in 0..2 {
        let series = series_at(&results, base + slot);
        assert_eq!(series[0], 0.0, "initial-step guard pins slot {slot} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "pop->row_sum slot {slot} step {step} must score +1; got {series:?}"
            );
        }
    }

    // The sibling link score still carries real values (nothing regressed).
    let sibling = "$\u{205A}ltm\u{205A}link_score\u{205A}row_sum\u{2192}growth";
    let sibling_series = series_at(&results, offset_of(&results, sibling));
    assert!(
        sibling_series.iter().any(|&v| v != 0.0),
        "the row_sum->growth link score must carry real values; got {sibling_series:?}"
    );
}

/// GH #737, arrayed-agg companion: a *scalar feeder* of an ARRAYED hoisted
/// reducer (`growth[r1] = 0.01 * pool[r1] + SUM(matrix[r1,*] * scale)` --
/// `result_dims = [r1]`, so the agg is A2A over `r1`) in a feedback loop.
/// Such a cycle mixes scalar (`scale`) and arrayed nodes, so it was already
/// CrossElementOrMixed and traversed `scale → $⁚ltm⁚agg⁚0[<slot>]` element
/// hops -- but `emit_source_to_agg_link_scores` early-returned for the
/// scalar feeder, leaving the loop score referencing a never-emitted name
/// (silently stubbed to 0).
///
/// The scalar-feeder emission now produces ONE A2A link score
/// `$⁚ltm⁚link_score⁚scale→$⁚ltm⁚agg⁚0` dimensioned over `result_dims`
/// (the changed-last equation text is ApplyToAll-compatible: the agg's own
/// reducer body iterated over `r1`, with only the scalar feeder frozen at
/// PREVIOUS), and each per-slot cross-element loop references it
/// subscripted-after-quote (`"…scale→$⁚ltm⁚agg⁚0"[a]`).
#[test]
fn arrayed_agg_scalar_feeder_loop_scores_sustained() {
    let project = TestProject::new("arrayed_agg_scalar_feeder")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("r1", &["a", "b"])
        .named_dimension("r2", &["x", "y"])
        .array_aux("matrix[r1,r2]", "2")
        .array_stock("pool[r1]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[r1]",
            "0.01 * pool[r1] + SUM(matrix[r1,*] * scale)",
            None,
        )
        // Closes the loop: pool -> $agg1 -> scale -> $agg0 -> growth -> pool.
        .scalar_aux("scale", "0.001 * SUM(pool[*]) + 0.01")
        .build_datamodel();

    let (results, ltm_vars) = run_ltm(&project);
    assert!(results.step_count > STARTUP_STEPS);

    // The scalar-feeder link score is one A2A var over the agg's result dims.
    let feeder_score_name =
        "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0";
    let feeder_score = ltm_var(&ltm_vars, feeder_score_name);
    assert_eq!(
        feeder_score.dimensions,
        vec!["r1".to_string()],
        "the scalar-feeder link score must be dimensioned over the agg's result dims"
    );

    // Each per-slot cross-element loop references the feeder score at its
    // slot and is sustained non-zero past the startup guard.
    let mut slot_loops = 0usize;
    for v in ltm_vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
    {
        let eq = v.equation.source_text();
        if !eq.contains(feeder_score_name) {
            continue;
        }
        slot_loops += 1;
        let s = series_at(&results, offset_of(&results, &v.name));
        assert!(
            s.iter()
                .skip(STARTUP_STEPS)
                .all(|val| val.is_finite() && val.abs() > MEANINGFUL_SCORE),
            "agg-routed loop score {} must be sustained non-zero; series: {s:?}",
            v.name
        );
    }
    assert_eq!(
        slot_loops, 2,
        "one cross-element loop per r1 slot must reference the feeder score"
    );
}

/// GH #745: a scored loop through an ARRAYED synthetic agg must classify
/// concretely (r/b), not Undetermined, when its agg-hop polarities are
/// statically derivable.
///
/// An arrayed agg's element-graph hops carry a slot subscript
/// (`scale → $⁚ltm⁚agg⁚0[a]` / `$⁚ltm⁚agg⁚0[a] → growth[a]`), but
/// `recover_agg_hop_polarities` compared the bare synthetic agg name
/// against the link's FULL ident, so neither endpoint ever matched: the
/// hops stayed `Unknown` and every per-slot loop through an arrayed agg
/// degraded to `u{n}` -- while the detected surface (whose spliced agg
/// hops are bare-named) recovered the same hops fine, a cross-surface
/// polarity disagreement (GH #746).
///
/// Hand-derived polarities for the reinforcing fixture (per slot `e`):
///  - `pool[e] → $⁚ltm⁚agg⁚1` (`SUM(pool[*])`, scalar agg): SUM is
///    monotone increasing in every element it reads -> Positive;
///  - `$⁚ltm⁚agg⁚1 → scale` (`scale = 0.001*agg1 + 0.01`): Positive;
///  - `scale → $⁚ltm⁚agg⁚0[e]` (`SUM(matrix[e,*] * scale)`):
///    d(agg0[e])/d(scale) = SUM(matrix[e,*]) = 4 > 0 (matrix = 2) ->
///    Positive;
///  - `$⁚ltm⁚agg⁚0[e] → growth[e]` (`growth = 0.01*pool + agg0`):
///    Positive;
///  - `growth[e] → pool[e]`: flow into stock -> Positive.
///
/// Zero negative links -> Reinforcing. The balancing variant negates only
/// the scalar-feeder hop (d(agg0[e])/d(scale) = -SUM(matrix[e,*]) < 0 via
/// the `(1 - scale)` co-factor) -> exactly one negative link -> Balancing.
#[test]
fn arrayed_agg_feeder_loops_classify_concretely() {
    struct Case {
        growth_eqn: &'static str,
        want_prefix: char,
        detected: DetectedLoopPolarity,
    }
    let cases = [
        Case {
            growth_eqn: "0.01 * pool[r1] + SUM(matrix[r1,*] * scale)",
            want_prefix: 'r',
            detected: DetectedLoopPolarity::Reinforcing,
        },
        Case {
            growth_eqn: "0.01 * pool[r1] + SUM(matrix[r1,*] * (1 - scale))",
            want_prefix: 'b',
            detected: DetectedLoopPolarity::Balancing,
        },
    ];

    for case in &cases {
        let project = TestProject::new("arrayed_agg_polarity")
            .with_sim_time(0.0, 6.0, 1.0)
            .named_dimension("r1", &["a", "b"])
            .named_dimension("r2", &["x", "y"])
            .array_aux("matrix[r1,r2]", "2")
            .array_stock("pool[r1]", "100", &["growth"], &[], None)
            .array_flow("growth[r1]", case.growth_eqn, None)
            // Closes the per-slot feeder loops:
            // pool[e] -> $agg1 -> scale -> $agg0[e] -> growth[e] -> pool[e].
            .scalar_aux("scale", "0.001 * SUM(pool[*]) + 0.01")
            .build_datamodel();

        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        let ltm = model_ltm_variables(&db, source_model, sync.project);

        // Scored surface: the per-slot loops referencing the arrayed agg's
        // scalar-feeder link score must carry the derived polarity prefix.
        let feeder_score_name =
            "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0";
        let feeder_loop_ids: Vec<&str> = ltm
            .vars
            .iter()
            .filter(|v| {
                v.name.starts_with(LOOP_SCORE_PREFIX)
                    && v.equation.source_text().contains(feeder_score_name)
            })
            .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
            .collect();
        assert_eq!(
            feeder_loop_ids.len(),
            2,
            "[{}] one per-slot feeder loop per r1 element; loop scores: {:?}",
            case.growth_eqn,
            ltm.vars
                .iter()
                .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
        for id in &feeder_loop_ids {
            assert!(
                id.starts_with(case.want_prefix),
                "[{}] per-slot arrayed-agg feeder loop must classify {} (every hop is \
                 statically derivable -- see the hand derivation above), not Undetermined; \
                 got id {id:?}",
                case.growth_eqn,
                case.want_prefix
            );
        }

        // Cross-surface agreement (GH #746): the detected surface's spliced
        // agg hops are bare-named and were always recovered; post-fix the
        // scored surface must agree with it rather than reporting U.
        let detected = model_detected_loops(&db, source_model, sync.project).clone();
        let feeder = detected
            .loops
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "scale"))
            .expect("the feeder loop must be detected");
        assert_eq!(
            feeder.polarity, case.detected,
            "[{}] the detected surface derives the same hop polarities from the same \
             recovery pass",
            case.growth_eqn
        );
    }
}

/// GH #745, agg-identity probe: stripping the slot subscript before the
/// agg-endpoint match must NOT confuse WHICH arrayed agg a hop belongs to
/// -- `$⁚ltm⁚agg⁚0[a]` strips to `$⁚ltm⁚agg⁚0`, keeping the agg index
/// significant. Two arrayed aggs of OPPOSING feeder-hop signs in one
/// equation pin this: if the strip aliased the aggs, the per-slot loops
/// through `SUM(matrix[e,*] * scale)` (d/d scale > 0, reinforcing) and
/// `SUM(matrix2[e,*] * (1 - scale))` (d/d scale < 0, balancing) could be
/// analyzed against the WRONG agg body and come back with swapped or
/// Unknown polarities. (Same hand derivation as
/// `arrayed_agg_feeder_loops_classify_concretely`; only the scalar-feeder
/// hop's sign differs between the two agg routes.)
#[test]
fn opposing_arrayed_multi_agg_loops_keep_agg_identity() {
    let project = TestProject::new("opposing_arrayed_multi_agg")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("r1", &["a", "b"])
        .named_dimension("r2", &["x", "y"])
        .array_aux("matrix[r1,r2]", "2")
        .array_aux("matrix2[r1,r2]", "3")
        .array_stock("pool[r1]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[r1]",
            "0.01 * pool[r1] + SUM(matrix[r1,*] * scale) + SUM(matrix2[r1,*] * (1 - scale))",
            None,
        )
        .scalar_aux("scale", "0.001 * SUM(pool[*]) + 0.01")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // agg⁚0 = SUM(matrix[r1,*] * scale) (left-to-right minting order),
    // agg⁚1 = SUM(matrix2[r1,*] * (1 - scale)); both arrayed over r1.
    let cases = [
        (
            "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0",
            'r',
        ),
        (
            "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}1",
            'b',
        ),
    ];
    for (feeder_score_name, want_prefix) in cases {
        let ids: Vec<&str> = ltm
            .vars
            .iter()
            .filter(|v| {
                v.name.starts_with(LOOP_SCORE_PREFIX)
                    && v.equation.source_text().contains(feeder_score_name)
            })
            .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "one per-slot loop per r1 element through {feeder_score_name}; loop scores: {:?}",
            ltm.vars
                .iter()
                .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
        for id in &ids {
            assert!(
                id.starts_with(want_prefix),
                "loops through {feeder_score_name} must be {want_prefix}-prefixed (the agg \
                 index decides which body the hop analysis reads); got id {id:?}"
            );
        }
    }
}

/// GH #737 follow-up (review C1): the structural FFI loop surface
/// (`model_detected_loops`) and the scored surface (`model_ltm_variables`)
/// must assign IDENTICAL ids (and matching structural polarities) to the
/// scalar-feeder reducer loops, because the runtime join is keyed purely on
/// the id: `reclassify_loops_from_results` (and its production caller
/// `simlin_analyze_get_loops_runtime`, plus pysimlin's
/// `get_relative_loop_score(loop.id)`) reads `$⁚ltm⁚loop_score⁚{id}` for
/// each detected loop.
///
/// Pre-fix the two surfaces diverged AND collided on these fixtures: the
/// scored surface routed the feeder loop through the agg and recovered its
/// polarity (r/b prefix), while the detected surface still derived polarity
/// from the variable-level `scale→grow` link (Unknown → `u1`) -- so scored
/// = {r1: feeder, r2: pop} vs detected = {u1: feeder, r1: pop}, and the
/// runtime join classified the pop-growth loop from the FEEDER loop's
/// series. On the negating-body variant that reported the pop-growth loop
/// as confidently Balancing -- a wrong polarity with confidence 1.0 on a
/// production FFI surface.
///
/// Covers both the headline fixture (`SUM(pop[*] * scale)`, feeder loop
/// reinforcing) and the negating-body variant (`SUM(pop[*] * (1 - scale))`,
/// feeder loop balancing -- exercising the discriminating feeder-hop
/// polarity analysis, review I1).
#[test]
fn scalar_feeder_loop_cross_surface_ids_and_polarity_agree() {
    struct Case {
        grow_eqn: &'static str,
        feeder_structural: DetectedLoopPolarity,
        feeder_runtime: [DetectedLoopPolarity; 2],
    }
    let cases = [
        Case {
            grow_eqn: "1 + SUM(pop[*] * scale)",
            feeder_structural: DetectedLoopPolarity::Reinforcing,
            feeder_runtime: [
                DetectedLoopPolarity::Reinforcing,
                DetectedLoopPolarity::MostlyReinforcing,
            ],
        },
        Case {
            grow_eqn: "1 + SUM(pop[*] * (1 - scale))",
            feeder_structural: DetectedLoopPolarity::Balancing,
            feeder_runtime: [
                DetectedLoopPolarity::Balancing,
                DetectedLoopPolarity::MostlyBalancing,
            ],
        },
    ];

    for case in &cases {
        let project = TestProject::new("cross_surface_feeder")
            .with_sim_time(0.0, 6.0, 1.0)
            .named_dimension("region", &["north", "south"])
            .array_with_ranges("pop0[region]", vec![("north", "100"), ("south", "300")])
            .array_stock("pop[region]", "pop0[region]", &["pgrow"], &[], None)
            .array_flow("pgrow[region]", "pop[region] * 0.05", None)
            .scalar_aux("scale", "0.001 * total + 0.01")
            .stock("total", "100", &["grow"], &[], None)
            .flow("grow", case.grow_eqn, None)
            .build_datamodel();

        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("LTM-enabled compilation should succeed");
        let source_model = sync.models["main"].source_model;
        let ltm = model_ltm_variables(&db, source_model, sync.project);

        // The scored surface's loop-score ids.
        let scored_ids: std::collections::BTreeSet<String> = ltm
            .vars
            .iter()
            .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
            .map(|id| id.to_string())
            .collect();

        // The structural FFI surface's loops.
        let detected = model_detected_loops(&db, source_model, sync.project).clone();
        let detected_ids: std::collections::BTreeSet<String> =
            detected.loops.iter().map(|l| l.id.clone()).collect();
        assert_eq!(
            detected_ids, scored_ids,
            "[{}] the detected-loop ids must equal the scored loop-score ids (the runtime \
             join and get_relative_loop_score are keyed purely on the id)",
            case.grow_eqn
        );

        let feeder = detected
            .loops
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "scale"))
            .expect("the feeder loop must be detected");
        let pop_loop = detected
            .loops
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "pgrow"))
            .expect("the pop-growth loop must be detected");
        assert_eq!(
            feeder.polarity, case.feeder_structural,
            "[{}] the feeder loop's structural polarity must match the scored surface",
            case.grow_eqn
        );
        assert_eq!(
            pop_loop.polarity,
            DetectedLoopPolarity::Reinforcing,
            "[{}] the pop-growth loop is reinforcing on both surfaces",
            case.grow_eqn
        );

        // The runtime join: reclassify each detected loop from its own
        // loop_score series. Pre-fix the id collision made the pop-growth
        // loop read the feeder loop's series here (confidently wrong).
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();
        let mut runtime_loops = detected.loops.clone();
        reclassify_loops_from_results(&mut runtime_loops, &results, &ltm.loop_partitions);
        let pop_runtime = runtime_loops
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "pgrow"))
            .unwrap();
        assert!(
            matches!(
                pop_runtime.polarity,
                DetectedLoopPolarity::Reinforcing | DetectedLoopPolarity::MostlyReinforcing
            ),
            "[{}] the pop-growth loop's RUNTIME polarity must stay reinforcing (a wrong \
             classification here means the id join read another loop's series); got {:?}",
            case.grow_eqn,
            pop_runtime.polarity
        );
        let feeder_runtime = runtime_loops
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "scale"))
            .unwrap();
        assert!(
            case.feeder_runtime.contains(&feeder_runtime.polarity),
            "[{}] the feeder loop's runtime polarity must match its series sign; got {:?}",
            case.grow_eqn,
            feeder_runtime.polarity
        );
    }
}

/// GH #737 round-2 review, probe A (C1b): MULTI-AGG edges must keep the two
/// surfaces bijective. `grow = 1 + SUM(pop[*] * scale) + SUM(q[*] * scale)`
/// hoists TWO synthetic aggs, both reading `scale`, so the scored surface
/// has TWO feeder loops (one per agg) -- the detected surface must expand
/// the ThroughAgg-routed `scale → grow` edge into one loop per routed agg,
/// or the id sets diverge and every id after the divergence joins the wrong
/// loop's series (the round-1 failure mode again: detected r2 = pop loop
/// read scored r2 = feeder-via-agg1's series).
#[test]
fn multi_agg_feeder_edge_cross_surface_ids_agree() {
    let project = TestProject::new("multi_agg_feeder")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_stock("q[region]", "200", &["qgrow"], &[], None)
        .array_flow("qgrow[region]", "q[region] * 0.04", None)
        .scalar_aux("scale", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale) + SUM(q[*] * scale)", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let scored_ids: std::collections::BTreeSet<String> = ltm
        .vars
        .iter()
        .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
        .map(|id| id.to_string())
        .collect();
    let detected = model_detected_loops(&db, source_model, sync.project).clone();
    let detected_ids: std::collections::BTreeSet<String> =
        detected.loops.iter().map(|l| l.id.clone()).collect();
    assert_eq!(
        detected_ids, scored_ids,
        "multi-agg edge: detected ids must equal the scored loop-score ids"
    );

    // The feeder cycle must surface once PER ROUTED AGG (the bijection), and
    // the user-facing variable lists must not leak the agg nodes.
    let feeder_loops: Vec<_> = detected
        .loops
        .iter()
        .filter(|l| l.variables.iter().any(|v| v == "scale"))
        .collect();
    assert_eq!(
        feeder_loops.len(),
        2,
        "the feeder cycle routes through two hoisted reducers, so it must surface as two \
         detected loops; got: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );
    for l in &detected.loops {
        assert!(
            !l.variables
                .iter()
                .any(|v| v.contains("\u{205A}agg\u{205A}")),
            "DetectedLoop.variables must not leak synthetic agg nodes; {:?}: {:?}",
            l.id,
            l.variables
        );
    }

    // The runtime join: every detected loop reads its OWN series. All four
    // loops here are reinforcing with sustained-positive scores, so the
    // joint assertion with teeth is the id-set equality above plus every
    // runtime classification staying in the reinforcing family.
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let mut runtime_loops = detected.loops.clone();
    reclassify_loops_from_results(&mut runtime_loops, &results, &ltm.loop_partitions);
    for l in &runtime_loops {
        assert!(
            matches!(
                l.polarity,
                DetectedLoopPolarity::Reinforcing | DetectedLoopPolarity::MostlyReinforcing
            ),
            "loop {} ({:?}) must classify reinforcing from its own series; got {:?}",
            l.id,
            l.variables,
            l.polarity
        );
    }
}

/// GH #737 round-2 review, probe B (C1b): a feeder edge through two aggs of
/// OPPOSING signs, plus an unrelated genuinely-Undetermined loop. Pre-fix
/// the detected surface collapsed the feeder cycle into one Unknown loop
/// (u1) while the scored surface had it as two definite loops (r/b) plus
/// the unrelated loop as u1 -- so after simulation the feeder cycle was
/// confidently classified FROM THE UNRELATED LOOP'S SERIES. With the
/// per-agg expansion the surfaces are bijective: the feeder surfaces as one
/// reinforcing and one balancing loop on BOTH sides, and the only
/// u-prefixed loop is the genuinely-undetermined one.
#[test]
fn opposing_multi_agg_feeder_does_not_misjoin_undetermined_loop() {
    let project = TestProject::new("opposing_multi_agg")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_stock("q[region]", "200", &["qgrow"], &[], None)
        .array_flow("qgrow[region]", "q[region] * 0.04", None)
        .scalar_aux("scale", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow(
            "grow",
            "1 + SUM(pop[*] * scale) + SUM(q[*] * (1 - scale))",
            None,
        )
        // An unrelated, genuinely structurally-Undetermined loop: the
        // SIN(TIME) factor defeats static polarity analysis.
        .stock("u_s", "100", &["u_f"], &[], None)
        .flow("u_f", "u_s * 0.01 * SIN(TIME)", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let scored_ids: std::collections::BTreeSet<String> = ltm
        .vars
        .iter()
        .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
        .map(|id| id.to_string())
        .collect();
    let detected = model_detected_loops(&db, source_model, sync.project).clone();
    let detected_ids: std::collections::BTreeSet<String> =
        detected.loops.iter().map(|l| l.id.clone()).collect();
    assert_eq!(
        detected_ids, scored_ids,
        "opposing multi-agg: detected ids must equal the scored loop-score ids"
    );

    // Exactly one u-prefixed loop, and it is the unrelated SIN loop -- the
    // feeder cycle must NOT degrade to Undetermined (its two agg variants
    // have definite, opposing polarities).
    let u_loops: Vec<_> = detected
        .loops
        .iter()
        .filter(|l| l.id.starts_with('u'))
        .collect();
    assert_eq!(
        u_loops.len(),
        1,
        "exactly one genuinely-Undetermined loop; got: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );
    assert!(
        u_loops[0].variables.iter().any(|v| v == "u_s"),
        "the u-prefixed loop must be the unrelated SIN loop, not the feeder cycle; got {:?}",
        u_loops[0].variables
    );
    let feeder_polarities: std::collections::BTreeSet<&str> = detected
        .loops
        .iter()
        .filter(|l| l.variables.iter().any(|v| v == "scale"))
        .map(|l| match l.polarity {
            DetectedLoopPolarity::Reinforcing => "R",
            DetectedLoopPolarity::Balancing => "B",
            _ => "other",
        })
        .collect();
    assert_eq!(
        feeder_polarities,
        ["R", "B"].into_iter().collect(),
        "the feeder cycle surfaces as one reinforcing (via SUM(pop[*]*scale)) and one \
         balancing (via SUM(q[*]*(1-scale))) loop"
    );

    // Runtime join: the two feeder loops classify from their OWN series with
    // their structural signs (pre-fix the feeder read the SIN loop's series
    // and came back confidently wrong).
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let mut runtime_loops = detected.loops.clone();
    reclassify_loops_from_results(&mut runtime_loops, &results, &ltm.loop_partitions);
    for l in &runtime_loops {
        if !l.variables.iter().any(|v| v == "scale") {
            continue;
        }
        let structural = detected
            .loops
            .iter()
            .find(|d| d.id == l.id)
            .map(|d| d.polarity)
            .unwrap();
        match structural {
            DetectedLoopPolarity::Reinforcing => assert!(
                matches!(
                    l.polarity,
                    DetectedLoopPolarity::Reinforcing | DetectedLoopPolarity::MostlyReinforcing
                ),
                "feeder loop {} runtime polarity must match its reinforcing series; got {:?}",
                l.id,
                l.polarity
            ),
            DetectedLoopPolarity::Balancing => assert!(
                matches!(
                    l.polarity,
                    DetectedLoopPolarity::Balancing | DetectedLoopPolarity::MostlyBalancing
                ),
                "feeder loop {} runtime polarity must match its balancing series; got {:?}",
                l.id,
                l.polarity
            ),
            other => panic!(
                "feeder loop {} has unexpected structural polarity {other:?}",
                l.id
            ),
        }
    }
}

/// GH #737 round-2 review, I1b: an ARRAYED CO-SOURCE feeder of a hoisted
/// reducer must not get the blanket monotone-Positive hop label.
/// `grow = 1 + SUM(pop[*] * (1 - weight[*]))` with
/// `weight[region] = 0.001 * total + 0.01` fed back from the stock `total`:
/// ∂agg/∂weight[e] = -pop[e] < 0, so the total → weight → grow → total loop
/// is genuinely BALANCING. The discriminating source-hop analysis must label
/// it Balancing on both surfaces (pre-fix: the detected surface reported
/// Reinforcing at confidence 1.0; at the base commit it was Undetermined --
/// strictly worse after the round-1 change).
#[test]
fn arrayed_co_source_feeder_loop_is_balancing() {
    let project = TestProject::new("co_source_feeder")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_aux("weight[region]", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * (1 - weight[*]))", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    let source_model = sync.models["main"].source_model;

    // Detected surface: the weight loop must be Balancing -- and above all
    // NOT a confident Reinforcing. (The runtime weight→agg series for this
    // fixture is pinned NEGATIVE by the GH #744 body-aware partial -- see
    // `co_source_weight_to_agg_link_score_tracks_true_partial` -- so the
    // static label and the runtime series now agree in sign.)
    let detected = model_detected_loops(&db, source_model, sync.project).clone();
    let weight_loop = detected
        .loops
        .iter()
        .find(|l| l.variables.iter().any(|v| v == "weight"))
        .expect("the weight loop must be detected");
    assert_eq!(
        weight_loop.polarity,
        DetectedLoopPolarity::Balancing,
        "∂agg/∂weight[e] = -pop[e] < 0: the weight loop is balancing; a Reinforcing label \
         here is the I1b confidently-wrong-label regression"
    );

    // Scored surface: the per-element weight loops' ids must carry the b
    // prefix (same discriminating hop analysis, shared helper).
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let weight_loop_ids: Vec<&str> = ltm
        .vars
        .iter()
        .filter(|v| {
            v.name.starts_with(LOOP_SCORE_PREFIX)
                && v.equation.source_text().contains(agg_name)
                && v.equation.source_text().contains("weight")
        })
        .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !weight_loop_ids.is_empty(),
        "scored weight loops must exist; loop vars: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| (&v.name, v.equation.source_text()))
            .collect::<Vec<_>>()
    );
    for id in &weight_loop_ids {
        assert!(
            id.starts_with('b'),
            "scored weight loop id must be b-prefixed (balancing); got {id:?}"
        );
    }
}

/// GH #528 end-to-end: the strict-prefix *broadcast* arrayed-agg case --
/// `SUM(matrix[D1,*])` as a sub-expression of an A2A body over `D1 x D2`
/// mints an arrayed synthetic agg over `[D1]` that broadcasts into
/// `growth[D1,D2]` -- is genuinely scored. The per-target-element
/// `agg[d1] → growth[d1,d2]` link-score equation must pin the agg ident to
/// the PROJECTED slot `[d1]`, not the full `(d1,d2)` target tuple: the full
/// tuple over-subscripts the 1-D agg, so pre-fix the fragment failed to
/// compile, was stubbed to a constant 0 (with an Assembly Warning), and
/// every loop score through the agg was identically 0.
///
/// Post-fix this pins: no LTM fragment-compile warnings; the agg→growth
/// link score is exactly 1 past the initial step (the agg is growth's only
/// driver); and every loop score -- all of which route through the agg in
/// this model -- is finite everywhere and sustained non-zero past the
/// startup guard (mirroring
/// `whole_extent_sum_agg_loop_scores_are_finite_and_sustained`).
#[test]
fn broadcast_agg_loop_scores_are_finite_and_sustained() {
    let project = TestProject::new("broadcast_agg")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("matrix[D1,D2]", "100", &["mflow"], &[], None)
        // Broadcast: the A2A body iterates D1 x D2 but the sliced reducer
        // iterates only D1 -> arrayed agg over [D1], broadcast over D2.
        .array_aux("growth[D1,D2]", "SUM(matrix[D1,*]) * 0.01 + 1")
        // Same-element diagonal, closing the per-(d1,d2) loops through the
        // agg (and the cross-D2 petal combinations within each D1 row).
        .array_flow("mflow[D1,D2]", "growth[D1,D2]", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // No LTM synthetic fragment may fail to compile: the pre-fix failure
    // mode was each over-subscripted agg→growth fragment stubbing to a
    // constant 0 with an Assembly "failed to compile" Warning.
    let fragment_failures: Vec<String> = collect_all_diagnostics(&db, sync.project)
        .into_iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(&d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile"))
        })
        .map(|d| d.variable.unwrap_or_default())
        .collect();
    assert!(
        fragment_failures.is_empty(),
        "no LTM synthetic fragment may fail to compile (GH #528: the broadcast agg→target \
         link score over-subscripted the agg); failing fragments: {fragment_failures:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(
        results.step_count > STARTUP_STEPS,
        "the fixture must simulate past the {STARTUP_STEPS}-step startup guard, got {} step(s)",
        results.step_count
    );

    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The agg→growth link score is exactly 1 past step 0: `growth =
    // agg * 0.01 + 1` and the agg is growth's only driver. A constant 0
    // here is the GH #528 stubbed-fragment degradation.
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name = format!("{LINK_SCORE_PREFIX}{agg_name}[{d1}]\u{2192}growth[{d1},{d2}]");
        let s = series_at(&results, offset_of(&results, &name));
        assert_eq!(s[0], 0.0, "{name}: step 0 is pinned to 0 by the TIME guard");
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() <= 1e-9,
                "{name} at step {step}: got {v}, expected exactly 1 (the agg is growth's \
                 only driver; 0 means the fragment was silently stubbed -- GH #528)"
            );
        }
    }

    // Every feedback loop in this model routes through the synthetic agg
    // (`growth` reads `matrix` only via the reducer). Each loop score must
    // be finite at every step and sustained non-zero past the startup guard.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_names.is_empty(),
        "the broadcast-agg feedback model must produce at least one loop score"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm_vars, name);
        assert!(
            var.equation.source_text().contains(agg_name),
            "every loop in this model routes through the synthetic agg; {name} does not: {}",
            var.equation.source_text()
        );
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
            assert!(
                s.iter()
                    .skip(STARTUP_STEPS)
                    .all(|v| v.abs() > MEANINGFUL_SCORE),
                "loop score {name} slot {slot} must be sustained non-zero (> \
                 {MEANINGFUL_SCORE}) past step {STARTUP_STEPS}; an all-zero agg-routed loop \
                 score is the GH #528 degradation. series: {s:?}"
            );
        }
    }
}

// ── GH #744: source→agg per-row link scores must honor the reducer body's
// coefficient on the source ──────────────────────────────────────────────
//
// The SUM/MEAN linear shortcut used to score every co-source row of a
// hoisted reducer as if the body were the bare source element (implicit
// ∂agg/∂source[e] = 1), ignoring the body's coefficient on that source --
// which can be negative. The per-row changed-first partial now evaluates
// the reducer's BODY at the row with the source's reference live and every
// other model reference frozen at PREVIOUS, so the score tracks the true
// per-row partial (sign and magnitude).

/// The GH #744 repro fixture: `grow = 1 + SUM(pop[*] * (1 - weight[*]))`
/// with `weight` fed back from the stock `total` (the same fixture as
/// `arrayed_co_source_feeder_loop_is_balancing`, which pins the STATIC
/// label; these tests pin the runtime series).
fn co_source_weight_fixture() -> datamodel::Project {
    TestProject::new("co_source_744")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_aux("weight[region]", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * (1 - weight[*]))", None)
        .build_datamodel()
}

/// The bilinear feeder fixture: `grow = 1 + SUM(pop[*] * scale)` with the
/// scalar feeder `scale` fed back from `total` (the GH #737 shape) AND the
/// pop rows on a loop through the agg (`pgrow` reads `grow`), so both the
/// per-row changed-first scores and the feeder's changed-last score are
/// emitted. Per-row gains differ so the two rows' dynamics (and scores)
/// are not symmetric.
fn bilinear_feeder_fixture() -> datamodel::Project {
    TestProject::new("bilinear_744")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_with_ranges("gain[region]", vec![("north", "0.04"), ("south", "0.07")])
        .array_flow("pgrow[region]", "pop * gain * 0.001 * grow", None)
        .aux("scale", "0.001 * total + 0.01", None)
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale)", None)
        .build_datamodel()
}

/// Relative-tolerance float comparison for series-derived expectations.
fn assert_close(actual: f64, expected: f64, rel_tol: f64, what: &str) {
    let scale = expected.abs().max(1e-12);
    assert!(
        (actual - expected).abs() <= rel_tol * scale,
        "{what}: got {actual}, expected {expected} (rel tol {rel_tol})"
    );
}

/// GH #744 (i): the `weight[e] → agg` runtime link-score series must track
/// the true per-row partial `∂agg/∂weight[e] = -pop[e] < 0` -- a sustained
/// NEGATIVE series whose value is the changed-first numerator
/// `PREVIOUS(pop[e]) * (PREVIOUS(weight[e]) - weight[e])` normalized by
/// `|Δagg|` and signed by `SIGN(Δweight[e])`. Pre-fix the linear shortcut
/// scored the row as the bare `Δweight[e]` -- a sustained POSITIVE series
/// (wrong sign AND wrong magnitude) that runtime polarity reclassification
/// then rubber-stamped. Post-fix the runtime series AGREES with the static
/// Balancing label pinned by `arrayed_co_source_feeder_loop_is_balancing`.
#[test]
fn co_source_weight_to_agg_link_score_tracks_true_partial() {
    let project = co_source_weight_fixture();
    let (results, _) = run_ltm(&project);
    let agg = series_at(
        &results,
        offset_of(&results, "$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );

    for elem in ["north", "south"] {
        let score_name =
            format!("{LINK_SCORE_PREFIX}weight[{elem}]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0");
        let score = series_at(&results, offset_of(&results, &score_name));
        let pop = series_at(&results, offset_of(&results, &format!("pop[{elem}]")));
        let weight = series_at(&results, offset_of(&results, &format!("weight[{elem}]")));

        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_agg = agg[t] - agg[t - 1];
            let d_w = weight[t] - weight[t - 1];
            if d_agg == 0.0 || d_w == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            // Changed-first per-row partial: pop frozen at PREVIOUS, the
            // other rows cancel against PREVIOUS(agg).
            let numerator = pop[t - 1] * (weight[t - 1] - weight[t]);
            let expected = numerator / d_agg.abs() * d_w.signum();
            assert!(
                expected < 0.0,
                "fixture must exercise the negative-partial case at step {t}"
            );
            assert!(
                score[t] < 0.0,
                "{score_name} at step {t}: the true partial ∂agg/∂weight[e] = -pop[e] < 0, \
                 so the score must be negative; got {} (the pre-fix shortcut emitted a \
                 sustained positive series)",
                score[t]
            );
            assert_close(
                score[t],
                expected,
                1e-9,
                &format!("{score_name} at step {t}"),
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

/// GH #744 (ii): for `SUM(pop[*] * scale)` the per-row `pop[e] → agg`
/// changed-first score must carry the body's coefficient on the row --
/// `Δpop[e] * PREVIOUS(scale)` normalized by `|Δagg|` -- not the bare
/// `Δpop[e]` the shortcut asserted (wrong magnitude whenever `scale != 1`;
/// here `scale` stays far below 1, so the pre-fix value was ~9x too big).
#[test]
fn bilinear_row_to_agg_link_score_reflects_body_coefficient() {
    let project = bilinear_feeder_fixture();
    let (results, _) = run_ltm(&project);
    let agg = series_at(
        &results,
        offset_of(&results, "$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    let scale = series_at(&results, offset_of(&results, "scale"));

    for elem in ["north", "south"] {
        let score_name =
            format!("{LINK_SCORE_PREFIX}pop[{elem}]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0");
        let score = series_at(&results, offset_of(&results, &score_name));
        let pop = series_at(&results, offset_of(&results, &format!("pop[{elem}]")));

        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_agg = agg[t] - agg[t - 1];
            let d_pop = pop[t] - pop[t - 1];
            if d_agg == 0.0 || d_pop == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            // The coefficient must genuinely discriminate from the bare
            // shortcut (which asserted coefficient 1).
            assert!(
                (scale[t - 1] - 1.0).abs() > 0.5,
                "fixture must keep PREVIOUS(scale) far from 1 at step {t}"
            );
            let expected = d_pop * scale[t - 1] / d_agg.abs() * d_pop.signum();
            assert_close(
                score[t],
                expected,
                1e-9,
                &format!("{score_name} at step {t}"),
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

/// GH #744 (iv): the changed-first/changed-last complementarity for a
/// bilinear body (documented on `generate_scalar_feeder_to_agg_equation`)
/// must survive the body-aware per-row partial: the per-row changed-first
/// numerators (`Δpop[e] * PREVIOUS(scale)`) plus the feeder's changed-last
/// numerator (`Σ_e pop[e] * Δscale`) sum exactly to `Δagg`. Numerators are
/// reconstructed from the emitted scores (`numerator = score * |Δagg| *
/// SIGN(Δsource)`), so this pins the additivity of what LTM actually
/// reports.
#[test]
fn bilinear_feeder_plus_row_scores_are_additive() {
    let project = bilinear_feeder_fixture();
    let (results, _) = run_ltm(&project);
    let agg = series_at(
        &results,
        offset_of(&results, "$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    let scale = series_at(&results, offset_of(&results, "scale"));
    let feeder_score = series_at(
        &results,
        offset_of(
            &results,
            &format!("{LINK_SCORE_PREFIX}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
        ),
    );
    let row_data: Vec<(Vec<f64>, Vec<f64>)> = ["north", "south"]
        .iter()
        .map(|elem| {
            let score = series_at(
                &results,
                offset_of(
                    &results,
                    &format!(
                        "{LINK_SCORE_PREFIX}pop[{elem}]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"
                    ),
                ),
            );
            let pop = series_at(&results, offset_of(&results, &format!("pop[{elem}]")));
            (score, pop)
        })
        .collect();

    let mut contributing = 0usize;
    for t in 1..agg.len() {
        let d_agg = agg[t] - agg[t - 1];
        let d_scale = scale[t] - scale[t - 1];
        if d_agg == 0.0 || d_scale == 0.0 {
            continue;
        }
        let mut sum = feeder_score[t] * d_agg.abs() * d_scale.signum();
        let mut all_rows_scored = true;
        for (score, pop) in &row_data {
            let d_pop = pop[t] - pop[t - 1];
            if d_pop == 0.0 {
                all_rows_scored = false;
                break;
            }
            sum += score[t] * d_agg.abs() * d_pop.signum();
        }
        if !all_rows_scored {
            continue;
        }
        assert_close(
            sum,
            d_agg,
            1e-9,
            &format!("sum of source-row + feeder numerators at step {t}"),
        );
        contributing += 1;
    }
    assert!(
        contributing >= 3,
        "expected at least 3 steps where every source scored, got {contributing}"
    );
}

/// GH #744 (iii)/(v): shapes the fix must NOT change, pinned byte-for-byte
/// against the pre-fix emission (recorded at `ltm-fix-batch-2` HEAD
/// `9517d77e` before the change):
/// - a BARE reducer body (`SUM(pop[*])`) keeps the legacy linear-shortcut
///   equation text exactly;
/// - the scalar feeder's changed-last equation
///   (`generate_scalar_feeder_to_agg_equation`) is untouched by the
///   body-aware per-row partial.
#[test]
fn bare_body_and_feeder_agg_equations_unchanged() {
    // Bare body: `grow = 1 + SUM(pop[*])`, pop rows on a loop through the
    // agg (pgrow reads total, total integrates grow).
    let bare = TestProject::new("bare_744")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05 * total * 0.001", None)
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*])", None)
        .build_datamodel();
    let (_, ltm_vars) = run_ltm(&bare);
    let bare_score = ltm_var(
        &ltm_vars,
        &format!("{LINK_SCORE_PREFIX}pop[north]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    assert_eq!(
        bare_score.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")) = 0) OR ((pop[region\u{B7}north] - \
         PREVIOUS(pop[region\u{B7}north])) = 0) then 0 else \
         SAFEDIV((PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\") + (pop[region\u{B7}north] - \
         PREVIOUS(pop[region\u{B7}north])) - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")), \
         ABS((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"))), 0) * \
         SIGN((pop[region\u{B7}north] - PREVIOUS(pop[region\u{B7}north])))",
        "the bare-body linear shortcut must stay byte-identical"
    );

    // Scalar feeder: changed-last equation untouched.
    let (_, ltm_vars) = run_ltm(&bilinear_feeder_fixture());
    let feeder_score = ltm_var(
        &ltm_vars,
        &format!("{LINK_SCORE_PREFIX}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    assert_eq!(
        feeder_score.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")) = 0) OR ((scale - PREVIOUS(scale)) = 0) \
         then 0 else SAFEDIV((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - (sum(pop[*] * \
         PREVIOUS(scale)))), ABS((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"))), 0) * SIGN((scale - PREVIOUS(scale)))",
        "the scalar feeder's changed-last equation must stay byte-identical"
    );
}

/// GH #744: the same body-coefficient defect existed on the
/// VARIABLE-BACKED reducer path (`try_cross_dimensional_link_scores`):
/// a whole-RHS `tp = SUM(pop[*] * (1 - weight[*]))` is not hoisted into a
/// synthetic agg (the variable is its own agg), but `classify_reducer`'s
/// `is_bare` only describes arithmetic AROUND the reducer, so the linear
/// shortcut scored `weight[e] → tp` as bare `Δweight[e]` (sustained
/// positive). The body-aware partial fixes this site identically.
#[test]
fn variable_backed_co_source_link_score_tracks_true_partial() {
    let project = TestProject::new("var_backed_744")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_aux("weight[region]", "0.001 * total + 0.01")
        .aux("tp", "SUM(pop[*] * (1 - weight[*]))", None)
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "tp * 0.05", None)
        .build_datamodel();
    let (results, _) = run_ltm(&project);
    let tp = series_at(&results, offset_of(&results, "tp"));

    for elem in ["north", "south"] {
        let score_name = format!("{LINK_SCORE_PREFIX}weight[{elem}]\u{2192}tp");
        let score = series_at(&results, offset_of(&results, &score_name));
        let pop = series_at(&results, offset_of(&results, &format!("pop[{elem}]")));
        let weight = series_at(&results, offset_of(&results, &format!("weight[{elem}]")));

        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_tp = tp[t] - tp[t - 1];
            let d_w = weight[t] - weight[t - 1];
            if d_tp == 0.0 || d_w == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            let numerator = pop[t - 1] * (weight[t - 1] - weight[t]);
            let expected = numerator / d_tp.abs() * d_w.signum();
            assert!(
                score[t] < 0.0,
                "{score_name} at step {t}: must be negative, got {}",
                score[t]
            );
            assert_close(
                score[t],
                expected,
                1e-9,
                &format!("{score_name} at step {t}"),
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

/// GH #744 review I1 (end-to-end): `tp = SUM(pop[*] * pop[north])` -- a
/// fixed-literal self-reference inside the reducer body. The north row's
/// body partial would drop the other rows' `pop[i] * pop[north]`
/// cross-terms (they reference the live element, so they do NOT cancel
/// against PREVIOUS(tp)), emitting a confidently-wrong score (0.5 vs the
/// true changed-first 0.7497 here, and sign-flippable for mixed-sign
/// sources). Both rows must consistently use the delta-ratio fallback
/// (score = SIGN(Δpop[e]) when the guards pass).
#[test]
fn fixed_literal_self_reference_rows_fall_back_consistently() {
    let project = TestProject::new("self_ref_744")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_with_ranges("gain[region]", vec![("north", "0.003"), ("south", "0.005")])
        .array_flow("pgrow[region]", "pop * gain * 0.0001 * tp", None)
        .aux("tp", "SUM(pop[*] * pop[north])", None)
        .build_datamodel();
    let (results, ltm_vars) = run_ltm(&project);
    let tp = series_at(&results, offset_of(&results, "tp"));

    for elem in ["north", "south"] {
        let score_name = format!("{LINK_SCORE_PREFIX}pop[{elem}]\u{2192}tp");
        let var = ltm_var(&ltm_vars, &score_name);
        let text = var.equation.source_text();
        assert!(
            text.contains("SAFEDIV((tp - PREVIOUS(tp))"),
            "{score_name} must use the delta-ratio fallback; got: {text}"
        );
        assert!(
            !text.contains("PREVIOUS(tp) + "),
            "{score_name} must not carry a body/shortcut partial; got: {text}"
        );

        let score = series_at(&results, offset_of(&results, &score_name));
        let pop = series_at(&results, offset_of(&results, &format!("pop[{elem}]")));
        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_tp = tp[t] - tp[t - 1];
            let d_pop = pop[t] - pop[t - 1];
            if d_tp == 0.0 || d_pop == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            // Delta-ratio degenerates to SIGN(Δsource); pop grows here.
            assert!(
                (score[t] - 1.0).abs() <= 1e-9,
                "{score_name} at step {t}: delta-ratio fallback must read \
                 SIGN(Δpop) = 1; got {} (0.5-flavored values are the dropped \
                 cross-term defect)",
                score[t]
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

// ── GH #762: nonlinear (MIN/MAX/STDDEV) per-row partials must honor the
// reducer body ───────────────────────────────────────────────────────────
//
// The sibling of GH #744: `generate_nonlinear_partial` substituted the
// BARE source elements into the MIN/MAX nested-binary and STDDEV
// unrolled-variance shapes, ignoring arithmetic inside the reducer
// argument -- `MIN(pop[*] * scale)` compared raw `pop` terms against a
// scaled aggregate, producing garbage scores. Each per-row term is now
// the row-pinned BODY (live at the scored row, fully frozen elsewhere).

/// Shared GH #762 fixture: a nonlinear reducer over `pop[*] * scale` with
/// the rows on a loop through the agg and `scale` fed back from `total`.
fn nonlinear_body_fixture(grow_eqn: &str) -> datamodel::Project {
    TestProject::new("nonlinear_762")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_with_ranges("gain[region]", vec![("north", "0.04"), ("south", "0.07")])
        .array_flow("pgrow[region]", "pop * gain * 0.001 * grow", None)
        .aux("scale", "0.001 * total + 0.01", None)
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", grow_eqn, None)
        .build_datamodel()
}

/// Per-step expected-vs-actual assertion for the GH #762 value tests: the
/// numerator terms are O(10), so float noise is absolute (~1e-12); use a
/// relative tolerance with an absolute floor of 1.0 so an expected score
/// of exactly 0 (a frozen-argmin row) still admits arithmetic noise.
fn assert_score_close(actual: f64, expected: f64, what: &str) {
    let tol = 1e-9 * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tol,
        "{what}: got {actual}, expected {expected} (tol {tol})"
    );
}

/// GH #762 (MIN): the per-row changed-first partial for
/// `grow = 1 + MIN(pop[*] * scale)` w.r.t. `pop[e]` is
/// `MIN(pop[e] * PREVIOUS(scale), PREVIOUS(pop[o]) * PREVIOUS(scale))`
/// anchored against `PREVIOUS(agg)` -- the scored row's body term live,
/// every other row's body term fully frozen. Pre-fix the terms were the
/// bare `pop` elements (raw units vs the scaled agg): scores ~73 where
/// the truth is ~0.003 (north) and exactly 0 (south, the frozen-argmin
/// row whose change never moves the MIN).
#[test]
fn min_body_coefficient_row_scores_track_true_partial() {
    let project = nonlinear_body_fixture("1 + MIN(pop[*] * scale)");
    let (results, _) = run_ltm(&project);
    let agg = series_at(
        &results,
        offset_of(&results, "$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    let scale = series_at(&results, offset_of(&results, "scale"));
    let pop_n = series_at(&results, offset_of(&results, "pop[north]"));
    let pop_s = series_at(&results, offset_of(&results, "pop[south]"));

    for (elem, pop_live, pop_other) in [("north", &pop_n, &pop_s), ("south", &pop_s, &pop_n)] {
        let score_name =
            format!("{LINK_SCORE_PREFIX}pop[{elem}]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0");
        let score = series_at(&results, offset_of(&results, &score_name));
        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_agg = agg[t] - agg[t - 1];
            let d_pop = pop_live[t] - pop_live[t - 1];
            if d_agg == 0.0 || d_pop == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            let term_live = pop_live[t] * scale[t - 1];
            let term_other = pop_other[t - 1] * scale[t - 1];
            let partial = term_live.min(term_other);
            let expected = (partial - agg[t - 1]) / d_agg.abs() * d_pop.signum();
            assert_score_close(score[t], expected, &format!("{score_name} at step {t}"));
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

/// GH #762 (STDDEV): the per-row changed-first partial for
/// `grow = 1 + STDDEV(pop[*] * scale)` uses the same row-pinned body
/// terms inside the unrolled population-variance form (divisor N,
/// inlined mean -- the GH #483 shape). Expected values mirror the emitted
/// sqrt-variance arithmetic so float agreement is tight.
#[test]
fn stddev_body_coefficient_row_scores_track_true_partial() {
    let project = nonlinear_body_fixture("1 + STDDEV(pop[*] * scale)");
    let (results, _) = run_ltm(&project);
    let agg = series_at(
        &results,
        offset_of(&results, "$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    let scale = series_at(&results, offset_of(&results, "scale"));
    let pop_n = series_at(&results, offset_of(&results, "pop[north]"));
    let pop_s = series_at(&results, offset_of(&results, "pop[south]"));

    for (elem, pop_live, pop_other) in [("north", &pop_n, &pop_s), ("south", &pop_s, &pop_n)] {
        let score_name =
            format!("{LINK_SCORE_PREFIX}pop[{elem}]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0");
        let score = series_at(&results, offset_of(&results, &score_name));
        let mut contributing = 0usize;
        for t in 1..score.len() {
            let d_agg = agg[t] - agg[t - 1];
            let d_pop = pop_live[t] - pop_live[t - 1];
            if d_agg == 0.0 || d_pop == 0.0 {
                assert_eq!(score[t], 0.0, "{score_name} at step {t}: guard must zero");
                continue;
            }
            let term_live = pop_live[t] * scale[t - 1];
            let term_other = pop_other[t - 1] * scale[t - 1];
            let mean = (term_live + term_other) / 2.0;
            let partial = (((term_live - mean).powi(2) + (term_other - mean).powi(2)) / 2.0).sqrt();
            let expected = (partial - agg[t - 1]) / d_agg.abs() * d_pop.signum();
            assert_score_close(score[t], expected, &format!("{score_name} at step {t}"));
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "{score_name}: expected at least 3 scored steps, got {contributing}"
        );
    }
}

/// GH #762 (bare bodies unchanged): `MIN(pop[*])` / `STDDEV(pop[*])` keep
/// the legacy analytic partials byte-for-byte, pinned against the
/// pre-fix emission recorded at `ec72e190` before the change.
#[test]
fn nonlinear_bare_body_equations_unchanged() {
    let (_, ltm_vars) = run_ltm(&nonlinear_body_fixture("1 + MIN(pop[*])"));
    let min_score = ltm_var(
        &ltm_vars,
        &format!("{LINK_SCORE_PREFIX}pop[north]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    assert_eq!(
        min_score.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")) = 0) OR ((pop[region\u{B7}north] - \
         PREVIOUS(pop[region\u{B7}north])) = 0) then 0 else \
         SAFEDIV((MIN(pop[region\u{B7}north], PREVIOUS(pop[region\u{B7}south])) - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")), \
         ABS((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"))), 0) * \
         SIGN((pop[region\u{B7}north] - PREVIOUS(pop[region\u{B7}north])))",
        "the bare-body MIN partial must stay byte-identical"
    );

    let (_, ltm_vars) = run_ltm(&nonlinear_body_fixture("1 + STDDEV(pop[*])"));
    let stddev_score = ltm_var(
        &ltm_vars,
        &format!("{LINK_SCORE_PREFIX}pop[north]\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
    );
    assert_eq!(
        stddev_score.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")) = 0) OR ((pop[region\u{B7}north] - \
         PREVIOUS(pop[region\u{B7}north])) = 0) then 0 else \
         SAFEDIV((sqrt((((pop[region\u{B7}north] - ((pop[region\u{B7}north] + \
         PREVIOUS(pop[region\u{B7}south])) / 2))^2) + ((PREVIOUS(pop[region\u{B7}south]) - \
         ((pop[region\u{B7}north] + PREVIOUS(pop[region\u{B7}south])) / 2))^2)) / 2) - \
         PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\")), \
         ABS((\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"))), 0) * \
         SIGN((pop[region\u{B7}north] - PREVIOUS(pop[region\u{B7}north])))",
        "the bare-body STDDEV partial must stay byte-identical"
    );
}

// ---------------------------------------------------------------------------
// GH #743: un-hoisted multi-source iterated-dim-feeder reducer
// ---------------------------------------------------------------------------

/// The GH #743 fixture: an apply-to-all equation over `D1` whose RHS embeds
/// a multi-source reducer whose per-row feeder is arrayed over the ITERATED
/// dimension -- `growth[D1] = SUM(matrix[D1,*] * frac[D1])`.
/// `combined_read_slice` declines to hoist it (the multi-source
/// slice-disagreement carve-out: `matrix[D1,*]` reads `[Iterated, Reduced]`
/// while `frac[D1]` reads `[Iterated]`), so the references stay on the
/// conservative path. The feedback loop closes through `frac`:
/// `pop[r] -> frac[r] -> growth[r] -> pop[r]`, trivially reinforcing.
fn gh743_feeder_closure_fixture() -> datamodel::Project {
    TestProject::new("gh743_feeder")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * frac[D1])", None)
        .build_datamodel()
}

/// GH #743 (the silent-garbage half): the feeder-closure loop must be
/// CORRECTLY scored on the conservative (un-hoisted) path.
///
/// Before the fix, the Bare `frac→growth` link score's changed-first
/// ceteris-paribus partial froze the wildcard-sliced co-source as
/// `PREVIOUS(matrix[PREVIOUS(d1), *])`. `PREVIOUS` of an array slice has no
/// codegen path: as a *user* equation it is a hard compile error, but as an
/// LTM implicit helper (`$⁚$⁚ltm⁚link_score⁚frac→growth⁚0⁚arg0`) it failed
/// fragment-compile SILENTLY -- keeping a layout slot with no bytecode, so
/// it read a constant 0. The partial then evaluated to
/// `sum(0 * frac) = 0` and the score degenerated to
/// `-PREVIOUS(growth)/|Δgrowth| = -1/g` (g = the per-step growth rate):
/// constant `-20` on this fixture, the issue's `-250` at 0.4%/step --
/// plausible-looking garbage with NO diagnostic.
///
/// After the fix the partial uses the changed-last attribution (only the
/// feeder frozen: `sum(matrix[d1, *] * PREVIOUS(frac))`, which compiles),
/// and the trivially-reinforcing isolated loop scores exactly +1 per
/// element -- the LTM isolated-loop invariant.
#[test]
fn un_hoisted_iterated_dim_feeder_loop_scores_correct() {
    let project = gh743_feeder_closure_fixture();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The reducer must STAY un-hoisted (no synthetic agg node): this test
    // pins the conservative-path fix, not a hoisting change. (GH #743's
    // direction-1 follow-up -- extending the hoist to feeder-sub-slice
    // combinations -- would relax this.)
    assert!(
        ltm_vars
            .iter()
            .all(|v| !v.name.contains("$\u{205A}ltm\u{205A}agg\u{205A}")),
        "the slice-disagreeing multi-source reducer must not be hoisted; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // The conservative path must be CORRECT here, so there must be no
    // degradation warnings (a warned skip would mean the loud floor fired
    // where the changed-last form should have produced a real score).
    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the feeder-closure fixture must compile every LTM fragment cleanly; got: {assembly:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Exactly one loop: pop -> frac -> growth -> pop, A2A over D1.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        1,
        "expected exactly one loop score; got {loop_names:?}"
    );
    let loop_var = ltm_var(&ltm_vars, &loop_names[0]);
    assert_eq!(
        loop_var.dimensions,
        vec!["D1".to_string()],
        "the feeder-closure loop must be A2A over D1"
    );

    const TOL: f64 = 1e-9;

    // The Bare frac→growth link score: +1 at every step past the first
    // (the changed-last numerator `growth - sum(matrix[d1,*]*PREVIOUS(frac))`
    // equals Δgrowth exactly while matrix is constant).
    let frac_growth = format!("{LINK_SCORE_PREFIX}frac\u{2192}growth");
    let var = ltm_var(&ltm_vars, &frac_growth);
    let n_slots = slot_count(var, &project.dimensions);
    let base = offset_of(&results, &frac_growth);
    for slot in 0..n_slots {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "{frac_growth} slot {slot} at step {step}: got {v}, expected +1. \
                 A constant negative value here (-1/growth-rate) is the GH #743 \
                 silent-garbage signature: the partial silently lost the frozen \
                 co-source term."
            );
        }
    }

    // The loop score: +1 per element at every step past startup (the
    // trivially-reinforcing isolated-loop invariant).
    let base = offset_of(&results, &loop_names[0]);
    for slot in 0..2 {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "{} slot {slot} at step {step}: got {v}, expected +1 (isolated \
                 reinforcing loop). The pre-fix garbage read -20 at every step.",
                loop_names[0]
            );
        }
    }
}

/// GH #743 (the loud-floor half, characterization): closing the loop through
/// the wildcard-read co-source (`matrix`) instead of the feeder keeps the
/// LOUD degraded behavior -- cross-element loop scores that fail fragment
/// compile (their equations reference per-(row,slot) link-score names the
/// emitters never produce for this un-hoisted shape), each surfacing an
/// Assembly `Warning` and reading a constant 0.
///
/// This is the warned sibling of the #758/#764 zero-stub class, NOT silent
/// garbage; making these loops genuinely scoreable requires hoisting the
/// feeder-sub-slice combination (GH #743's direction-1 follow-up). This test
/// pins that the degradation stays visible: if the warnings disappear, the
/// scores must be real (a silent zero here would be a regression).
#[test]
fn un_hoisted_iterated_dim_feeder_co_source_closure_stays_loud() {
    let project = TestProject::new("gh743_co_source")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "pop[D1] * 0.05",
            None,
        )
        .array_aux("frac[D1]", "0.5")
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * frac[D1])", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Every loop score through the un-hoisted matrix→growth edge fails
    // fragment compile and is WARNED -- the loud conservative floor.
    let diags = collect_all_diagnostics(&db, sync.project);
    let warned_loop_scores: Vec<&str> = diags
        .iter()
        .filter(|d| {
            matches!(d.severity, DiagnosticSeverity::Warning)
                && matches!(d.error, DiagnosticError::Assembly(_))
        })
        .filter_map(|d| d.variable.as_deref())
        .filter(|v| v.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !warned_loop_scores.is_empty(),
        "the co-source closure's unscoreable loops must surface Assembly warnings \
         (silent zero would be a regression); diagnostics: {diags:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Each warned loop score is a stub: constant 0 at every step.
    for name in &warned_loop_scores {
        let base = offset_of(&results, name);
        let series = series_at(&results, base);
        assert!(
            series.iter().all(|&v| v == 0.0),
            "warned loop score {name} must read the documented 0 stub; got {series:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GH #758: declined element-mapped sliced reducer -> loud unscoreable edge
// ---------------------------------------------------------------------------

/// The GH #758 fixture: an inline sliced reducer over an ELEMENT-mapped
/// dimension pair -- `growth[State] = 1 + SUM(matrix[State,*])` where
/// `matrix` is declared over `Region` and `State` carries an explicit
/// element map to `Region` (not a positional correspondence).
/// `mapped_element_correspondence` declines it (the GH #756
/// positional-only gate), so the reducer is NOT hoisted and the
/// `matrix → growth` reference stays on the conservative path. The
/// feedback loops close through `pop → matrix → growth → SUM(growth[*])
/// → inflow → pop`, so every enumerated loop traverses the declined edge.
///
/// With `with_drain`, a second, independent loop `pop → drain → pop`
/// (A2A over Region, not traversing the declined edge) is added so tests
/// can pin that the degradation is surgical.
fn gh758_element_mapped_fixture(with_drain: bool) -> datamodel::Project {
    let mut p = TestProject::new("gh758_element_mapped")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["west", "east"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_element_mapping(
            "State",
            &["CA", "NY"],
            "Region",
            &[("CA", "east"), ("NY", "west")],
        )
        .array_aux_direct(
            "matrix",
            vec!["Region".into(), "D2".into()],
            "pop[Region] * 0.05",
            None,
        )
        .array_aux("growth[State]", "1 + SUM(matrix[State, *])")
        .array_flow("inflow[Region]", "SUM(growth[*]) * 0.01", None);
    if with_drain {
        p = p
            .array_stock("pop[Region]", "100", &["inflow"], &["drain"], None)
            .array_flow("drain[Region]", "pop[Region] * 0.01", None);
    } else {
        p = p.array_stock("pop[Region]", "100", &["inflow"], &[], None);
    }
    p.build_datamodel()
}

/// The Assembly warnings of a compiled project's diagnostics.
fn assembly_warnings(
    db: &SimlinDb,
    project: simlin_engine::db::SourceProject,
) -> Vec<simlin_engine::db::Diagnostic> {
    collect_all_diagnostics(db, project)
        .into_iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(d.error, DiagnosticError::Assembly(_))
        })
        .collect()
}

/// GH #758: the declined element-mapped sliced-reducer edge must degrade
/// LOUDLY -- one Warning naming the edge, NO link-score variable, and NO
/// loop scores through it -- instead of emitting a broken-by-construction
/// scalar link score (a scalar equation referencing the arrayed `matrix` /
/// `growth` idents) that failed fragment compile and dragged every loop
/// score through the edge into a warned 0-stub (17 Assembly warnings on
/// this fixture before the fix).
#[test]
fn declined_element_mapped_reducer_edge_skips_loudly() {
    let project = gh758_element_mapped_fixture(false);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // The declined edge must not mint the uncompilable scalar link score.
    let doomed = format!("{LINK_SCORE_PREFIX}matrix\u{2192}growth");
    assert!(
        !ltm.vars.iter().any(|v| v.name == doomed),
        "the declined element-mapped edge must NOT emit the uncompilable scalar \
         link score {doomed:?}; got vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // Every enumerated loop traverses the unscoreable edge, so no loop
    // scores are emitted at all (and the partition map is empty).
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the unscoreable edge must not emit loop scores; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    assert!(
        ltm.loop_partitions.is_empty(),
        "no scored loops => no loop partitions; got: {:?}",
        ltm.loop_partitions.keys().collect::<Vec<_>>()
    );

    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Exactly ONE Assembly warning: the unscoreable-edge diagnostic naming
    // both endpoints. (Before the fix: 17 -- the link score's
    // fragment-compile failure plus one per stubbed loop score.)
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable edge); got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("matrix") && msg.contains("growth"),
        "the warning must name the unscoreable edge's endpoints; got: {msg}"
    );

    // The model still simulates, and every emitted score series is finite
    // (no garbage values anywhere).
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let score_names = ltm_score_var_names(&results);
    assert!(
        score_names
            .iter()
            .all(|n| !n.starts_with(LOOP_SCORE_PREFIX)),
        "no loop-score series should exist in the results; got: {score_names:?}"
    );
    for name in &score_names {
        let var = ltm_var(&ltm.vars, name);
        let base = offset_of(&results, name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

/// GH #758 (surgical degradation): a second feedback loop that does NOT
/// traverse the unscoreable edge keeps its real loop score while the
/// doomed loops are dropped -- the skip is per-loop, not a blanket
/// collapse of LTM output.
#[test]
fn declined_element_mapped_reducer_keeps_unaffected_loops() {
    let project = gh758_element_mapped_fixture(true);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Still exactly one warning: the unscoreable edge.
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable edge); got: {warnings:?}"
    );

    // Exactly one loop score survives: the pop -> drain -> pop loop, A2A
    // over Region (it never touches matrix -> growth).
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_vars.len(),
        1,
        "exactly the drain loop must keep its score; got: {:?}",
        loop_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        loop_vars[0].dimensions,
        vec!["Region".to_string()],
        "the surviving loop is the A2A pop/drain loop"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // The surviving loop score is real: finite everywhere, non-zero past
    // startup.
    let base = offset_of(&results, &loop_vars[0].name);
    for slot in 0..slot_count(loop_vars[0], &project.dimensions) {
        let series = series_at(&results, base + slot);
        assert!(
            series.iter().all(|v| v.is_finite()),
            "surviving loop score slot {slot} must be finite; got {series:?}"
        );
        assert!(
            series.iter().skip(STARTUP_STEPS + 1).any(|&v| v != 0.0),
            "surviving loop score slot {slot} must carry real values; got {series:?}"
        );
    }
}

/// GH #758 regression guard for the POSITIONAL twin: the same model shape
/// with a positional `State -> Region` mapping is hoisted per GH #534
/// (never reaches the conservative path), compiles every LTM fragment
/// cleanly, and scores its loops end-to-end -- the new unscoreable-edge
/// gate must not fire for it.
#[test]
fn positional_mapped_twin_of_declined_edge_scores_cleanly() {
    let project = TestProject::new("gh758_positional_twin")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["west", "east"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["CA", "NY"], "Region")
        .array_stock("pop[Region]", "100", &["inflow"], &[], None)
        .array_aux_direct(
            "matrix",
            vec!["Region".into(), "D2".into()],
            "pop[Region] * 0.05",
            None,
        )
        .array_aux("growth[State]", "1 + SUM(matrix[State, *])")
        .array_flow("inflow[Region]", "SUM(growth[*]) * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The positionally-mapped sliced reducer is hoisted: no conservative
    // matrix->growth score and no unscoreable-edge warning.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the positional twin must compile every LTM fragment cleanly; got: {warnings:?}"
    );

    // Loops through the hoisted reducer ARE scored.
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_vars.is_empty(),
        "the positional twin's loops must be scored; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let mut saw_nonzero = false;
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "loop score {} slot {slot} must be finite; got {series:?}",
                lv.name
            );
            if series.iter().skip(STARTUP_STEPS + 1).any(|&v| v != 0.0) {
                saw_nonzero = true;
            }
        }
    }
    assert!(
        saw_nonzero,
        "at least one positional-twin loop score must carry real non-zero values"
    );
}

// ---------------------------------------------------------------------------
// GH #759: dimension-name subscript indices must not be PREVIOUS-wrapped
// (these fixtures previously pinned the GH #741 implicit-helper warnings the
// pre-fix doomed helpers produced; the #741 diagnostic pass itself stays
// covered by the guard-injected
// `test_model_ltm_fragment_diagnostics_covers_implicit_helpers`)
// ---------------------------------------------------------------------------

/// The GH #759 pinned-index fixture: `growth[D1] = matrix[D1, c1] * frac[D1]`
/// -- a mundane Bare-shape apply-to-all equation with a pinned literal
/// co-source index -- inside the feedback loop `pop -> frac -> growth ->
/// grow -> pop`. Before the GH #759 fix, the ceteris-paribus partial for
/// the `frac -> growth` link score froze the co-source as
/// `PREVIOUS(matrix[PREVIOUS(d1), d2·c1])`, PREVIOUS-wrapping the
/// iterated-dimension NAME inside the subscript index; the PREVIOUS-capture
/// helpers minted for that expression
/// (`$⁚$⁚ltm⁚link_score⁚frac→growth⁚0⁚arg0⁚{r1,r2}`) failed to compile and
/// the score read a constant -40 off the 0-stubbed helpers. Post-fix the
/// dimension name stays verbatim (`PREVIOUS(matrix[d1, d2·c1])`), every
/// fragment compiles, and the score reads its true value.
fn gh759_pinned_index_fixture() -> datamodel::Project {
    TestProject::new("gh759_pinned_index")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("growth[D1]", "matrix[D1, c1] * frac[D1]")
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("grow[D1]", "growth[D1]", None)
        .array_stock("pop[D1]", "100", &["grow"], &[], None)
        .build_datamodel()
}

/// GH #759: the Bare-shape pinned-index partial must compile cleanly and
/// score its true value. With `matrix` constant, `growth = matrix[D1,c1] *
/// frac` is linear in `frac` with a frozen coefficient, so the changed-first
/// partial reproduces Δgrowth exactly and the `frac -> growth` link score is
/// `+1` from the first post-initial step; the whole reinforcing loop scores
/// `+1` once its flow-to-stock link's two-step startup guard clears.
///
/// Before the fix the partial froze the co-source as
/// `PREVIOUS(matrix[PREVIOUS(d1), d2·c1])` -- the iterated-dim NAME wrapped
/// inside the subscript index -- so the PREVIOUS-capture helpers failed to
/// compile (warned per GH #741, stubbed to 0) and the score read a constant
/// -40. The guard-injected
/// `test_model_ltm_fragment_diagnostics_covers_implicit_helpers` (in
/// `ltm_unified_tests.rs`) keeps the #741 diagnostic pass covered now that
/// this fixture no longer fails.
#[test]
fn pinned_index_partial_compiles_and_scores_correctly() {
    let project = gh759_pinned_index_fixture();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Every LTM fragment (helpers included) compiles: no degradation warnings.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the pinned-index shape must compile every LTM fragment cleanly; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // `frac -> growth` is exact: growth is linear in frac with the (frozen,
    // constant) coefficient matrix[D1,c1], so the score is +1 per element
    // from the first post-initial step.
    let frac_growth = ltm
        .vars
        .iter()
        .find(|v| v.name == format!("{LINK_SCORE_PREFIX}frac\u{2192}growth"))
        .expect("frac->growth link score must be emitted");
    let base = offset_of(&results, &frac_growth.name);
    for slot in 0..slot_count(frac_growth, &project.dimensions) {
        let series = series_at(&results, base + slot);
        assert_eq!(series[0], 0.0, "initial-step guard pins slot {slot} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "frac->growth slot {slot} step {step} must score +1; got {series:?}"
            );
        }
    }

    // The reinforcing loop scores +1 per element once the flow-to-stock
    // link's two-step startup guard clears.
    let loop_var = ltm
        .vars
        .iter()
        .find(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .expect("the pop/frac/growth/grow loop must be scored");
    let base = offset_of(&results, &loop_var.name);
    for slot in 0..slot_count(loop_var, &project.dimensions) {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                (v - 1.0).abs() < 1e-6,
                "loop score slot {slot} step {step} must be +1; got {series:?}"
            );
        }
    }
}

/// GH #748 x #741 x #759 joint end-to-end: a module-only root whose
/// sub-model contains the GH #759 pinned-index shape.
///
/// * #748: `main`'s only state lives inside the `sub` module (`level` and
///   `pop` stocks), so before the module-state-aware early-return gate the
///   root's LTM pass emitted NOTHING -- the `driver -> sub -> reader ->
///   driver` loop went unscored with no diagnostic.
/// * #741/#759: `sub` has input ports, so its own LTM pass scores ALL its
///   edges -- including `frac -> growth` and `matrix -> growth`, whose
///   PREVIOUS-capture helpers were doomed by the #759 dimension-name wrap
///   (six helpers: two for `frac -> growth`, four for `matrix -> growth`,
///   plus the `matrix -> growth` synthetic score itself). #741 made those
///   failures warn; #759's fix makes them compile, so this fixture must now
///   be warning-free while the root loop still scores.
#[test]
fn module_only_root_with_pinned_index_sub_scores_cleanly() {
    let mut project = TestProject::new("joint_748_741")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .aux("driver", "100 + reader * 0.5", None)
        .aux("reader", "sub.level", None)
        .build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "sub".to_string(),
            model_name: "sub".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "driver".to_string(),
                dst: "sub.input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    // The sub-model: a scalar smooth-like input->chg->level chain (the state
    // the parent loop traverses) plus the GH #759 doomed shape, disjoint
    // from the chain. Built via a second TestProject purely for the
    // variable-construction helpers; dimensions stay on the real project.
    let sub_body = TestProject::new("sub_body")
        .aux("input", "0", None)
        .flow("chg", "(input - level) / 3", None)
        .stock("level", "0", &["chg"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("growth[D1]", "matrix[D1, c1] * frac[D1]")
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("grow[D1]", "growth[D1]", None)
        .array_stock("pop[D1]", "100", &["grow"], &[], None)
        .build_datamodel();
    let mut sub_model = sub_body.models.into_iter().next().expect("one model");
    sub_model.name = "sub".to_string();
    project.models.push(sub_model);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // #748 leg: the module-only root runs the pass and scores its loop.
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "the module-only root must score the driver/sub/reader loop; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // #759 leg: with dimension-name subscript indices no longer wrapped,
    // every sub-model fragment -- the six previously-doomed PREVIOUS-capture
    // helpers AND the `matrix -> growth` synthetic score -- compiles, so
    // nothing warns. (#741's diagnostic pass stays covered by the
    // guard-injected `test_model_ltm_fragment_diagnostics_covers_implicit_helpers`.)
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the pinned-index sub-model must compile every LTM fragment cleanly; got: {warnings:?}"
    );

    // End to end: the model simulates and the root loop score carries real
    // (finite, eventually non-zero) values.
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let root_loop = ltm
        .vars
        .iter()
        .find(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .unwrap();
    let series = series_at(&results, offset_of(&results, &root_loop.name));
    assert!(
        series.iter().all(|v| v.is_finite()),
        "root loop score must stay finite; got {series:?}"
    );
    assert!(
        series.iter().any(|&v| v != 0.0),
        "root loop score must carry signal once behavior begins; got {series:?}"
    );
}

// ---------------------------------------------------------------------------
// GH #742: PREVIOUS-captured array-valued non-reducing builtins (RANK)
// ---------------------------------------------------------------------------

/// GH #742, engine leg: `PREVIOUS(RANK(pop, 1))` in an apply-to-all equation
/// must compile and read the per-element rank of the *lagged* array.
///
/// `RANK(arr, dir)` is array-valued (the rank of each element -- Vensim's
/// VECTOR RANK), but `builtins_visitor::arg_has_bare_var_ref` treated every
/// `reducer_kind_from_name` builtin as scalar-collapsing and refused to
/// descend, so the PREVIOUS capture landed in a per-element SCALAR helper
/// whose equation `rank(pop, 1)` is ill-typed (array-valued in scalar
/// context) and the model failed to compile. Treating RANK as
/// array-valued routes the capture through the GH #541 ARRAYED helper
/// (`Equation::ApplyToAll` over the active dims, referenced at the active
/// element), which compiles exactly like the model's own A2A equation.
#[test]
fn previous_of_rank_compiles_per_element() {
    let project = TestProject::new("prev_rank")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .array_with_ranges("seed[Region]", vec![("north", "100"), ("south", "200")])
        .array_stock("pop[Region]", "seed[Region]", &["inflow"], &[], None)
        .array_flow("inflow[Region]", "pop[Region] * 0.01", None)
        .array_aux("prev_rank[Region]", "PREVIOUS(RANK(pop, 1))")
        .build_datamodel();

    let results = run_plain_sim(&project);
    let north = series_at(&results, offset_of(&results, "prev_rank[north]"));
    let south = series_at(&results, offset_of(&results, "prev_rank[south]"));
    // PREVIOUS(x) defaults to 0 at the initial step; pop[north] < pop[south]
    // throughout, so the lagged ascending ranks are a constant [1, 2].
    assert_eq!(north[0], 0.0, "PREVIOUS defaults to 0 at t=0");
    assert_eq!(south[0], 0.0, "PREVIOUS defaults to 0 at t=0");
    assert!(
        north.iter().skip(1).all(|&v| v == 1.0),
        "prev_rank[north] must read the lagged rank 1; got {north:?}"
    );
    assert!(
        south.iter().skip(1).all(|&v| v == 2.0),
        "prev_rank[south] must read the lagged rank 2; got {south:?}"
    );
}

/// GH #742, LTM leg: a link-score partial that freezes an array-valued
/// non-reducing builtin subtree (`PREVIOUS(rank(pop, 1))` inside the
/// `scale -> grow` partial of `grow[Region] = scale[Region] * RANK(pop, 1)`)
/// must compile through the arrayed capture helper and score its true value.
///
/// With distinct, order-preserving populations the ranks are constant, so
/// `grow` is linear in `scale` with a frozen coefficient and the
/// changed-first partial reproduces delta-grow exactly: the score is +1 per
/// element from the first post-initial step (pre-fix it read a constant
/// -100-class value off the 0-stubbed scalar helpers).
///
/// The one remaining degradation in this fixture is OUT of #742's scope:
/// `enumerate_agg_nodes` hoists the whole-extent `RANK(pop, 1)` into a
/// scalar `$⁚ltm⁚agg⁚0` whose own (array-valued) equation cannot compile --
/// the "RANK as the scored reducer itself" path (docs/tech-debt.md entry
/// 27's family). That failure is loud (one synthetic-variable Warning,
/// pinned here) and zeroes only the agg-routed loop scores.
#[test]
fn rank_frozen_subtree_link_score_scores_correctly() {
    let project = TestProject::new("gh742_rank")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .array_with_ranges("seed[Region]", vec![("north", "100"), ("south", "200")])
        .array_aux("scale[Region]", "pop[Region] * 0.01")
        .array_aux("grow[Region]", "scale[Region] * RANK(pop, 1)")
        .array_flow("inflow[Region]", "grow[Region]", None)
        .array_stock("pop[Region]", "seed[Region]", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The capture helpers compile: the ONLY degradation left is the
    // out-of-scope RANK-hoisted scalar agg (see the doc comment).
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "only the RANK-hoisted agg may warn; got: {warnings:?}"
    );
    assert_eq!(
        warnings[0].variable.as_deref(),
        Some("$\u{205A}ltm\u{205A}agg\u{205A}0"),
        "the remaining warning must be the RANK-hoisted synthetic agg; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // The frozen-RANK capture helper is ONE arrayed (deduped) helper whose
    // slots read the current-step per-element ranks -- constant [1, 2].
    let helper =
        "$\u{205A}$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}grow\u{205A}0\u{205A}arg0";
    let helper_base = offset_of(&results, helper);
    let helper_north = series_at(&results, helper_base);
    let helper_south = series_at(&results, helper_base + 1);
    assert!(
        helper_north.iter().all(|&v| v == 1.0),
        "the arrayed capture helper's north slot must hold rank 1; got {helper_north:?}"
    );
    assert!(
        helper_south.iter().all(|&v| v == 2.0),
        "the arrayed capture helper's south slot must hold rank 2; got {helper_south:?}"
    );

    // `grow` is linear in `scale` with the (constant) rank as coefficient,
    // so the scale -> grow link score is exactly +1 per element.
    let score_name = format!("{LINK_SCORE_PREFIX}scale\u{2192}grow");
    let score = ltm_var(&ltm.vars, &score_name);
    let base = offset_of(&results, &score.name);
    for slot in 0..slot_count(score, &project.dimensions) {
        let series = series_at(&results, base + slot);
        assert_eq!(series[0], 0.0, "initial-step guard pins slot {slot} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "scale->grow slot {slot} step {step} must score +1; got {series:?}"
            );
        }
    }

    // The direct (non-agg-routed) per-element loop -- the A2A loop over
    // Region (pop -> scale -> grow -> inflow -> pop) -- scores +1 once the
    // flow-to-stock startup guard clears.
    let loop_var = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .find(|v| v.dimensions == vec!["Region".to_string()])
        .expect("the per-element pop/scale/grow/inflow loop must be scored");
    let base = offset_of(&results, &loop_var.name);
    for slot in 0..slot_count(loop_var, &project.dimensions) {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                (v - 1.0).abs() < 1e-6,
                "loop score slot {slot} step {step} must be +1; got {series:?}"
            );
        }
    }
}
