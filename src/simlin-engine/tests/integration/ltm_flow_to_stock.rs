// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The flow-to-stock link score, pinned against the 2023 correction paper's
//! own numbers and its aggregation-invariance argument.
//!
//! Schoenberg, Hayward and Eberlein (2023), "Improving Loops that Matter",
//! define the flow-to-stock link score (their Eq. 3) as the change in the
//! flow over the change in the stock's net flow, and observe (their section
//! 4.4) that an implementation may equivalently aggregate every stock's flows
//! into one net flow and score each flow's link into that net flow with the
//! ordinary instantaneous formula, the net flow's own link into the stock
//! being exactly 1. Simlin takes that second form: every stock with a scored
//! flow-to-stock edge gets a synthetic net-flow auxiliary
//! `$⁚ltm⁚net⁚{stock}` and the score is `sign * |Δflow / Δnet|`, read over
//! the same `[t - dt, t]` window as every other link score in the model.
//!
//! These tests pin the numbers that form implies -- the paper's worked table
//! at the step of the change (not one step later), exact agreement between a
//! model with separate flows and its twin with one net flow at the same
//! time, the 67/33 split of the births/deaths model, the per-element score
//! of a scalar flow feeding an arrayed stock and the loop through it, an
//! outflow-only stock, a stock whose initial value reads its own flow, and
//! the net aux's absence from every loop and link surface -- through the
//! real compile-and-simulate pipeline.
//!
//! The family's other arms are pinned elsewhere: a flow declared over
//! dimensions that MAP onto its stock's, per slot, in `ltm_array_agg.rs`
//! (`a_flow_feeding_its_stock_through_a_parent_mapping_scores_per_slot`); a
//! flow inside a module instance, whose net aux lives in the instance's
//! namespace, in `db/ltm_tests.rs`
//! (`delay3_input_to_stock_link_score_reads_the_bound_port`); the generated
//! text of every spelling (scalar, arrayed, outflow, other-dimensioned and
//! scalar flows, missing sides) in `ltm_augment_tests.rs` (`flow_to_stock_*`
//! and `net_flow_equation_*`).

use simlin_engine::datamodel::{Equation, Variable};
use simlin_engine::ltm_finding;
use simlin_engine::ltm_post;
use simlin_engine::test_common::TestProject;

use crate::test_helpers::{ltm_discovery_inputs, ltm_run, ltm_series};

/// `$⁚ltm⁚link_score⁚{from}→{to}`.
fn link_score(from: &str, to: &str) -> String {
    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")
}

/// Every step's absolute difference between two same-length series.
fn max_abs_diff(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "series lengths differ");
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

/// The 2023 paper's table (section 4.3): a stock with inflow `in` stepping
/// 5 -> 10 and outflow `out` stepping 4 -> 5 at one step, dt = 1.
///
/// Hand calculation at the step of the change (t = 2):
///
/// ```text
/// Δin  = 10 - 5 = 5
/// Δout =  5 - 4 = 1
/// net  = in - out:  1 at t = 1,  5 at t = 2,  so Δnet = 4
/// LS(in  -> S) = +|Δin  / Δnet| = +5/4 = +1.25
/// LS(out -> S) = -|Δout / Δnet| = -1/4 = -0.25
/// ```
///
/// which are the paper's 1.25 and 0.25 (its magnitudes) with the structural
/// signs. Every other step has `Δin = Δout = 0`, so both scores are 0 there
/// by the zero-delta guard -- including t = 3, where a score computed over
/// the previous interval would put the paper's values one step late.
///
/// The `0 * s` terms close a (causally inert) loop through each flow so
/// exhaustive mode scores the two flow-to-stock edges.
#[test]
fn paper_2023_table_scores_the_step_of_the_change() {
    let project = TestProject::new("paper_2023_table")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("s", "100", &["inflow"], &["outflow"], None)
        .flow("inflow", "5 + STEP(5, 2) + 0 * s", None)
        .flow("outflow", "4 + STEP(1, 2) + 0 * s", None)
        .build_datamodel();
    let results = ltm_run(&project, false).results;

    let inflow_score = ltm_series(&results, &link_score("inflow", "s"), 0);
    let outflow_score = ltm_series(&results, &link_score("outflow", "s"), 0);
    assert_eq!(
        inflow_score,
        vec![0.0, 0.0, 1.25, 0.0, 0.0, 0.0],
        "LS(inflow -> s) at t = 0..5"
    );
    assert_eq!(
        outflow_score,
        vec![0.0, 0.0, -0.25, 0.0, 0.0, 0.0],
        "LS(outflow -> s) at t = 0..5"
    );
}

/// The 2023 paper's aggregation invariance (its section 4.4), at the same
/// time step: a stock with separate nonlinear inflow and outflow, and its
/// twin where the same two expressions are auxiliaries feeding one net flow,
/// give the same flow-to-stock link score and the same relative loop scores
/// at every t.
///
/// In the aggregated twin the net flow is the stock's only flow, so its
/// flow-to-stock score is `|Δnet / Δnet| = 1` wherever it is non-zero and
/// the chain `LS(out -> net) * LS(net -> s)` reduces to the ordinary
/// instantaneous score of `net = in - out` with respect to `out`, which is
/// `-|Δout / Δnet|` -- the disaggregated twin's `LS(out -> s)` by
/// construction. The remaining differences are floating-point rounding in
/// the two spellings of `Δnet`, far below the tolerance.
#[test]
fn flow_to_stock_scores_are_aggregation_invariant_at_the_same_step() {
    let dt = 0.5;
    let disaggregated = TestProject::new("agg_dis")
        .with_sim_time(0.0, 20.0, dt)
        .stock("s", "10", &["inflow"], &["outflow"], None)
        .flow("inflow", "0.3 * s ^ 0.8", None)
        .flow("outflow", "0.02 * s ^ 1.3", None)
        .build_datamodel();
    let aggregated = TestProject::new("agg_agg")
        .with_sim_time(0.0, 20.0, dt)
        .stock("s", "10", &["net"], &[], None)
        .aux("inflow", "0.3 * s ^ 0.8", None)
        .aux("outflow", "0.02 * s ^ 1.3", None)
        .flow("net", "inflow - outflow", None)
        .build_datamodel();
    let dis = ltm_run(&disaggregated, false);
    let agg = ltm_run(&aggregated, false);

    let net_to_s = ltm_series(&agg.results, &link_score("net", "s"), 0);
    for flow in ["inflow", "outflow"] {
        let direct = ltm_series(&dis.results, &link_score(flow, "s"), 0);
        let through_net: Vec<f64> = ltm_series(&agg.results, &link_score(flow, "net"), 0)
            .iter()
            .zip(&net_to_s)
            .map(|(a, b)| a * b)
            .collect();
        // The stock moves at every step of this run, so the invariance is
        // exercised on non-zero scores, not on a vacuous pair of zero series.
        assert!(
            direct.iter().skip(1).all(|v| v.abs() > 1e-3),
            "LS({flow} -> s) is active at every step after the first: {direct:?}"
        );
        let diff = max_abs_diff(&direct, &through_net);
        assert!(
            diff < 1e-12,
            "LS({flow} -> s) differs from LS({flow} -> net) * LS(net -> s) by {diff:e}: \
             {direct:?} vs {through_net:?}"
        );
    }

    let dis_rel = ltm_post::compute_rel_loop_scores(&dis.results, &dis.loop_partitions);
    let agg_rel = ltm_post::compute_rel_loop_scores(&agg.results, &agg.loop_partitions);
    // Each twin has exactly one loop through the inflow and one through the
    // outflow (the static polarity of `s ^ p` is unknown, so the ids are
    // `u{n}` and their numbering is not shared across the twins).
    for flow in ["inflow", "outflow"] {
        let dis_id = dis.loop_through(flow);
        let agg_id = agg.loop_through(flow);
        let diff = max_abs_diff(&dis_rel[dis_id], &agg_rel[agg_id]);
        assert!(
            diff < 1e-12,
            "relative score of the loop through {flow} ({dis_id} / {agg_id}) differs \
             between the twins by {diff:e}"
        );
    }
}

/// Seamlessly Integrating (Schoenberg et al. 2020), figure 5: a stock with
/// births `0.1 * a` and deaths `a / 20` splits 67/33 between its reinforcing
/// and balancing loops.
///
/// Hand calculation, valid at every step after the first (`Δa > 0`):
///
/// ```text
/// LS(a -> births) = +1            (births = 0.1 a exactly, so Δ_a births = Δbirths)
/// LS(births -> a) = +|Δbirths / Δnet| = |0.1 Δa / 0.05 Δa| = +2
/// LS(a -> deaths) = +1
/// LS(deaths -> a) = -|Δdeaths / Δnet| = -|0.05 Δa / 0.05 Δa| = -1
/// loop r1 = +2, loop b1 = -1, so the relative scores are +2/3 and -1/3.
/// ```
///
/// The split holds from the first step after the start: every score's
/// window is `[t - dt, t]`, so no score needs a second step of history.
#[test]
fn births_and_deaths_split_two_thirds_one_third_from_the_first_step() {
    let project = TestProject::new("births_deaths")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("a", "100", &["births"], &["deaths"], None)
        .flow("births", "0.1 * a", None)
        .flow("deaths", "a / 20", None)
        .build_datamodel();
    let run = ltm_run(&project, false);
    let results = &run.results;
    let rel = ltm_post::compute_rel_loop_scores(results, &run.loop_partitions);

    let expected = |value: f64| -> Vec<f64> {
        let mut v = vec![value; results.step_count];
        v[0] = 0.0;
        v
    };
    for (id, value) in [("r1", 2.0 / 3.0), ("b1", -1.0 / 3.0)] {
        let diff = max_abs_diff(&rel[id], &expected(value));
        assert!(
            diff < 1e-9,
            "relative score of {id} should be {value} from t = 1 on, got {:?}",
            rel[id]
        );
    }
}

/// A scalar flow feeding an arrayed stock: the score is one arrayed
/// variable over the stock's dimensions (the scalar flow broadcast into
/// every element's net flow), never a partial of the stock's initial-value
/// equation.
///
/// `tank[D]` has the scalar inflow `fill = 5 + STEP(5, 2)` and the arrayed
/// outflow `drain[D] = tank[D] * rate[D]` with rates 0.1 and 0.2. Discovery
/// mode scores every causal edge, so no loop is needed. Hand calculation at
/// t = 2 (dt = 1):
///
/// ```text
/// element a (rate 0.1): tank 100, 95, 90.5   drain 10, 9.5, 9.05
///   net = fill - drain:  -4.5 at t = 1,  0.95 at t = 2,  Δnet = 5.45
///   LS(fill  -> tank[a]) = +|5 / 5.45|    = 0.91743...
///   LS(drain -> tank[a]) = -|-0.45 / 5.45| = -0.08257...
/// element b (rate 0.2): tank 100, 85, 73     drain 20, 17, 14.6
///   net = fill - drain:  -12 at t = 1,  -4.6 at t = 2,  Δnet = 7.4
///   LS(fill  -> tank[b]) = +|5 / 7.4|     = 0.67568...
///   LS(drain -> tank[b]) = -|-2.4 / 7.4|  = -0.32432...
/// ```
///
/// and at every step each element's `fill` score is `|Δfill / Δnet[e]|`
/// computed from the run's own `fill` and `drain[e]` series.
#[test]
fn a_scalar_flow_into_an_arrayed_stock_is_scored_per_element() {
    let project = TestProject::new("scalar_flow_arrayed_stock")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("D", &["a", "b"])
        .array_stock("tank[D]", "100", &["fill"], &["drain"], None)
        .flow("fill", "5 + STEP(5, 2)", None)
        .array_flow("drain[D]", "tank[D] * rate[D]", None)
        .array_with_ranges("rate[D]", vec![("a", "0.1"), ("b", "0.2")])
        .build_datamodel();
    let results = ltm_run(&project, true).results;

    let fill_name = link_score("fill", "tank");
    assert!(
        results
            .offsets
            .keys()
            .all(|k| !k.as_str().contains("fill\u{2192}tank[")),
        "the scalar flow's score is one arrayed variable, not per-element scalars"
    );
    let fill = ltm_series(&results, "fill", 0);
    for (slot, (elem, fill_at_2, drain_at_2)) in [
        ("a", 5.0 / 5.45, -0.45 / 5.45),
        ("b", 5.0 / 7.4, -2.4 / 7.4),
    ]
    .into_iter()
    .enumerate()
    {
        let fill_score = ltm_series(&results, &fill_name, slot);
        let drain_score = ltm_series(&results, &link_score("drain", "tank"), slot);
        assert!(
            (fill_score[2] - fill_at_2).abs() < 1e-9,
            "LS(fill -> tank[{elem}]) at t = 2: got {}, expected {fill_at_2}",
            fill_score[2]
        );
        assert!(
            (drain_score[2] - drain_at_2).abs() < 1e-9,
            "LS(drain -> tank[{elem}]) at t = 2: got {}, expected {drain_at_2}",
            drain_score[2]
        );
        // Model variables are keyed per element; LTM variables once, bare.
        let drain = ltm_series(&results, &format!("drain[{elem}]"), 0);
        for t in 1..results.step_count {
            let d_fill = fill[t] - fill[t - 1];
            let d_net = (fill[t] - drain[t]) - (fill[t - 1] - drain[t - 1]);
            let expected = if d_fill == 0.0 || d_net == 0.0 {
                0.0
            } else {
                (d_fill / d_net).abs()
            };
            assert!(
                (fill_score[t] - expected).abs() < 1e-9,
                "LS(fill -> tank[{elem}]) at t = {t}: got {}, expected {expected}",
                fill_score[t]
            );
        }
    }
}

/// A scalar inflow that reads one element of the arrayed stock it feeds:
/// `tank[D]` (a: 50, b: 100), `fill = 2 + 0.05 * tank[a]`,
/// `drain[D] = tank[D] * rate[D]` with rates 0.1 and 0.2, dt 1.
fn scalar_flow_loop_project() -> simlin_engine::datamodel::Project {
    TestProject::new("scalar_flow_loop")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D", &["a", "b"])
        .array_with_ranges("init[D]", vec![("a", "50"), ("b", "100")])
        .array_stock("tank[D]", "init[D]", &["fill"], &["drain"], None)
        .flow("fill", "2 + 0.05 * tank[a]", None)
        .array_flow("drain[D]", "tank[D] * rate[D]", None)
        .array_with_ranges("rate[D]", vec![("a", "0.1"), ("b", "0.2")])
        .build_datamodel()
}

/// The loop through a scalar flow into one element of an arrayed stock, in
/// exhaustive mode, on [`scalar_flow_loop_project`].
///
/// Hand calculation, valid at every step after the first (`Δtank[a] != 0`):
///
/// ```text
/// net[a] = fill - drain[a] = 2 + 0.05 tank[a] - 0.1 tank[a] = 2 - 0.05 tank[a]
/// Δfill = 0.05 Δtank[a],  Δnet[a] = -0.05 Δtank[a]
/// LS(fill -> tank[a])  = +|Δfill / Δnet[a]|      = 1
/// LS(drain -> tank[a]) = -|Δdrain[a] / Δnet[a]|  = -|0.1 / -0.05| = -2
/// LS(tank[a] -> fill)  = +1  (fill is linear in its one moving input)
/// LS(tank[a] -> drain[a]) = +1
/// loop tank[a] -> fill -> tank[a]   = +1 * +1 = +1
/// loop tank[a] -> drain -> tank[a]  = +1 * -2 = -2
/// ```
///
/// and for the b slot, which no loop through `fill` reaches,
/// `LS(fill -> tank[b]) = |Δfill / Δnet[b]|` from the run's own series with
/// `net[b] = fill - drain[b]`.
#[test]
fn a_loop_through_a_scalar_flow_into_an_arrayed_stock_scores_its_slot() {
    let run = ltm_run(&scalar_flow_loop_project(), false);
    let results = &run.results;
    let fill_score = |slot: usize| ltm_series(results, &link_score("fill", "tank"), slot);
    let drain_score = |slot: usize| ltm_series(results, &link_score("drain", "tank"), slot);
    let from_step_one = |series: &[f64], expected: f64, what: &str| {
        assert_eq!(series[0], 0.0, "{what} is guarded to 0 at the start");
        for (t, v) in series.iter().enumerate().skip(1) {
            assert!(
                (v - expected).abs() < 1e-12,
                "{what} at t = {t}: got {v}, expected {expected}"
            );
        }
    };
    from_step_one(&fill_score(0), 1.0, "LS(fill -> tank[a])");
    from_step_one(&drain_score(0), -2.0, "LS(drain -> tank[a])");
    let fill = ltm_series(results, "fill", 0);
    let drain_b = ltm_series(results, "drain[b]", 0);
    let fill_b = fill_score(1);
    for t in 1..results.step_count {
        let d_fill = fill[t] - fill[t - 1];
        let d_net = (fill[t] - drain_b[t]) - (fill[t - 1] - drain_b[t - 1]);
        let expected = if d_fill == 0.0 || d_net == 0.0 {
            0.0
        } else {
            (d_fill / d_net).abs()
        };
        assert!(
            (fill_b[t] - expected).abs() < 1e-12,
            "LS(fill -> tank[b]) at t = {t}: got {}, expected {expected}",
            fill_b[t]
        );
    }

    // The loop through `fill` is one loop, visiting `tank` at `a`; its raw
    // score is the product +1 * +1 at every step after the first.
    let through_fill = run.loop_through("fill");
    let fill_loop = ltm_series(
        results,
        &format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{through_fill}"),
        0,
    );
    from_step_one(&fill_loop, 1.0, "the loop through fill");
    // The drain loop is one loop over D; its `a` slot is +1 * -2.
    let through_drain: Vec<&simlin_engine::db::DetectedLoop> = run
        .loops
        .iter()
        .filter(|l| l.variables.iter().any(|v| v == "drain"))
        .collect();
    assert_eq!(
        through_drain.len(),
        1,
        "one loop through drain (arrayed over D); got {:?}",
        run.loops
            .iter()
            .map(|l| (l.id.clone(), l.variables.clone()))
            .collect::<Vec<_>>()
    );
    let drain_loop_a = ltm_series(
        results,
        &format!(
            "$\u{205A}ltm\u{205A}loop_score\u{205A}{}",
            through_drain[0].id
        ),
        0,
    );
    from_step_one(&drain_loop_a, -2.0, "the loop through drain at a");
}

/// A stock with only an outflow: the net aux is `(0) - (decay)`, so the
/// flow-to-stock score is `-|Δdecay / -Δdecay| = -1` exactly, the aux is the
/// negated flow exactly, and the balancing loop `s -> decay -> s` is
/// `+1 * -1 = -1` exactly (`decay = s * 0.1` is linear in its one input).
#[test]
fn an_outflow_only_stock_scores_minus_one() {
    let project = TestProject::new("outflow_only")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("s", "100", &[], &["decay"], None)
        .flow("decay", "s * 0.1", None)
        .build_datamodel();
    let run = ltm_run(&project, false);
    let results = &run.results;

    let decay_score = ltm_series(results, &link_score("decay", "s"), 0);
    assert_eq!(decay_score, vec![0.0, -1.0, -1.0, -1.0, -1.0, -1.0]);
    let net = ltm_series(results, "$\u{205A}ltm\u{205A}net\u{205A}s", 0);
    let decay = ltm_series(results, "decay", 0);
    assert_eq!(
        net,
        decay.iter().map(|d| -d).collect::<Vec<_>>(),
        "the net-flow aux of an outflow-only stock is the negated outflow"
    );
    let loop_id = run.loop_through("decay");
    let loop_score = ltm_series(
        results,
        &format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}"),
        0,
    );
    assert_eq!(loop_score, vec![0.0, -1.0, -1.0, -1.0, -1.0, -1.0]);
}

/// Replace stock `name`'s initial-value equation in `project` with a
/// per-element one (`Equation::Arrayed` over `dims`).
fn set_stock_initial_per_element(
    project: &mut simlin_engine::datamodel::Project,
    name: &str,
    dims: &[&str],
    elements: &[(&str, &str)],
) {
    let stock = project.models[0]
        .variables
        .iter_mut()
        .find_map(|v| match v {
            Variable::Stock(s) if s.ident == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no stock named {name}"));
    stock.equation = Equation::Arrayed(
        dims.iter().map(|d| d.to_string()).collect(),
        elements
            .iter()
            .map(|(e, eqn)| (e.to_string(), eqn.to_string(), None, None))
            .collect(),
        None,
        false,
    );
}

/// A stock's initial value is the one equation it has, so the reference-site
/// IR classifies a flow the initial reads as that stock's site for the
/// `flow -> stock` edge -- but the edge is the wiring, not that read, and its
/// score has exactly one shape. A per-element initial reading its own flow
/// (`s[a] = f[a] * 5`, `s[b] = f[b] * 7`) must yield ONE arrayed `f -> s`
/// score, not one identical score per element; a pinned-element read in a
/// two-dimensional initial (`s[D,E] = f[a, E] * 5`) must yield one arrayed
/// score too, with no warning, rather than being diverted to the
/// per-element arm. In both, `f = 3 + STEP(2, 2)` is the stock's only flow,
/// so every slot's score is `|Δf / Δf| = 1` at t = 2 and 0 elsewhere.
/// Discovery mode scores the edge without a loop.
#[test]
fn a_per_element_initial_value_reading_the_flow_does_not_split_the_score() {
    let one_dim = {
        let mut project = TestProject::new("per_element_init_1d")
            .with_sim_time(0.0, 4.0, 1.0)
            .named_dimension("D", &["a", "b"])
            .array_stock("s[D]", "0", &["f"], &[], None)
            .array_flow("f[D]", "3 + STEP(2, 2)", None)
            .build_datamodel();
        set_stock_initial_per_element(
            &mut project,
            "s",
            &["D"],
            &[("a", "f[a] * 5"), ("b", "f[b] * 7")],
        );
        (project, 2usize)
    };
    let two_dims = {
        let mut project = TestProject::new("per_element_init_2d")
            .with_sim_time(0.0, 4.0, 1.0)
            .named_dimension("D", &["a", "b"])
            .named_dimension("E", &["x", "y"])
            .array_stock("s[D,E]", "0", &["f"], &[], None)
            .array_flow("f[D,E]", "3 + STEP(2, 2)", None)
            .build_datamodel();
        // Every element's initial reads the flow's `a` row: a pinned element
        // on D, iterated on E.
        let Some(Variable::Stock(stock)) = project.models[0]
            .variables
            .iter_mut()
            .find(|v| matches!(v, Variable::Stock(s) if s.ident == "s"))
        else {
            panic!("no stock named s");
        };
        stock.equation = Equation::ApplyToAll(
            vec!["D".to_string(), "E".to_string()],
            "f[a, E] * 5".to_string(),
        );
        (project, 4usize)
    };
    for (project, slots) in [one_dim, two_dims] {
        let run = ltm_run(&project, true);
        let name = link_score("f", "s");
        let mut scores: Vec<&str> = run
            .results
            .offsets
            .keys()
            .map(|k| k.as_str())
            .filter(|k| k.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}"))
            .collect();
        scores.sort_unstable();
        assert_eq!(
            scores,
            vec![name.as_str()],
            "{}: one arrayed f -> s score and no per-element score",
            project.name
        );
        assert!(
            run.diagnostics.is_empty(),
            "{}: the structural edge raises no LTM warning; got {:?}",
            project.name,
            run.diagnostics
        );
        for slot in 0..slots {
            assert_eq!(
                ltm_series(&run.results, &name, slot),
                vec![0.0, 0.0, 1.0, 0.0, 0.0],
                "{}: slot {slot} of f -> s",
                project.name
            );
        }
    }
}

/// The net-flow aux is LTM machinery, not a causal node: it is a results
/// series, but it appears in no detected loop's node sequence, no causal
/// edge (the graph the FFI's link listing is built over), and no discovered
/// loop's links.
#[test]
fn the_net_flow_aux_is_no_loop_node_and_no_link() {
    let project = scalar_flow_loop_project();
    let net_marker = "\u{205A}net\u{205A}";
    let run = ltm_run(&project, false);
    assert!(
        run.results
            .offsets
            .keys()
            .any(|k| k.as_str() == "$\u{205A}ltm\u{205A}net\u{205A}tank"),
        "the net aux is a results series (so this test is not vacuous)"
    );
    for l in &run.loops {
        assert!(
            l.variables.iter().all(|v| !v.contains(net_marker)),
            "loop {} names the net aux: {:?}",
            l.id,
            l.variables
        );
    }
    assert!(
        run.edges
            .iter()
            .all(|(from, to)| !from.contains(net_marker) && !to.contains(net_marker)),
        "a causal edge names the net aux: {:?}",
        run.edges
    );

    let inputs = ltm_discovery_inputs(&project, "main");
    let found = ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery runs")
    .loops;
    assert!(!found.is_empty(), "discovery finds the loops");
    for fl in &found {
        assert!(
            fl.loop_info
                .links
                .iter()
                .all(|l| !l.from.as_str().contains(net_marker)
                    && !l.to.as_str().contains(net_marker)),
            "discovered loop {} names the net aux: {}",
            fl.loop_info.id,
            fl.loop_info.format_path()
        );
    }
}
