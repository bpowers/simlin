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
    DiagnosticError, DiagnosticSeverity, LtmSyntheticVar, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, model_ltm_variables, set_project_ltm_enabled,
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

/// GH #533, end-to-end: a feedback loop whose only path back to a *scalar*
/// stock runs through a *scalar* feeder of a hoisted reducer *with a scalar
/// target*. The fixture is a scalar stock `total` grown by
/// `grow = 1 + SUM(pop[*] * scale)`, with `scale = 0.001 * total + 0.01`
/// feeding back from `total`. `SUM(pop[*] * scale)` is a sub-expression, so it
/// is hoisted into a synthetic `$⁚ltm⁚agg⁚0`.
///
/// #533's well-specified fix is the ELEMENT GRAPH: the `(scale, grow)` causal
/// edge is classified `ThroughAgg`, but `scale`/`grow` are both scalar, so
/// before the fix `model_element_causal_edges`'s both-scalar fast path emitted
/// a direct `scale → grow` element edge instead of `scale → $⁚ltm⁚agg⁚0`. That
/// element-graph fix is pinned directly by
/// `element_graph_tests::element_graph_scalar_feeder_*`. This end-to-end test
/// pins the robustly-observable consequence: with LTM enabled the model still
/// COMPILES and SIMULATES (no crash, no NaN/Inf) and the loop is enumerated --
/// the well-formedness the element-graph fix preserves.
///
/// CHARACTERIZATION of two pre-existing gaps this scalar-target scenario
/// exposes (BOTH independent of #533's element-graph fast path -- both behave
/// identically with and without it, since the element edge the fix adds is
/// never consumed downstream here):
///
/// 1. The synthetic agg `$⁚ltm⁚agg⁚0` for `SUM(pop[*] * scale)` (arrayed `pop`
///    times scalar `scale`, reduced to a scalar target) currently FAILS to
///    compile and is stubbed to a constant `0` with an `Assembly` Warning.
///    This is an agg-augmentation gap (the emitted agg equation is rejected by
///    the compiler), distinct from the element-graph edge.
///
/// 2. Even were the agg computed, the loop-score *builder* would not route the
///    pure-scalar loop through it: `build_loops_from_tiered` materializes a
///    `PureScalar` fast-path cycle straight from the *variable-level* circuit
///    `total → scale → grow → total` (`var_graph.circuit_to_links`), linking
///    `scale → grow` directly -- never the agg. The
///    cross-element-through-aggregate recovery (`recover_cross_agg_loops`) runs
///    only on the slow (cross-element) path.
///
/// So the loop score is `0` here -- not because of #533, but because of (1)
/// and (2). Pinning the current behavior keeps both gaps observable; closing
/// them is separate, tracked work.
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

    // The pure-scalar loop `total → scale → grow → total` is enumerated and
    // scored via the direct `scale → grow` link (the variable-level circuit;
    // gap #2 in the doc comment). Exactly one such loop exists.
    let u_loops: Vec<&LtmSyntheticVar> = ltm_vars
        .iter()
        .filter(|v| {
            v.name.starts_with(LOOP_SCORE_PREFIX)
                && v.equation.source_text().contains("scale\u{2192}grow")
        })
        .collect();
    assert_eq!(
        u_loops.len(),
        1,
        "the scalar feedback loop must be enumerated as exactly one loop scored via the \
         direct scale→grow link; loop vars: {:?}",
        ltm_vars
            .iter()
            .filter(|v| v.name.starts_with(LOOP_SCORE_PREFIX))
            .map(|v| (v.name.as_str(), v.equation.source_text()))
            .collect::<Vec<_>>()
    );

    // Every LTM score variable must be finite at every timestep -- no NaN/Inf
    // leaks from the agg wiring or the stubbed link scores. This is the
    // well-formedness guarantee the #533 element-graph fix preserves; the
    // characterization of *which* scores are currently 0 (gaps #1/#2) lives in
    // the doc comment, not as a brittle per-value assertion.
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

/// Graceful-degradation pin for an LTM synthetic fragment that the compiler
/// genuinely cannot lower: it must be *stubbed to a constant 0 with a
/// `Warning`*, never crash the compiler.
///
/// The model below is the GH #525 shape: an arrayed stock `pop[region,age]`,
/// a row-reducer `row_sum[region] = pop[region,young] * 2` (a partially
/// iterated subscript -- `young` is fixed, `region` is iterated), and a flow
/// `growth[region,age] = row_sum[region] * 0.0001 * pop[region,age]` that
/// closes a feedback loop. With LTM enabled, the `pop -> row_sum` link
/// score's ceteris-paribus partial classifies the `pop[region,young]`
/// reference as `DynamicIndex` (#525) and wraps it as
/// `PREVIOUS(SUM(pop[region,young]))`; inside the SUM-reducer's array-view
/// index walk that inner `PREVIOUS` survives helper rewriting as a
/// non-variable expression and lowers to `NotSimulatable`.
///
/// Before the fix (#363 follow-up), the array-view index walk in
/// `codegen.rs` did `walk_expr(...).unwrap().unwrap()` and PANICKED on that
/// `Err`, killing the whole compile and defeating the LTM assemble path's
/// `Err(_) => None` graceful-stub handler. After the fix the `Err`
/// propagates, `module.compile()` returns `Err`, the synthetic fragment is
/// dropped, and the model compiles and simulates.
///
/// NOTE: the assertions that `$⁚ltm⁚link_score⁚pop→row_sum` reads a constant
/// `0` and that a `Warning` names it pin the CURRENT, intentionally-DEGRADED
/// #525 behavior -- a placeholder for the genuinely-unscoreable partial, NOT
/// the desired end state. GH #525's future reference-classifier fix will
/// replace the `DynamicIndex` stub with a real per-element score; when that
/// lands, the constant-0 / Warning expectations here flip and this test is
/// updated. What this test pins permanently is the *graceful* contract: a
/// synthetic LTM fragment the compiler rejects degrades to 0-plus-Warning,
/// never a panic. (No such graceful-degradation pin existed before.)
#[test]
fn unloweable_ltm_link_score_degrades_gracefully_no_panic() {
    let project = TestProject::new("ltm525_graceful")
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

    // The headline assertion: compilation SUCCEEDS rather than panicking
    // inside the array-view index walk.
    let compiled = compile_project_incremental(&db, sync.project, "main").expect(
        "GH #525-shaped arrayed LTM model must COMPILE (with the offending link \
         score stubbed), not panic in the array-view index walk",
    );

    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion with the link score stubbed");
    let results = vm.into_results();

    // Pin the CURRENT degraded #525 behavior: the un-scoreable link score is
    // stubbed to a constant 0. (When #525's classifier fix lands this flips.)
    let link_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}row_sum";
    let series = series_at(&results, offset_of(&results, link_name));
    assert!(
        series.iter().all(|&v| v == 0.0),
        "the un-scoreable {link_name} link score should be stubbed to a constant 0 \
         (current degraded #525 behavior); got {series:?}"
    );

    // A sibling link score on the SAME model whose partial *is* scoreable must
    // still carry real (non-zero) values -- so the failure is surgical (only
    // the genuinely-unloweable fragment is stubbed), not a blanket collapse.
    let sibling = "$\u{205A}ltm\u{205A}link_score\u{205A}row_sum\u{2192}growth";
    let sibling_series = series_at(&results, offset_of(&results, sibling));
    assert!(
        sibling_series.iter().any(|&v| v != 0.0),
        "the scoreable sibling {sibling} link score must still be computed; got \
         {sibling_series:?}"
    );

    // And the fragment-compile failure surfaces as a Warning that names the
    // stubbed link score (so the degradation is never silent).
    let diagnostics = collect_all_diagnostics(&db, sync.project);
    let names_link = diagnostics.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && d.variable.as_deref() == Some(link_name)
            && matches!(&d.error, DiagnosticError::Assembly(_))
    });
    assert!(
        names_link,
        "a Warning naming the stubbed link score {link_name:?} must be emitted; \
         diagnostics: {:?}",
        diagnostics
            .iter()
            .map(|d| (&d.variable, &d.severity))
            .collect::<Vec<_>>()
    );
}
