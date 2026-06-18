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
    model_element_causal_edges, model_ltm_variables, reclassify_loops_from_results,
    set_project_ltm_discovery_mode, set_project_ltm_enabled, sync_from_datamodel_incremental,
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

/// The GH #525 partially-iterated-subscript shape compiles AND scores --
/// since T6 of the shape-expressiveness design via the `PerElement`
/// classification (the iterated+literal mix `pop[region,young]` no longer
/// lands in `DynamicIndex`).
///
/// The model: an arrayed stock `pop[region,age]`, a row-reducer
/// `row_sum[region] = pop[region,young] * 2` (a partially iterated
/// subscript -- `young` is fixed, `region` is iterated), and a flow
/// `growth[region,age] = row_sum[region] * 0.0001 * pop[region,age]` that
/// closes a feedback loop. The reference's link scores are the
/// per-(row, full-target-element) scalars
/// `$⁚ltm⁚link_score⁚pop[{r},young]→row_sum[{r}]` -- one per target
/// element, with the row projected from it -- replacing the merged
/// Bare-named conservative score of the pre-T6 `DynamicIndex` family.
///
/// `pop[region,young]` is the slot's only moving input, so the
/// per-element partial reproduces Δrow_sum exactly: each scalar scores
/// `+1` from the first post-initial step.
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

    // The merged Bare-named conservative score is retired; the
    // per-(row, full-target-element) scalars carry the edge instead.
    let score_names = ltm_score_var_names(&results);
    assert!(
        !score_names
            .iter()
            .any(|n| n == "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}row_sum"),
        "the merged Bare pop\u{2192}row_sum score must not exist post-T6; got: {score_names:?}"
    );
    // `pop[region,young]` is the slot's only moving input, so each
    // per-element scalar is exact: +1 from the first post-initial step.
    for r in ["a", "b"] {
        let link_name = format!("{LINK_SCORE_PREFIX}pop[{r},young]\u{2192}row_sum[{r}]");
        let series = series_at(&results, offset_of(&results, &link_name));
        assert_eq!(series[0], 0.0, "initial-step guard pins {link_name} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "{link_name} step {step} must score +1; got {series:?}"
            );
        }
    }
    // The unread `old` rows get no score at all (the pre-T6 cross-product
    // family would have attributed them through the merged Bare name).
    assert!(
        !score_names
            .iter()
            .any(|n| n.contains("pop[a,old]\u{2192}row_sum")
                || n.contains("pop[b,old]\u{2192}row_sum")),
        "unread rows must have no pop\u{2192}row_sum scores; got: {score_names:?}"
    );

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

    // Detected surface: the weight loops must be Balancing -- and above all
    // NOT a confident Reinforcing. (The runtime weight→agg series for this
    // fixture is pinned NEGATIVE by the GH #744 body-aware partial -- see
    // `co_source_weight_to_agg_link_score_tracks_true_partial` -- so the
    // static label and the runtime series now agree in sign.) Since GH #746
    // the detected surface shares the scored surface's per-element loop
    // builder, so the weight cycle surfaces once per region with
    // element-subscripted variables (`weight[north]`).
    let detected = model_detected_loops(&db, source_model, sync.project).clone();
    let weight_loops: Vec<_> = detected
        .loops
        .iter()
        .filter(|l| {
            l.variables
                .iter()
                .any(|v| v == "weight" || v.starts_with("weight["))
        })
        .collect();
    assert!(
        !weight_loops.is_empty(),
        "the weight loop(s) must be detected; got {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );
    for weight_loop in &weight_loops {
        assert_eq!(
            weight_loop.polarity,
            DetectedLoopPolarity::Balancing,
            "∂agg/∂weight[e] = -pop[e] < 0: the weight loop is balancing; a Reinforcing label \
             here is the I1b confidently-wrong-label regression ({:?})",
            weight_loop.variables
        );
    }

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
// GH #743 / GH #767: the iterated-dim-feeder reducer (now hoisted, T5)
// ---------------------------------------------------------------------------

/// The GH #743/#767 fixture: an apply-to-all equation over `D1` whose RHS is
/// a multi-source reducer whose per-row feeder is arrayed over the ITERATED
/// dimension -- `growth[D1] = SUM(matrix[D1,*] * frac[D1])`. Since T5 of the
/// shape-expressiveness design the I1 feeder clause ACCEPTS this combination
/// (`matrix` the co-source with the canonical `[Iterated, Reduced]` slice,
/// `frac` a projection feeder with its own `[Iterated]` slice); the whole-RHS
/// form is VARIABLE-BACKED (growth IS the agg, no synthetic minted). The
/// feedback loop closes through `frac`:
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

/// GH #767 (T5, the feeder half): the feeder edge of the hoisted shape gets
/// per-`(row, slot)` changed-last scores (`frac[r1]→growth[r1]`, 1:1 rows --
/// the feeder's own `[Iterated]` slice through `read_slice_rows`), replacing
/// the GH #743 Bare changed-last conservative score for THIS shape (the
/// changed-last chooser machinery itself stays, serving the still-declined
/// non-projection shapes -- see
/// `non_projection_feeder_co_source_closure_stays_loud`).
///
/// Hand-derived per-slot equation (changed-last: only the feeder frozen,
/// the co-source slice verbatim -- the changed-FIRST partial would freeze
/// the wildcard slice as an uncompilable lagged whole-array read):
///
/// ```text
/// numerator = growth[r1] - sum(matrix[r1,*] * PREVIOUS(frac[r1]))
///           = Σ_c matrix[r1,c] * Δfrac[r1]
/// ```
///
/// With `matrix` constant the numerator equals `Δgrowth[r1]` exactly, so the
/// score is +1 from step 1, and each per-circuit loop (the feeder hop routes
/// circuits to the per-circuit scalar path -- its only scores are the
/// per-row scalars) is +1 past startup: the isolated-loop invariant.
#[test]
fn iterated_dim_feeder_closure_scores_via_hoist() {
    let project = gh743_feeder_closure_fixture();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The whole-RHS form is VARIABLE-BACKED: no synthetic agg node.
    assert!(
        ltm_vars
            .iter()
            .all(|v| !v.name.contains("$\u{205A}ltm\u{205A}agg\u{205A}")),
        "the whole-RHS feeder reducer is variable-backed, not synthetic; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // Everything compiles cleanly: zero assembly warnings.
    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the feeder-closure fixture must compile every LTM fragment cleanly; got: {assembly:?}"
    );

    // The Bare A2A frac→growth score is RETIRED for this shape, replaced by
    // the per-(row, slot) scalars.
    let bare = format!("{LINK_SCORE_PREFIX}frac\u{2192}growth");
    assert!(
        !ltm_vars.iter().any(|v| v.name == bare),
        "the Bare conservative feeder score must be replaced by per-row scores"
    );

    // The per-row feeder equation, hand-derived (changed-last, slot-pinned).
    let feeder_r1 = format!("{LINK_SCORE_PREFIX}frac[r1]\u{2192}growth[r1]");
    let var = ltm_var(&ltm_vars, &feeder_r1);
    assert_eq!(
        var.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((growth[d1\u{B7}r1] - \
         PREVIOUS(growth[d1\u{B7}r1])) = 0) OR ((frac[d1\u{B7}r1] - \
         PREVIOUS(frac[d1\u{B7}r1])) = 0) then 0 else \
         SAFEDIV((growth[d1\u{B7}r1] - (sum(matrix[d1\u{B7}r1, *] * \
         PREVIOUS(frac[d1\u{B7}r1])))), ABS((growth[d1\u{B7}r1] - \
         PREVIOUS(growth[d1\u{B7}r1]))), 0) * SIGN((frac[d1\u{B7}r1] - \
         PREVIOUS(frac[d1\u{B7}r1])))",
        "the per-row feeder changed-last equation must match the hand-derived form"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    const TOL: f64 = 1e-9;

    // Per-row feeder scores: +1 at every step past the first (matrix is
    // constant, so the changed-last numerator IS Δgrowth).
    for row in ["r1", "r2"] {
        let name = format!("{LINK_SCORE_PREFIX}frac[{row}]\u{2192}growth[{row}]");
        let series = series_at(&results, offset_of(&results, &name));
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "{name} at step {step}: got {v}, expected +1. A constant negative \
                 value here (-1/growth-rate) is the GH #743 silent-garbage \
                 signature."
            );
        }
    }

    // The feeder hop routes circuits to the per-circuit scalar path: one
    // scalar loop per D1 element, each +1 past startup.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "expected one scalar loop per D1 element; got {loop_names:?}"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm_vars, name);
        assert!(
            var.dimensions.is_empty(),
            "feeder-closure loops are per-circuit scalars; {name} got dims {:?}",
            var.dimensions
        );
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "{name} at step {step}: got {v}, expected +1 (isolated reinforcing \
                 loop)."
            );
        }
    }
}

/// GH #767 (T5, the co-source half -- THE FLIP of
/// `un_hoisted_iterated_dim_feeder_co_source_closure_stays_loud`): closing
/// the loop through the wildcard-read co-source (`matrix`) is now genuinely
/// scoreable. Pre-T5 the hoist declined, the element graph emitted the
/// conservative cross-product, and every cross-row loop score failed
/// fragment compile (warned zero-stubs -- the loud floor GH #767's body
/// names). Post-T5 the gate admits both edges, the element edges shrink to
/// the read rows (no cross-row circuits exist to warn about), and each
/// per-circuit loop composes real per-`(row, slot)` scores.
///
/// Hand-derived: `matrix = pop[D1]*0.05`, `frac = 0.5` constant. Per slot
/// `r`, `growth[r] = Σ_c matrix[r,c]*0.5 = 0.05·pop[r]·Σ_c 0.5·... `; the
/// changed-first per-row partial holds the feeder frozen AT THE ROW
/// (`PREVIOUS(frac[d1·r])` -- the GH #744 body machinery extended to the
/// mismatched-arity feeder dep), so
///
/// ```text
/// score(matrix[r,c] → growth[r]) = PREVIOUS(frac[r]) · Δmatrix[r,c] / |Δgrowth[r]|
///                                = 0.5·Δm / (0.5·(Δm + Δm)) = 0.5
/// ```
///
/// and each of the 4 per-circuit loops scores 0.5 sustained (the two
/// loops through a slot's c1/c2 cells sum to the slot's full +1).
#[test]
fn iterated_dim_feeder_co_source_closure_scores_real_values() {
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
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The warned zero-stubs are GONE: every LTM fragment compiles.
    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the co-source closure must compile every LTM fragment cleanly; got: {assembly:?}"
    );

    // The read-slice element edges admit no cross-row circuits: only the
    // 4 diagonal per-(row, slot) score names exist (no
    // `matrix[r1,..]→growth[r2]`-style names anywhere).
    for v in &ltm_vars {
        assert!(
            !(v.name.contains("matrix[r1") && v.name.contains("growth[r2]")),
            "no cross-row score may exist; got {}",
            v.name
        );
        assert!(
            !(v.name.contains("matrix[r2") && v.name.contains("growth[r1]")),
            "no cross-row score may exist; got {}",
            v.name
        );
    }

    // The per-row co-source equation, hand-derived: the changed-first
    // body partial with the FEEDER pinned to the row and frozen
    // (`PREVIOUS(frac[d1·r1])`) -- the GH #744 machinery's
    // mismatched-arity by-dim-name pin.
    let m_r1c1 = format!("{LINK_SCORE_PREFIX}matrix[r1,c1]\u{2192}growth[r1]");
    let var = ltm_var(&ltm_vars, &m_r1c1);
    assert_eq!(
        var.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((growth[d1\u{B7}r1] - \
         PREVIOUS(growth[d1\u{B7}r1])) = 0) OR ((matrix[d1\u{B7}r1,d2\u{B7}c1] - \
         PREVIOUS(matrix[d1\u{B7}r1,d2\u{B7}c1])) = 0) then 0 else \
         SAFEDIV((PREVIOUS(growth[d1\u{B7}r1]) + ((matrix[d1\u{B7}r1, d2\u{B7}c1] * \
         PREVIOUS(frac[d1\u{B7}r1])) - (PREVIOUS(matrix[d1\u{B7}r1, d2\u{B7}c1]) * \
         PREVIOUS(frac[d1\u{B7}r1]))) - PREVIOUS(growth[d1\u{B7}r1])), \
         ABS((growth[d1\u{B7}r1] - PREVIOUS(growth[d1\u{B7}r1]))), 0) * \
         SIGN((matrix[d1\u{B7}r1,d2\u{B7}c1] - PREVIOUS(matrix[d1\u{B7}r1,d2\u{B7}c1])))",
        "the per-row co-source changed-first equation must hold the feeder frozen at the row"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    const TOL: f64 = 1e-9;

    // Per-row co-source scores: 0.5 at every step past the first.
    for (row, cell) in [("r1", "c1"), ("r1", "c2"), ("r2", "c1"), ("r2", "c2")] {
        let name = format!("{LINK_SCORE_PREFIX}matrix[{row},{cell}]\u{2192}growth[{row}]");
        let series = series_at(&results, offset_of(&results, &name));
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() <= TOL,
                "{name} at step {step}: got {v}, expected 0.5 (the row's share of \
                 the two-cell slot)."
            );
        }
    }

    // 4 per-circuit scalar loops, each 0.5 sustained past startup -- real
    // values where the pre-T5 path emitted warned zero-stubs.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        4,
        "expected one scalar loop per (row, cell) circuit; got {loop_names:?}"
    );
    for name in &loop_names {
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 0.5).abs() <= TOL,
                "{name} at step {step}: got {v}, expected a sustained 0.5 (the \
                 pre-T5 loud floor stubbed these to 0)."
            );
        }
    }
}

/// GH #767 (T5, bilinear additivity -- the GH #744 (iv) property extended
/// to BOTH halves being sources of the same variable-backed agg): with
/// `matrix` AND `frac` both varying, the co-source rows' changed-FIRST
/// numerators (`PREVIOUS(frac[r])·Δmatrix[r,c]`) plus the feeder's
/// changed-LAST numerator (`Σ_c matrix[r,c]·Δfrac[r]`) telescope exactly
/// to `Δgrowth[r]` per slot:
///
/// ```text
/// Δgrowth[r] = Σ_c (m_c·f - m'_c·f') = Σ_c m_c·(f - f') + Σ_c f'·(m_c - m'_c)
/// ```
///
/// Numerators are reconstructed from the emitted scores
/// (`numerator = score · |Δgrowth| · SIGN(Δsource)`), so this pins the
/// additivity of what LTM actually reports.
#[test]
fn feeder_plus_co_source_row_scores_are_additive() {
    let project = TestProject::new("gh767_additive")
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
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * frac[D1])", None)
        .build_datamodel();

    let (results, _) = run_ltm(&project);

    for row in ["r1", "r2"] {
        let growth = series_at(&results, offset_of(&results, &format!("growth[{row}]")));
        let frac = series_at(&results, offset_of(&results, &format!("frac[{row}]")));
        let feeder_score = series_at(
            &results,
            offset_of(
                &results,
                &format!("{LINK_SCORE_PREFIX}frac[{row}]\u{2192}growth[{row}]"),
            ),
        );
        let cells: Vec<(Vec<f64>, Vec<f64>)> = ["c1", "c2"]
            .iter()
            .map(|cell| {
                let score = series_at(
                    &results,
                    offset_of(
                        &results,
                        &format!("{LINK_SCORE_PREFIX}matrix[{row},{cell}]\u{2192}growth[{row}]"),
                    ),
                );
                let m = series_at(
                    &results,
                    offset_of(&results, &format!("matrix[{row},{cell}]")),
                );
                (score, m)
            })
            .collect();

        let mut contributing = 0usize;
        for t in 1..growth.len() {
            let d_growth = growth[t] - growth[t - 1];
            let d_frac = frac[t] - frac[t - 1];
            if d_growth == 0.0 || d_frac == 0.0 {
                continue;
            }
            let mut sum = feeder_score[t] * d_growth.abs() * d_frac.signum();
            let mut all_scored = true;
            for (score, m) in &cells {
                let d_m = m[t] - m[t - 1];
                if d_m == 0.0 {
                    all_scored = false;
                    break;
                }
                sum += score[t] * d_growth.abs() * d_m.signum();
            }
            if !all_scored {
                continue;
            }
            assert!(
                (sum - d_growth).abs() <= 1e-9,
                "slot {row} step {t}: feeder + co-source numerators {sum} != Δgrowth {d_growth}"
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "slot {row}: expected at least 3 steps where every source scored, got {contributing}"
        );
    }
}

/// GH #767 review (the REPEATED-DIM co-source hazard): a square co-source
/// declared over the same dimension twice (`matrix[D1,D1]`) read as
/// `SUM(matrix[*, D1] * frac[D1])` -- slice `[Reduced, Iterated]`, the
/// ITERATED axis at position 1 while position 0 shares the dim NAME -- is
/// ACCEPTED by the feeder clause, so the co-source row partial must pin
/// the feeder dep at the slice's ITERATED axis position
/// (`PREVIOUS(frac[d1·r2])` in `matrix[r1,r2]→growth[r2]`), never a
/// first-match name lookup (which silently pinned position 0's REDUCED
/// element, `frac[d1·r1]` -- a wrong-magnitude score with zero warnings,
/// the exact silent-wrong-number class this epic forbids). The asymmetric
/// per-element matrix and seeded pops make the wrong pin numerically
/// visible: per-slot additivity (feeder changed-last + co-source rows
/// changed-first == Δgrowth) holds only under the correct pin.
#[test]
fn repeated_dim_co_source_pins_feeder_at_iterated_axis() {
    let project = TestProject::new("gh767_repeated_dim")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .array_with_ranges_direct(
            "seed",
            vec!["D1".into()],
            vec![("r1", "100"), ("r2", "160")],
            None,
        )
        .array_stock("pop[D1]", "seed[D1]", &["growth"], &[], None)
        .array_with_ranges_direct(
            "matrix",
            vec!["D1".into(), "D1".into()],
            vec![
                ("r1,r1", "pop[r1] * 0.05"),
                ("r1,r2", "pop[r1] * 0.06"),
                ("r2,r1", "pop[r2] * 0.07"),
                ("r2,r2", "pop[r2] * 0.08"),
            ],
            None,
        )
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("growth[D1]", "SUM(matrix[*, D1] * frac[D1])", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the repeated-dim fixture must compile every LTM fragment cleanly; got: {assembly:?}"
    );

    // The off-iterated-row partial: the feeder must be pinned to the SLOT
    // (the Iterated axis's element, r2), not the same-named Reduced axis's
    // row element (r1).
    let m_r1r2 = format!("{LINK_SCORE_PREFIX}matrix[r1,r2]\u{2192}growth[r2]");
    let var = ltm_var(&ltm_vars, &m_r1r2);
    let eq = var.equation.source_text();
    assert!(
        eq.contains("PREVIOUS(frac[d1\u{B7}r2])"),
        "the feeder must be frozen at the slice's ITERATED axis element (the slot): {eq}"
    );
    assert!(
        !eq.contains("frac[d1\u{B7}r1]"),
        "the feeder must NOT be pinned to the same-named Reduced axis's row element: {eq}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Per-slot additivity: feeder changed-last + co-source rows
    // changed-first == Δgrowth exactly (numerators reconstructed from the
    // emitted scores). Under the first-match mis-pin this is off by
    // (frac[r] - frac[r1])·Σ_a Δmatrix[a,r] for slot r2.
    for slot in ["r1", "r2"] {
        let growth = series_at(&results, offset_of(&results, &format!("growth[{slot}]")));
        let frac = series_at(&results, offset_of(&results, &format!("frac[{slot}]")));
        let feeder_score = series_at(
            &results,
            offset_of(
                &results,
                &format!("{LINK_SCORE_PREFIX}frac[{slot}]\u{2192}growth[{slot}]"),
            ),
        );
        let rows: Vec<(Vec<f64>, Vec<f64>)> = ["r1", "r2"]
            .iter()
            .map(|a| {
                let score = series_at(
                    &results,
                    offset_of(
                        &results,
                        &format!("{LINK_SCORE_PREFIX}matrix[{a},{slot}]\u{2192}growth[{slot}]"),
                    ),
                );
                let m = series_at(
                    &results,
                    offset_of(&results, &format!("matrix[{a},{slot}]")),
                );
                (score, m)
            })
            .collect();

        let mut contributing = 0usize;
        for t in 1..growth.len() {
            let d_growth = growth[t] - growth[t - 1];
            let d_frac = frac[t] - frac[t - 1];
            if d_growth == 0.0 || d_frac == 0.0 {
                continue;
            }
            let mut sum = feeder_score[t] * d_growth.abs() * d_frac.signum();
            let mut all_scored = true;
            for (score, m) in &rows {
                let d_m = m[t] - m[t - 1];
                if d_m == 0.0 {
                    all_scored = false;
                    break;
                }
                sum += score[t] * d_growth.abs() * d_m.signum();
            }
            if !all_scored {
                continue;
            }
            assert!(
                (sum - d_growth).abs() <= 1e-9 * d_growth.abs().max(1.0),
                "slot {slot} step {t}: feeder + co-source numerators {sum} != Δgrowth \
                 {d_growth} -- the repeated-dim mis-pin signature"
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "slot {slot}: expected at least 3 steps where every source scored, got {contributing}"
        );
    }
}

/// GH #767 review (Pinned-bearing canonical + feeder): a canonical slice
/// with a PINNED axis (`SUM(cube[D1, c1, *] * frac[D1])`, slice
/// `[Iterated, Pinned(c1), Reduced]`) is within the feeder clause's scope
/// -- the clause keys only on the Iterated target dims, so the feeder's
/// `[Iterated]` projection is accepted and everything scores: per-row
/// names exist ONLY for the read `c1` cells (the Pinned axis admits no
/// `c2` rows), zero warnings, and per-slot additivity (feeder
/// changed-last + read-row changed-first numerators == Δgrowth) holds
/// exactly.
#[test]
fn pinned_canonical_with_feeder_scores_additively() {
    let project = TestProject::new("gh767_pinned_canonical")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .named_dimension("D3", &["k1", "k2"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "pop[D1] * 0.05",
            None,
        )
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("growth[D1]", "SUM(cube[D1, c1, *] * frac[D1])", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the Pinned-canonical feeder fixture must compile cleanly; got: {assembly:?}"
    );

    // Only the read `c1` rows get per-(row, slot) scores; the Pinned axis
    // admits no c2 rows.
    for v in &ltm_vars {
        assert!(
            !(v.name.starts_with(LINK_SCORE_PREFIX) && v.name.contains(",c2,")),
            "an unread c2 row must get no score; got {}",
            v.name
        );
    }
    for slot in ["r1", "r2"] {
        for k in ["k1", "k2"] {
            let name = format!("{LINK_SCORE_PREFIX}cube[{slot},c1,{k}]\u{2192}growth[{slot}]");
            assert!(
                ltm_vars.iter().any(|v| v.name == name),
                "missing read-row score {name}"
            );
        }
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    for slot in ["r1", "r2"] {
        let growth = series_at(&results, offset_of(&results, &format!("growth[{slot}]")));
        let frac = series_at(&results, offset_of(&results, &format!("frac[{slot}]")));
        let feeder_score = series_at(
            &results,
            offset_of(
                &results,
                &format!("{LINK_SCORE_PREFIX}frac[{slot}]\u{2192}growth[{slot}]"),
            ),
        );
        let rows: Vec<(Vec<f64>, Vec<f64>)> = ["k1", "k2"]
            .iter()
            .map(|k| {
                let score = series_at(
                    &results,
                    offset_of(
                        &results,
                        &format!("{LINK_SCORE_PREFIX}cube[{slot},c1,{k}]\u{2192}growth[{slot}]"),
                    ),
                );
                let c = series_at(
                    &results,
                    offset_of(&results, &format!("cube[{slot},c1,{k}]")),
                );
                (score, c)
            })
            .collect();

        let mut contributing = 0usize;
        for t in 1..growth.len() {
            let d_growth = growth[t] - growth[t - 1];
            let d_frac = frac[t] - frac[t - 1];
            if d_growth == 0.0 || d_frac == 0.0 {
                continue;
            }
            let mut sum = feeder_score[t] * d_growth.abs() * d_frac.signum();
            let mut all_scored = true;
            for (score, c) in &rows {
                let d_c = c[t] - c[t - 1];
                if d_c == 0.0 {
                    all_scored = false;
                    break;
                }
                sum += score[t] * d_growth.abs() * d_c.signum();
            }
            if !all_scored {
                continue;
            }
            assert!(
                (sum - d_growth).abs() <= 1e-9 * d_growth.abs().max(1.0),
                "slot {slot} step {t}: feeder + read-row numerators {sum} != Δgrowth {d_growth}"
            );
            contributing += 1;
        }
        assert!(
            contributing >= 3,
            "slot {slot}: expected at least 3 steps where every source scored, got {contributing}"
        );
    }
}

/// GH #767 (T5, the SYNTHETIC half): the INLINE form of the feeder shape
/// (`growth[D1] = 1 + SUM(matrix[D1,*] * frac[D1])`) mints a synthetic agg
/// whose feeder half rides the same per-`(row, slot)` changed-last emission
/// (`frac[r1]→$⁚ltm⁚agg⁚0[r1]`), and the closure through the feeder is
/// genuinely scored: zero warnings, +1 sustained per-circuit loops
/// (`pop[r] → frac[r] → agg[r] → growth[r] → pop[r]`, with the agg trimmed
/// from the reported loop and the agg→growth partial of `1 + agg` = +1).
#[test]
fn inline_feeder_reducer_synthetic_agg_closure_scores() {
    let project = TestProject::new("gh767_inline")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow("growth[D1]", "1 + SUM(matrix[D1, *] * frac[D1])", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The inline form mints the synthetic agg, with the feeder half's
    // per-(row, slot) names.
    let agg_aux = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    assert!(
        ltm_vars.iter().any(|v| v.name == agg_aux),
        "the inline feeder reducer must mint a synthetic agg; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    for row in ["r1", "r2"] {
        let name = format!("{LINK_SCORE_PREFIX}frac[{row}]\u{2192}{agg_aux}[{row}]");
        assert!(
            ltm_vars.iter().any(|v| v.name == name),
            "missing per-row feeder score {name}; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let diags = collect_all_diagnostics(&db, sync.project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.error, DiagnosticError::Assembly(_)))
        .collect();
    assert!(
        assembly.is_empty(),
        "the inline feeder fixture must compile every LTM fragment cleanly; got: {assembly:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    const TOL: f64 = 1e-9;
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "expected one per-circuit loop per D1 element; got {loop_names:?}"
    );
    for name in &loop_names {
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "{name} at step {step}: got {v}, expected +1 (isolated reinforcing \
                 loop through the synthetic agg)."
            );
        }
    }
}

/// The design's conservative boundary (the "feeders that are not
/// projections" clause): a no-`Reduced` source whose slice is NOT the pure
/// iterated projection -- here a PINNED axis, `w[D1, c1]` -- still declines
/// the hoist, and the co-source closure keeps the LOUD degraded behavior:
/// cross-element loop scores whose equations would reference per-(row,slot)
/// names the cartesian emitters never produce for the off-diagonal hops are
/// warned and dropped before fragment compilation can stub them to 0.
///
/// This is the loud sibling of the #758/#764 zero-stub class, NOT silent
/// garbage. It pins that the T5 feeder clause widened acceptance ONLY for
/// the all-`Iterated` projection: if the warnings here disappear, the
/// scores must be real (a silent drop would be a regression). (The design
/// doc's named non-projection example, `SUM(matrix[D1,*] * other[D2])`
/// with a free reduced-axis index, is not expressible -- the engine
/// rejects the equation outright -- so the Pinned-axis mix is the nearest
/// compiling shape on the boundary.)
#[test]
fn non_projection_feeder_co_source_closure_stays_loud() {
    let project = TestProject::new("gh767_non_projection")
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
        .array_aux_direct("w", vec!["D1".into(), "D2".into()], "0.5", None)
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * w[D1, c1])", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The Pinned-axis mix must NOT be hoisted: no agg, so growth is not an
    // agg target and the per-row feeder names don't exist.
    assert!(
        ltm_vars
            .iter()
            .all(|v| !v.name.contains("$\u{205A}ltm\u{205A}agg\u{205A}")),
        "the Pinned-axis feeder mix must stay un-hoisted; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // Every cross-row loop score through the un-hoisted matrix→growth edge
    // is WARNED and not emitted -- the loud conservative floor.
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
        "the non-projection closure's unscoreable loops must surface Assembly \
         warnings (silent drop would be a regression); diagnostics: {diags:?}"
    );
    for name in &warned_loop_scores {
        assert!(
            ltm_vars.iter().all(|v| v.name != *name),
            "warned missing-name loop score {name} must be dropped before it can compile \
             through a zero stub"
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
}

// ---------------------------------------------------------------------------
// GH #779: bare-spelled feeder of an un-hoisted multi-source reducer
// ---------------------------------------------------------------------------

/// Build the GH #779 fixture: an A2A flow `growth[D1]` whose RHS is a
/// multi-source reducer over `matrix[D1,*]` with the per-row coefficient
/// `frac` (arrayed over `D1`) referenced BARE -- unsubscripted. Feedback
/// loops close through `matrix` (`pop -> matrix -> growth -> pop`) and
/// through `frac` (`pop -> frac -> growth -> pop`).
///
/// `reducer` selects the array-reducing builtin (`SUM`, `MEAN`, `MAX`, ...)
/// so the decline can be exercised across the whole reducer class, not just
/// `SUM`.
fn gh779_bare_feeder_fixture(reducer: &str) -> datamodel::Project {
    TestProject::new("gh779_bare_feeder")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "pop[D1] * 0.05",
            None,
        )
        .array_aux("frac[D1]", "pop[D1] * 0.005")
        .array_flow(
            "growth[D1]",
            &format!("{reducer}(matrix[D1, *] * frac)"),
            None,
        )
        .build_datamodel()
}

/// GH #779: the BARE-spelled feeder of an un-hoisted multi-source reducer
/// must be DECLINED LOUDLY, not given a silent wrong changed-last score.
///
/// In `growth[D1] = SUM(matrix[D1, *] * frac)` the bare `frac` reference is
/// not expressible by the read-slice vocabulary (`compute_read_slice` maps
/// `Expr2::Var` to all-`Reduced`, the slices disagree), so the reducer is
/// not hoisted. The bare spelling's EXECUTION semantics are themselves
/// anomalous (GH #789: an asymmetric probe shows the engine computes
/// `growth[r] = |D1| * Σ_d2 matrix[r,d2] * frac[r]`, a spurious `|D1|`
/// factor). Pre-fix, the `frac -> growth` edge classified `Bare` and the
/// GH #743 changed-last chooser emitted the per-element partial
/// `sum(matrix[d1,*] * PREVIOUS(frac))`, which provably disagrees with
/// whatever execution computes for the bare spelling -- a sustained,
/// unwarned link score of ~2.92 (~3x wrong) and an identically-wrong
/// frac-closure loop score. That is the silent-wrong-number this test
/// guards against (the worst kind).
///
/// The fix declines the bare-feeder shape via the GH #780 machinery: the
/// shaped query returns `Unscoreable`, the edge is recorded, ONE warning
/// names the edge, no `frac -> growth` link-score variable is emitted, and
/// the frac-closure loop is DROPPED. The matrix-closure loops are untouched
/// (they ride the pre-existing loud degraded path, out of scope for #779).
/// The subscripted spelling `frac[D1]` stays fully hoisted and correct
/// (`iterated_dim_feeder_closure_scores_via_hoist`) -- users hitting this
/// have that easy workaround.
#[test]
fn bare_feeder_of_unhoisted_reducer_declines_loudly() {
    let project = gh779_bare_feeder_fixture("SUM");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // No `frac -> growth` link-score variable: the edge is declined, not
    // given the silently-wrong changed-last per-element partial.
    let frac_growth = format!("{LINK_SCORE_PREFIX}frac\u{2192}growth");
    assert!(
        !ltm_vars.iter().any(|v| v.name == frac_growth),
        "the bare-feeder edge must be DECLINED -- no {frac_growth:?} link score; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // Exactly ONE warning names the declined `frac -> growth` edge (the
    // matrix-closure loops surface their own separate warnings -- those are
    // the pre-existing loud degraded path, out of scope here).
    let all_warnings = assembly_warnings(&db, sync.project);
    let frac_edge_warnings = all_warnings
        .iter()
        .filter(|d| {
            d.variable
                .as_deref()
                .is_some_and(|v| v == frac_growth.as_str())
        })
        .count();
    assert_eq!(
        frac_edge_warnings, 1,
        "the declined bare-feeder edge must surface EXACTLY ONE warning naming it; got: {all_warnings:?}"
    );

    // No frac-closure loop survives: every loop score that references the
    // (now non-existent) `frac -> growth` link must be dropped, not stubbed.
    for v in &ltm_vars {
        if v.name.starts_with(LOOP_SCORE_PREFIX) {
            assert!(
                !v.equation.source_text().contains("frac\u{2192}growth"),
                "the frac-closure loop must be DROPPED, not retained referencing the \
                 declined edge; loop {} has equation {}",
                v.name,
                v.equation.source_text()
            );
        }
    }

    // The model still simulates; no frac-closure loop reads the silent
    // wrong ~2.92.
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(
        !results
            .offsets
            .keys()
            .any(|k| k.as_str() == frac_growth.as_str()),
        "the declined edge must not appear in the simulated results either"
    );

    // DISCOVERY mode reaches the same shaped query and consumes
    // `unscoreable_edges` in its pinned-loop pass, so the decline holds there
    // too: no `frac -> growth` link score is minted.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let disc_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    assert!(
        !disc_vars.iter().any(|v| v.name == frac_growth),
        "discovery mode must also decline the bare-feeder edge; got: {:?}",
        disc_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// GH #779, the whole reducer class: MEAN/MIN/MAX/STDDEV bare-feeder shapes
/// decline identically to SUM. A bare arrayed reference inside ANY
/// array-reducer argument carries the same execution-vs-partial
/// disagreement (and the same anomalous execution semantics, GH #789), so
/// the decline must cover the class.
#[test]
fn bare_feeder_decline_covers_reducer_class() {
    for reducer in ["MEAN", "MIN", "MAX", "STDDEV"] {
        let project = gh779_bare_feeder_fixture(reducer);
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
            .vars
            .clone();
        compile_project_incremental(&db, sync.project, "main")
            .unwrap_or_else(|e| panic!("{reducer}: LTM-enabled compilation should succeed: {e:?}"));

        let frac_growth = format!("{LINK_SCORE_PREFIX}frac\u{2192}growth");
        assert!(
            !ltm_vars.iter().any(|v| v.name == frac_growth),
            "{reducer}: the bare-feeder edge must be declined -- no {frac_growth:?}; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        for v in &ltm_vars {
            if v.name.starts_with(LOOP_SCORE_PREFIX) {
                assert!(
                    !v.equation.source_text().contains("frac\u{2192}growth"),
                    "{reducer}: frac-closure loop must drop, not retain the declined edge; {}",
                    v.name
                );
            }
        }
    }
}

/// GH #788: a bare arrayed reducer arg inside an A2A equation is currently
/// outside LTM's scoreable vocabulary. Execution does not use the scalar
/// synthetic-agg value LTM would get from hoisting `SUM(other)`, and a feeder
/// partial such as `frac -> growth` would freeze that same wrong reducer
/// value. Until LTM can model the bare spelling's active-slot semantics, the
/// whole target edge surface must decline loudly.
#[test]
fn bare_arrayed_reducer_arg_in_a2a_target_declines_loudly() {
    let project = TestProject::new("gh788_bare_arrayed_reducer_arg")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux("other[D1]", "pop * 0.01")
        .array_aux("frac[D1]", "pop * 0.005")
        .array_flow("growth[D1]", "SUM(other) * frac", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    for edge in ["other\u{2192}growth", "frac\u{2192}growth"] {
        let link_name = format!("{LINK_SCORE_PREFIX}{edge}");
        assert!(
            !ltm_vars.iter().any(|v| v.name == link_name),
            "edge {edge} must be declined, not scored; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    assert!(
        !ltm_vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the declined bare-arrayed reducer target must be dropped; got: {:?}",
        ltm_vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        !ltm_vars
            .iter()
            .any(|v| v.name.contains("$\u{205A}ltm\u{205A}agg")),
        "SUM(other) must not be hoisted into a whole-array scalar agg; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    let warnings = assembly_warnings(&db, sync.project);
    for edge in ["other -> growth", "frac -> growth"] {
        assert!(
            warnings.iter().any(|d| match &d.error {
                DiagnosticError::Assembly(msg) => {
                    let lower = msg.to_ascii_lowercase();
                    msg.contains(edge)
                        && msg.contains("bare arrayed reducer argument")
                        && lower.contains("sum(other)")
                }
                _ => false,
            }),
            "expected warning for declined {edge}; got: {warnings:?}"
        );
    }

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let disc_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    for edge in ["other\u{2192}growth", "frac\u{2192}growth"] {
        let link_name = format!("{LINK_SCORE_PREFIX}{edge}");
        assert!(
            !disc_vars.iter().any(|v| v.name == link_name),
            "discovery mode must also decline {edge}; got: {:?}",
            disc_vars
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// GH #788/#795 review regression: a bare arrayed reducer in an A2A target
/// does not make every incoming edge unscoreable. Independent additive sources
/// still have sound ceteris-paribus partials because changing them does not
/// freeze or perturb the unsafe reducer value.
#[test]
fn bare_arrayed_reducer_decline_keeps_independent_additive_source() {
    let project = TestProject::new("gh795_bare_arrayed_reducer_scope")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux("local[D1]", "pop * 0.01")
        .array_flow("growth[D1]", "local + SUM(pop)", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let local_growth = format!("{LINK_SCORE_PREFIX}local\u{2192}growth");
    assert!(
        ltm_vars.iter().any(|v| v.name == local_growth),
        "independent additive edge {local_growth:?} must remain scoreable; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    let pop_growth = format!("{LINK_SCORE_PREFIX}pop\u{2192}growth");
    assert!(
        !ltm_vars.iter().any(|v| v.name == pop_growth),
        "edge reading the bare reducer arg must be declined; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.iter().any(|d| match &d.error {
            DiagnosticError::Assembly(msg) => {
                let lower = msg.to_ascii_lowercase();
                msg.contains("pop -> growth")
                    && msg.contains("bare arrayed reducer argument")
                    && lower.contains("sum(pop)")
            }
            _ => false,
        }),
        "expected warning for declined pop -> growth; got: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|d| match &d.error {
            DiagnosticError::Assembly(msg) => {
                msg.contains("local -> growth") && msg.contains("bare arrayed reducer argument")
            }
            _ => false,
        }),
        "local -> growth is independent of the bare reducer and must not warn; got: {warnings:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let local_base = offset_of(&results, &local_growth);
    for (slot, elem) in ["a", "b"].into_iter().enumerate() {
        let score = series_at(&results, local_base + slot);
        for (step, &value) in score.iter().enumerate() {
            assert!(
                value.is_finite(),
                "local -> growth score for {elem} at step {step} must stay finite; got {score:?}"
            );
        }
    }
}

/// GH #779 precision pin: the WHOLE-RHS bare reducer (`total = SUM(pop)`)
/// is NOT the declined feeder shape -- it is variable-backed and its
/// `pop -> total` edge is scored per read row by
/// `try_cross_dimensional_link_scores`, never reaching the changed-last
/// chooser the #779 gate lives in. With `growth[D1] = total * 0.01` closing
/// the loop, the two per-circuit loops each score 0.5 (the per-row link
/// scores split the +1 across |D1| = 2 rows) with zero warnings.
#[test]
fn whole_rhs_bare_reducer_stays_scored() {
    let project = TestProject::new("gh779_whole_rhs_bare")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .scalar_aux("total", "SUM(pop)")
        .array_flow("growth[D1]", "total * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "the whole-RHS bare reducer must stay warning-free"
    );
    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}pop[{row}]\u{2192}total");
        assert!(
            ltm_vars.iter().any(|v| v.name == name),
            "the per-row {name:?} score must exist; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "one loop per D1 row; got {loop_names:?}"
    );
    for name in &loop_names {
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 0.5).abs() <= 1e-9,
                "{name} at step {step}: got {v}, expected 0.5"
            );
        }
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

/// GH #796 review follow-up: `RANK(matrix[Region,*], 1)` ranks Product within
/// each Region context. The source-to-RANK helper scores should therefore fan
/// a north matrix row across north product rank slots only, never into south
/// rank slots.
#[test]
fn rank_context_axis_link_scores_stay_pinned_per_row() {
    let project = TestProject::new("rank_context_axis_link_scores")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .named_dimension("Product", &["x", "y"])
        .array_stock("matrix[Region,Product]", "100", &["growth"], &[], None)
        .array_aux("ranked[Region,Product]", "RANK(matrix[Region,*], 1)")
        .array_flow(
            "growth[Region,Product]",
            "ranked[Region,Product] * 0.01",
            None,
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "mixed-context RANK helper scores must compile without warnings"
    );

    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let rank_agg = ltm_vars
        .iter()
        .find(|v| {
            v.name.starts_with("$\u{205A}ltm\u{205A}agg\u{205A}")
                && v.dimensions == vec!["Region".to_string(), "Product".to_string()]
        })
        .unwrap_or_else(|| {
            panic!(
                "expected Region/Product RANK helper; got: {:?}",
                ltm_vars
                    .iter()
                    .map(|v| (v.name.as_str(), v.dimensions.as_slice()))
                    .collect::<Vec<_>>()
            )
        });
    for product in ["x", "y"] {
        let expected = format!(
            "{LINK_SCORE_PREFIX}matrix[north,x]\u{2192}{}[north,{product}]",
            rank_agg.name
        );
        assert!(
            ltm_vars.iter().any(|v| v.name == expected),
            "north row should feed north rank slot {product}; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        let phantom = format!(
            "{LINK_SCORE_PREFIX}matrix[north,x]\u{2192}{}[south,{product}]",
            rank_agg.name
        );
        assert!(
            !ltm_vars.iter().any(|v| v.name == phantom),
            "north row must not feed south rank slot {product}; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
}

/// GH #796 review follow-up: a RANK helper over a source dimension can feed a
/// target dimension only through a positional mapping. The source-to-helper
/// half is over the ranked source's `State` slots, and the helper-to-target
/// half must project each `Region` target element back to the mapped `State`
/// helper slot instead of emitting an ill-typed bare helper score.
#[test]
fn rank_mapped_helper_slots_score_mapped_targets() {
    let project = TestProject::new("rank_mapped_helper_target_scores")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("score[State]", "100", &["growth"], &[], None)
        .array_aux("ranked[Region]", "RANK(score[*], 1)")
        .array_flow("growth[State]", "ranked * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "mapped RANK helper target scores must compile without warnings: {:?}",
        assembly_warnings(&db, sync.project)
    );

    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let rank_agg = ltm_vars
        .iter()
        .find(|v| {
            v.name.starts_with("$\u{205A}ltm\u{205A}agg\u{205A}")
                && v.dimensions == vec!["State".to_string()]
        })
        .unwrap_or_else(|| {
            panic!(
                "expected State-dimensioned RANK helper; got: {:?}",
                ltm_vars
                    .iter()
                    .map(|v| (v.name.as_str(), v.dimensions.as_slice()))
                    .collect::<Vec<_>>()
            )
        });

    for (state, region) in [("s1", "r1"), ("s2", "r2")] {
        let expected = format!(
            "{LINK_SCORE_PREFIX}{}[{state}]\u{2192}ranked[{region}]",
            rank_agg.name
        );
        assert!(
            ltm_vars.iter().any(|v| v.name == expected),
            "rank helper slot {state} should score ranked[{region}]; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        let bare = format!(
            "{LINK_SCORE_PREFIX}{}\u{2192}ranked[{region}]",
            rank_agg.name
        );
        assert!(
            !ltm_vars.iter().any(|v| v.name == bare),
            "mapped target score must not use the bare helper name {bare}"
        );
    }

    let loop_through_rank = ltm_vars.iter().any(|v| {
        v.name.starts_with(LOOP_SCORE_PREFIX)
            && v.equation.source_text().contains(
                format!("{LINK_SCORE_PREFIX}{}[s1]\u{2192}ranked[r1]", rank_agg.name).as_str(),
            )
            && v.equation.source_text().contains(
                format!("{LINK_SCORE_PREFIX}score[s1]\u{2192}{}[s1]", rank_agg.name).as_str(),
            )
    });
    assert!(
        loop_through_rank,
        "expected a loop score traversing score[s1] through the mapped RANK helper; loop scores: {:?}",
        ltm_vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| (v.name.as_str(), v.equation.source_text()))
            .collect::<Vec<_>>()
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for (state, region) in [("s1", "r1"), ("s2", "r2")] {
        let name = format!(
            "{LINK_SCORE_PREFIX}{}[{state}]\u{2192}ranked[{region}]",
            rank_agg.name
        );
        let series = series_at(&results, offset_of(&results, &name));
        assert!(
            series.iter().all(|v| v.is_finite()),
            "{name} must stay finite; got {series:?}"
        );
    }
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
/// GH #776: `RANK` is array-valued, so it routes through an arrayed synthetic
/// agg rather than a scalar reducer agg. Every ranked source row feeds every
/// rank output slot in its iterated context, so loops through rank ordering
/// are enumerated, including cross-element loops. The score remains the
/// documented conservative delta-ratio stand-in; with constant ranks all
/// rank-mediated loops score ~0 here.
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

    // RANK uses an array-valued helper, so no ill-shaped scalar agg exists and
    // NOTHING warns -- the capture helpers and every link/loop score compile.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "RANK arrayed agg must leave zero warnings; got: {warnings:?}"
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

    // The model has one Region-dimensioned scale-mediated loop
    // (pop -> scale -> grow -> inflow -> pop) plus scalar rank-mediated
    // loops through the arrayed RANK helper. Tell them apart by the
    // link-score names their loop-score equations reference.
    fn eqn_text(v: &LtmSyntheticVar) -> &str {
        match &v.equation {
            datamodel::Equation::Scalar(t) => t,
            datamodel::Equation::ApplyToAll(_, t) => t,
            datamodel::Equation::Arrayed(..) => panic!("unexpected Arrayed loop score"),
        }
    }
    let loop_scores: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();

    // The scale-mediated loop scores +1 once the flow-to-stock startup
    // guard clears (grow is linear in scale with a constant rank coefficient).
    let scale_loop = loop_scores
        .iter()
        .find(|v| {
            v.dimensions == vec!["Region".to_string()] && eqn_text(v).contains("scale\u{2192}grow")
        })
        .expect("the per-element pop/scale/grow/inflow loop must be scored");
    let base = offset_of(&results, &scale_loop.name);
    for slot in 0..slot_count(scale_loop, &project.dimensions) {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                (v - 1.0).abs() < 1e-6,
                "scale-loop score slot {slot} step {step} must be +1; got {series:?}"
            );
        }
    }

    // The rank-mediated loops score ~0: ranks are constant in this regime,
    // so each RANK-helper slot has zero delta and the conservative
    // delta-ratio source half contributes 0. There are three scalar loops:
    // north self-rank, south self-rank, and the true cross-element
    // north/south rank-ordering loop.
    let rank_agg_name = ltm
        .vars
        .iter()
        .find(|v| match &v.equation {
            datamodel::Equation::ApplyToAll(dims, text) => {
                dims.len() == 1 && dims[0] == "Region" && text == "rank(pop, 1)"
            }
            datamodel::Equation::Scalar(_) | datamodel::Equation::Arrayed(..) => false,
        })
        .expect("RANK(pop, 1) must be emitted as an arrayed aggregate helper")
        .name
        .clone();
    let agg = rank_agg_name.as_str();
    let rank_loops: Vec<&LtmSyntheticVar> = loop_scores
        .iter()
        .copied()
        .filter(|v| eqn_text(v).contains(&format!("pop[north]\u{2192}{agg}")))
        .chain(loop_scores.iter().copied().filter(|v| {
            eqn_text(v).contains(&format!("pop[south]\u{2192}{agg}"))
                && !eqn_text(v).contains(&format!("pop[north]\u{2192}{agg}"))
        }))
        .collect();
    assert_eq!(
        rank_loops.len(),
        3,
        "expected north, south, and cross-element rank-mediated loops; got: {:?}",
        loop_scores
            .iter()
            .map(|v| (v.name.as_str(), eqn_text(v)))
            .collect::<Vec<_>>()
    );
    assert!(
        rank_loops.iter().any(|v| {
            let text = eqn_text(v);
            text.contains(&format!("pop[north]\u{2192}{agg}[south]"))
                && text.contains(&format!("pop[south]\u{2192}{agg}[north]"))
        }),
        "expected a cross-element rank-ordering loop through both rank slots"
    );
    for rank_loop in rank_loops {
        assert!(rank_loop.dimensions.is_empty());
        let series = series_at(&results, offset_of(&results, &rank_loop.name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                v.abs() < 1e-9,
                "rank-loop score {} step {step} must be ~0 (constant ranks); \
                 got {series:?}",
                rank_loop.name
            );
        }
    }
}

/// GH #525, the exact filed repro: `row_sum[Region] = pop[Region, young] +
/// pop[Region, old]` -- TWO partially-iterated references (one iterated
/// index, one literal element, multi-dim source) in one A2A equation, with
/// `growth[Region, Age] = row_sum[Region] * 0.0001 * pop[Region, Age]`
/// closing the feedback loop.
///
/// Post-T6 of the shape-expressiveness design both `pop[Region, <elem>]`
/// references classify `RefShape::PerElement`, and the three parts of the
/// documented flip hold:
///
/// (a) the merged Bare arrayed link score `pop→row_sum` (which scored +1
///     per slot with BOTH references live) is no longer emitted; instead
///     the four per-(row, element) scalars `pop[{r},{age}]→row_sum[{r}]`
///     each score ~0.5 in this symmetric repro -- each site now attributes
///     only its own reference's share;
/// (b) the single ApplyToAll loop through row_sum flips to per-circuit
///     element-subscripted scalar loops (one per `(Region, Age)` circuit)
///     whose equations reference the per-(row, element) names and carry
///     real non-zero post-startup values;
/// (c) the phantom cross-element loops (~0.245 silent confident scores,
///     four of them pre-T6) are no longer enumerated AT ALL: the element
///     graph emits only the pinned diagonal, so no loop-score equation
///     contains the plain substring `pop→row_sum` -- exact as a substring
///     check because every per-(row, element) name interposes the
///     from-side `]` before the arrow (`pop[r,a]→row_sum[r]`).
///
/// The detected surface must biject with the scored one (the #746-era
/// invariant) -- the per-circuit loops ride the same builders.
#[test]
fn gh525_two_reference_partially_iterated_row_sum_scores() {
    let project = TestProject::new("gh525_repro")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("row_sum[Region]", "pop[Region, young] + pop[Region, old]")
        .array_flow(
            "growth[Region, Age]",
            "row_sum[Region] * 0.0001 * pop[Region, Age]",
            None,
        )
        .array_stock("pop[Region, Age]", "100", &["growth"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the GH #525 repro must compile with LTM enabled");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "every LTM fragment (capture helpers included) must compile; got: {warnings:?}"
    );

    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // (a) the merged Bare arrayed score is gone...
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}row_sum"),
        "the merged Bare pop\u{2192}row_sum score must not be emitted; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    // ...replaced by the four per-(row, element) scalars at ~0.5 each:
    // both pops move identically in this symmetric repro, so each site's
    // share of delta-row_sum is exactly half.
    for r in ["a", "b"] {
        for age in ["young", "old"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[{r},{age}]\u{2192}row_sum[{r}]");
            let series = series_at(&results, offset_of(&results, &name));
            assert_eq!(series[0], 0.0, "initial-step guard pins {name} to 0");
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} step {step} must score 0.5 (its own reference's \
                     share); got {series:?}"
                );
            }
        }
    }

    // (b) the row_sum loop family is per-circuit element-subscripted
    // scalar loops -- one per (Region, Age) circuit -- referencing the
    // per-(row, element) names, with real non-zero post-startup values.
    let eqn_text = |v: &LtmSyntheticVar| -> String {
        match &v.equation {
            datamodel::Equation::Scalar(t) => t.clone(),
            datamodel::Equation::ApplyToAll(_, t) => t.clone(),
            other => format!("{other:?}"),
        }
    };
    let row_sum_loops: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .filter(|v| eqn_text(v).contains("row_sum"))
        .collect();
    assert_eq!(
        row_sum_loops.len(),
        4,
        "one element-subscripted scalar loop per (Region, Age) circuit \
         through row_sum; got: {:?}",
        row_sum_loops.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    let mut seen_rows: Vec<String> = Vec::new();
    for lv in &row_sum_loops {
        assert!(
            lv.dimensions.is_empty(),
            "per-circuit loop {} must be scalar; got dims {:?}",
            lv.name,
            lv.dimensions
        );
        let text = eqn_text(lv);
        let row = ["a", "b"]
            .iter()
            .flat_map(|r| ["young", "old"].iter().map(move |a| (r, a)))
            .find(|(r, a)| text.contains(&format!("pop[{r},{a}]\u{2192}row_sum[{r}]")))
            .map(|(r, a)| format!("{r},{a}"))
            .unwrap_or_else(|| {
                panic!(
                    "loop {} must reference a per-(row, element) \
                     pop→row_sum scalar; eqn: {text}",
                    lv.name
                )
            });
        assert!(
            !seen_rows.contains(&row),
            "each circuit references a distinct row; duplicate {row}"
        );
        seen_rows.push(row);
        let series = series_at(&results, offset_of(&results, &lv.name));
        assert!(
            series.iter().all(|v| v.is_finite()),
            "loop score {} must stay finite; got {series:?}",
            lv.name
        );
        assert!(
            series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0),
            "loop score {} must carry real non-zero values; got {series:?}",
            lv.name
        );
    }

    // (c) the phantoms are gone at enumeration: no loop-score equation
    // contains the plain substring `pop→row_sum` (the per-(row, element)
    // names interpose `]` before the arrow, so this matches only the
    // retired Bare A2A form the phantoms composed).
    for lv in ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
    {
        let text = match &lv.equation {
            datamodel::Equation::Scalar(t) => t.clone(),
            datamodel::Equation::ApplyToAll(_, t) => t.clone(),
            datamodel::Equation::Arrayed(_, slots, default, _) => {
                let mut t: String = slots.iter().map(|(_, eq, _, _)| eq.clone()).collect();
                if let Some(d) = default {
                    t.push_str(d);
                }
                t
            }
        };
        assert!(
            !text.contains("pop\u{2192}row_sum"),
            "no loop-score equation may reference the retired Bare \
             pop\u{2192}row_sum name ({}): {text}",
            lv.name
        );
    }

    // The direct pop -> growth A2A loop survives unchanged (its edge is
    // Bare-only).
    assert!(
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .any(|v| v.dimensions == vec!["Region".to_string(), "Age".to_string()]),
        "the direct pop/growth A2A loop must keep its ApplyToAll form; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // Cross-surface (the #746-era invariant): the detected loop set must
    // biject with the scored loop-score ids -- the per-circuit loops ride
    // the same builders on both surfaces.
    let scored_ids: std::collections::BTreeSet<String> = ltm
        .vars
        .iter()
        .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
        .map(|id| id.to_string())
        .collect();
    let detected = model_detected_loops(&db, source_model, sync.project);
    let detected_ids: std::collections::BTreeSet<String> =
        detected.loops.iter().map(|l| l.id.clone()).collect();
    assert_eq!(
        detected_ids, scored_ids,
        "detected ids must equal the scored loop-score ids for the \
         PerElement circuit family"
    );
}

/// GH #525 (T6, BROADCAST): a `PerElement` reference whose Iterated dims
/// are a strict SUBSET of the target's -- `mid[D1,D2] = pop[D1, young] *
/// 0.05` (`D1` iterated, `Age` pinned, `D2` broadcast). One row feeds every
/// target element it projects from, so the per-(row, FULL-target-element)
/// scalars are `pop[{r},young]→mid[{r},{d2}]` -- the to-side subscript is
/// always the complete element tuple (a partial to-subscript would resolve
/// nowhere). The loop through the hoisted `SUM(mid[D1,*])` closes the
/// feedback, so the scores must compile AND resolve into real loop values.
#[test]
fn per_element_broadcast_mixed_subscript_scores() {
    let project = TestProject::new("per_element_broadcast_e2e")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("pop[D1,Age]", "100", &["inflow"], &[], None)
        .array_aux_direct(
            "mid",
            vec!["D1".into(), "D2".into()],
            "pop[D1, young] * 0.05",
            None,
        )
        .array_flow("inflow[D1,Age]", "1 + SUM(mid[D1, *]) * 0.001", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the broadcast PerElement fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "every LTM fragment must compile; got: {warnings:?}"
    );

    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // One scalar per (row, full target element): the row (r, young)
    // broadcast over D2. `pop[D1,young]` is each mid slot's only moving
    // input, so every scalar is exactly +1 from the first post-initial
    // step.
    for r in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[{r},young]\u{2192}mid[{r},{d2}]");
            let series = series_at(&results, offset_of(&results, &name));
            assert_eq!(series[0], 0.0, "initial-step guard pins {name} to 0");
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 1.0).abs() < 1e-9,
                    "{name} step {step} must score +1; got {series:?}"
                );
            }
        }
    }
    // No cross-row or unread-row scores exist.
    let score_names = ltm_score_var_names(&results);
    assert!(
        !score_names
            .iter()
            .any(|n| n.contains("pop[a,young]\u{2192}mid[b")
                || n.contains("pop[b,young]\u{2192}mid[a")
                || n.contains("pop[a,old]\u{2192}mid")
                || n.contains("pop[b,old]\u{2192}mid")),
        "only the pinned-diagonal broadcast rows may carry scores; got: {score_names:?}"
    );

    // The loops through the PerElement hop + hoisted reducer are scored:
    // finite everywhere, non-zero post-startup.
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_vars.is_empty(),
        "the broadcast PerElement loops must be scored; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    let mut saw_nonzero = false;
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "loop score {} slot {slot} must stay finite; got {series:?}",
                lv.name
            );
            saw_nonzero |= series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0);
        }
    }
    assert!(
        saw_nonzero,
        "at least one loop through the broadcast PerElement hop must carry \
         real values"
    );

    // Cross-surface bijection (the #746-era invariant).
    let scored_ids: std::collections::BTreeSet<String> = ltm
        .vars
        .iter()
        .filter_map(|v| v.name.strip_prefix(LOOP_SCORE_PREFIX))
        .map(|id| id.to_string())
        .collect();
    let detected_ids: std::collections::BTreeSet<String> =
        model_detected_loops(&db, source_model, sync.project)
            .loops
            .iter()
            .map(|l| l.id.clone())
            .collect();
    assert_eq!(detected_ids, scored_ids);
}

/// GH #525 (T6, MIXED `Bare`+`PerElement` edge): `growth[R,A] = (pop[R,A] +
/// pop[R,young]) * 0.0001` -- the same `(pop, growth)` edge carries a Bare
/// site (the all-iterated `pop[R,A]`) AND a PerElement site
/// (`pop[R,young]`). Pins the resolver precedence from the design's
/// section 3: the edge emits BOTH the Bare A2A score and the per-(row,
/// element) scalars; every circuit routes per-circuit (the edge has a
/// PerElement shape), and a hop both sites produce -- the
/// `pop[r,young] → growth[r,young]` diagonal -- resolves to the
/// `PerElement` scalar (the exact element-in-name form wins before the
/// Bare-A2A subscripting fallback), attributing only the literal
/// reference's share to that hop; the `(r,old)` circuits' hops resolve to
/// the subscripted Bare name.
///
/// Values: the two pops stay element-wise equal in this symmetric model,
/// so every score is exactly 0.5 -- the Bare A2A partial holds `pop[R,A]`
/// live with `pop[R,young]` frozen, and each PerElement scalar holds the
/// literal reference live with the Bare occurrence frozen.
#[test]
fn mixed_bare_and_per_element_edge_resolver_precedence() {
    // NOTE: dimension names must not canonicalize to an element name (a
    // dim literally named `A` collides with element `a` of the other dim,
    // making the bare-element subscript `pop[a, young]` ambiguous at
    // dimension resolution) -- use Region/Age like the GH #525 repro.
    let project = TestProject::new("mixed_bare_per_element")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_stock("pop[Region,Age]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[Region,Age]",
            "(pop[Region,Age] + pop[Region,young]) * 0.0001",
            None,
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the mixed Bare+PerElement fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "every LTM fragment must compile; got: {warnings:?}"
    );

    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Both forms exist: the Bare A2A score (the Bare site's) AND the four
    // per-(row, element) scalars (the PerElement site's).
    let bare_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}growth";
    let bare_var = ltm_var(&ltm.vars, bare_name);
    assert_eq!(
        bare_var.dimensions,
        vec!["Region".to_string(), "Age".to_string()],
        "the Bare site keeps its A2A score over the target's dims"
    );
    let bare_base = offset_of(&results, bare_name);
    for slot in 0..4 {
        let series = series_at(&results, bare_base + slot);
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-6,
                "Bare A2A slot {slot} step {step} must score 0.5 (the \
                 PerElement occurrence is frozen); got {series:?}"
            );
        }
    }
    for r in ["a", "b"] {
        for a in ["young", "old"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[{r},young]\u{2192}growth[{r},{a}]");
            let series = series_at(&results, offset_of(&results, &name));
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-6,
                    "{name} step {step} must score 0.5 (the Bare occurrence \
                     is frozen); got {series:?}"
                );
            }
        }
    }

    // Every circuit is per-circuit scalar (the edge carries a PerElement
    // shape): 4 scalar loops, no ApplyToAll collapse.
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_vars.len(),
        4,
        "one scalar loop per (Region, Age) circuit; got: {:?}",
        loop_vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    let eqn_text = |v: &LtmSyntheticVar| -> String {
        match &v.equation {
            datamodel::Equation::Scalar(t) => t.clone(),
            other => format!("{other:?}"),
        }
    };
    // Resolver precedence: the (r,young) circuits' pop→growth hop resolves
    // to the PerElement scalar; the (r,old) circuits' hop falls back to
    // the subscripted Bare A2A name.
    let young_loops: Vec<&&LtmSyntheticVar> = loop_vars
        .iter()
        .filter(|v| {
            let t = eqn_text(v);
            t.contains("pop[a,young]\u{2192}growth[a,young]")
                || t.contains("pop[b,young]\u{2192}growth[b,young]")
        })
        .collect();
    assert_eq!(
        young_loops.len(),
        2,
        "the two (r,young) circuits resolve their hop to the PerElement \
         scalar; loops: {:?}",
        loop_vars
            .iter()
            .map(|v| (v.name.as_str(), eqn_text(v)))
            .collect::<Vec<_>>()
    );
    let old_loops: Vec<&&LtmSyntheticVar> = loop_vars
        .iter()
        .filter(|v| {
            let t = eqn_text(v);
            t.contains(&format!("\"{bare_name}\"[a,old]"))
                || t.contains(&format!("\"{bare_name}\"[b,old]"))
        })
        .collect();
    assert_eq!(
        old_loops.len(),
        2,
        "the two (r,old) circuits subscript the Bare A2A name; loops: {:?}",
        loop_vars
            .iter()
            .map(|v| (v.name.as_str(), eqn_text(v)))
            .collect::<Vec<_>>()
    );

    // Loop values: each circuit's product is link(0.5) x flow-to-stock(1).
    for lv in &loop_vars {
        let series = series_at(&results, offset_of(&results, &lv.name));
        assert!(series.iter().all(|v| v.is_finite()));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 0.5).abs() < 1e-6,
                "loop {} step {step} must score 0.5; got {series:?}",
                lv.name
            );
        }
    }
}

/// GH #525 (T6, risk 2: the MIXED scalar-node-bearing branch): a cycle
/// through a `PerElement` hop that also contains a SCALAR node
/// (`total = SUM(row_sum[*])`) takes the loop builder's mixed branch, whose
/// link builder must keep BOTH subscripts on the PerElement hop so the
/// per-circuit loop score resolves the `pop[{r},{age}]→row_sum[{r}]`
/// scalar -- a routing/emission disagreement here would reference a
/// nonexistent name and silently stub the loop to 0.
#[test]
fn per_element_hop_in_mixed_scalar_cycle_scores() {
    // Region/Age (not R/A) for the same canonical-name-collision reason as
    // the mixed fixture above.
    let project = TestProject::new("per_element_mixed_branch")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_stock("pop[Region,Age]", "100", &["growth"], &[], None)
        .array_aux("row_sum[Region]", "pop[Region, young] + pop[Region, old]")
        .scalar_aux("total", "SUM(row_sum[*])")
        .array_flow("growth[Region,Age]", "total * 0.0001", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the mixed-branch fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "every LTM fragment must compile; got: {warnings:?}"
    );

    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Every circuit contains the scalar `total` node, so each is a scalar
    // mixed-branch loop whose pop→row_sum hop references the per-(row,
    // element) scalar.
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_vars.len(),
        4,
        "one mixed-branch scalar loop per (Region, Age) circuit; got: {:?}",
        loop_vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    let mut saw_per_element_ref = 0usize;
    for lv in &loop_vars {
        let text = match &lv.equation {
            datamodel::Equation::Scalar(t) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(
            !text.contains("pop\u{2192}row_sum"),
            "no loop may reference the retired Bare pop\u{2192}row_sum name: {text}"
        );
        if ["a", "b"]
            .iter()
            .flat_map(|r| ["young", "old"].iter().map(move |a| (r, a)))
            .any(|(r, a)| text.contains(&format!("pop[{r},{a}]\u{2192}row_sum[{r}]")))
        {
            saw_per_element_ref += 1;
        }
        let series = series_at(&results, offset_of(&results, &lv.name));
        assert!(
            series.iter().all(|v| v.is_finite()),
            "loop score {} must stay finite; got {series:?}",
            lv.name
        );
        assert!(
            series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0),
            "loop score {} must carry real non-zero values (a silent stub \
             means the mixed branch dropped the PerElement subscripts); \
             got {series:?}",
            lv.name
        );
    }
    assert_eq!(
        saw_per_element_ref, 4,
        "every mixed-branch circuit references its per-(row, element) scalar"
    );
}

/// GH #769: a `FixedIndex` reference into a DISJOINT-dim **ApplyToAll**
/// target (`hub[D2] = pop[a1] * 0.05`, `pop` over `D1`) is recoverable via
/// the #510 per-element construction generalized to A2A targets: one
/// `$⁚ltm⁚link_score⁚pop[a1]→hub` emitted as an `Equation::ApplyToAll`
/// over the TARGET's dims holding `pop[a1]` live. Pre-#769 the edge was
/// swept into the GH #758 loud skip (one Warning, no link-score variable,
/// every loop through it dropped); now the loop is genuinely scored with
/// zero warnings.
#[test]
fn gh769_fixed_index_into_disjoint_a2a_target_scores() {
    let project = TestProject::new("gh769_disjoint_a2a")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a1", "a2"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("pop[D1]", "100", &["inflow"], &[], None)
        .array_aux("hub[D2]", "pop[a1] * 0.05")
        .scalar_aux("refill", "SUM(hub[*])")
        .array_flow("inflow[D1]", "refill * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the GH #769 fixture must compile with LTM enabled");

    // Zero warnings: the edge is no longer loud-skipped.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the FixedIndex-into-A2A edge is scoreable; no warning may fire: {warnings:?}"
    );

    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // The per-element construction: `pop[a1]→hub` ApplyToAll over D2.
    let score_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop[a1]\u{2192}hub";
    let score_var = ltm_var(&ltm.vars, score_name);
    assert_eq!(
        score_var.dimensions,
        vec!["D2".to_string()],
        "the GH #769 score is an ApplyToAll over the TARGET's dims"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // `pop[a1]` is each hub slot's only input: +1 from step 1 per slot.
    let base = offset_of(&results, score_name);
    for slot in 0..2 {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "pop[a1]→hub slot {slot} step {step} must score +1; got {series:?}"
            );
        }
    }

    // The loops through the edge are scored: finite, non-zero post-startup.
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_vars.is_empty(),
        "loops through the GH #769 edge must be scored; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    let mut saw_nonzero = false;
    for lv in &loop_vars {
        let series = series_at(&results, offset_of(&results, &lv.name));
        assert!(
            series.iter().all(|v| v.is_finite()),
            "loop score {} must stay finite; got {series:?}",
            lv.name
        );
        saw_nonzero |= series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0);
    }
    assert!(saw_nonzero, "the GH #769 loop must carry real values");
}

/// GH #525 (T6, risk 4: the forced-DISCOVERY twin of the repro). The
/// per-(row, full-target-element) names are a new producer of the
/// element-in-name grammar on NON-reducer edges, so discovery's
/// `parse_link_offsets` must resolve them symmetrically with the
/// exhaustive surface (the #748/#698 lesson): the strongest-path search
/// finds the real per-element circuits through row_sum and never invents a
/// phantom cross-element pop→row_sum pathway.
#[test]
fn gh525_discovery_twin_parses_per_element_names() {
    let project = TestProject::new("gh525_discovery_twin")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("row_sum[Region]", "pop[Region, young] + pop[Region, old]")
        .array_flow(
            "growth[Region, Age]",
            "row_sum[Region] * 0.0001 * pop[Region, Age]",
            None,
        )
        .array_stock("pop[Region, Age]", "100", &["growth"], &[], None)
        .build_datamodel();

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the GH #525 repro")
    .loops;

    // Eight REAL loops are found, with parseable (finite) scores -- the new
    // per-(row, element) names did not corrupt the search graph. FOUR are the
    // direct same-element diagonals (`pop[r,a] -> growth[r,a] -> pop[r,a]`);
    // FOUR run through the `row_sum[r]` reducer node
    // (`pop[r,a] -> row_sum[r] -> growth[r,a] -> pop[r,a]`). The latter four
    // are now discoverable since GH #754: `row_sum[Region]` is BOTH the
    // partial-collapse target of `pop[Region,Age]` (from has MORE dims) and
    // the lower-dim broadcast source of `growth[Region,Age]` (from has FEWER
    // dims), and both projections now spell `row_sum[r]` (1-D) instead of the
    // phantom `row_sum[r,a]`, matching the element graph. Before GH #754 the
    // `row_sum` hop dangled and only the four direct loops surfaced.
    assert_eq!(
        found.len(),
        8,
        "discovery must find the four direct + four row_sum-routed loops; got: {:?}",
        found
            .iter()
            .map(|fl| (&fl.loop_info.id, &fl.loop_info.links))
            .collect::<Vec<_>>()
    );
    let mut row_sum_loops = 0usize;
    for fl in &found {
        // No phantom: a loop visiting row_sum[r] must read it from the SAME
        // region -- the per-element link scores parse into the pinned diagonal
        // / region-projection, never a cross-region pathway.
        let mut visits_row_sum = false;
        for (i, link) in fl.loop_info.links.iter().enumerate() {
            let to = link.to.as_str();
            if let Some(r) = to
                .strip_prefix("row_sum[")
                .and_then(|rest| rest.strip_suffix("]"))
            {
                visits_row_sum = true;
                let from = link.from.as_str();
                assert!(
                    from.starts_with(&format!("pop[{r},")),
                    "link {i} of {:?} reads row_sum[{r}] from {from} -- a \
                     phantom cross-region hop",
                    fl.loop_info.id
                );
            }
            // A `row_sum[r] -> growth[...]` hop must broadcast within region r.
            if let Some(r) = link
                .from
                .as_str()
                .strip_prefix("row_sum[")
                .and_then(|rest| rest.strip_suffix("]"))
            {
                assert!(
                    link.to.as_str().starts_with(&format!("growth[{r},")),
                    "link {i} of {:?} broadcasts row_sum[{r}] to {} -- a \
                     phantom cross-region hop",
                    fl.loop_info.id,
                    link.to.as_str()
                );
            }
        }
        if visits_row_sum {
            row_sum_loops += 1;
        }
        // The per-timestep scores parse and stay finite.
        assert!(
            fl.scores.iter().all(|(_, v)| v.is_finite()),
            "discovered loop {} scores must stay finite",
            fl.loop_info.id
        );
        // Same-REGION: every node's region coordinate is consistent (the
        // direct loops are fully same-element; the row_sum loops mix the 1-D
        // `row_sum[r]` with 2-D `pop[r,a]`/`growth[r,a]`, but never two
        // regions).
        let regions: std::collections::BTreeSet<&str> = fl
            .loop_info
            .links
            .iter()
            .flat_map(|l| [l.from.as_str(), l.to.as_str()])
            .filter_map(|n| n.split_once('[').map(|(_, rest)| rest))
            .map(|rest| rest.split([',', ']']).next().unwrap_or(rest))
            .collect();
        assert_eq!(
            regions.len(),
            1,
            "discovered loop {} must be same-region (no phantom mixing); \
             nodes: {:?}",
            fl.loop_info.id,
            fl.loop_info.links
        );
    }
    assert_eq!(
        row_sum_loops, 4,
        "exactly the four region-paired row_sum circuits must be discovered"
    );
}

/// GH #525/GH #793 (aliased ThroughAgg routing boundary): the same
/// `(pop, out)` edge carries a HOISTED `SUM(pop[R,*])` site and a
/// mixed-subscript site inside a DECLINED `MEAN(w[R,*] * pop[R,young])`
/// (differing co-source slices, so the MEAN is not hoisted).
///
/// Before GH #793, routing was per-edge (`in_reducer && routed_aggs`), so
/// the declined MEAN site was absorbed by the SUM agg and the edge looked
/// fully attributed. The correct contract is louder: once a residual
/// unhoisted reducer read remains on the same edge, the edge is declined as
/// a unit. Emitting only the SUM agg halves would publish incomplete
/// attribution for `pop -> out` with no warning.
#[test]
fn aliased_through_agg_residual_strict_site_declines_edge() {
    let fixture = || {
        TestProject::new("aliased_through_agg")
            .with_sim_time(0.0, 8.0, 1.0)
            .named_dimension("R", &["r1", "r2"])
            .named_dimension("Age", &["young", "old"])
            .array_stock("pop[R,Age]", "100", &["growth"], &[], None)
            .array_aux_direct("w", vec!["R".into(), "Age".into()], "0.5", None)
            .array_aux_direct(
                "out",
                vec!["R".into()],
                "SUM(pop[R,*]) + MEAN(w[R,*] * pop[R,young])",
                None,
            )
            .array_flow("growth[R,Age]", "out[R] * 0.0001", None)
            .build_datamodel()
    };

    let assert_declined =
        |ltm: &[LtmSyntheticVar], warnings: &[simlin_engine::db::Diagnostic], mode: &str| {
            assert_eq!(
                warnings.len(),
                1,
                "{mode}: expected one warning for the residual strict pop -> out \
             read; got: {warnings:?}"
            );
            let DiagnosticError::Assembly(msg) = &warnings[0].error else {
                unreachable!("filtered to Assembly above");
            };
            assert!(
                msg.contains("pop -> out") && msg.contains("pop[r,young]"),
                "{mode}: warning must name the declined edge and strict residual \
             slice; got: {msg}"
            );
            assert!(
                !ltm.iter().any(|v| {
                    v.name.starts_with(LINK_SCORE_PREFIX)
                        && (v.name == format!("{LINK_SCORE_PREFIX}pop\u{2192}out")
                            || (v.name.starts_with(&format!("{LINK_SCORE_PREFIX}pop["))
                                && v.name.contains("\u{2192}out")))
                }),
                "{mode}: the partially-unscoreable pop -> out edge must emit no \
             original-edge pop -> out score; got: {:?}",
                ltm.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
            assert!(
                !ltm.iter().any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
                "{mode}: loops through the declined pop -> out edge must be \
             dropped, not scored from partial agg attribution; got: {:?}",
                ltm.iter()
                    .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
            );
        };

    // Exhaustive surface: the residual strict read declines the edge before
    // the SUM agg halves are emitted.
    let project = fixture();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    compile_project_incremental(&db, sync.project, "main")
        .expect("the aliased-routing fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    assert_declined(&ltm.vars, &warnings, "exhaustive");

    // Discovery surface: same drop contract. The pinned-loop pass consumes
    // `unscoreable_edges`, so no loop survives by multiplying the sibling agg
    // halves.
    let project = fixture();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    compile_project_incremental(&db, sync.project, "main")
        .expect("the aliased-routing fixture must compile in discovery mode");
    let warnings = assembly_warnings(&db, sync.project);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    assert_declined(&ltm.vars, &warnings, "discovery");
}

/// GH #525 (T6 review corner pin): a `PerElement` target body that also
/// references ANOTHER arrayed dep by iterated subscript --
/// `row_sum[Region] = pop[Region, young] * other[Region]`. The per-(row,
/// element) equation must pin `other[Region]` to the target element
/// (`subscript_idents_at_element`'s dimension-name pinning), and with the
/// constant `other = 2` the score is exact: the live `pop[r,young]` term
/// reproduces delta-row_sum, so every scalar reads +1.
///
/// Parent (`ad6bdeb8`) evidence: this fixture emitted the merged Bare
/// `pop→row_sum` score and a phantom-bearing u1..u5 loop set; the names
/// asserted here did not exist (the test fails there).
#[test]
fn per_element_body_with_iterated_other_dep_scores() {
    let project = TestProject::new("per_element_other_dep")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_stock("pop[Region,Age]", "100", &["growth"], &[], None)
        .array_aux_direct("other", vec!["Region".into()], "2", None)
        .array_aux_direct(
            "row_sum",
            vec!["Region".into()],
            "pop[Region, young] * other[Region]",
            None,
        )
        .array_flow("growth[Region,Age]", "row_sum[Region] * 0.0001", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the other-dep fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "every LTM fragment must compile; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name == format!("{LINK_SCORE_PREFIX}pop\u{2192}row_sum")),
        "the merged Bare score must not exist; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    for r in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}pop[{r},young]\u{2192}row_sum[{r}]");
        // The other dep is pinned to the element (qualified, so the frozen
        // PREVIOUS compiles to a direct LoadPrev), not left as an
        // unresolvable bare dimension index.
        let var = ltm_var(&ltm.vars, &name);
        let eqn = match &var.equation {
            datamodel::Equation::Scalar(t) => t.clone(),
            other => format!("{other:?}"),
        };
        assert!(
            eqn.contains(&format!("other[region\u{B7}{r}]")),
            "{name} must pin other[Region] to the target element; eqn: {eqn}"
        );
        let series = series_at(&results, offset_of(&results, &name));
        assert_eq!(series[0], 0.0, "initial-step guard pins {name} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "{name} step {step} must score +1 (other is constant, the \
                 live pop term reproduces delta-row_sum); got {series:?}"
            );
        }
    }
    // The unread `old` rows have no scores.
    let score_names = ltm_score_var_names(&results);
    assert!(
        !score_names
            .iter()
            .any(|n| n.contains("old]\u{2192}row_sum")),
        "unread rows must carry no pop\u{2192}row_sum scores; got: {score_names:?}"
    );
    // Loops through the hop are scored with real values.
    let mut saw_nonzero = false;
    for lv in ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
    {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(series.iter().all(|v| v.is_finite()));
            saw_nonzero |= series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0);
        }
    }
    assert!(saw_nonzero, "loops through the per-element hop must score");
}

/// GH #525 (T6 review corner pin): a MAPPED `PerElement` reference --
/// `mid[State] = pop[State, young] * 0.05` over `pop[Region, Age]` with a
/// positional `State→Region` mapping -- exercises
/// `per_element_row_for_target`'s `mapped_element_correspondence` arm: the
/// row's Region element is the positional preimage of the target's State
/// element (s1↔r1, s2↔r2), so the emitted names carry the SOURCE-dim row
/// and the diagonal only.
///
/// Parent (`ad6bdeb8`) evidence: this fixture classified `DynamicIndex`
/// and took the GH #758 loud skip -- NO `pop→mid` score of any form plus
/// exactly one unscoreable-edge Warning -- so both the zero-warnings and
/// the name assertions here fail there (verified in a temp worktree).
#[test]
fn mapped_per_element_subscript_scores_positional_diagonal() {
    let project = TestProject::new("mapped_per_element")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("pop[Region,Age]", "100", &["inflow"], &[], None)
        .array_aux_direct(
            "mid",
            vec!["State".into()],
            "pop[State, young] * 0.05",
            None,
        )
        .scalar_aux("total", "SUM(mid[*])")
        .array_flow("inflow[Region,Age]", "total * 0.0001", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the mapped PerElement fixture must compile with LTM enabled");
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the mapped PerElement edge is scoreable; no warning may fire \
         (pre-T6 it took the GH #758 loud skip): {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // The positional diagonal, with the row carrying the SOURCE (Region)
    // element: exactly these two names, scoring exactly +1 (pop[r,young]
    // is each mid slot's only moving input).
    for (s, r) in [("s1", "r1"), ("s2", "r2")] {
        let name = format!("{LINK_SCORE_PREFIX}pop[{r},young]\u{2192}mid[{s}]");
        let series = series_at(&results, offset_of(&results, &name));
        assert_eq!(series[0], 0.0, "initial-step guard pins {name} to 0");
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "{name} step {step} must score +1; got {series:?}"
            );
        }
    }
    let score_names = ltm_score_var_names(&results);
    assert!(
        !score_names
            .iter()
            .any(|n| n.contains("pop[r1,young]\u{2192}mid[s2]")
                || n.contains("pop[r2,young]\u{2192}mid[s1]")
                || n.contains("old]\u{2192}mid")
                || *n == format!("{LINK_SCORE_PREFIX}pop\u{2192}mid")),
        "only the positional diagonal's young rows may carry scores; got: {score_names:?}"
    );

    // The loop through the mapped hop is scored with real values.
    let mut saw_nonzero = false;
    for lv in ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
    {
        let series = series_at(&results, offset_of(&results, &lv.name));
        assert!(series.iter().all(|v| v.is_finite()));
        saw_nonzero |= series.iter().skip(STARTUP_STEPS).any(|&v| v != 0.0);
    }
    assert!(
        saw_nonzero,
        "loops through the mapped PerElement hop must carry real values"
    );
}

/// GH #746: for a feedback cycle through an ARRAYED variable the detected
/// surface (`model_detected_loops`) and the scored surface
/// (`model_ltm_variables`) must enumerate THE SAME loop set with identical
/// ids, because the runtime join (`reclassify_loops_from_results`, pysimlin's
/// `get_relative_loop_score`) reads `$⁚ltm⁚loop_score⁚{id}` keyed purely on
/// the detected id.
///
/// Pre-fix the two surfaces enumerated DIFFERENT loop sets here: detected
/// reported one variable-level loop per cycle ({feeder, pool/growth} -- two
/// ids), while scored enumerated the pool/growth cycle once as an A2A loop
/// plus the feeder cycle PER ELEMENT ({A2A, feeder[a], feeder[b]} -- three
/// ids), both numbering their own sequence from the shared `r{n}` namespace.
/// The detected pool/growth loop's id then resolved to a per-slot FEEDER
/// loop's scalar series: a silent wrong-series join on the production FFI
/// surface (and `loop_partitions[id]` reported 1 slot where the true A2A
/// series has 2).
#[test]
fn gh746_arrayed_cycle_detected_and_scored_ids_biject() {
    let project = TestProject::new("gh746_arrayed_cycle")
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
        .scalar_aux("scale", "0.001 * SUM(pool[*]) + 0.01")
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
        detected_ids,
        scored_ids,
        "arrayed cycle: detected ids must equal the scored loop-score ids (the runtime \
         join is keyed purely on the id); detected loops: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );

    // The pool/growth cycle is the A2A loop: its id must resolve to the
    // ARRAYED loop-score series (one slot per r1 element). Pre-fix the
    // detected pool/growth loop's id landed on a per-slot feeder loop's
    // SCALAR series instead.
    let a2a_loop = detected
        .loops
        .iter()
        .find(|l| !l.variables.iter().any(|v| v == "scale"))
        .expect("the pool/growth A2A loop must be detected");
    assert_eq!(
        ltm.loop_partitions
            .get(&a2a_loop.id)
            .map(|slots| slots.len()),
        Some(2),
        "the detected pool/growth loop's id must join the 2-slot A2A series \
         (loop_partitions: {:?})",
        ltm.loop_partitions
    );

    // The feeder cycle surfaces per element, mirroring the scored surface.
    let feeder_loops: Vec<_> = detected
        .loops
        .iter()
        .filter(|l| l.variables.iter().any(|v| v == "scale"))
        .collect();
    assert_eq!(
        feeder_loops.len(),
        2,
        "one detected feeder loop per r1 element; got: {:?}",
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

    // Runtime join: every detected loop classifies from its OWN series. All
    // hops are positive here (matrix > 0, both reducers SUM), so every loop
    // is reinforcing both structurally and at runtime.
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

/// GH #766 (shape-expressiveness T1): an INLINE reducer over a *proper
/// subdimension* StarRange (`x = 1 + MEAN(arr[*:Core])`, `Core = {a, b}` a
/// proper subdimension of `arr`'s `Region = {a, b, c}`) hoists into a
/// synthetic agg whose read slice carries the SUBSET, so:
///
/// - only the subset rows get `arr[<row>] → agg` link scores -- no
///   `arr[c] → agg` score variable exists (pre-fix the full parent extent
///   was enumerated, minting a spurious `arr[c]` score);
/// - the MEAN divisor is the SUBSET size (2), not the parent extent (3):
///   the two subset rows grow by identical per-step deltas, so each row's
///   changed-first linear partial reads exactly `(Δrow/2) / Δagg = 0.5`
///   (the pre-fix full-extent divisor read 1/3);
/// - no feedback loop is enumerated through the unread `arr[c]` row
///   (pre-fix the element graph routed `arr[c] → agg`, so the enumerator
///   discovered a loop the reducer never closes).
///
/// The `c` row grows 5x faster than the subset rows, so the agg-value
/// assertion also pins that the SIMULATION reduces over the subset only
/// (the agg aux keeps the original `mean(arr[*:core])` equation text --
/// the compiled simulation already evaluates the subset; only LTM's row
/// enumeration was coarse).
#[test]
fn inline_subset_reducer_mean_divisor_and_scores() {
    let project = TestProject::new("gh766_inline_subset")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_with_ranges("factor[Region]", vec![("a", "1"), ("b", "1"), ("c", "5")])
        .scalar_aux("x", "1 + MEAN(arr[*:Core])")
        .array_flow("inflow[Region]", "x * 0.1 * factor[Region]", None)
        .array_stock("arr[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "subset reducer must compile with zero warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Only the subset rows are scored into the agg.
    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}{agg}");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected subset-row link score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    let phantom = format!("{LINK_SCORE_PREFIX}arr[c]\u{2192}{agg}");
    assert!(
        ltm.vars.iter().all(|v| v.name != phantom),
        "the unread `arr[c]` row must get NO link score into the agg; got {phantom:?}"
    );

    // No enumerated loop traverses the unread `arr[c]` row.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v == "arr[c]"),
            "no loop may run through the unread arr[c] row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // The agg value is the mean over the SUBSET only; `c` diverges from the
    // subset rows after the first step, so a full-extent mean would differ.
    let agg_series = series_at(&results, offset_of(&results, agg));
    let arr_a = series_at(&results, offset_of(&results, "arr[a]"));
    let arr_b = series_at(&results, offset_of(&results, "arr[b]"));
    let arr_c = series_at(&results, offset_of(&results, "arr[c]"));
    for step in 0..agg_series.len() {
        let subset_mean = (arr_a[step] + arr_b[step]) / 2.0;
        assert!(
            (agg_series[step] - subset_mean).abs() < 1e-9,
            "agg at step {step} must be the subset mean {subset_mean}; got {}",
            agg_series[step]
        );
    }
    assert!(
        arr_c.last().unwrap() > arr_a.last().unwrap(),
        "fixture sanity: the out-of-subset row must diverge from the subset rows"
    );

    // The MEAN divisor is the subset size: with Δarr[a] == Δarr[b] each
    // subset row's link score is exactly (Δrow/2)/Δagg = 0.5 once PREVIOUS
    // has one step of history. (The buggy full-extent divisor read 1/3.)
    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}{agg}");
        let series = series_at(&results, offset_of(&results, &name));
        assert_eq!(
            series[0], 0.0,
            "initial-step guard pins arr[{row}]→agg to 0"
        );
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-9,
                "arr[{row}]→agg at step {step} must score 0.5 (subset divisor); got {series:?}"
            );
        }
    }

    // The subset loops (arr[a]/arr[b] → agg → x → inflow → arr) carry real,
    // finite, non-zero scores once the flow-to-stock startup guard clears.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(
        !loop_vars.is_empty(),
        "the subset feedback loops must be scored"
    );
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
                assert!(
                    v.is_finite() && v != 0.0,
                    "loop score {} slot {slot} step {step} must be finite and non-zero; \
                     got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// GH #766 (composition, end-to-end): a subset StarRange composed with an
/// iterated axis -- `out[D1] = 1 + SUM(matrix[D1, *:SubD2])` closed in a
/// feedback loop -- compiles with zero warnings and scores. The synthetic
/// agg is ARRAYED over `D1` (`read_slice = [Iterated(d1), Reduced{subset}]`,
/// pinned by `star_range_subset_composes_with_iterated_axis`); this fixture
/// pins the emission side:
///
/// - per-(row, slot) link scores exist ONLY for the subset rows of each
///   slot (`matrix[a,x]→agg[a]`, never `matrix[a,z]→agg[a]`);
/// - with the two subset rows of a slot changing by identical deltas, each
///   row's SUM partial reads exactly (Δrow/Δslot) = 0.5;
/// - no loop is enumerated through the unread `z` rows, and every emitted
///   loop score is finite and non-zero once the startup guard clears.
#[test]
fn iterated_subset_reducer_loop_scores_end_to_end() {
    let project = TestProject::new("gh766_iterated_subset")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y", "z"])
        .named_dimension("SubD2", &["x", "y"])
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_aux_direct(
            "out",
            vec!["D1".into()],
            "1 + SUM(matrix[D1, *:SubD2])",
            None,
        )
        .array_flow("inflow[D1]", "out[D1]", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "iterated+subset reducer must compile with zero warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Per-(row, slot) scores for the subset rows of each slot only.
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}{agg}[{d1}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected subset-row link score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
        }
        let phantom = format!("{LINK_SCORE_PREFIX}matrix[{d1},z]\u{2192}{agg}[{d1}]");
        assert!(
            ltm.vars.iter().all(|v| v.name != phantom),
            "the unread z row must get NO link score into the agg slot; got {phantom:?}"
        );
    }

    // No enumerated loop traverses the unread z rows.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the subset loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v.contains(",z]")),
            "no loop may run through an unread z row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // SUM over the 2-row subset with equal per-step deltas: each subset
    // row's changed-first partial is exactly half the slot's change.
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}{agg}[{d1}]");
            let series = series_at(&results, offset_of(&results, &name));
            assert_eq!(
                series[0], 0.0,
                "initial-step guard pins matrix[{d1},{d2}]→agg[{d1}] to 0"
            );
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "matrix[{d1},{d2}]→agg[{d1}] at step {step} must score 0.5; got {series:?}"
                );
            }
        }
    }

    // Every emitted loop score is finite and non-zero post-startup.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the subset loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS) {
                assert!(
                    v.is_finite() && v != 0.0,
                    "loop score {} slot {slot} step {step} must be finite and non-zero; \
                     got {series:?}",
                    lv.name
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GH #765 / shape-expressiveness T3: variable-backed reduce slices score by
// their read slice (Pinned axes fixed, subset axes enumerated, unread rows
// silent), and the one inexpressible residual degrades loudly.
// ---------------------------------------------------------------------------

/// GH #765 (the headline fixture): a variable-backed Pinned-mixed partial
/// reduce in a feedback loop -- `outf[D1] = MEAN(cube[D1,x,*])` over
/// `cube[D1,D2,D3]`, closed through a `D1` stock. The per-result-row read
/// slice is `{cube[d1,x,p], cube[d1,x,q]}` -- 2 cells -- so:
///
/// - the MEAN divisor is 2, not the full-cartesian 4: with both read rows
///   changing by identical deltas each row's link score reads exactly 0.5
///   (the pre-fix full-cartesian co-reduced slice read 0.25 -- the silent
///   wrong-divisor value this fixture's 0.5 assertion guards, which is also
///   the design's atomicity guard: deleting the gate's Pinned exclusion
///   without the `read_slice_rows` derivation swap re-fires it);
/// - the unread `cube[*,y,*]` rows get NO link score (pre-fix they got
///   delta-ratio garbage of constant +1) and no enumerated loop traverses
///   them (pre-fix: 16 warned 0-stub loop scores through the cross-product
///   edges, plus one silently-wrong 0.25 loop);
/// - zero assembly warnings: every emitted fragment compiles, the loud
///   conservative regime this shape used to ride is gone.
#[test]
fn pinned_mixed_reduce_divisor_and_scores() {
    let project = TestProject::new("gh765_pinned_mixed")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_aux_direct("outf", vec!["D1".into()], "MEAN(cube[D1, x, *])", None)
        .array_flow("inflow[D1]", "outf[D1]", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the Pinned-mixed variable-backed reduce must compile with zero \
         warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // Read rows only: the x slab of each D1 row gets a per-(row, slot)
    // score; the y slab gets nothing.
    for d1 in ["a", "b"] {
        for d3 in ["p", "q"] {
            let name = format!("{LINK_SCORE_PREFIX}cube[{d1},x,{d3}]\u{2192}outf[{d1}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected read-row link score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
            let phantom = format!("{LINK_SCORE_PREFIX}cube[{d1},y,{d3}]\u{2192}outf[{d1}]");
            assert!(
                ltm.vars.iter().all(|v| v.name != phantom),
                "the unread y row must get NO link score; got {phantom:?}"
            );
        }
    }

    // No enumerated loop traverses an unread y row.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the read-row loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v.contains(",y,")),
            "no loop may run through an unread y row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // MEAN over the 2-cell read slice with equal per-step deltas: each read
    // row's changed-first partial is exactly (Δrow/2)/Δoutf = 0.5. The
    // pre-fix full-cartesian divisor read 0.25.
    for d1 in ["a", "b"] {
        for d3 in ["p", "q"] {
            let name = format!("{LINK_SCORE_PREFIX}cube[{d1},x,{d3}]\u{2192}outf[{d1}]");
            let series = series_at(&results, offset_of(&results, &name));
            assert_eq!(series[0], 0.0, "initial-step guard pins {name} to 0");
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step} must score 0.5 (read-slice divisor 2, \
                     not the full-cartesian 4 => 0.25); got {series:?}"
                );
            }
        }
    }

    // Each per-circuit loop score is the product 1 * 0.5 * 1 * 1 = 0.5 once
    // the flow-to-stock startup guard clears.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the read-row loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "loop score {} slot {slot} step {step} must read 0.5; got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// GH #766 x T3: a VARIABLE-BACKED subset partial reduce in a feedback loop
/// -- `out[D1] = MEAN(matrix[D1,*:SubD2])` as the whole RHS (the inline
/// sibling is covered by `iterated_subset_reducer_loop_scores_end_to_end`;
/// this shape was excluded from the variable-backed gate until T3). The
/// per-slot read slice is the 2-element subset, so each subset row scores
/// (Δrow/2)/Δout = 0.5, the unread `z` rows get no score and no loop, and
/// the model compiles with zero warnings.
#[test]
fn variable_backed_subset_reduce_divisor_and_scores() {
    let project = TestProject::new("gh766_vb_subset")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y", "z"])
        .named_dimension("SubD2", &["x", "y"])
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_aux_direct("out", vec!["D1".into()], "MEAN(matrix[D1, *:SubD2])", None)
        .array_flow("inflow[D1]", "out[D1]", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the subset variable-backed reduce must compile with zero warnings; \
         got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}out[{d1}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected subset-row link score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
        }
        let phantom = format!("{LINK_SCORE_PREFIX}matrix[{d1},z]\u{2192}out[{d1}]");
        assert!(
            ltm.vars.iter().all(|v| v.name != phantom),
            "the unread z row must get NO link score; got {phantom:?}"
        );
    }

    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the subset loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v.contains(",z]")),
            "no loop may run through an unread z row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}out[{d1}]");
            let series = series_at(&results, offset_of(&results, &name));
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step} must score 0.5 (subset divisor); got {series:?}"
                );
            }
        }
    }
}

/// Shape-expressiveness section 6 (scalar owner, Pinned slice):
/// `total = SUM(pop[nyc,*])` in a feedback loop. The gate admits the
/// scalar-result slice for a SCALAR owner -- the slot is the bare `total`
/// node -- so element edges and link scores both cover exactly the read
/// rows (`pop[nyc,*]`), and they match:
///
/// - `pop[nyc,p]→total` / `pop[nyc,q]→total` exist and read 0.5 (equal
///   deltas, Δtotal twice each row's delta);
/// - the unread `pop[boston,*]` rows get NO score (pre-T3 the
///   full-cartesian derivation emitted nonzero garbage for them) and no
///   loop runs through boston at all -- crucially no warned-phantom
///   circuit through unread rows;
/// - zero assembly warnings.
#[test]
fn scalar_owner_pinned_slice_reduce_scores_read_rows_only() {
    let project = TestProject::new("t3_scalar_owner_pinned")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .scalar_aux("total", "SUM(pop[nyc, *])")
        .array_flow("inflow[Region]", "total * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the scalar-owner pinned slice must compile with zero warnings; \
         got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    for d2 in ["p", "q"] {
        let name = format!("{LINK_SCORE_PREFIX}pop[nyc,{d2}]\u{2192}total");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected read-row link score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        let phantom = format!("{LINK_SCORE_PREFIX}pop[boston,{d2}]\u{2192}total");
        assert!(
            ltm.vars.iter().all(|v| v.name != phantom),
            "the unread boston row must get NO link score; got {phantom:?}"
        );
    }

    // No loop traverses boston at all: the only edges into `total` come from
    // the nyc rows, so boston's stock cannot close a circuit.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the read-row loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v.contains("boston")),
            "no loop may run through the unread boston rows; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    for d2 in ["p", "q"] {
        let name = format!("{LINK_SCORE_PREFIX}pop[nyc,{d2}]\u{2192}total");
        let series = series_at(&results, offset_of(&results, &name));
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-9,
                "{name} at step {step} must score 0.5; got {series:?}"
            );
        }
    }

    // Per-circuit loop scores: 1 * 0.5 * 1 * 1 = 0.5 post-startup.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the read-row loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "loop score {} slot {slot} step {step} must read 0.5; got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// Shape-expressiveness section 6 (scalar owner, subset slice):
/// `total = SUM(arr[*:Core])` in a feedback loop, `Core = {a, b}` a proper
/// subdimension of `Region = {a, b, c}`. The scalar-result subset slice is
/// admitted for the scalar owner, so only the subset rows get element edges
/// and link scores (each 0.5 with equal subset deltas), `arr[c]` gets
/// neither a score nor a loop, and the model compiles with zero warnings.
#[test]
fn scalar_owner_subset_slice_reduce_scores_read_rows_only() {
    let project = TestProject::new("t3_scalar_owner_subset")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_with_ranges("factor[Region]", vec![("a", "1"), ("b", "1"), ("c", "5")])
        .scalar_aux("total", "SUM(arr[*:Core])")
        .array_flow("inflow[Region]", "total * 0.001 * factor[Region]", None)
        .array_stock("arr[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the scalar-owner subset slice must compile with zero warnings; \
         got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}total");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected subset-row link score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    let phantom = format!("{LINK_SCORE_PREFIX}arr[c]\u{2192}total");
    assert!(
        ltm.vars.iter().all(|v| v.name != phantom),
        "the unread arr[c] row must get NO link score; got {phantom:?}"
    );

    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the subset loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v == "arr[c]"),
            "no loop may run through the unread arr[c] row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}total");
        let series = series_at(&results, offset_of(&results, &name));
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-9,
                "{name} at step {step} must score 0.5 (subset divisor); got {series:?}"
            );
        }
    }
}

/// GH #777 (shape-expressiveness section-6 deferred completion): an
/// ARRAYED-owner Pinned broadcast slice -- `share[Region] = SUM(pop[nyc,*])`,
/// a scalar-result slice broadcast over the owner's dims -- is now SCORED via
/// the per-(read-row, full-target-element) broadcast rule (the design's
/// section-3 `PerElement` rule applied to a variable-backed reducer owner),
/// not loud-skipped.
///
/// `share[e] = SUM(pop[nyc,p] + pop[nyc,q])` for EVERY `e`, with both rows
/// changing equally (`pop[nyc,d2] = stock[nyc]*0.1`). So each link score
/// `pop[nyc,d2]→share[e]` is the SUM contribution share -- one row's delta
/// over the sum of both equal deltas = 0.5 -- for all four (row, element)
/// pairs (`{nyc,p},{nyc,q}` x `{nyc,boston}`), including the broadcast row
/// `share[boston]` (which reads `pop[nyc,*]`, not `pop[boston,*]`). No
/// `pop[boston,*]→share[*]` score exists (the unread rows). Loops close only
/// through `share[nyc]` (only `pop[nyc,*]` is read, and only `stock[nyc]`
/// drives it -- `stock[boston] → pop[boston,*]` is a dead end), so the two
/// per-circuit loops (one per D2 element) score 1*1*0.5*1 = 0.5.
///
/// Pre-fix this shape took a loud GH #758 skip; pre-T3 it was SILENTLY WRONG
/// (full-cartesian per-(row, slot) garbage: `pop[boston,*]→share[boston]`
/// scored a constant +1.0 although the reducer never reads those rows).
#[test]
fn arrayed_owner_broadcast_pinned_slice_reduce_scores_read_rows_broadcast() {
    let project = TestProject::new("gh777_broadcast_pinned")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("share", vec!["Region".into()], "SUM(pop[nyc, *])", None)
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Zero warnings: the broadcast is fully scored, no loud skip.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the broadcast slice must compile with zero warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // Exactly the (read-row x full-target-element) link scores: the two read
    // rows `pop[nyc,p]`, `pop[nyc,q]` fanned across the FULL target element
    // set `share[nyc]`, `share[boston]`. The unread `pop[boston,*]` rows get
    // NO score.
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[nyc,{d2}]\u{2192}share[{e}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected broadcast link score {name:?}; have: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.starts_with(LINK_SCORE_PREFIX)
                        && v.name.contains("pop")
                        && v.name.contains("share"))
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
            );
            let phantom = format!("{LINK_SCORE_PREFIX}pop[boston,{d2}]\u{2192}share[{e}]");
            assert!(
                ltm.vars.iter().all(|v| v.name != phantom),
                "the unread boston row must get NO link score; got {phantom:?}"
            );
        }
    }

    // No loop traverses boston at all: only `pop[nyc,*]` is read, and only
    // `stock[nyc]` drives it, so the only circuits close through `share[nyc]`.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the read-row loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v.contains("boston")),
            "no loop may run through boston; loop {}: {:?}",
            l.id,
            l.variables
        );
        // Detected and scored surfaces agree: every detected loop has a
        // scored loop-score variable.
        let score_name = format!("{LOOP_SCORE_PREFIX}{}", l.id);
        assert!(
            ltm.vars.iter().any(|v| v.name == score_name),
            "detected loop {} must have a scored loop-score variable {score_name:?}",
            l.id
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Each of the four (row, element) link scores reads 0.5 (the SUM
    // contribution share: one of two equal deltas).
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[nyc,{d2}]\u{2192}share[{e}]");
            let series = series_at(&results, offset_of(&results, &name));
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step} must score 0.5; got {series:?}"
                );
            }
        }
    }

    // Per-circuit loop scores: 1 * 1 * 0.5 * 1 = 0.5 post-startup, finite.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the read-row loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    v.is_finite() && (v - 0.5).abs() < 1e-9,
                    "loop score {} slot {slot} step {step} must read 0.5; got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// GH #777, the DISJOINT-dim spelling: `share[D9] = SUM(arr[*:Core])`, where
/// `D9 = {m, n}` is DISJOINT from `arr`'s dim `Region`. The broadcast rule
/// expresses this IDENTICALLY to the related-dim spelling -- every `share[e]`
/// reads the same `arr[*:Core]` slice -- so the per-(read-row,
/// full-target-element) scores `arr[{row}]→share[{e}]` are emitted for the
/// subset rows x the full `D9` element set, the model compiles with zero
/// warnings, and the loop (closed through a scalar `total = SUM(share[*])`
/// so no dim-mismatch edge is needed) is scored and finite. This pins the
/// loud-skip-vs-broadcast scope decision: the disjoint spelling fires the
/// broadcast branch BEFORE `try_cross_dimensional_link_scores`'
/// `result_axis_names` containment check (which would early-return `None`),
/// and the disjoint hop keeps both subscripts via `is_broadcast_reduce_edge`
/// (where `is_partial_reduce_edge`'s containment check fails). No spelling is
/// silent-wrong.
#[test]
fn arrayed_owner_broadcast_disjoint_dim_slice_reduce_scores_read_rows() {
    let project = TestProject::new("gh777_broadcast_disjoint")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .named_dimension("D9", &["m", "n"])
        .array_with_ranges("factor[Region]", vec![("a", "1"), ("b", "1"), ("c", "5")])
        .array_aux_direct("share", vec!["D9".into()], "SUM(arr[*:Core])", None)
        .scalar_aux("total", "SUM(share[*])")
        .array_flow("inflow[Region]", "total * 0.0005 * factor[Region]", None)
        .array_stock("arr[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the disjoint-dim broadcast slice must compile with zero warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    // Subset rows (arr[a], arr[b]) x the FULL D9 target element set ({m, n}).
    for row in ["a", "b"] {
        for e in ["m", "n"] {
            let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}share[{e}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected disjoint broadcast link score {name:?}; have: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.starts_with(LINK_SCORE_PREFIX)
                        && v.name.contains("arr")
                        && v.name.contains("share"))
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
        // The unread arr[c] row never scores.
        for e in ["m", "n"] {
            let phantom = format!("{LINK_SCORE_PREFIX}arr[c]\u{2192}share[{e}]");
            assert!(
                ltm.vars.iter().all(|v| v.name != phantom),
                "the unread arr[c] row must get NO link score; got {phantom:?}"
            );
        }
    }

    // Loops are scored and finite (the loop hops resolve the per-(row, e)
    // names; no silent constant-0 stubs).
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the broadcast loops must exist");
    for l in &detected.loops {
        let score_name = format!("{LOOP_SCORE_PREFIX}{}", l.id);
        assert!(
            ltm.vars.iter().any(|v| v.name == score_name),
            "detected loop {} must have a scored loop-score variable",
            l.id
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the broadcast loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    v.is_finite(),
                    "loop score {} slot {slot} step {step} must be finite (no stub); \
                     got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// GH #777, the subset twin: an ARRAYED-owner subset broadcast slice --
/// `share[Region] = SUM(arr[*:Core])`, `Core = {a, b}` a proper
/// subdimension of `Region = {a, b, c}` -- is scored by the same broadcast
/// rule. The reducer reads only the subset rows `arr[a]`, `arr[b]`, and its
/// single scalar value broadcasts over every `share[e]`, so each link score
/// `arr[{row}]→share[{e}]` exists for `row in {a,b}` x `e in {a,b,c}` (the
/// MEAN/SUM divisor is the SUBSET size, not the full extent), `arr[c]` gets
/// no score, and the model compiles with zero warnings. The two subset rows
/// change equally (`factor[a]=factor[b]=1`), so each SUM contribution share
/// is 0.5. Loops close only through the read subset rows (`arr[c]` never
/// feeds `share`); they are scored, finite, and the detected/scored surfaces
/// agree.
#[test]
fn arrayed_owner_broadcast_subset_slice_reduce_scores_read_rows_broadcast() {
    let project = TestProject::new("gh777_broadcast_subset")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_with_ranges("factor[Region]", vec![("a", "1"), ("b", "1"), ("c", "5")])
        .array_aux_direct("share", vec!["Region".into()], "SUM(arr[*:Core])", None)
        .array_flow(
            "inflow[Region]",
            "share[Region] * 0.001 * factor[Region]",
            None,
        )
        .array_stock("arr[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the subset broadcast slice must compile with zero warnings; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    // Subset rows x full target element set; the unread `arr[c]` row never
    // scores.
    for row in ["a", "b"] {
        for e in ["a", "b", "c"] {
            let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}share[{e}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected subset broadcast link score {name:?}; have: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.starts_with(LINK_SCORE_PREFIX)
                        && v.name.contains("arr")
                        && v.name.contains("share"))
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
    for e in ["a", "b", "c"] {
        let phantom = format!("{LINK_SCORE_PREFIX}arr[c]\u{2192}share[{e}]");
        assert!(
            ltm.vars.iter().all(|v| v.name != phantom),
            "the unread arr[c] row must get NO link score; got {phantom:?}"
        );
    }

    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert!(!detected.loops.is_empty(), "the subset loops must exist");
    for l in &detected.loops {
        assert!(
            !l.variables.iter().any(|v| v == "arr[c]"),
            "no loop may run through the unread arr[c] row; loop {}: {:?}",
            l.id,
            l.variables
        );
        let score_name = format!("{LOOP_SCORE_PREFIX}{}", l.id);
        assert!(
            ltm.vars.iter().any(|v| v.name == score_name),
            "detected loop {} must have a scored loop-score variable {score_name:?}",
            l.id
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // Each subset-row link score reads 0.5 (subset divisor, equal deltas).
    for row in ["a", "b"] {
        for e in ["a", "b", "c"] {
            let name = format!("{LINK_SCORE_PREFIX}arr[{row}]\u{2192}share[{e}]");
            let series = series_at(&results, offset_of(&results, &name));
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step} must score 0.5 (subset divisor); got {series:?}"
                );
            }
        }
    }

    // Loop scores are finite and non-zero post-startup.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the subset loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    v.is_finite(),
                    "loop score {} slot {slot} step {step} must be finite; got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// GH #777 (risk 4: the forced-DISCOVERY twin). The broadcast reduce's
/// per-(read-row, full-target-element) names (`arr[a]→share[b]`) are a new
/// producer of the element-in-name grammar on a REDUCER edge, so discovery's
/// `parse_link_offsets` must resolve them symmetrically with the exhaustive
/// surface (the #748/#698 lesson). With every variable 1-D over `Region`
/// (the subset reducer reads `arr[*:Core]`, broadcasting over `Region`), the
/// broadcast loops are fully traceable: discovery finds the two same-element
/// circuits (`arr[r]→share[r]→inflow[r]→arr[r]`, score 0.5) AND the genuine
/// cross-element circuit the broadcast creates (`arr[a]→share[b]→...→
/// arr[b]→share[a]→...→arr[a]`, score 0.25 = 0.5*0.5). Every link reads
/// `share` from a Core member (`arr[a]`/`arr[b]`) -- never the unread
/// `arr[c]` -- so the new names produce no phantom pathway.
#[test]
fn gh777_broadcast_discovery_twin_parses_per_element_names() {
    let project = TestProject::new("gh777_broadcast_discovery")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_with_ranges("factor[Region]", vec![("a", "1"), ("b", "1"), ("c", "5")])
        .array_aux_direct("share", vec!["Region".into()], "SUM(arr[*:Core])", None)
        .array_flow(
            "inflow[Region]",
            "share[Region] * 0.001 * factor[Region]",
            None,
        )
        .array_stock("arr[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the broadcast repro")
    .loops;

    // The broadcast loops trace: the two same-element circuits plus the one
    // cross-element circuit the broadcast couples (arr[a]<->arr[b] via share).
    assert!(
        !found.is_empty(),
        "discovery must find the broadcast loops; got none"
    );
    for fl in &found {
        // No phantom: every hop reading `share[e]` must come from a Core
        // member (`arr[a]` or `arr[b]`) -- the broadcast reducer never reads
        // the out-of-subset `arr[c]`, so a discovered hop from it would be a
        // parse-corruption phantom.
        for link in &fl.loop_info.links {
            let to = link.to.as_str();
            if to.starts_with("share[") {
                let from = link.from.as_str();
                assert!(
                    from == "arr[a]" || from == "arr[b]",
                    "discovered hop {from}->{to} reads share from a non-Core \
                     row -- a phantom; loop {}: {:?}",
                    fl.loop_info.id,
                    fl.loop_info.links
                );
            }
            assert!(
                !link.from.as_str().contains("arr[c]") && !link.to.as_str().contains("arr[c]"),
                "no loop may run through the unread arr[c]; loop {}: {:?}",
                fl.loop_info.id,
                fl.loop_info.links
            );
        }
        // Scores parse and stay finite.
        assert!(
            fl.scores.iter().all(|(_, v)| v.is_finite()),
            "discovered loop {} scores must stay finite",
            fl.loop_info.id
        );
    }
}

/// T3 golden pin (the design's "explicit golden assertion"): the ALIGNED
/// variable-backed partial reduce (`inflow[D1] = SUM(matrix[D1,*])` in a
/// loop) keeps byte-identical per-(row, slot) link-score emissions across
/// the `read_slice_rows` derivation swap -- for an all-Iterated/full-extent
/// slice the read rows ARE the cartesian rows, in the same row-major order
/// with the same co-reduced grouping, so nothing about the emitted names or
/// equation text may change.
#[test]
fn aligned_partial_reduce_emissions_stay_byte_identical() {
    let project = TestProject::new("t3_aligned_golden")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_flow("inflow[D1]", "SUM(matrix[D1, *])", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // All four read rows, in row-major order, each a scalar var.
    let expected_names = [
        format!("{LINK_SCORE_PREFIX}matrix[a,x]\u{2192}inflow[a]"),
        format!("{LINK_SCORE_PREFIX}matrix[a,y]\u{2192}inflow[a]"),
        format!("{LINK_SCORE_PREFIX}matrix[b,x]\u{2192}inflow[b]"),
        format!("{LINK_SCORE_PREFIX}matrix[b,y]\u{2192}inflow[b]"),
    ];
    let emitted: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LINK_SCORE_PREFIX) && v.name.contains("matrix["))
        .collect();
    assert_eq!(
        emitted.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        expected_names
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        "aligned per-(row, slot) names (and their order) must not change"
    );

    // The exact equation text of the first row's score, captured at the T3
    // parent commit. Byte-identity here is the regression guard for the
    // derivation swap on already-correct shapes.
    let golden = "if (TIME = INITIAL_TIME) then 0 else if ((inflow[d1\u{B7}a] - \
                  PREVIOUS(inflow[d1\u{B7}a])) = 0) OR ((matrix[d1\u{B7}a,d2\u{B7}x] - \
                  PREVIOUS(matrix[d1\u{B7}a,d2\u{B7}x])) = 0) then 0 else \
                  SAFEDIV((PREVIOUS(inflow[d1\u{B7}a]) + (matrix[d1\u{B7}a,d2\u{B7}x] - \
                  PREVIOUS(matrix[d1\u{B7}a,d2\u{B7}x])) - PREVIOUS(inflow[d1\u{B7}a])), \
                  ABS((inflow[d1\u{B7}a] - PREVIOUS(inflow[d1\u{B7}a]))), 0) * \
                  SIGN((matrix[d1\u{B7}a,d2\u{B7}x] - PREVIOUS(matrix[d1\u{B7}a,d2\u{B7}x])))";
    match &emitted[0].equation {
        datamodel::Equation::Scalar(text) => assert_eq!(
            text, golden,
            "the aligned per-(row, slot) equation text must stay byte-identical"
        ),
        other => panic!("aligned per-(row, slot) score must be scalar; got {other:?}"),
    }
}

/// T3 review follow-up pin (the corrected dispatch-widening justification):
/// the OWN-dimension mixed StarRange family -- `out[D1] = SUM(matrix[D1,
/// *:D2])` where `*:D2` names the axis's own (full) dimension, closed in a
/// feedback loop. `classify_axis_access` resolves `*:D2` to
/// `Reduced{subset: None}` (full extent, no bare `*`), so the slice is
/// `[Iterated(d1), Reduced{None}]` and the variable-backed gate accepted it
/// even PRE-T3 -- but `classify_subscript_shape` says `DynamicIndex` (a
/// mixed iterated+StarRange subscript, the documented partial-StarRange
/// classifier residual), and the element-graph dispatch's old
/// `Wildcard`-only shape condition refused to route it while the loop
/// builder's routing consults only the gate. Pre-T3 that family was
/// therefore internally inconsistent: conservative CROSS-PRODUCT element
/// edges feeding per-circuit loop routing -- 4 phantom warned 0-stub loops
/// alongside the 4 real ones. The T3 `Wildcard | DynamicIndex` widening
/// makes the dispatch consistent with the gate, so the family now gets
/// first-class read-slice treatment:
///
/// - diagonal element edges only (`matrix[a,*] → out[a]`, never
///   `matrix[a,*] → out[b]`);
/// - per-(row, slot) scores for the diagonal only, each (Δrow/Δslot) = 0.5
///   with the two rows of a slot changing by identical deltas;
/// - exactly the 4 real loops, no phantoms, ZERO assembly warnings.
#[test]
fn own_dim_star_range_mixed_reduce_scores_read_slice() {
    let project = TestProject::new("t3_own_dim_star_range")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_aux_direct("out", vec!["D1".into()], "SUM(matrix[D1, *:D2])", None)
        .array_flow("inflow[D1]", "out[D1]", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the own-dim StarRange mixed reduce must compile with zero warnings \
         (pre-T3: phantom cross-product loop scores fail-warned); got: {warnings:?}"
    );

    // Diagonal element edges only.
    let edges = model_element_causal_edges(&db, sync.models["main"].source_model, sync.project);
    let has_edge = |f: &str, t: &str| edges.edges.get(f).is_some_and(|ts| ts.contains(t));
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            assert!(
                has_edge(&format!("matrix[{d1},{d2}]"), &format!("out[{d1}]")),
                "expected diagonal edge matrix[{d1},{d2}] -> out[{d1}]; edges: {:?}",
                edges.edges
            );
        }
    }
    assert!(
        !has_edge("matrix[a,x]", "out[b]") && !has_edge("matrix[b,x]", "out[a]"),
        "no cross-product edges may survive the read-slice routing; edges: {:?}",
        edges.edges
    );

    // Per-(row, slot) scores for the diagonal only.
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}out[{d1}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected read-row link score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
        }
        let other = if d1 == "a" { "b" } else { "a" };
        for d2 in ["x", "y"] {
            let phantom = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}out[{other}]");
            assert!(
                ltm.vars.iter().all(|v| v.name != phantom),
                "no off-diagonal score may be emitted; got {phantom:?}"
            );
        }
    }

    // Exactly the 4 real same-element circuits, no phantoms.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert_eq!(
        detected.loops.len(),
        4,
        "exactly the 4 read-row circuits (pre-T3: 4 real + 4 phantom); got: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );
    for l in &detected.loops {
        let elems: Vec<&str> = l
            .variables
            .iter()
            .filter_map(|v| {
                let start = v.find('[')?;
                Some(&v[start + 1..start + 2])
            })
            .collect();
        assert!(
            elems.windows(2).all(|w| w[0] == w[1]),
            "every loop must stay on one D1 element; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // SUM over the 2-row full extent with equal per-step deltas: 0.5/row.
    for d1 in ["a", "b"] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}out[{d1}]");
            let series = series_at(&results, offset_of(&results, &name));
            for (step, &v) in series.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step} must score 0.5; got {series:?}"
                );
            }
        }
    }

    // Each per-circuit loop score reads 1 * 0.5 * 1 * 1 = 0.5 post-startup.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert!(!loop_vars.is_empty(), "the read-row loops must be scored");
    for lv in &loop_vars {
        let base = offset_of(&results, &lv.name);
        for slot in 0..slot_count(lv, &project.dimensions) {
            let series = series_at(&results, base + slot);
            for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "loop score {} slot {slot} step {step} must read 0.5; got {series:?}",
                    lv.name
                );
            }
        }
    }
}

/// T3 review follow-up pin (intended in-scope churn, previously unfixtured):
/// the ALL-Pinned scalar-owner slice -- `total = SUM(pop[nyc,p])` in a
/// feedback loop. The slice `[Pinned(nyc), Pinned(p)]` is non-trivial with
/// no `Iterated` axis on a scalar owner, so the gate admits it and the
/// read-slice derivation enumerates exactly ONE read row:
///
/// - only the true `pop[nyc,p]→total` link score is emitted, reading +1
///   (`total` IS `pop[nyc,p]`, so the changed-first partial is exact);
///   pre-T3 the full-cartesian derivation emitted 4 scores -- the 3 unread
///   rows' constant delta-ratio +1.0 garbage alongside the true one;
/// - the single real loop (through `pop[nyc,p]`) scores +1 post-startup,
///   and no loop traverses an unread row;
/// - zero assembly warnings.
///
/// (The element edges were already correct pre-T3 -- the reference site
/// classifies `FixedIndex`, whose routing emits only the pinned row -- so
/// this flip is purely the score side catching up to the edges.)
#[test]
fn scalar_owner_all_pinned_slice_reduce_scores_single_row() {
    let project = TestProject::new("t3_all_pinned_scalar_owner")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .scalar_aux("total", "SUM(pop[nyc, p])")
        .array_flow("inflow[Region]", "total * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the all-Pinned scalar-owner slice must compile with zero warnings; \
         got: {warnings:?}"
    );

    // Only the single true score; none of the pre-T3 garbage names.
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let true_score = format!("{LINK_SCORE_PREFIX}pop[nyc,p]\u{2192}total");
    assert!(
        ltm.vars.iter().any(|v| v.name == true_score),
        "expected the single read-row link score {true_score:?}; have: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    for unread in ["nyc,q", "boston,p", "boston,q"] {
        let phantom = format!("{LINK_SCORE_PREFIX}pop[{unread}]\u{2192}total");
        assert!(
            ltm.vars.iter().all(|v| v.name != phantom),
            "the unread pop[{unread}] row must get NO link score (pre-T3 it \
             read constant +1.0 garbage); got {phantom:?}"
        );
    }

    // The single real loop; nothing through unread rows.
    let detected = model_detected_loops(&db, sync.models["main"].source_model, sync.project);
    assert_eq!(
        detected.loops.len(),
        1,
        "exactly one circuit (stock[nyc] -> pop[nyc,p] -> total -> \
         inflow[nyc] -> stock[nyc]); got: {:?}",
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
                .any(|v| v.contains("boston") || v.contains(",q")),
            "no loop may run through an unread row; loop {}: {:?}",
            l.id,
            l.variables
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    // `total` IS `pop[nyc,p]`: the changed-first partial reads exactly +1.
    let series = series_at(&results, offset_of(&results, &true_score));
    assert_eq!(series[0], 0.0, "initial-step guard pins {true_score} to 0");
    for (step, &v) in series.iter().enumerate().skip(1) {
        assert!(
            (v - 1.0).abs() < 1e-9,
            "{true_score} at step {step} must score +1; got {series:?}"
        );
    }

    // The lone loop score is the product 1 * 1 * 1 * 1 = +1 post-startup.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(loop_vars.len(), 1, "exactly one loop score variable");
    let base = offset_of(&results, &loop_vars[0].name);
    let series = series_at(&results, base);
    for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
        assert!(
            (v - 1.0).abs() < 1e-9,
            "loop score {} step {step} must read +1; got {series:?}",
            loop_vars[0].name
        );
    }
}

// ---------------------------------------------------------------------------
// GH #764 (shape-expressiveness T4): non-aligned variable-backed reduces --
// result dims a BROADCAST (strict subset of the owner's dims) or a
// PERMUTATION (different order) of the owner's dims -- mint SYNTHETIC aggs
// (the GH #534 carve-out generalized) instead of keeping the conservative
// cross-product with no matching scores.
// ---------------------------------------------------------------------------

/// GH #764, the BROADCAST shape end-to-end: `out[D1,D3] = SUM(matrix[D1,*])`
/// (the reducer's result axis `D1` a strict subset of the owner's
/// `[D1,D3]`) closed in a feedback loop. Pre-T4 the `matrix -> out` edge
/// took the GH #758 loud conservative skip (its endpoint dims `[D1,D2]` vs
/// `[D1,D3]` do not correspond): exactly one Assembly Warning, NO
/// `matrix -> out` link score of any shape, and ZERO loop scores -- every
/// loop through the edge was dropped. Post-T4 the whole-RHS reducer mints a
/// synthetic agg arrayed over `[D1]`; the source half scores each read row
/// per slot and the agg half broadcasts over `D3` via the GH #528
/// projection, so the loops are genuinely scored.
#[test]
fn whole_rhs_broadcast_reduce_loop_scores_finite_and_sustained() {
    let project = TestProject::new("t4_broadcast_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "stock[D1] * 0.1",
            None,
        )
        // The whole RHS is the broadcast partial reduce: result dims [D1],
        // owner dims [D1,D3].
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D3".into()],
            "SUM(matrix[D1, *])",
            None,
        )
        // Closes the loop through the ALIGNED variable-backed reduce family
        // (well-tested by `variable_backed_partial_reduce_loop_scores_*`).
        .array_flow("inflow[D1]", "SUM(out[D1, *])", None)
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Zero warnings: pre-T4 this fixture surfaced exactly one GH #758
    // "dimensions do not correspond" Warning on matrix -> out and dropped
    // every loop.
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the broadcast whole-RHS reduce must compile every LTM fragment cleanly; \
         got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The synthetic agg aux is arrayed over the reducer's result dims [D1].
    let agg_var = ltm_var(&ltm.vars, agg_name);
    assert_eq!(
        agg_var.dimensions,
        vec!["D1".to_string()],
        "the minted synthetic agg must be arrayed over the reducer's result dims"
    );

    // Source half: one per-(row, slot) score per read row.
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}{agg_name}[{d1}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected the source-half score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    // Agg half: the GH #528 projection broadcasts the [D1] slot over D3.
    for (d1, d3) in [("a", "p"), ("a", "q"), ("b", "p"), ("b", "q")] {
        let name = format!("{LINK_SCORE_PREFIX}{agg_name}[{d1}]\u{2192}out[{d1},{d3}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected the agg-half score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(results.step_count > STARTUP_STEPS);

    // Per-row attribution: the two co-reduced rows of each slot change by
    // identical deltas (`matrix[d1,*] = stock[d1] * 0.1`), so each row's
    // changed-first partial reads exactly 0.5.
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name = format!("{LINK_SCORE_PREFIX}matrix[{d1},{d2}]\u{2192}{agg_name}[{d1}]");
        let s = series_at(&results, offset_of(&results, &name));
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-9,
                "{name} at step {step}: expected 0.5 (two equal-delta co-reduced rows); \
                 got {s:?}"
            );
        }
    }
    // Slot attribution: `out` IS the agg's value, so every agg-half score
    // reads exactly 1 past the initial step.
    for (d1, d3) in [("a", "p"), ("a", "q"), ("b", "p"), ("b", "q")] {
        let name = format!("{LINK_SCORE_PREFIX}{agg_name}[{d1}]\u{2192}out[{d1},{d3}]");
        let s = series_at(&results, offset_of(&results, &name));
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "{name} at step {step}: expected exactly 1 (out IS the agg); got {s:?}"
            );
        }
    }

    // Exactly the 8 real loops (per (d1, d2, d3): stock[d1] -> matrix[d1,d2]
    // -> agg[d1] -> out[d1,d3] -> inflow[d1] -> stock[d1]), every one
    // routing through the agg, finite everywhere, and sustained non-zero.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        8,
        "expected one loop per (d1, d2, d3) combination; got: {loop_names:?}"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm.vars, name);
        assert!(
            var.equation.source_text().contains(agg_name),
            "every loop in this model routes through the synthetic agg; {name} does not: {}",
            var.equation.source_text()
        );
        let base = offset_of(&results, name);
        for slot in 0..slot_count(var, &project.dimensions) {
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
                "loop score {name} slot {slot} must be sustained non-zero past the \
                 startup guard; got: {s:?}"
            );
        }
    }
}

/// GH #764, the PERMUTED shape end-to-end: `out[D2,D1] = SUM(cube[D1,D2,*])`
/// (result dims `[D1,D2]` in slice order, the owner declared `[D2,D1]`)
/// closed in a feedback loop. Pre-T4 the variable-backed gate declined the
/// permutation but the OLD cartesian derivation still emitted per-(row,
/// slot) scores while the element graph kept the conservative
/// cross-product, so the phantom cross-product circuits referenced missing
/// names: 184 "failed to compile" warned 0-stub loop scores alongside the
/// real ones. Post-T4 the synthetic agg's slots are keyed by `result_dims`
/// order and the GH #528 projection reorders per target element: exactly
/// the real loops, zero warnings.
#[test]
fn whole_rhs_permuted_reduce_loop_scores_finite_and_sustained() {
    let project = TestProject::new("t4_permuted_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "stock[D1,D2] * 0.1",
            None,
        )
        // The whole RHS is the permuted partial reduce: result dims [D1,D2]
        // (slice order), owner dims [D2,D1].
        .array_aux_direct(
            "out",
            vec!["D2".into(), "D1".into()],
            "SUM(cube[D1, D2, *])",
            None,
        )
        .array_flow("inflow[D1,D2]", "out[D2, D1] * 0.5", None)
        .array_stock("stock[D1,D2]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the permuted whole-RHS reduce must compile every LTM fragment cleanly \
         (pre-T4: 184 warned 0-stub phantom loop scores); got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm_var(&ltm.vars, agg_name);
    assert_eq!(
        agg_var.dimensions,
        vec!["D1".to_string(), "D2".to_string()],
        "the synthetic agg's slots are keyed by result_dims (slice) order"
    );

    // Slot attribution under permutation: the agg side of the agg-half name
    // carries the target element PROJECTED onto result_dims order --
    // `agg[a,x] -> out[x,a]`, never the un-permuted `agg[x,a] -> out[x,a]`.
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let correct = format!("{LINK_SCORE_PREFIX}{agg_name}[{d1},{d2}]\u{2192}out[{d2},{d1}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == correct),
            "expected the permuted agg-half score {correct:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        let wrong = format!("{LINK_SCORE_PREFIX}{agg_name}[{d2},{d1}]\u{2192}out[{d2},{d1}]");
        assert!(
            !ltm.vars.iter().any(|v| v.name == wrong),
            "the agg slot must be the result_dims-ordered projection; found mis-ordered \
             {wrong:?}"
        );
    }
    // Source half: one score per read row, slot in result_dims order.
    for (d1, d2, d3) in [
        ("a", "x", "p"),
        ("a", "x", "q"),
        ("a", "y", "p"),
        ("a", "y", "q"),
        ("b", "x", "p"),
        ("b", "x", "q"),
        ("b", "y", "p"),
        ("b", "y", "q"),
    ] {
        let name = format!("{LINK_SCORE_PREFIX}cube[{d1},{d2},{d3}]\u{2192}{agg_name}[{d1},{d2}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected the source-half score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(results.step_count > STARTUP_STEPS);

    // Exactly the 8 real loops (per (d1, d2, d3): stock[d1,d2] ->
    // cube[d1,d2,d3] -> agg[d1,d2] -> out[d2,d1] -> inflow[d1,d2] ->
    // stock[d1,d2]); finite and sustained.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        8,
        "expected one loop per (d1, d2, d3) combination; got: {loop_names:?}"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm.vars, name);
        assert!(
            var.equation.source_text().contains(agg_name),
            "every loop in this model routes through the synthetic agg; {name} does not: {}",
            var.equation.source_text()
        );
        let base = offset_of(&results, name);
        for slot in 0..slot_count(var, &project.dimensions) {
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
                "loop score {name} slot {slot} must be sustained non-zero past the \
                 startup guard; got: {s:?}"
            );
        }
    }
}

/// GH #764 ∩ GH #765 (the T3-report intersection): a non-aligned whole-RHS
/// reduce that ALSO carries a Pinned axis -- `out[D1,D2] =
/// SUM(cube[D1,nyc,*])` over `cube[D1,Region,D2]`, closed in a loop.
/// Pre-T4 this shape rode the OLD full-cartesian link-score derivation
/// byte-identically: it emitted per-(row, slot) scores for EVERY cube row
/// including the unread `boston` rows (`cube[a,boston,x]→out[a,x]` read
/// constant garbage), while the conservative cross-product element edges
/// minted 184 warned 0-stub phantom loops. Post-T4 the minted synthetic
/// agg's halves derive from `read_slice_rows` (Pinned-correct): only the
/// `nyc` rows get scores, no LTM variable mentions `boston`, and the loops
/// are genuinely scored with zero warnings.
#[test]
fn whole_rhs_broadcast_pinned_mix_scores_read_rows_only() {
    let project = TestProject::new("t4_pinned_mix_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "Region".into(), "D2".into()],
            "stock[D1,D2] * 0.1",
            None,
        )
        // Whole-RHS, Pinned Region axis, result dims [D1] broadcast over
        // the owner's [D1,D2].
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D2".into()],
            "SUM(cube[D1, nyc, *])",
            None,
        )
        .array_flow("inflow[D1,D2]", "out[D1, D2] * 0.5", None)
        .array_stock("stock[D1,D2]", "10", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the Pinned-bearing broadcast reduce must compile every LTM fragment cleanly \
         (pre-T4: 184 warned 0-stub phantom loop scores); got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm_var(&ltm.vars, agg_name);
    assert_eq!(agg_var.dimensions, vec!["D1".to_string()]);

    // Pinned-correctness: only the nyc rows get source-half scores...
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name = format!("{LINK_SCORE_PREFIX}cube[{d1},nyc,{d2}]\u{2192}{agg_name}[{d1}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected the read-row score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    // ... and NO LTM variable of any kind mentions an unread boston row
    // (pre-T4 the cartesian derivation scored them all).
    let boston_garbage: Vec<&str> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("boston"))
        .map(|v| v.name.as_str())
        .collect();
    assert!(
        boston_garbage.is_empty(),
        "unread (boston) rows must get NO scores or loops; got: {boston_garbage:?}"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(results.step_count > STARTUP_STEPS);

    // Per-row attribution: the two read rows of each slot change by equal
    // deltas, so each scores 0.5.
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name = format!("{LINK_SCORE_PREFIX}cube[{d1},nyc,{d2}]\u{2192}{agg_name}[{d1}]");
        let s = series_at(&results, offset_of(&results, &name));
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - 0.5).abs() < 1e-9,
                "{name} at step {step}: expected 0.5 (two equal-delta read rows); got {s:?}"
            );
        }
    }

    // Exact loop census -- all causally real, no phantoms. Per D1 row the
    // element graph is stock[d1,d2] -> cube[d1,nyc,d2] -> agg[d1] ->
    // out[d1,d2'] -> inflow[d1,d2'] -> stock[d1,d2']:
    // - 4 elementary DIAGONAL circuits (d2' == d2, one per (d1, d2));
    // - 2 petal-stitched CROSS-D2 loops (one per d1): the two petals
    //   agg[d1] -> out[d1,x] -> ... -> cube[d1,nyc,x] -> agg[d1] and the
    //   y-twin revisit agg[d1], so Johnson cannot emit their combination
    //   directly; `recover_cross_agg_loops` stitches each pairwise-disjoint
    //   petal pair into ONE canonical loop. These are causally real
    //   (out[d1,y] genuinely depends on stock[d1,x] through the agg).
    // Total: exactly 6. Any drift means phantom loops were reintroduced
    // (or real ones dropped).
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        6,
        "expected 4 diagonal + 2 cross-D2 petal-stitched loops; got: {loop_names:?}"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm.vars, name);
        assert!(
            var.equation.source_text().contains(agg_name),
            "every loop in this model routes through the synthetic agg; {name} does not: {}",
            var.equation.source_text()
        );
        let base = offset_of(&results, name);
        for slot in 0..slot_count(var, &project.dimensions) {
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
                "loop score {name} slot {slot} must be sustained non-zero past the \
                 startup guard; got: {s:?}"
            );
        }
    }
}

/// T4 blast-radius golden pin: the GH #534 MAPPED whole-RHS reducer
/// (`growth[State] = SUM(matrix[State,*])` over a positional `State→Region`
/// mapping, in a loop) -- the carve-out T4 generalizes -- must keep
/// byte-identical emissions across the unified minting condition: the same
/// synthetic agg (arrayed over the TARGET dim), the same remapped
/// source-half names, and the exact source-half equation text captured at
/// the T4 parent commit.
#[test]
fn whole_rhs_mapped_reduce_emissions_stay_byte_identical() {
    let project = TestProject::new("t4_mapped_golden")
        .with_sim_time(0.0, 6.0, 1.0)
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
        .array_aux_direct(
            "growth",
            vec!["State".into()],
            "SUM(matrix[State, *])",
            None,
        )
        .array_flow("inflow[Region]", "SUM(growth[*]) * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the mapped whole-RHS reduce must stay warning-free; got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let agg0 = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm_var(&ltm.vars, agg0);
    assert_eq!(agg_var.dimensions, vec!["State".to_string()]);
    assert_eq!(agg_var.equation.source_text(), "sum(matrix[state, *])");

    // The remapped source-half names: each Region row feeds the slot of its
    // positionally-corresponding State element.
    for (region, state) in [("west", "ca"), ("east", "ny")] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{region},{d2}]\u{2192}{agg0}[{state}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected the remapped source-half score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
        }
    }

    // The exact source-half equation text captured at the T4 parent commit
    // (148a17d8).
    let golden = "if (TIME = INITIAL_TIME) then 0 else if ((\"$\u{205A}ltm\u{205A}agg\u{205A}0\"\
[ca] - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"[ca])) = 0) OR ((matrix[region\u{B7}west,\
d2\u{B7}x] - PREVIOUS(matrix[region\u{B7}west,d2\u{B7}x])) = 0) then 0 else SAFEDIV((PREVIOUS\
(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"[ca]) + (matrix[region\u{B7}west,d2\u{B7}x] - PREVIOUS(\
matrix[region\u{B7}west,d2\u{B7}x])) - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"[ca])), \
ABS((\"$\u{205A}ltm\u{205A}agg\u{205A}0\"[ca] - PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}0\"\
[ca]))), 0) * SIGN((matrix[region\u{B7}west,d2\u{B7}x] - PREVIOUS(matrix[region\u{B7}west,\
d2\u{B7}x])))";
    let name = format!("{LINK_SCORE_PREFIX}matrix[west,x]\u{2192}{agg0}[ca]");
    let var = ltm_var(&ltm.vars, &name);
    assert_eq!(
        var.equation.source_text(),
        golden,
        "the mapped whole-RHS source-half equation text must stay byte-identical"
    );
}

/// The mapped ∩ non-aligned INTERSECTION (T4 review finding 1):
/// `growth[State,D3] = SUM(matrix[State,*])` over a positional
/// `State→Region` mapping is mapped AND broadcast (`result_dims = [State]`,
/// a strict subset of the owner's `[State,D3]`) -- mapped does NOT imply
/// aligned. The mapped clause of `variable_backed_shape_is_expressible`
/// fires first, and the synthetic machinery composes both halves cleanly:
/// the source half remaps each Region row to its positionally-corresponding
/// State slot, and the GH #528 projection broadcasts the [State]-arrayed
/// agg over the owner's extra D3 dim. This pins the intersection the
/// minting predicate's rustdoc documents, guarding against a future
/// "simplification" that reorders/merges the clauses on an assumed
/// mapped ⇒ aligned.
#[test]
fn whole_rhs_mapped_broadcast_intersection_scores_cleanly() {
    let project = TestProject::new("t4_mapped_broadcast")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["west", "east"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .named_dimension_with_mapping("State", &["CA", "NY"], "Region")
        .array_stock("pop[Region]", "100", &["inflow"], &[], None)
        .array_aux_direct(
            "matrix",
            vec!["Region".into(), "D2".into()],
            "pop[Region] * 0.05",
            None,
        )
        // Mapped (State→Region) AND broadcast (owner [State,D3], result
        // dims [State]).
        .array_aux_direct(
            "growth",
            vec!["State".into(), "D3".into()],
            "SUM(matrix[State, *])",
            None,
        )
        .array_flow("inflow[Region]", "SUM(growth[*, *]) * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the mapped+broadcast intersection must compile every LTM fragment cleanly; \
         got: {warnings:?}"
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    // Canonical-sorted variable walk: `growth` < `inflow`, so growth's
    // reducer is agg 0 (arrayed over the TARGET dim State) and inflow's
    // whole-extent `SUM(growth[*,*])` sub-reducer is agg 1 (scalar).
    let agg0 = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm_var(&ltm.vars, agg0);
    assert_eq!(
        agg_var.dimensions,
        vec!["State".to_string()],
        "the synthetic agg is arrayed over the reducer's TARGET result dim"
    );

    // Source half: REMAPPED rows -- each Region row feeds the slot of its
    // positionally-corresponding State element.
    for (region, state) in [("west", "ca"), ("east", "ny")] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{region},{d2}]\u{2192}{agg0}[{state}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "expected the remapped source-half score {name:?}; have: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            );
        }
    }
    // Agg half: the GH #528 projection BROADCASTS the [State] slot over the
    // owner's extra D3 dim.
    for (state, d3) in [("ca", "p"), ("ca", "q"), ("ny", "p"), ("ny", "q")] {
        let name = format!("{LINK_SCORE_PREFIX}{agg0}[{state}]\u{2192}growth[{state},{d3}]");
        assert!(
            ltm.vars.iter().any(|v| v.name == name),
            "expected the broadcast agg-half score {name:?}; have: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    assert!(results.step_count > STARTUP_STEPS);

    // Per-row attribution: the two co-reduced D2 rows of each State slot
    // change by identical deltas (`matrix[Region,*] = pop[Region] * 0.05`,
    // equal initial stocks), so each remapped source half reads 0.5...
    for (region, state) in [("west", "ca"), ("east", "ny")] {
        for d2 in ["x", "y"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{region},{d2}]\u{2192}{agg0}[{state}]");
            let s = series_at(&results, offset_of(&results, &name));
            for (step, &v) in s.iter().enumerate().skip(1) {
                assert!(
                    (v - 0.5).abs() < 1e-9,
                    "{name} at step {step}: expected 0.5 (two equal-delta co-reduced \
                     rows); got {s:?}"
                );
            }
        }
    }
    // ... and `growth` IS the agg's value, so every broadcast agg half
    // reads exactly 1.
    for (state, d3) in [("ca", "p"), ("ca", "q"), ("ny", "p"), ("ny", "q")] {
        let name = format!("{LINK_SCORE_PREFIX}{agg0}[{state}]\u{2192}growth[{state},{d3}]");
        let s = series_at(&results, offset_of(&results, &name));
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "{name} at step {step}: expected exactly 1 (growth IS the agg); got {s:?}"
            );
        }
    }

    // Exactly the 8 real elementary loops -- one per (region, d2, d3):
    // pop[region] -> matrix[region,d2] -> agg0[state] -> growth[state,d3]
    // -> agg1 -> inflow[region] -> pop[region]. Each loop's score is the
    // link product 0.5 (matrix row, two co-reduced D2 rows) * 1.0
    // (agg0 -> growth) * 0.25 (growth -> agg1: four equal-delta growth
    // elements feed the whole-extent SUM) * 1.0 (agg1 -> inflow) * 1.0
    // (flow-to-stock, single-inflow stock) * 1.0 (pop -> matrix) = 0.125.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        8,
        "expected one loop per (region, d2, d3) combination; got: {loop_names:?}"
    );
    for name in &loop_names {
        let var = ltm_var(&ltm.vars, name);
        assert!(
            var.equation.source_text().contains(agg0),
            "every loop routes through the mapped+broadcast agg; {name} does not: {}",
            var.equation.source_text()
        );
        let base = offset_of(&results, name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let s = series_at(&results, base + slot);
            for (step, &v) in s.iter().enumerate() {
                assert!(
                    v.is_finite(),
                    "loop score {name} slot {slot} at step {step} is not finite: {v}"
                );
            }
            for (step, &v) in s.iter().enumerate().skip(STARTUP_STEPS + 1) {
                assert!(
                    (v - 0.125).abs() < 1e-9,
                    "loop score {name} slot {slot} at step {step}: expected exactly \
                     0.125 (see the link-product derivation above); got {s:?}"
                );
            }
        }
    }
}

/// GH #751: a target equation hoisting TWO distinct ARRAYED sliced reducers
/// (`two_sums[d1] = SUM(m1[d1,*]) + SUM(m2[d1,*])`) must pin the FROZEN
/// co-agg's reference in each agg→target partial to the co-agg's own
/// projected slot. Pre-fix, agg A's per-target-element partial froze co-agg
/// B as the bare `previous("$⁚ltm⁚agg⁚1")` -- a multi-slot reference in a
/// scalar equation -- so all four `agg[{r}]→two_sums[{r}]` fragments failed
/// to compile (4 Assembly warnings), stubbing every agg→target link score
/// AND all 8 loop scores through `two_sums` to constant 0.
///
/// Score derivation (`two_sums = A + B`, `A = SUM(m1[d1,*]) = 0.1·rowsum`,
/// `B = 0.05·rowsum`): the changed-first numerator for A is
/// `(A_t + B_{t-1}) - two_sums_{t-1} = ΔA`, so
/// `score_A = |ΔA/Δtwo_sums| = 0.1/0.15 = 2/3` and `score_B = 1/3`.
/// Each m1-family loop multiplies `score_A` by the row link (`0.5` -- each
/// of the 2 co-reduced cells carries half the row's change) and three
/// unit links, giving `1/3`; the m2 family gives `1/6`.
#[test]
fn gh751_two_arrayed_co_aggs_pin_frozen_agg_to_slot() {
    let project = TestProject::new("gh751")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["r1", "r2"])
        .named_dimension("d2", &["c1", "c2"])
        .array_stock("pop[d1,d2]", "100", &["growth"], &[], None)
        .array_flow("growth[d1,d2]", "two_sums[d1] * 0.01", None)
        .array_aux("m1[d1,d2]", "pop[d1,d2] * 0.1")
        .array_aux("m2[d1,d2]", "pop[d1,d2] * 0.05")
        .array_aux("two_sums[d1]", "SUM(m1[d1,*]) + SUM(m2[d1,*])")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // Every fragment compiles: no Assembly warnings at all (pre-fix: 4,
    // one per failed agg[{r}]→two_sums[{r}] fragment).
    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "two distinct arrayed co-aggs must compile warning-free; got: {warnings:?}"
    );

    // The frozen co-agg is pinned to ITS projected slot, never bare.
    // (agg⁚0 = SUM(m1[d1,*]), agg⁚1 = SUM(m2[d1,*]) -- left-to-right
    // minting order.)
    let a0_to_r1 = ltm_var(
        &ltm.vars,
        "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0[r1]\u{2192}two_sums[r1]",
    );
    let eqn = a0_to_r1.equation.source_text();
    assert!(
        eqn.contains("previous(\"$\u{205A}ltm\u{205A}agg\u{205A}1\"[d1\u{B7}r1])"),
        "the frozen co-agg must be PREVIOUS-pinned to its own projected slot; got: {eqn}"
    );
    assert!(
        !eqn.contains("previous(\"$\u{205A}ltm\u{205A}agg\u{205A}1\")"),
        "the frozen co-agg must not be referenced bare (a multi-slot reference in a \
         scalar equation cannot compile); got: {eqn}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Both aggs' agg→target halves score their analytic share.
    for (agg_idx, expected) in [(0usize, 2.0 / 3.0), (1usize, 1.0 / 3.0)] {
        for r in ["r1", "r2"] {
            let name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}{agg_idx}[{r}]\u{2192}two_sums[{r}]"
            );
            let s = series_at(&results, offset_of(&results, &name));
            for (step, &v) in s.iter().enumerate().skip(1) {
                assert!(
                    (v - expected).abs() < 1e-9,
                    "{name} at step {step}: expected {expected}; got {s:?}"
                );
            }
        }
    }

    // Every loop score is finite and sustained at its analytic value:
    // 1/3 for the m1-family loops, 1/6 for the m2 family. Classify each
    // loop by which agg its equation references.
    let loop_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_vars.len(),
        8,
        "one loop per (agg, d1 row, co-reduced d2 cell); got: {:?}",
        loop_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    for v in loop_vars {
        let expected = if v.equation.source_text().contains("agg\u{205A}0") {
            1.0 / 3.0
        } else {
            1.0 / 6.0
        };
        let s = series_at(&results, offset_of(&results, &v.name));
        for (step, &val) in s.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                (val - expected).abs() < 1e-6,
                "loop score {} at step {step}: expected {expected}; got {s:?}",
                v.name
            );
        }
    }
}

/// GH #751's SCALAR-co-agg twin (the GH #737-era family): two distinct
/// WHOLE-EXTENT reducers in one scalar target (`tot = SUM(m1[*]) +
/// SUM(m2[*])`) mint two SCALAR aggs, and a scalar co-agg's bare
/// `PREVIOUS("$⁚ltm⁚agg⁚1")` freeze compiles as-is -- this shape already
/// worked and must stay byte-identical under the per-ident pin map (a
/// scalar agg has empty `result_dims`, so it gets no pin entry by
/// construction). Pins the bare-PREVIOUS equation text, zero warnings, and
/// the same 2/3 / 1/3 analytic shares as the arrayed twin.
#[test]
fn gh751_scalar_co_aggs_keep_bare_previous_freeze() {
    let project = TestProject::new("gh751_scalar_twin")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["r1", "r2"])
        .array_stock("pop[d1]", "100", &["growth"], &[], None)
        .array_flow("growth[d1]", "tot * 0.01", None)
        .array_aux("m1[d1]", "pop[d1] * 0.1")
        .array_aux("m2[d1]", "pop[d1] * 0.05")
        .scalar_aux("tot", "SUM(m1[*]) + SUM(m2[*])")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "scalar co-aggs must compile warning-free; got: {warnings:?}"
    );

    // The scalar co-agg keeps its bare PREVIOUS freeze (byte-identity pin:
    // no slot subscript exists to pin a scalar agg to).
    let a0_to_tot = ltm_var(
        &ltm.vars,
        "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}tot",
    );
    let eqn = a0_to_tot.equation.source_text();
    assert!(
        eqn.contains("PREVIOUS(\"$\u{205A}ltm\u{205A}agg\u{205A}1\")"),
        "a scalar co-agg's freeze stays the bare PREVIOUS reference; got: {eqn}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    for (agg_idx, expected) in [(0usize, 2.0 / 3.0), (1usize, 1.0 / 3.0)] {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}{agg_idx}\u{2192}tot"
        );
        let s = series_at(&results, offset_of(&results, &name));
        for (step, &v) in s.iter().enumerate().skip(1) {
            assert!(
                (v - expected).abs() < 1e-9,
                "{name} at step {step}: expected {expected}; got {s:?}"
            );
        }
    }
}

/// GH #526: a TRANSPOSED non-live array dep in a loop's target equation.
/// `growth[d1,d2] = pop[d1,d2] * 0.1 + arr[d2,d1] * 0.001` -- `arr[d2,d1]`
/// is a genuine positional transposition (slot `(i,j)` reads element
/// `(j,i)`), so the pre-fix collapse to a bare `PREVIOUS(arr)` froze the
/// WRONG element: with the asymmetric constant `arr` here, the off-diagonal
/// `pop→growth` slots scored 0.9940/1.0059 instead of exactly 1.0 (a
/// silent magnitude error -- zero warnings; the recorded pre-fix
/// signature). Post-fix the threaded dep dims flag the mismatch and the
/// partial falls to the CHANGED-LAST convention, which keeps `arr[d2,d1]`
/// live and verbatim: the numerator is `(growth - frozen)` = exactly
/// `0.1·Δpop`, so every slot scores exactly 1.0, still with zero warnings.
#[test]
fn gh526_transposed_dep_partial_takes_changed_last() {
    let project = TestProject::new("gh526")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["a", "b"])
        .named_dimension("d2", &["x", "y"])
        .array_with_ranges("base1[d1]", vec![("a", "1"), ("b", "2")])
        .array_with_ranges("base2[d2]", vec![("x", "3"), ("y", "7")])
        .array_aux("arr[d1,d2]", "base1[d1] * 10 + base2[d2]")
        .array_stock("pop[d1,d2]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[d1,d2]",
            "pop[d1,d2] * 0.1 + arr[d2,d1] * 0.001",
            None,
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let warnings = assembly_warnings(&db, sync.project);
    assert!(
        warnings.is_empty(),
        "the changed-last fallback compiles; no warnings expected, got: {warnings:?}"
    );

    let v = ltm_var(
        &ltm.vars,
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}growth",
    );
    let eqn = v.equation.source_text();
    assert!(
        !eqn.contains("PREVIOUS(arr)"),
        "the transposed dep must NOT be collapsed to the wrong-element bare freeze; got: {eqn}"
    );
    assert!(
        eqn.contains("arr[d2, d1]"),
        "changed-last keeps the transposed dep live and verbatim; got: {eqn}"
    );
    assert!(
        eqn.contains("(growth - (PREVIOUS(pop) * 0.1 + arr[d2, d1] * 0.001))"),
        "the numerator is the changed-last `(target - frozen)` form; got: {eqn}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let base = offset_of(
        &results,
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}growth",
    );
    for slot in 0..4 {
        let s = series_at(&results, base + slot);
        for (step, &val) in s.iter().enumerate().skip(1) {
            assert!(
                (val - 1.0).abs() < 1e-9,
                "pop→growth slot {slot} step {step}: arr is constant, so the score is \
                 exactly 1.0 (pre-fix the off-diagonal slots read 0.9940/1.0059 -- the \
                 wrong-element freeze); got {s:?}"
            );
        }
    }
    // The isolated loop's score is exactly 1 per slot past startup.
    let loop_base = offset_of(&results, "$\u{205A}ltm\u{205A}loop_score\u{205A}r1");
    for slot in 0..4 {
        let s = series_at(&results, loop_base + slot);
        for (step, &val) in s.iter().enumerate().skip(STARTUP_STEPS) {
            assert!(
                (val - 1.0).abs() < 1e-6,
                "loop score slot {slot} step {step}: expected 1.0; got {s:?}"
            );
        }
    }
}

/// GH #526 byte-identity controls: the NATURAL-position dep keeps the
/// historical changed-first collapse (`PREVIOUS(arr)` -- exact, the bare
/// freeze reads the same element), and a POSITIONALLY-MAPPED dep
/// (`cost[State]` for `cost` declared over `Region`, `State→Region`) keeps
/// it too via the shared `iterated_axis_slot_elements` gate. Both shapes
/// collapsed before the fix and must keep collapsing -- the strict verdict
/// only changes provably-mismatched references.
#[test]
fn gh526_natural_and_mapped_deps_keep_changed_first_collapse() {
    // Natural position: arr[d1,d2] matching declared order.
    let natural = TestProject::new("gh526_natural")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["a", "b"])
        .named_dimension("d2", &["x", "y"])
        .array_with_ranges("base1[d1]", vec![("a", "1"), ("b", "2")])
        .array_with_ranges("base2[d2]", vec![("x", "3"), ("y", "7")])
        .array_aux("arr[d1,d2]", "base1[d1] * 10 + base2[d2]")
        .array_stock("pop[d1,d2]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[d1,d2]",
            "pop[d1,d2] * 0.1 + arr[d1,d2] * 0.001",
            None,
        )
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &natural, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    compile_project_incremental(&db, sync.project, "main").expect("compiles");
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "natural-position fixture stays warning-free"
    );
    let eqn = ltm_var(
        &ltm.vars,
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}growth",
    )
    .equation
    .source_text()
    .to_string();
    assert!(
        eqn.contains("(pop * 0.1 + PREVIOUS(arr) * 0.001)"),
        "the natural-position dep keeps the changed-first bare-PREVIOUS collapse; got: {eqn}"
    );

    // Positionally mapped: cost declared over region, referenced cost[state]
    // inside an A2A-over-state equation with a positional state→region
    // mapping -- the executed read resolves positionally, matching the bare
    // broadcast, so the collapse is exact and retained.
    let mapped = TestProject::new("gh526_mapped")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["west", "east"])
        .named_dimension_with_mapping("state", &["ca", "ny"], "region")
        .array_with_ranges("cost[region]", vec![("west", "3"), ("east", "7")])
        .array_stock("pop[state]", "100", &["growth"], &[], None)
        .array_flow(
            "growth[state]",
            "pop[state] * 0.1 + cost[state] * 0.001",
            None,
        )
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &mapped, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    compile_project_incremental(&db, sync.project, "main").expect("compiles");
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "mapped fixture stays warning-free"
    );
    let eqn = ltm_var(
        &ltm.vars,
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}growth",
    )
    .equation
    .source_text()
    .to_string();
    assert!(
        eqn.contains("PREVIOUS(cost)") && !eqn.contains("PREVIOUS(cost["),
        "the positionally-mapped dep keeps the changed-first collapse; got: {eqn}"
    );
}

// ── GH #778/#785: degenerate square-source reducers decline + loud skip ──
//
// A reducer whose Iterated axes repeat a target dim mints (pre-fix) a
// synthetic agg arrayed over `result_dims == [D1, D1]`. The executed A2A
// simulation reads only the DIAGONAL (`cube[e,e,*]` per target slot `e`), but
// the LTM machinery enumerated the full square -- so the source→agg co-source
// half emitted CONFIDENT per-`(row, slot)` link scores on the off-diagonal
// rows the simulation never reads (`cube[r1,r2,*]→agg[r1,r2]`), reaching the
// results slab UNWARNED (#785's live silent-wrong-number), while the
// agg→target element edges fanned out off-diagonal too (#778). PR #787
// defended only the feeder half.
//
// The fix declines the whole shape at agg minting
// (`result_dims_has_repeated_dim`) and closes the remaining un-hoisted
// cartesian-branch landing with the GH #758 loud skip. The contract per
// spelling, in BOTH exhaustive and discovery modes:
//   - NO per-element co-source link score reaches the slab on a phantom row;
//   - the edge is loudly skipped (one Warning naming it), warn-once;
//   - every loop through the shape is dropped (no loop-score cascade);
//   - the model still simulates and every emitted score series is finite.

/// Assert the post-fix square-source contract for `project` in the given
/// `discovery` mode. The duplicated-dim sources `expected_skipped` (the
/// canonical co-source `cube`, plus the feeder `frac` when present) must each
/// surface exactly one loud edge-level Warning, emit NO per-element link
/// score, and drag NO loop score into existence.
fn assert_square_source_loudly_skipped(
    project: &datamodel::Project,
    discovery: bool,
    expected_skipped: &[&str],
) {
    let mode = if discovery { "discovery" } else { "exhaustive" };
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    if discovery {
        set_project_ltm_discovery_mode(&mut db, sync.project, true);
    }
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // No synthetic agg is minted for the declined shape.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}agg\u{205A}")),
        "{mode}: the declined square-source reducer must mint no agg variable; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // No per-element link score for any duplicated-dim source: the phantom
    // off-diagonal scores must never reach the slab.
    for src in expected_skipped {
        let prefix = format!("{LINK_SCORE_PREFIX}{src}[");
        let leaked: Vec<&str> = ltm
            .vars
            .iter()
            .filter(|v| v.name.starts_with(&prefix))
            .map(|v| v.name.as_str())
            .collect();
        assert!(
            leaked.is_empty(),
            "{mode}: no per-element link score may reach the slab for the \
             duplicated-dim source {src}; got: {leaked:?}"
        );
    }

    // Every loop through the unscoreable edges is dropped.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "{mode}: loops through the unscoreable square-source edges must be \
         dropped, not emitted as warned 0-stubs; got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the declined-square model must still compile with LTM enabled");

    // The skip is loud and warn-once: exactly one Assembly warning per
    // duplicated-dim source edge, each naming the square-source decline.
    let warnings = assembly_warnings(&db, sync.project);
    let square: Vec<&str> = warnings
        .iter()
        .filter_map(|w| match &w.error {
            DiagnosticError::Assembly(m) if m.contains("square-source") => Some(m.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        square.len(),
        expected_skipped.len(),
        "{mode}: expected exactly {} square-source skip warnings (warn-once per \
         edge); got: {square:?}",
        expected_skipped.len()
    );
    for src in expected_skipped {
        assert!(
            square.iter().any(|m| m.contains(&format!("{src} -> x"))),
            "{mode}: the {src} -> x edge must surface a square-source skip warning; \
             got: {square:?}"
        );
    }
    // No OTHER Assembly warning (no fragment-failure cascade).
    assert_eq!(
        warnings.len(),
        expected_skipped.len(),
        "{mode}: the only Assembly warnings are the loud square-source skips; \
         got: {:?}",
        warnings.iter().map(|w| &w.error).collect::<Vec<_>>()
    );

    // The model simulates and every emitted score series stays finite.
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for name in ltm_score_var_names(&results) {
        let var = ltm_var(&ltm.vars, &name);
        let base = offset_of(&results, &name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "{mode}: emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

fn square_source_whole_rhs_fixture() -> datamodel::Project {
    TestProject::new("square_whole_rhs")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("cube[D1,D1,D2]", "0.0001 * pop[D1]")
        .array_stock("pop[D1]", "100", &["x"], &[], None)
        .array_flow("x[D1]", "SUM(cube[D1, D1, *])", None)
        .build_datamodel()
}

fn square_source_inline_fixture() -> datamodel::Project {
    TestProject::new("square_inline")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("cube[D1,D1,D2]", "0.0001 * pop[D1]")
        .array_aux("base[D1]", "0")
        .array_stock("pop[D1]", "100", &["x"], &[], None)
        .array_flow("x[D1]", "base[D1] + SUM(cube[D1, D1, *])", None)
        .build_datamodel()
}

fn square_source_with_feeder_fixture() -> datamodel::Project {
    TestProject::new("square_feeder")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("cube[D1,D1,D2]", "0.0001 * pop[D1]")
        .array_aux("frac[D1,D1]", "0.0001 * pop[D1]")
        .array_stock("pop[D1]", "100", &["x"], &[], None)
        .array_flow("x[D1]", "1 + SUM(cube[D1, D1, *] * frac[D1, D1])", None)
        .build_datamodel()
}

/// Whole-RHS spelling (`x[D1] = SUM(cube[D1,D1,*])`): the co-source half's
/// phantom off-diagonal scores are gone, the `cube -> x` edge is loudly
/// skipped, and loops through it drop -- in both modes.
#[test]
fn square_source_whole_rhs_declines_and_skips_loudly() {
    assert_square_source_loudly_skipped(&square_source_whole_rhs_fixture(), false, &["cube"]);
    assert_square_source_loudly_skipped(&square_source_whole_rhs_fixture(), true, &["cube"]);
}

/// Inline spelling (`x[D1] = base[D1] + SUM(cube[D1,D1,*])`): identical
/// contract to the whole-RHS form (this spelling predates the
/// shape-expressiveness phase and was reachable on `main`).
#[test]
fn square_source_inline_declines_and_skips_loudly() {
    assert_square_source_loudly_skipped(&square_source_inline_fixture(), false, &["cube"]);
    assert_square_source_loudly_skipped(&square_source_inline_fixture(), true, &["cube"]);
}

/// With-feeder spelling (`x[D1] = 1 + SUM(cube[D1,D1,*] * frac[D1,D1])`):
/// BOTH duplicated-dim sources -- the co-source `cube` AND the square feeder
/// `frac` -- are loudly skipped (PR #787 defended only the feeder; now both
/// halves share one decision).
#[test]
fn square_source_with_feeder_declines_and_skips_loudly() {
    assert_square_source_loudly_skipped(
        &square_source_with_feeder_fixture(),
        false,
        &["cube", "frac"],
    );
    assert_square_source_loudly_skipped(
        &square_source_with_feeder_fixture(),
        true,
        &["cube", "frac"],
    );
}

/// Repeated-dim OWNER spelling (`x[D1,D1] = SUM(cube[D1,D1,*])`): an owner
/// genuinely declared over the same dim twice compiles and simulates (each
/// slot reads its own full row -- no diagonal restriction), and reaches
/// `walk_var_equation`'s mint gate with ALIGNED iterated dims
/// (`variable_backed_shape_is_expressible` returns true) -- so it is the
/// `result_dims_has_repeated_dim` check itself, live and load-bearing, that
/// declines the mint there. The per-element projection is still ambiguous
/// between the two `D1` occurrences, so the edge gets the same loud skip as
/// the diagonal spellings.
#[test]
fn square_owner_whole_rhs_declines_and_skips_loudly() {
    fn fixture() -> datamodel::Project {
        TestProject::new("square_owner")
            .with_sim_time(0.0, 8.0, 1.0)
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux("cube[D1,D1,D2]", "0.0001 * pop[D1,D1]")
            .array_stock("pop[D1,D1]", "100", &["x"], &[], None)
            .array_flow("x[D1,D1]", "SUM(cube[D1, D1, *])", None)
            .build_datamodel()
    }
    assert_square_source_loudly_skipped(&fixture(), false, &["cube"]);
    assert_square_source_loudly_skipped(&fixture(), true, &["cube"]);
}

// ---------------------------------------------------------------------------
// GH #791: I1-declined multi-source STRICT-SLICE reducer -> loud unscoreable
// edge (was: silent +1.0 cartesian garbage on unread rows)
// ---------------------------------------------------------------------------

/// GH #791: an I1-DECLINED multi-source whole-RHS reducer whose co-source
/// slices MISMATCH (`share[Region] = SUM(pop[nyc,*] * w[*])`: `pop`'s slice
/// `[Pinned(nyc), Reduced]` vs `w`'s `[Reduced]`) mints NO variable-backed
/// agg, so the `pop -> share` edge fell through `try_cross_dimensional_link_scores`
/// to the legacy CARTESIAN partial-reduce derivation, which emitted silent
/// confident link scores INCLUDING for source rows the equation NEVER reads.
///
/// Pre-fix, empirically (commit `0fcfba58`): `pop[boston,p]→share[boston]`
/// existed and read a constant +1.0 although `share` reads only `pop[nyc,*]`;
/// the read rows were ALSO wrong (`pop[nyc,p]→share[nyc] = 1.0` where the true
/// SUM contribution share is 0.5). Zero warnings on the link surface; only the
/// loop surface degraded loudly (5 warned 0-stubs). A silent-wrong-number on
/// the link surface, violating epic #488's standing invariant.
///
/// Post-fix the edge takes the GH #758/#780 loud skip: exactly ONE warning
/// naming the `pop -> share` edge, NO `pop[*]→share[*]` link-score variable,
/// loops through it dropped, and the OTHER edges' scores untouched. The decline
/// re-derives `pop`'s read slice via the same per-axis classifier the hoisting
/// path uses (`pop[nyc,*]` is a strict slice -> decline), so a full-extent read
/// would NOT be declined.
#[test]
fn gh791_arrayed_owner_mismatched_cosource_strict_slice_skips_loudly() {
    let project = TestProject::new("gh791_arrayed")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_aux_direct(
            "share",
            vec!["Region".into()],
            "SUM(pop[nyc, *] * w[*])",
            None,
        )
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // No `pop[*]→share[*]` link score of ANY row (read or unread) is emitted:
    // the cartesian-garbage rows are gone.
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            for row_region in ["nyc", "boston"] {
                let name = format!("{LINK_SCORE_PREFIX}pop[{row_region},{d2}]\u{2192}share[{e}]");
                assert!(
                    ltm.vars.iter().all(|v| v.name != name),
                    "the declined strict-slice edge must emit NO cartesian link score; \
                     found {name:?}"
                );
            }
        }
    }

    // Exactly ONE Assembly warning: the unscoreable `pop -> share` edge. (Before
    // the fix: 5 -- one per loop score that failed fragment compile, while the
    // link surface carried unwarned wrong +1.0s.)
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable pop -> share edge); \
         got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("share") && msg.contains("strict slice") && msg.contains("pop[nyc,*]"),
        "the warning must name the unscoreable edge, the strict-slice reason, and the \
         ACTUAL slice the equation reads (rendered from the computed AxisRead vector, \
         never a canned example); got: {msg}"
    );

    // No loop scores through the declined edge survive: the only enumerated
    // loops run pop -> share -> inflow -> stock -> pop, all through the doomed
    // edge, so none are scored.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the unscoreable edge must not emit loop scores; got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // OTHER edges are untouched: the flow-to-stock / aux edges keep their
    // scores, and every emitted score series stays finite (no garbage).
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name == format!("{LINK_SCORE_PREFIX}inflow\u{2192}stock")),
        "the unrelated inflow -> stock edge must keep its link score"
    );
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for name in ltm_score_var_names(&results) {
        let var = ltm_var(&ltm.vars, &name);
        let base = offset_of(&results, &name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

/// GH #791, the SCALAR-owner full-reduce arm: `total = SUM(pop[nyc,*] * w[*])`
/// is the EVEN-MORE-SILENT variant -- the scalar target means the cartesian
/// full-reduce arm emits `pop[boston,p]→total = 1.0` (unread) and
/// `pop[nyc,p]→total = 1.0` (true share 0.5) with ZERO warnings on ANY surface
/// (no loop fails because the scalar target has no off-diagonal naming issue).
/// The same strict-slice decline closes both arms: ONE warning, no `pop→total`
/// link scores, loop dropped.
#[test]
fn gh791_scalar_owner_mismatched_cosource_strict_slice_skips_loudly() {
    let project = TestProject::new("gh791_scalar")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .aux("total", "SUM(pop[nyc, *] * w[*])", None)
        .array_flow("inflow[Region]", "total * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    for region in ["nyc", "boston"] {
        for d2 in ["p", "q"] {
            let name = format!("{LINK_SCORE_PREFIX}pop[{region},{d2}]\u{2192}total");
            assert!(
                ltm.vars.iter().all(|v| v.name != name),
                "the declined scalar-target strict-slice edge must emit NO cartesian \
                 link score; found {name:?}"
            );
        }
    }

    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable pop -> total edge); \
         got: {warnings:?}"
    );

    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "the loop through the unscoreable pop -> total edge must be dropped"
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for name in ltm_score_var_names(&results) {
        let var = ltm_var(&ltm.vars, &name);
        let base = offset_of(&results, &name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

/// GH #791 discovery-mode twin: the strict-slice decline records the edge in
/// `unscoreable_edges`, which the pinned-loop pass consumes in discovery mode
/// too, so the same `pop -> share` edge is declined there (no cartesian
/// garbage link score minted) -- exhaustive and discovery agree.
#[test]
fn gh791_strict_slice_decline_holds_in_discovery_mode() {
    let project = TestProject::new("gh791_discovery")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_aux_direct(
            "share",
            vec!["Region".into()],
            "SUM(pop[nyc, *] * w[*])",
            None,
        )
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            for row_region in ["nyc", "boston"] {
                let name = format!("{LINK_SCORE_PREFIX}pop[{row_region},{d2}]\u{2192}share[{e}]");
                assert!(
                    ltm.vars.iter().all(|v| v.name != name),
                    "discovery mode must also decline the strict-slice edge; found {name:?}"
                );
            }
        }
    }
    // The model still compiles cleanly in discovery mode.
    compile_project_incremental(&db, sync.project, "main")
        .expect("discovery-mode LTM compilation should succeed");
}

/// GH #791 regression pin (the blast-radius boundary): a multi-source reducer
/// whose source read is the FULL EXTENT must keep its cartesian diagonal scores
/// -- the strict-slice decline fires ONLY for Pinned/subset reads. Here
/// `growth[D1] = SUM(matrix[D1,*] * frac)` declines the I1 acceptance (the bare
/// `frac` feeder, GH #779), so `matrix -> growth` lands on the cartesian
/// derivation, but `matrix[D1,*]`'s slice is `[Iterated(d1), Reduced]` -- a
/// full-extent read -- so its four correct diagonal scores
/// (`matrix[a,c]→growth[a]`, ...) are preserved, NOT loud-skipped. (The bare
/// `frac -> growth` edge keeps its own separate GH #779 decline.)
#[test]
fn gh791_full_extent_multisource_read_stays_scored() {
    let project = gh779_bare_feeder_fixture("SUM");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    for row in ["a", "b"] {
        for col in ["c", "d"] {
            let name = format!("{LINK_SCORE_PREFIX}matrix[{row},{col}]\u{2192}growth[{row}]");
            assert!(
                ltm.vars.iter().any(|v| v.name == name),
                "the full-extent matrix read must keep its cartesian diagonal score \
                 {name:?} (NOT be strict-slice-declined); got: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.contains("matrix") && v.name.contains("growth"))
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// GH #792: the PER-ELEMENT-EQUATION (`Ast::Arrayed`) sibling of the GH #791
/// shape. Each `share` slot holds an I1-declined strict-slice multi-source
/// reducer (`share[nyc] = SUM(pop[nyc,*] * w[*])`, `share[boston] =
/// SUM(pop[boston,*] * w[*])`). Such an owner never reaches the cartesian arm,
/// so BEFORE the fix the edge fell to `emit_per_shape_link_scores` shape Bare
/// and minted a single arrayed `link_score:pop->share` simulating to ~-0.0 at
/// every step with NO per-edge warning (only `loop_score:u5` warned -- the
/// other loops consumed the silent near-zero). The MDL importer expands a
/// single apply-to-all Vensim equation into N per-element equations (GH #651),
/// so this spelling is exactly what real imports produce.
///
/// The fix routes it to the same GH #758/#780 loud skip the A2A spelling takes:
/// one warning naming the edge + its actual slice, no link-score variable, the
/// edge recorded in `unscoreable_edges`, dependent loops dropped.
#[test]
fn gh792_per_element_owner_strict_slice_skips_loudly() {
    let project = gh792_per_element_strict_slice_fixture();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The silent Bare stand-in (`$⁚ltm⁚link_score⁚pop→share`, arrayed over
    // Region, ~-0.0 every step) must be GONE -- in EITHER its bare-edge or any
    // cartesian-row spelling.
    let bare = format!("{LINK_SCORE_PREFIX}pop\u{2192}share");
    assert!(
        ltm.vars.iter().all(|v| v.name != bare),
        "the declined strict-slice per-element edge must emit NO Bare stand-in \
         link score; found {bare:?}"
    );
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            for row_region in ["nyc", "boston"] {
                let name = format!("{LINK_SCORE_PREFIX}pop[{row_region},{d2}]\u{2192}share[{e}]");
                assert!(
                    ltm.vars.iter().all(|v| v.name != name),
                    "the declined per-element edge must emit no cartesian link score; \
                     found {name:?}"
                );
            }
        }
    }

    // Exactly ONE Assembly warning: the unscoreable `pop -> share` edge, naming
    // the strict-slice reason and the ACTUAL slice the slots read (rendered
    // from the computed AxisRead vector, never a canned example). Before the
    // fix: exactly one warning too -- but the WRONG one (a `loop_score:u5`
    // fragment-compile failure), while the link surface carried the silent ~0.
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable pop -> share edge); \
         got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("pop -> share")
            && msg.contains("per-element equations")
            && msg.contains("inside a reducer"),
        "the warning must name the unscoreable edge and the per-element reducer-read \
         reason; got: {msg}"
    );
    // The rendered slice is whichever slot the deterministic sorted-key walk
    // hits first (boston < nyc), but EITHER pinned spelling is acceptable.
    assert!(
        msg.contains("pop[boston,*]") || msg.contains("pop[nyc,*]"),
        "the warning must render the ACTUAL slice the slots read; got: {msg}"
    );

    // No loop scores survive: every enumerated loop runs through the doomed
    // `pop -> share` edge, so none are scored (no silent near-zero loop scores).
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the unscoreable edge must not emit loop scores; got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // Unrelated edges keep their scores, and every emitted score stays finite.
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name == format!("{LINK_SCORE_PREFIX}inflow\u{2192}stock")),
        "the unrelated inflow -> stock edge must keep its link score"
    );
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for name in ltm_score_var_names(&results) {
        let var = ltm_var(&ltm.vars, &name);
        let base = offset_of(&results, &name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

/// GH #792 discovery-mode twin: the strict-slice decline records the edge in
/// `unscoreable_edges`, which the pinned-loop pass consumes in discovery mode
/// too, so the same `pop -> share` edge is declined there -- no Bare stand-in
/// minted, exhaustive and discovery agree.
#[test]
fn gh792_per_element_owner_strict_slice_holds_in_discovery_mode() {
    let project = gh792_per_element_strict_slice_fixture();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let bare = format!("{LINK_SCORE_PREFIX}pop\u{2192}share");
    assert!(
        ltm.vars.iter().all(|v| v.name != bare),
        "discovery mode must also decline the strict-slice per-element edge; found {bare:?}"
    );
    compile_project_incremental(&db, sync.project, "main")
        .expect("discovery-mode LTM compilation should succeed");
}

/// GH #792, the MIXED-SLOT decision (any reducer-reading slot => decline whole
/// edge): ONLY ONE slot reads `pop`, inside a strict-slice reducer (the `nyc`
/// slot is `SUM(pop[nyc,*] * w[*])`); the other slot does not read `pop` at all
/// (`share[boston] = 0`). The edge's one arrayed Bare stand-in conflates both
/// slots, so the single reducer-reading slot proves it wrong for `nyc` -- we
/// decline the WHOLE edge loudly. This is the "only SOME slots contain the
/// reducer" mixed case of the unified per-element rule (ANY slot's un-hoisted
/// reducer read of `from` declines, regardless of strict/full/dynamic).
#[test]
fn gh792_mixed_slot_reducer_read_declines_whole_edge() {
    let project = TestProject::new("gh792_mixed")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![("nyc", "SUM(pop[nyc, *] * w[*])"), ("boston", "0")],
            None,
        )
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let bare = format!("{LINK_SCORE_PREFIX}pop\u{2192}share");
    assert!(
        ltm.vars.iter().all(|v| v.name != bare),
        "a mixed-slot owner with ANY reducer-reading slot must decline the whole edge; \
         found {bare:?}"
    );
    // The reducer-reading slot (`nyc`) drives the decline, so the rendered
    // slice names its pinned read.
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning for the mixed-slot decline; got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("pop -> share")
            && msg.contains("per-element equations")
            && msg.contains("pop[nyc,*]"),
        "the mixed-slot warning must name the edge, the per-element reason, and the \
         reducer-reading slot's slice (pop[nyc,*]); got: {msg}"
    );
}

// GH #792 no-regression: the working disjoint-dim FixedIndex per-element family
// (`try_disjoint_dim_arrayed_link_scores`, GH #510) reads `from` via LITERAL
// element subscripts OUTSIDE any reducer, so the unified per-element verdict
// (which declines only reducer reads) classifies it `NotDescribable` and the
// edge is byte-identically unaffected (unit pin:
// `ltm_agg::unhoisted_source_read_not_describable_for_per_element_non_reducer_refs`).
// The family's end-to-end behavior is pinned by
// `simulate_ltm.rs::test_disjoint_dim_arrayed_target_per_source_element_link_scores`
// (AC3.3) and the GH #510 element-graph guards, which the full suite re-runs.

/// Shared body for the GH #792 review-batch spellings: build the per-element
/// `share` fixture with the given slot equations, assert the loud-skip
/// contract (no `pop->share` link score of any spelling, exactly one
/// per-element decline warning, all loops through the edge dropped, unrelated
/// edges intact), and assert the EXECUTED `share[nyc]` value at t=0 -- the
/// empirical strictness pin (`pop = stock*0.1 = 10` per cell, `w = 0.5`:
/// a row-only read sums to 10.0, a whole-`pop` read to 20.0).
fn assert_gh792_per_element_decline(slots: Vec<(&str, &str)>, expected_share_nyc_t0: f64) {
    let project = TestProject::new("gh792_spelling")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_with_ranges_direct("share", vec!["Region".into()], slots, None)
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    let bare = format!("{LINK_SCORE_PREFIX}pop\u{2192}share");
    assert!(
        ltm.vars.iter().all(|v| v.name != bare),
        "the declined per-element edge must emit NO Bare stand-in link score; \
         found {bare:?}"
    );
    assert!(
        ltm.vars
            .iter()
            .all(|v| !(v.name.starts_with(LINK_SCORE_PREFIX)
                && v.name.contains("pop")
                && v.name.contains("share"))),
        "the declined per-element edge must emit no pop->share link score of ANY \
         spelling; got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.contains("pop") && v.name.contains("share"))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning (the unscoreable pop -> share edge); \
         got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("pop -> share") && msg.contains("per-element equations"),
        "the warning must name the edge and the per-element reducer-read reason; \
         got: {msg}"
    );
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the unscoreable edge must not emit loop scores"
    );
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name == format!("{LINK_SCORE_PREFIX}inflow\u{2192}stock")),
        "the unrelated inflow -> stock edge must keep its link score"
    );

    // The executed-semantics pin: verify what the simulation ACTUALLY computes
    // for the slot equation, so the strict-vs-full claim is empirical.
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    let nyc_off = offset_of(&results, "share[nyc]");
    let share_nyc_t0 = results.iter().next().map(|row| row[nyc_off]).unwrap();
    assert!(
        (share_nyc_t0 - expected_share_nyc_t0).abs() < 1e-9,
        "executed share[nyc] at t=0 must be {expected_share_nyc_t0} \
         (10.0 = strict row-only read, 20.0 = whole-pop read); got {share_nyc_t0}"
    );
}

/// GH #792 review finding 1, the DIM-NAME spelling: `share[nyc] =
/// SUM(pop[Region,*] * w[*])` per slot. EXECUTION resolves `Region` inside the
/// `nyc` slot to `nyc` -- the executed `share[nyc]` is 10.0 (the strict
/// row-only value, pinned below), NOT 20.0 -- so the read is semantically
/// PINNED at the slot's element even though it is spelled with the dim name.
/// The first fix classified this axis `Iterated` (full extent) by passing the
/// owner's declared dims as iterated dims -- `enumerate_agg_nodes`' Arrayed arm
/// deliberately uses EMPTY iterated dims -- so the silent ~-0.0 Bare stand-in
/// and confident 0.0 loops returned byte-for-byte. The unified rule (any
/// un-hoisted reducer read in any slot declines) closes it.
#[test]
fn gh792_per_element_dim_named_slice_skips_loudly() {
    assert_gh792_per_element_decline(
        vec![
            ("nyc", "SUM(pop[Region, *] * w[*])"),
            ("boston", "SUM(pop[Region, *] * w[*])"),
        ],
        10.0,
    );
}

/// GH #792 review finding 2, the FULL-EXTENT multi-source spelling:
/// `share[nyc] = SUM(pop[*,*] * w[*])` per slot (executed share[nyc] = 20.0,
/// the genuine whole-`pop` read, pinned below). The reducer is I1-declined
/// (mismatched co-source arity: `pop`'s `[Reduced,Reduced]` vs `w`'s
/// `[Reduced]`), so no agg is minted and the edge previously fell to the
/// silent ~-0.0 Bare stand-in. A `FullExtent` verdict is safe for scalar/A2A
/// owners ONLY because the cartesian arm scores them; a per-element owner has
/// no cartesian arm, so full-extent does NOT validate the stand-in and the
/// unified rule declines this spelling too.
#[test]
fn gh792_per_element_full_extent_multisource_skips_loudly() {
    assert_gh792_per_element_decline(
        vec![
            ("nyc", "SUM(pop[*, *] * w[*])"),
            ("boston", "SUM(pop[*, *] * w[*])"),
        ],
        20.0,
    );
}

/// The GH #792 fixture: a per-element-equation (`Ast::Arrayed`) `share` whose
/// every slot holds an I1-declined strict-slice multi-source reducer, closed in
/// a feedback loop pop -> share -> inflow -> stock -> pop.
fn gh792_per_element_strict_slice_fixture() -> datamodel::Project {
    TestProject::new("gh792_per_element")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "SUM(pop[nyc, *] * w[*])"),
                ("boston", "SUM(pop[boston, *] * w[*])"),
            ],
            None,
        )
        .array_flow("inflow[Region]", "share[Region] * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel()
}

// ---------------------------------------------------------------------------
// GH #793: hoisted reducer sibling must not hide a declined strict-slice read
// ---------------------------------------------------------------------------

fn gh793_routing_alias_fixture(strict_read: &str, strict_first: bool) -> datamodel::Project {
    let share_eqn = if strict_first {
        format!("SUM({strict_read} * w[*]) + SUM(pop[*, *])")
    } else {
        format!("SUM(pop[*, *]) + SUM({strict_read} * w[*])")
    };
    TestProject::new("gh793_routing_alias")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .array_aux_direct("w", vec!["D2".into()], "0.5", None)
        .array_aux_direct("share", vec!["Region".into()], &share_eqn, None)
        .array_flow("inflow[Region]", "share * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None)
        .build_datamodel()
}

fn assert_gh793_strict_sibling_declines(strict_first: bool) {
    let project = gh793_routing_alias_fixture("pop[nyc, *]", strict_first);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    assert!(
        ltm.vars
            .iter()
            .all(|v| !(v.name.starts_with(LINK_SCORE_PREFIX)
                && v.name.contains("pop")
                && v.name.contains("share"))),
        "the mixed hoisted+declined pop -> share edge must emit no partial \
         pop->share score; got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.contains("pop") && v.name.contains("share"))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    let warnings = assembly_warnings(&db, sync.project);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one Assembly warning for the unscoreable pop -> share \
         strict-slice sibling; got: {warnings:?}"
    );
    let DiagnosticError::Assembly(msg) = &warnings[0].error else {
        unreachable!("filtered to Assembly above");
    };
    assert!(
        msg.contains("pop -> share") && msg.contains("pop[nyc,*]"),
        "the warning must name the edge and the strict sibling slice \
         pop[nyc,*]; got: {msg}"
    );

    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.starts_with(LOOP_SCORE_PREFIX)),
        "loops through the partially-unscoreable pop -> share edge must be dropped; \
         got: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // The core simulation remains valid; only the incomplete LTM attribution
    // is declined.
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();
    for name in ltm_score_var_names(&results) {
        let var = ltm_var(&ltm.vars, &name);
        let base = offset_of(&results, &name);
        for slot in 0..slot_count(var, &project.dimensions) {
            let series = series_at(&results, base + slot);
            assert!(
                series.iter().all(|v| v.is_finite()),
                "emitted score {name} slot {slot} must stay finite; got {series:?}"
            );
        }
    }
}

#[test]
fn gh793_hoisted_sibling_does_not_absorb_strict_slice_first() {
    assert_gh793_strict_sibling_declines(true);
}

#[test]
fn gh793_hoisted_sibling_does_not_absorb_strict_slice_second() {
    assert_gh793_strict_sibling_declines(false);
}

// ---------------------------------------------------------------------------
// GH #790: scalar feeder of a whole-RHS variable-backed reduce
// ---------------------------------------------------------------------------

/// The GH #790 fixture: a scalar feeder (`scale`) of a WHOLE-RHS
/// variable-backed reduce (`growth[D1] = SUM(matrix[D1,*] * scale)` -- growth
/// IS the agg, no synthetic minted), closed in a feedback loop through `pop`:
/// `pop -> scale -> growth -> pop`. The `scale -> growth` hop is the scalar
/// feeder hop. `matrix_eqn` lets the spelling-agreement test swap the
/// constant matrix for a TIME-VARYING one.
fn gh790_whole_rhs_fixture_with(reducer: &str, matrix_eqn: &str) -> datamodel::Project {
    TestProject::new("gh790_whole_rhs")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], matrix_eqn, None)
        .scalar_aux("scale", "SUM(pop[*]) * 0.005")
        .array_flow(
            "growth[D1]",
            &format!("{reducer}(matrix[D1, *] * scale)"),
            None,
        )
        .build_datamodel()
}

fn gh790_whole_rhs_fixture(reducer: &str) -> datamodel::Project {
    gh790_whole_rhs_fixture_with(reducer, "5")
}

/// The SUBEXPRESSION spelling of the SAME dataflow: a constant `prefix`
/// (`"0.1 + "` or `"0 + "`) added to the reducer makes `growth` a non-bare
/// reducer, so LTM hoists the `SUM(matrix[D1,*] * scale)` into a SYNTHETIC
/// `$⁚ltm⁚agg⁚N` node and the `scale -> agg` hop rides
/// `emit_source_to_agg_link_scores`' (already correct) scalar-feeder arm. The
/// two spellings must score the feeder hop -- and the loop -- identically
/// (the strongest test in this task). The `"0 + "` prefix keeps the
/// SIMULATED DYNAMICS byte-identical to the whole-RHS spelling, which the
/// time-varying agreement case relies on.
fn gh790_subexpr_fixture_with(prefix: &str, matrix_eqn: &str) -> datamodel::Project {
    TestProject::new("gh790_subexpr")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], matrix_eqn, None)
        .scalar_aux("scale", "SUM(pop[*]) * 0.005")
        .array_flow(
            "growth[D1]",
            &format!("{prefix}SUM(matrix[D1, *] * scale)"),
            None,
        )
        .build_datamodel()
}

/// GH #790: the scalar feeder of a whole-RHS variable-backed reduce gets ONE
/// Bare A2A changed-last score (dimensioned over the agg's `result_dims`),
/// NOT the per-target-element `scale -> growth[a]` / `[b]` partials that fail
/// fragment compile and degrade every loop through the feeder to a warned
/// constant-0 stub.
///
/// Hand-derived (changed-last, only the scalar feeder frozen, the co-source
/// slice verbatim -- the changed-FIRST partial would freeze the wildcard
/// `matrix[d1,*]` slice as an uncompilable lagged whole-array read):
///
/// ```text
/// numerator(scale -> growth)[r] = growth[r] - sum(matrix[r,*] * PREVIOUS(scale))
///                               = (Σ_c matrix[r,c]) * Δscale
/// ```
///
/// With `matrix` constant the numerator equals `Δgrowth[r]` exactly, so the
/// A2A feeder score is +1 from step 1. The loop
/// `$agg(=SUM(pop[*])) -> scale -> growth[r] -> pop[r] -> $agg` has two
/// per-element circuits sharing the scalar `scale` hop, so each loop score
/// settles at ~0.5 (the scalar's sensitivity splits across the two source
/// elements feeding `$agg`).
#[test]
fn scalar_feeder_of_whole_rhs_reduce_scores_via_agg_arm() {
    let project = gh790_whole_rhs_fixture("SUM");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The `growth` reduce is VARIABLE-BACKED: growth IS the agg, so the feeder
    // edge targets `growth` directly, not a synthetic `$⁚ltm⁚agg⁚N` minted for
    // growth. (A `$⁚ltm⁚agg⁚0` DOES exist here, but for `scale`'s own
    // `SUM(pop[*])` reducer -- the `pop -> scale` chain -- not for growth.)
    assert!(
        !ltm_vars.iter().any(|v| v
            .name
            .starts_with(&format!("{LINK_SCORE_PREFIX}scale\u{2192}$"))),
        "the scalar-feeder edge into growth must be variable-backed (target growth, \
         not a synthetic agg); got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    // The broken per-target-element scores must NOT be emitted.
    for elem in ["a", "b"] {
        let broken = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth[{elem}]");
        assert!(
            !ltm_vars.iter().any(|v| v.name == broken),
            "the broken per-element feeder score {broken:?} must NOT be emitted; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }

    // The single Bare A2A feeder score is emitted, dimensioned over the agg's
    // result dims (`[D1]`), with the hand-derived changed-last equation.
    let feeder_name = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth");
    let feeder = ltm_var(&ltm_vars, &feeder_name);
    assert_eq!(
        feeder.dimensions,
        vec!["D1".to_string()],
        "the scalar-feeder score must be A2A over the agg's result dims"
    );
    assert_eq!(
        feeder.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((growth - PREVIOUS(growth)) = 0) OR \
         ((scale - PREVIOUS(scale)) = 0) then 0 else SAFEDIV((growth - (sum(matrix[d1, *] * \
         PREVIOUS(scale)))), ABS((growth - PREVIOUS(growth))), 0) * SIGN((scale - PREVIOUS(scale)))",
        "the scalar-feeder changed-last equation must match the hand-derived form"
    );

    // Zero assembly warnings: every emitted fragment compiles.
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "the whole-RHS scalar-feeder fixture must compile every LTM fragment cleanly; got: {:?}",
        assembly_warnings(&db, sync.project)
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    const TOL: f64 = 1e-9;

    // The A2A feeder score is +1 per slot at every step past the first.
    let base = offset_of(&results, &feeder_name);
    for slot in 0..2 {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "feeder score slot {slot} at step {step}: got {v}, expected +1. A \
                 constant 0 here is the GH #790 fragment-failure-stub signature."
            );
        }
    }

    // Two per-element loops, each sustained at ~0.5 (the shared scalar hop).
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "expected one per-element loop through the scalar feeder; got {loop_names:?}"
    );
    for name in &loop_names {
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 0.5).abs() <= 1e-6,
                "{name} at step {step}: got {v}, expected ~0.5 (shared-scalar loop). A \
                 constant 0 is the GH #790 silently-degraded signature."
            );
        }
    }
}

/// GH #790, the whole reducer class: MEAN/MIN/MAX scalar-feeder shapes are
/// scored identically to SUM. The changed-last convention freezes only the
/// scalar feeder; the reducer body is the agg's own text, so the score arm is
/// reducer-agnostic. Each must end CORRECT (one A2A feeder score, zero
/// warnings), never silent-wrong, never an unwarned 0-stub.
#[test]
fn scalar_feeder_of_whole_rhs_reduce_covers_reducer_class() {
    for reducer in ["MEAN", "MIN", "MAX"] {
        let project = gh790_whole_rhs_fixture(reducer);
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
            .vars
            .clone();
        compile_project_incremental(&db, sync.project, "main")
            .unwrap_or_else(|e| panic!("{reducer}: LTM-enabled compilation should succeed: {e:?}"));

        let feeder_name = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth");
        let feeder = ltm_var(&ltm_vars, &feeder_name);
        assert_eq!(
            feeder.dimensions,
            vec!["D1".to_string()],
            "{reducer}: the scalar-feeder score must be A2A over the agg's result dims"
        );
        for elem in ["a", "b"] {
            let broken = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth[{elem}]");
            assert!(
                !ltm_vars.iter().any(|v| v.name == broken),
                "{reducer}: the broken per-element feeder score {broken:?} must NOT be emitted"
            );
        }
        assert!(
            assembly_warnings(&db, sync.project).is_empty(),
            "{reducer}: every LTM fragment must compile cleanly; got: {:?}",
            assembly_warnings(&db, sync.project)
        );
    }
}

/// GH #790, the SPELLING-AGREEMENT test (the strongest in this task): the
/// whole-RHS spelling (`growth = SUM(matrix[D1,*] * scale)`, variable-backed)
/// and the subexpression spelling (`growth = <const> + SUM(matrix[D1,*] *
/// scale)`, synthetic-agg-backed) are two spellings of the SAME dataflow. The
/// scalar feeder's link score AND the loop scores must AGREE
/// element-for-element, step-for-step -- the whole-RHS path is now routed to
/// the same changed-last convention the subexpression path always used.
///
/// Two cases:
///  - CONSTANT matrix (`0.1 +` prefix): every score is the analytic constant
///    (feeder +1, loops 0.5), so agreement is exact but trivial.
///  - TIME-VARYING matrix (`matrix = pop[D1] * 0.05`, `0 +` prefix so the
///    simulated dynamics are byte-identical between spellings): the
///    changed-last feeder score is genuinely time-varying (the numerator
///    `Σ_c matrix_t[r,c]·Δscale` no longer equals `Δgrowth[r]`, which also
///    carries the `Δmatrix` terms), so pointwise agreement here pins the
///    CONVENTION, not just the constants. A vacuity guard asserts the score
///    actually departs from +/-1.
#[test]
fn scalar_feeder_whole_rhs_and_subexpr_spellings_agree() {
    struct Case {
        label: &'static str,
        matrix_eqn: &'static str,
        subexpr_prefix: &'static str,
        require_time_varying: bool,
        /// Loops in EACH spelling: the 2 per-element feeder circuits, plus --
        /// when `matrix` depends on `pop` -- the 4 per-(row, col) co-source
        /// circuits (`pop[r] -> matrix[r,c] -> growth[r] -> pop[r]`).
        expected_loops: usize,
    }
    let cases = [
        Case {
            label: "constant-matrix",
            matrix_eqn: "5",
            subexpr_prefix: "0.1 + ",
            require_time_varying: false,
            expected_loops: 2,
        },
        Case {
            label: "time-varying-matrix",
            matrix_eqn: "pop[D1] * 0.05",
            subexpr_prefix: "0 + ",
            require_time_varying: true,
            expected_loops: 6,
        },
    ];

    const TOL: f64 = 1e-9;

    for case in &cases {
        let (whole_results, _) = run_ltm(&gh790_whole_rhs_fixture_with("SUM", case.matrix_eqn));
        let (subexpr_results, _) = run_ltm(&gh790_subexpr_fixture_with(
            case.subexpr_prefix,
            case.matrix_eqn,
        ));

        // The feeder link score: A2A `scale->growth` (whole-RHS) vs A2A
        // `scale->$agg0` (subexpr). Both are dimensioned over `[D1]` --
        // compare per slot.
        let whole_feeder = offset_of(
            &whole_results,
            &format!("{LINK_SCORE_PREFIX}scale\u{2192}growth"),
        );
        let sub_feeder = offset_of(
            &subexpr_results,
            &format!("{LINK_SCORE_PREFIX}scale\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0"),
        );
        let mut saw_non_unit = false;
        for slot in 0..2 {
            let w = series_at(&whole_results, whole_feeder + slot);
            let s = series_at(&subexpr_results, sub_feeder + slot);
            assert_eq!(
                w.len(),
                s.len(),
                "[{}] spellings must run the same step count",
                case.label
            );
            for (step, (&wv, &sv)) in w.iter().zip(&s).enumerate() {
                assert!(
                    (wv - sv).abs() <= TOL,
                    "[{}] feeder score slot {slot} step {step}: whole-RHS {wv} != subexpr {sv}",
                    case.label
                );
                if step > 0 && (wv.abs() - 1.0).abs() > 0.01 {
                    saw_non_unit = true;
                }
            }
        }
        // Vacuity guard for the time-varying case: a feeder score pinned at
        // +/-1 would mean the dynamics degenerated back to the trivial
        // constant-matrix agreement.
        if case.require_time_varying {
            assert!(
                saw_non_unit,
                "[{}] the time-varying feeder score must depart from +/-1 at \
                 some step (otherwise this case is as weak as the constant one)",
                case.label
            );
        }

        // The loop scores: two per-element circuits in each spelling. Their
        // sorted step-by-step series must match (the loop products through
        // the shared scalar feeder are identical).
        let collect_loops = |results: &Results| -> Vec<Vec<f64>> {
            let mut series: Vec<Vec<f64>> = ltm_score_var_names(results)
                .into_iter()
                .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
                .map(|n| series_at(results, offset_of(results, &n)))
                .collect();
            // Deterministic comparison order: sort by the series content.
            series.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            series
        };
        let whole_loops = collect_loops(&whole_results);
        let sub_loops = collect_loops(&subexpr_results);
        assert_eq!(
            whole_loops.len(),
            sub_loops.len(),
            "[{}] both spellings must enumerate the same number of loops",
            case.label
        );
        assert_eq!(
            whole_loops.len(),
            case.expected_loops,
            "[{}] unexpected loop count",
            case.label
        );
        for (wl, sl) in whole_loops.iter().zip(&sub_loops) {
            for (step, (&wv, &sv)) in wl.iter().zip(sl).enumerate() {
                assert!(
                    (wv - sv).abs() <= TOL,
                    "[{}] loop score step {step}: whole-RHS {wv} != subexpr {sv}",
                    case.label
                );
            }
        }
    }
}

/// GH #790, DISCOVERY-mode twin (the #748/#698 symmetric-exercise lesson):
/// discovery mode reaches the same `try_scalar_to_arrayed_link_scores`
/// routing, so the scalar-feeder edge of a whole-RHS variable-backed reduce
/// must EMIT there too -- ONE A2A `scale->growth` score, no broken
/// per-element scores, zero warnings.
///
/// SCOPE: this test asserts the emitted vars and warnings only. The
/// companion `gh754_scalar_feeder_loop_discoverable_in_discovery_mode` drives
/// the SAME fixture through the discovery SEARCH and proves the loop through
/// the scalar feeder is now found, and
/// `gh754_scalar_feeder_discovery_matches_exhaustive_scores` pins per-step
/// parity with the exhaustive surface. Before GH #754 the emitted scalar-from
/// Bare A2A name was structurally undiscoverable: `expand_a2a_link_offsets`
/// subscripted BOTH endpoints over the score's dims, inventing a `scale[elem]`
/// from-node that matched no real node, so the edge dangled and every loop
/// through the feeder was silently absent. It now projects the from-node onto
/// the source's OWN dims (bare for a scalar feeder), in lockstep with the
/// element graph.
#[test]
fn scalar_feeder_of_whole_rhs_reduce_emits_cleanly_in_discovery_mode() {
    let project = gh790_whole_rhs_fixture("SUM");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    compile_project_incremental(&db, sync.project, "main")
        .expect("discovery-mode LTM compilation should succeed");

    let feeder_name = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth");
    let feeder = ltm_var(&ltm_vars, &feeder_name);
    assert_eq!(
        feeder.dimensions,
        vec!["D1".to_string()],
        "discovery mode must also emit the A2A scalar-feeder score; got: {:?}",
        ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    for elem in ["a", "b"] {
        let broken = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth[{elem}]");
        assert!(
            !ltm_vars.iter().any(|v| v.name == broken),
            "discovery mode must not emit the broken per-element feeder score {broken:?}"
        );
    }
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "discovery mode must compile every LTM fragment cleanly; got: {:?}",
        assembly_warnings(&db, sync.project)
    );
}

/// GH #754 (scalar-source leg), the must-fix the #790 review surfaced: the
/// scalar-feeder Bare A2A score (`scale->growth`) must be DISCOVERABLE in
/// discovery mode, not merely emitted cleanly. Drives the gh790 fixture
/// through the ACTUAL discovery search (`discover_loops_with_graph`, the
/// `analysis::run_ltm_pipeline` recipe) and asserts the feedback loop
/// `pop -> scale -> growth -> pop` is found, with `scale` (the bare scalar
/// from-node) appearing in the discovered loop's links.
///
/// Pre-fix `expand_a2a_link_offsets` subscripts BOTH endpoints over the
/// score's `[D1]` dims, inventing `scale[a]`/`scale[b]` from-nodes that
/// match no real node (the scalar source's node is bare `scale`); the edge
/// dangles, the loop is structurally unreachable, and the search returns
/// ZERO loops. This is the silent completeness gap the #790 trade widened
/// (pre-#790 the broken per-element scores at least warned via 0-stubs).
#[test]
fn gh754_scalar_feeder_loop_discoverable_in_discovery_mode() {
    let project = gh790_whole_rhs_fixture("SUM");

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the gh790 scalar-feeder repro")
    .loops;

    // The per-element circuit `pop[e] -> scale -> growth[e] -> pop[e]` must be
    // discovered for each element. The scalar `scale` hop is the one the
    // phantom `scale[e]` from-node broke.
    assert!(
        !found.is_empty(),
        "discovery must find the loop through the scalar feeder `scale`; got \
         none (the phantom scale[elem] from-node dangled the edge)"
    );
    let mut saw_scale_hop = false;
    for fl in &found {
        for link in &fl.loop_info.links {
            // The bare scalar `scale` node must appear verbatim -- never a
            // subscripted `scale[a]`/`scale[b]` phantom.
            assert!(
                !link.from.as_str().starts_with("scale["),
                "discovered loop {} runs through a phantom subscripted scale node: {:?}",
                fl.loop_info.id,
                fl.loop_info.links
            );
            assert!(
                !link.to.as_str().starts_with("scale["),
                "discovered loop {} targets a phantom subscripted scale node: {:?}",
                fl.loop_info.id,
                fl.loop_info.links
            );
            if link.from.as_str() == "scale" || link.to.as_str() == "scale" {
                saw_scale_hop = true;
            }
        }
        // The discovered loop's per-step scores parse and stay finite.
        assert!(
            fl.scores.iter().all(|(_, v)| v.is_finite()),
            "discovered loop {} scores must stay finite",
            fl.loop_info.id
        );
    }
    assert!(
        saw_scale_hop,
        "no discovered loop hops through the bare scalar `scale` node; loops: {:?}",
        found
            .iter()
            .map(|fl| &fl.loop_info.links)
            .collect::<Vec<_>>()
    );
}

/// GH #754 (scalar-source leg), the strongest parity assertion (the
/// #748/#698 symmetric-exercise lesson): the loop discovered through the
/// scalar feeder must score POINTWISE-identically to the same loop's
/// exhaustive-mode `loop_score` series on the same model and VM run. The
/// gh790 fixture's variable-level SCC is tiny (pop, scale, growth), so it
/// runs exhaustively by default; discovery is force-flipped by the harness.
/// Both modes share the VM oracle and the link-score values, so the
/// discovered per-element loop score must equal the exhaustive per-element
/// `loop_score` series the compiler emits.
#[test]
fn gh754_scalar_feeder_discovery_matches_exhaustive_scores() {
    let project = gh790_whole_rhs_fixture("SUM");

    // Discovery surface: the per-element loops the search finds.
    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let discovered = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed")
    .loops;
    assert!(
        !discovered.is_empty(),
        "discovery must find the scalar-feeder loop for the parity check"
    );

    // Exhaustive surface: the compiler's per-element loop_score series.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("exhaustive LTM compilation should succeed");
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end().expect("VM run should complete");
    let exhaustive_results = vm.into_results();
    let exhaustive_loop_series: Vec<Vec<f64>> = ltm_score_var_names(&exhaustive_results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .map(|n| series_at(&exhaustive_results, offset_of(&exhaustive_results, &n)))
        .collect();
    assert!(
        !exhaustive_loop_series.is_empty(),
        "the exhaustive surface must emit at least one loop_score for the parity check"
    );

    // Every discovered loop's |score| series must match some exhaustive
    // loop_score series pointwise (sign conventions differ across surfaces;
    // the magnitudes -- the dominance-bearing quantity -- must agree). The VM
    // run and link-score values are shared, so an exact match (modulo the
    // STARTUP_STEPS guard) is the right bar.
    const TOL: f64 = 1e-9;
    for fl in &discovered {
        let disc: Vec<f64> = fl.scores.iter().map(|(_, v)| v.abs()).collect();
        let matched = exhaustive_loop_series.iter().any(|ex| {
            ex.len() == disc.len()
                && disc
                    .iter()
                    .zip(ex.iter())
                    .skip(STARTUP_STEPS)
                    .all(|(d, e)| (d - e.abs()).abs() <= TOL)
        });
        assert!(
            matched,
            "discovered loop {} score series has no pointwise match among the \
             exhaustive loop_score series; discovered |scores|: {:?}; exhaustive: {:?}",
            fl.loop_info.id, disc, exhaustive_loop_series
        );
    }
}

/// GH #754 (lower-dim arrayed-source leg): a Bare A2A score whose source has
/// FEWER dims than the score (`stock[Region] -> pop[Region,Age]`, score dims
/// `[Region,Age]`) must project the from-node onto the source's OWN dims
/// (`stock[nyc]`, broadcast over the unshared `Age`), never the phantom
/// `stock[nyc,young]` the both-sides expansion mints. Driven through the
/// real discovery search; the same-element circuits must be found and every
/// `stock` hop must read its own region element.
#[test]
fn gh754_lower_dim_feeder_loop_discoverable_in_discovery_mode() {
    // `pop[Region,Age]` integrates `growth`, which broadcasts the lower-dim
    // arrayed `boost[Region]` over `Age` -- a Bare A2A edge
    // `boost[Region] -> growth[Region,Age]` whose score is over the target's
    // `[Region,Age]` dims while `boost`'s own declared dims are `[Region]`.
    let project = TestProject::new("gh754_lower_dim_feeder")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("boost[Region]", "pop[Region, young] * 0.0001")
        .array_flow(
            "growth[Region, Age]",
            "boost[Region] * pop[Region, Age]",
            None,
        )
        .array_stock("pop[Region, Age]", "100", &["growth"], &[], None)
        .build_datamodel();

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the lower-dim feeder repro")
    .loops;

    assert!(
        !found.is_empty(),
        "discovery must find the loop through the lower-dim feeder `boost`; \
         got none (the phantom boost[region,age] from-node dangled the edge)"
    );
    let mut saw_boost_hop = false;
    for fl in &found {
        for link in &fl.loop_info.links {
            // The `boost` from-node must carry exactly ONE element (its own
            // `Region`), never a phantom `[region,age]` pair.
            if let Some(sub) = link
                .from
                .as_str()
                .strip_prefix("boost[")
                .and_then(|r| r.strip_suffix("]"))
            {
                saw_boost_hop = true;
                assert!(
                    !sub.contains(','),
                    "discovered loop {} runs through a phantom multi-dim boost \
                     node `boost[{sub}]`; boost is declared over Region only: {:?}",
                    fl.loop_info.id,
                    fl.loop_info.links
                );
            }
        }
        assert!(
            fl.scores.iter().all(|(_, v)| v.is_finite()),
            "discovered loop {} scores must stay finite",
            fl.loop_info.id
        );
    }
    assert!(
        saw_boost_hop,
        "no discovered loop hops through the lower-dim `boost` node; loops: {:?}",
        found
            .iter()
            .map(|fl| &fl.loop_info.links)
            .collect::<Vec<_>>()
    );
}

/// GH #754 (the ORIGINAL mapped-dimension leg, positional #527): a Bare A2A
/// edge between POSITIONALLY-MAPPED dimensions (`pop[Region] -> mid[State]`
/// with a `State->Region` mapping) is scored over the target's `[State]`
/// dims, so before this fix `expand_a2a_link_offsets` minted `pop[s1]`/`pop[s2]`
/// -- elements of the WRONG dimension, naming no real node. The projection now
/// runs through `expand_same_element`'s mapped diagonal, spelling the from-node
/// `pop[<mapped source elem>]` (e.g. `pop[r1]`) in lockstep with the element
/// graph, so the mapped loop is discoverable.
///
/// (Only POSITIONAL mappings reach here: an element-mapped pair is declined
/// upstream by `link_score_dimensions` -- no Bare A2A score is emitted, the
/// GH #756 positional-only gate -- so no phantom can be minted for it.)
#[test]
fn gh754_mapped_feeder_loop_discoverable_in_discovery_mode() {
    let project = TestProject::new("gh754_mapped_feeder")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("pop[Region]", "100", &["inflow"], &[], None)
        // Bare reference to the `[Region]` `pop` from a `[State]`-iterated
        // equation: the positional `State->Region` mapping resolves it to the
        // diagonal, so `pop -> mid` is a mapped Bare A2A edge over `[State]`.
        .array_aux("mid[State]", "pop[State] * 0.0001")
        .array_flow("inflow[Region]", "mid[Region] * pop[Region]", None)
        .build_datamodel();

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the mapped feeder repro")
    .loops;

    // The mapped diagonal loops `pop[r] -> mid[s] -> inflow[r] -> pop[r]` (with
    // s the positional image of r) must be found, and every `pop -> mid` hop
    // must read pop from the positionally-mapped region -- never a phantom
    // `pop[s]` (an element of the wrong dimension).
    assert!(
        !found.is_empty(),
        "discovery must find the mapped feeder loop; got none (the phantom \
         pop[state] from-node dangled the edge)"
    );
    let positional = [("s1", "r1"), ("s2", "r2")];
    let mut saw_mapped_hop = false;
    for fl in &found {
        for link in &fl.loop_info.links {
            // A `pop -> mid[s]` hop must come from the mapped region pop[r].
            if let Some(s) = link
                .to
                .as_str()
                .strip_prefix("mid[")
                .and_then(|r| r.strip_suffix("]"))
            {
                saw_mapped_hop = true;
                let expected_region = positional
                    .iter()
                    .find(|(state, _)| *state == s)
                    .map(|(_, r)| *r)
                    .unwrap_or_else(|| panic!("unexpected mid state {s}"));
                assert_eq!(
                    link.from.as_str(),
                    format!("pop[{expected_region}]"),
                    "mapped hop into mid[{s}] must read pop[{expected_region}] \
                     (the positional image), not {} -- a phantom wrong-dimension node",
                    link.from.as_str()
                );
            }
            // No phantom: a `pop[...]` node must never carry a STATE element.
            if let Some(e) = link
                .from
                .as_str()
                .strip_prefix("pop[")
                .and_then(|r| r.strip_suffix("]"))
            {
                assert!(
                    e == "r1" || e == "r2",
                    "phantom pop node `pop[{e}]` (not a Region element)"
                );
            }
        }
        assert!(
            fl.scores.iter().all(|(_, v)| v.is_finite()),
            "discovered loop {} scores must stay finite",
            fl.loop_info.id
        );
    }
    assert!(
        saw_mapped_hop,
        "no discovered loop hops through the mapped `mid` node; loops: {:?}",
        found
            .iter()
            .map(|fl| &fl.loop_info.links)
            .collect::<Vec<_>>()
    );
}

/// GH #754 slot-attribution oracle (review hardening): the gh790-family
/// integration fixtures are SLOT-SYMMETRIC -- constant co-sources make the
/// feeder score identically +1 in every slot -- so a mutation that rotates
/// each expanded edge's result slot by one passes them all (only the
/// `parse_link_offsets` unit tests catch it). This test is the
/// defense-in-depth layer: an ASYMMETRIC fixture whose Bare A2A scores are
/// per-slot DISTINCT on BOTH projection arms, plus a per-loop oracle
/// asserting each discovered loop's score series equals the product of
/// INDEPENDENTLY slot-resolved link-score series pointwise.
///
/// Fixture: `growth[D1,D2] = SUM(m3[D1,D2,*] * scale * pop[D1,D2])` with
/// per-element-distinct `m3` overrides and `scale = SUM(pop[*,*])` (whole-RHS
/// reduces only -- NO synthetic agg nodes, so the reported loop links ARE the
/// scored links). The distinct m3 rows make the pop slots diverge, so:
///
///  - `scale -> growth` (the SCALAR projection arm) scores per-slot
///    distinctly (scale's share of each slot's growth change differs), and
///  - `growth -> inflow` / `pop -> inflow` (the `expand_same_element`
///    equal-dim arm, via `inflow[e] = growth[e] + pop[e]*0.001`) score
///    per-slot distinctly (the growth-vs-pop contribution ratio differs).
///
/// A slot-rotation mutation in EITHER arm makes the engine's loop product
/// read a neighboring slot's series while the oracle reads the right one --
/// they diverge at the first step where the slots differ. (Mutation-validated
/// before commit: rotating each arm's offsets by one fails this test.)
#[test]
fn gh754_asymmetric_loop_scores_match_slot_resolved_link_products() {
    let project = TestProject::new("gh754_slot_oracle")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .named_dimension("D3", &["p", "q"])
        .array_with_ranges_direct(
            "m3",
            vec!["D1".into(), "D2".into(), "D3".into()],
            vec![
                ("a,c,p", "0.00001"),
                ("a,c,q", "0.00002"),
                ("a,d,p", "0.00003"),
                ("a,d,q", "0.00004"),
                ("b,c,p", "0.00005"),
                ("b,c,q", "0.00006"),
                ("b,d,p", "0.00007"),
                ("b,d,q", "0.00008"),
            ],
            None,
        )
        .array_stock("pop[D1,D2]", "100", &["inflow"], &[], None)
        .scalar_aux("scale", "SUM(pop[*, *])")
        .array_aux("growth[D1,D2]", "SUM(m3[D1, D2, *] * scale * pop[D1, D2])")
        .array_flow(
            "inflow[D1,D2]",
            "growth[D1, D2] + pop[D1, D2] * 0.001",
            None,
        )
        .build_datamodel();

    let inputs = crate::test_helpers::ltm_discovery_inputs(&project, "main");
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on the asymmetric oracle fixture")
    .loops;
    let results = &inputs.vm_results;

    // Per-dim canonical element lists, for independent row-major slot
    // resolution of Bare A2A scores.
    let dim_elements: std::collections::HashMap<String, Vec<String>> = inputs
        .dims
        .iter()
        .map(|d| {
            let elems = match &d.elements {
                datamodel::DimensionElements::Named(names) => names
                    .iter()
                    .map(|n| n.to_lowercase())
                    .collect::<Vec<String>>(),
                datamodel::DimensionElements::Indexed(size) => {
                    (1..=*size).map(|i| i.to_string()).collect()
                }
            };
            (d.name().to_lowercase(), elems)
        })
        .collect();

    // Independently resolve a loop link's result-slot offset: an exact
    // element-level score name (the passthrough families) wins; otherwise the
    // Bare A2A name's base offset plus the TARGET element's row-major slot
    // over the score's own dims -- the layout rule the runtime wrote the
    // score with, derived here from the datamodel rather than via
    // `parse_link_offsets`, so a slot-misattribution there cannot fool the
    // oracle into agreeing with itself.
    let resolve = |from: &str, to: &str| -> usize {
        let exact = format!("{LINK_SCORE_PREFIX}{from}\u{2192}{to}");
        if let Some((_, off)) = results.offsets.iter().find(|(k, _)| k.as_str() == exact) {
            return *off;
        }
        let from_base = from.split('[').next().unwrap();
        let (to_base, to_elem) = match to.split_once('[') {
            Some((b, rest)) => (b, rest.strip_suffix(']').unwrap()),
            None => (to, ""),
        };
        let bare = format!("{LINK_SCORE_PREFIX}{from_base}\u{2192}{to_base}");
        let base = offset_of(results, &bare);
        if to_elem.is_empty() {
            return base;
        }
        let score_var = inputs
            .ltm_vars
            .iter()
            .find(|v| v.name == bare)
            .unwrap_or_else(|| panic!("no LtmSyntheticVar for {bare}"));
        let parts: Vec<&str> = to_elem.split(',').collect();
        assert_eq!(
            parts.len(),
            score_var.dimensions.len(),
            "target element {to_elem} arity must match {bare}'s score dims {:?}",
            score_var.dimensions
        );
        let mut slot = 0usize;
        for (part, dim_name) in parts.iter().zip(&score_var.dimensions) {
            let elems = &dim_elements[&dim_name.to_lowercase()];
            let idx = elems
                .iter()
                .position(|e| e == part)
                .unwrap_or_else(|| panic!("element {part} not in dim {dim_name}: {elems:?}"));
            slot = slot * elems.len() + idx;
        }
        base + slot
    };

    // Fixture-asymmetry guard: the two arm-exercising Bare A2A scores must be
    // per-slot pairwise DISTINCT at the final step, otherwise a rotation
    // mutation is invisible and this oracle proves nothing.
    let last = results.step_count - 1;
    for bare in ["scale\u{2192}growth", "growth\u{2192}inflow"] {
        let base = offset_of(results, &format!("{LINK_SCORE_PREFIX}{bare}"));
        let vals: Vec<f64> = (0..4)
            .map(|slot| results.data[last * results.step_size + base + slot])
            .collect();
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert!(
                    (vals[i] - vals[j]).abs() > 1e-9,
                    "{bare} slots {i} and {j} coincide at the final step ({vals:?}); \
                     the fixture degenerated to slot-symmetric and cannot catch \
                     slot misattribution"
                );
            }
        }
    }

    // Both projection arms must appear in the discovered loop set, with the
    // bare scalar `scale` node and the equal-dim diagonal hops.
    let mut saw_scalar_arm_hop = false;
    let mut saw_equal_dim_arm_hop = false;

    // THE ORACLE: each discovered loop's per-step score equals the product of
    // its links' independently slot-resolved score series, pointwise (the
    // engine multiplies in link order; mirror it exactly).
    assert!(!found.is_empty(), "discovery must find the fixture's loops");
    let mut compared_finite = 0usize;
    for fl in &found {
        let offsets: Vec<usize> = fl
            .loop_info
            .links
            .iter()
            .map(|l| {
                if l.from.as_str() == "scale" {
                    saw_scalar_arm_hop = true;
                }
                if l.from.as_str().starts_with("growth[") && l.to.as_str().starts_with("inflow[") {
                    saw_equal_dim_arm_hop = true;
                }
                resolve(l.from.as_str(), l.to.as_str())
            })
            .collect();
        for (step, (_, engine_score)) in fl.scores.iter().enumerate() {
            let mut oracle = 1.0_f64;
            let mut has_nan = false;
            for &off in &offsets {
                let v = results.data[step * results.step_size + off];
                if v.is_nan() {
                    has_nan = true;
                    break;
                }
                oracle *= v;
            }
            if has_nan {
                assert!(
                    engine_score.is_nan(),
                    "loop {} step {step}: oracle product is NaN but the engine \
                     scored {engine_score} -- the engine read different slots",
                    fl.loop_info.id
                );
                continue;
            }
            assert!(
                (oracle - engine_score).abs() <= 1e-9 * oracle.abs().max(1.0),
                "loop {} step {step}: engine score {engine_score} != slot-resolved \
                 oracle product {oracle}; links: {:?} -- a result-slot \
                 misattribution in the discovery A2A expansion",
                fl.loop_info.id,
                fl.loop_info.links
            );
            if engine_score.is_finite() && *engine_score != 0.0 {
                compared_finite += 1;
            }
        }
    }
    assert!(
        compared_finite > 0,
        "the oracle never compared a finite non-zero step -- the fixture carries no signal"
    );
    assert!(
        saw_scalar_arm_hop,
        "no discovered loop hops through the scalar `scale` feeder; the scalar \
         projection arm is unexercised"
    );
    assert!(
        saw_equal_dim_arm_hop,
        "no discovered loop traverses growth[e] -> inflow[e]; the equal-dim \
         projection arm is unexercised"
    );
}

/// GH #790, sibling audit: a scalar feeder ALONGSIDE an iterated-dim
/// projection feeder (`growth[D1] = SUM(matrix[D1,*] * frac[D1] * scale)`).
/// The scalar `scale` rides the new A2A feeder arm; the arrayed `frac[D1]`
/// rides the GH #767 per-`(row, slot)` projection-feeder arm. Both must end
/// correct, zero warnings -- the two feeder kinds coexist on one reducer.
#[test]
fn scalar_and_iterated_feeders_coexist_cleanly() {
    let project = TestProject::new("gh790_mixed_feeders")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "pop[D1] * 0.001")
        .scalar_aux("scale", "SUM(pop[*]) * 0.005")
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * frac[D1] * scale)", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The scalar feeder gets the A2A score.
    let scale_feeder = ltm_var(
        &ltm_vars,
        &format!("{LINK_SCORE_PREFIX}scale\u{2192}growth"),
    );
    assert_eq!(
        scale_feeder.dimensions,
        vec!["D1".to_string()],
        "the scalar feeder must get the A2A score even alongside an iterated feeder"
    );
    // The iterated feeder gets the per-(row, slot) projection scores.
    for row in ["a", "b"] {
        let name = format!("{LINK_SCORE_PREFIX}frac[{row}]\u{2192}growth[{row}]");
        assert!(
            ltm_vars.iter().any(|v| v.name == name),
            "missing the iterated feeder's per-row score {name}; got: {:?}",
            ltm_vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
    }
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "the mixed-feeder fixture must compile every LTM fragment cleanly; got: {:?}",
        assembly_warnings(&db, sync.project)
    );
}

/// GH #790 review follow-up (the GH #777 broadcast sub-shape): a scalar
/// feeder of an ARRAYED-owner scalar-result BROADCAST reduce --
/// `growth[D9] = SUM(matrix[a,*] * scale)` (Pinned slice, NO Iterated axis,
/// `result_dims` empty, owner arrayed over a dim disjoint from the
/// source's). `variable_backed_reduce_agg`'s broadcast arm admits it, so the
/// scalar-feeder routing fires with EMPTY `result_dims`; the initial GH #790
/// commit emitted `Equation::Scalar` for it, whose bare multi-slot `growth`
/// reference failed assembly (1 fragment warning + 2 stubbed-loop warnings,
/// constant-0 series, edge NOT in `unscoreable_edges`) -- the exact
/// #758/#780 middle state #790 was filed to eliminate, one shape over.
///
/// The fix emits `Equation::ApplyToAll` over the OWNER's dims: the single
/// scalar reducer value feeds every `growth[e]` identically, the bare
/// `growth` reference element-resolves inside its own A2A context, and the
/// frozen reducer body (`sum(matrix[a,*] * PREVIOUS(scale))`) is
/// scalar-valued so it broadcasts cleanly.
///
/// Hand-derived per slot `e` (matrix constant = 5, |D2| = 2):
///
/// ```text
/// numerator = growth[e] - sum(matrix[a,*] * PREVIOUS(scale))
///           = (Σ_c matrix[a,c]) * Δscale = 10·Δscale = Δgrowth[e]
/// ```
///
/// so the A2A feeder score is +1 per slot from step 1, and each of the two
/// per-element circuits through the shared scalar `scale` scores ~0.5 -- and
/// must agree pointwise with the SUBEXPRESSION spelling
/// (`0 + SUM(matrix[a,*] * scale)`, synthetic SCALAR agg) of the same
/// dataflow.
#[test]
fn scalar_feeder_of_broadcast_reduce_scores_via_agg_arm() {
    let broadcast_fixture = |rhs: &str| -> datamodel::Project {
        TestProject::new("gh790_broadcast")
            .with_sim_time(0.0, 8.0, 1.0)
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["c", "d"])
            .named_dimension("D9", &["p", "q"])
            .array_stock("pop[D9]", "100", &["growth"], &[], None)
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .scalar_aux("scale", "SUM(pop[*]) * 0.005")
            .array_flow("growth[D9]", rhs, None)
            .build_datamodel()
    };

    let project = broadcast_fixture("SUM(matrix[a, *] * scale)");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm_vars = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .vars
        .clone();
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");

    // The feeder score is A2A over the OWNER's dims (result_dims is empty
    // for the broadcast slice), with the changed-last equation.
    let feeder_name = format!("{LINK_SCORE_PREFIX}scale\u{2192}growth");
    let feeder = ltm_var(&ltm_vars, &feeder_name);
    assert_eq!(
        feeder.dimensions,
        vec!["D9".to_string()],
        "the broadcast scalar-feeder score must be A2A over the OWNER's dims"
    );
    assert_eq!(
        feeder.equation.source_text(),
        "if (TIME = INITIAL_TIME) then 0 else if ((growth - PREVIOUS(growth)) = 0) OR \
         ((scale - PREVIOUS(scale)) = 0) then 0 else SAFEDIV((growth - (sum(matrix[a, *] * \
         PREVIOUS(scale)))), ABS((growth - PREVIOUS(growth))), 0) * SIGN((scale - PREVIOUS(scale)))",
        "the broadcast scalar-feeder changed-last equation must match the hand-derived form"
    );

    // Zero assembly warnings: the Scalar-emission regression produced 3
    // (the fragment failure + two stubbed loops).
    assert!(
        assembly_warnings(&db, sync.project).is_empty(),
        "the broadcast scalar-feeder fixture must compile every LTM fragment cleanly; got: {:?}",
        assembly_warnings(&db, sync.project)
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    let results = vm.into_results();

    const TOL: f64 = 1e-9;

    // +1 per slot at every step past the first.
    let base = offset_of(&results, &feeder_name);
    for slot in 0..2 {
        let series = series_at(&results, base + slot);
        for (step, &v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - 1.0).abs() <= TOL,
                "broadcast feeder score slot {slot} at step {step}: got {v}, expected +1. \
                 A constant 0 is the Scalar-emission fragment-failure-stub signature."
            );
        }
    }

    // Two per-element circuits, each ~0.5 sustained.
    let loop_names: Vec<String> = ltm_score_var_names(&results)
        .into_iter()
        .filter(|n| n.starts_with(LOOP_SCORE_PREFIX))
        .collect();
    assert_eq!(
        loop_names.len(),
        2,
        "expected one per-element loop through the broadcast feeder; got {loop_names:?}"
    );
    for name in &loop_names {
        let series = series_at(&results, offset_of(&results, name));
        for (step, &v) in series.iter().enumerate().skip(STARTUP_STEPS + 1) {
            assert!(
                (v - 0.5).abs() <= 1e-6,
                "{name} at step {step}: got {v}, expected ~0.5 (shared-scalar loop)."
            );
        }
    }

    // Spelling agreement: the SUBEXPRESSION spelling of the same dataflow
    // (`0 +` keeps the dynamics identical) routes the feeder through the
    // synthetic SCALAR agg's feeder arm; its scalar score must equal every
    // whole-RHS A2A slot pointwise.
    let (sub_results, sub_vars) = run_ltm(&broadcast_fixture("0 + SUM(matrix[a, *] * scale)"));
    let sub_feeder_name = &sub_vars
        .iter()
        .find(|v| {
            v.name
                .starts_with(&format!("{LINK_SCORE_PREFIX}scale\u{2192}$"))
        })
        .expect("the subexpr spelling must emit a scale -> synthetic-agg feeder score")
        .name;
    let sub_series = series_at(&sub_results, offset_of(&sub_results, sub_feeder_name));
    for slot in 0..2 {
        let w = series_at(&results, base + slot);
        assert_eq!(
            w.len(),
            sub_series.len(),
            "spellings must run the same step count"
        );
        for (step, (&wv, &sv)) in w.iter().zip(&sub_series).enumerate() {
            assert!(
                (wv - sv).abs() <= TOL,
                "broadcast feeder slot {slot} step {step}: whole-RHS {wv} != subexpr {sv}"
            );
        }
    }
}
