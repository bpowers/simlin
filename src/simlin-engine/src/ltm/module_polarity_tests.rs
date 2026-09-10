// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static polarity of an `input -> module` edge: the composition of the
//! sub-model's own link polarities along every pathway from the entry port
//! `from` feeds to the output port(s) the parent reads.  One test per arm of
//! the rule, each through the real pipeline (`sync_from_datamodel` ->
//! `model_detected_loops`) so the loop id and polarity a user sees are what
//! is pinned; the edge-level tests read the same `get_link_polarity` the loop
//! builders call.
//!
//! Arms: every pathway Negative (DELAY3's delay-time port); every pathway
//! Positive (SMTH1's input port); Negative by the Div convention (SMTH1's
//! delay-time port); pathways of one port disagreeing; two read output ports
//! disagreeing; an entry port with no pathway to a read output; a truncated
//! pathway enumeration; one source feeding two agreeing entry ports; an
//! Unknown link inside a pathway; a module the parent reads nothing from (the
//! sub-model's own output convention decides); and the `module -> variable`
//! edge, unchanged, in `test_module_polarity_through_output_ref`.

use super::*;
use crate::db::DetectedLoopsResult;
use crate::ltm::ModulePathwayBudgetGuard;
use crate::test_common::TestProject;
use crate::testutils::x_module;

/// The detected loops of `project`'s `main` model.
fn detected(project: &crate::datamodel::Project) -> DetectedLoopsResult {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    model_detected_loops(&db, sync.models["main"].source, sync.project).clone()
}

/// `(id, polarity)` of the single detected loop of `project`'s `main` model.
fn the_loop(project: &crate::datamodel::Project) -> (String, DetectedLoopPolarity) {
    let detected = detected(project);
    assert_eq!(
        detected.loops.len(),
        1,
        "expected exactly one loop, got {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.variables))
            .collect::<Vec<_>>()
    );
    (detected.loops[0].id.clone(), detected.loops[0].polarity)
}

/// The static polarity of the parent-model edge `from -> to` as the loop
/// builders read it.
fn edge_polarity(project: &crate::datamodel::Project, from: &str, to: &str) -> LinkPolarity {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    let graph = causal_graph_with_modules(&db, sync.models["main"].source, sync.project);
    graph.get_link_polarity(&Ident::new(from), &Ident::new(to))
}

/// A stock `s` whose inflow `f` is a stdlib module call closed through
/// `tau = 2 + 0.02 * s`: `s -> tau -> module -> f -> s`.
fn stdlib_delay_port_project(flow_eqn: &str) -> crate::datamodel::Project {
    TestProject::new("delay_port")
        .with_sim_time(0.0, 30.0, 0.5)
        .stock("s", "50", &["f"], &[], None)
        .aux("tau", "2 + 0.02 * s", None)
        .aux("inp", "10", None)
        .flow("f", flow_eqn, None)
        .build_datamodel()
}

/// A parent model `s -> x -> m -> f -> s` around the user sub-model `m`
/// (`x_module` instantiates the model of the same name) whose read output is
/// `f_eqn`'s reference.
fn user_module_project(
    sub_model: crate::datamodel::Model,
    refs: &[(&str, &str)],
    f_eqn: &str,
) -> crate::datamodel::Project {
    let main = x_model(
        "main",
        vec![
            x_stock("s", "100", &["f"], &[], None),
            x_aux("x", "0.1 * s", None),
            x_module("m", refs, None),
            x_flow("f", f_eqn, None),
        ],
    );
    x_project(sim_specs_with_units("years"), &[main, sub_model])
}

#[test]
fn delay_time_port_of_delay3_makes_the_loop_balancing() {
    // Inside delay3 every pathway from `delay_time` to `output` is Negative:
    // the direct `output = stock_3/(delay_time/3)`, and the ones through
    // `flow_2` and `flow_1` into the stock chain (each `stock_n/(delay_time/3)`
    // flow is Negative in the delay time, the stock hops Positive).  The
    // edge is Negative, the loop has one negative link, and its id says so;
    // its runtime score is negative at every step.
    let project = stdlib_delay_port_project("DELAY3(inp, tau)");
    let module = "$\u{205A}f\u{205A}0\u{205A}delay3";
    assert_eq!(
        edge_polarity(&project, "tau", module),
        LinkPolarity::Negative
    );
    assert_eq!(
        the_loop(&project),
        ("b1".to_string(), DetectedLoopPolarity::Balancing)
    );
}

#[test]
fn input_port_of_smth1_keeps_the_loop_reinforcing() {
    // `s -> SMTH1 at input (through the hoisted `0.1 * s` helper) -> f -> s`:
    // the one pathway `input -> flow -> output` is Positive.
    let project = TestProject::new("smth_input")
        .with_sim_time(0.0, 30.0, 0.5)
        .stock("s", "50", &["f"], &[], None)
        .flow("f", "SMTH1(0.1 * s, 4)", None)
        .build_datamodel();
    let module = "$\u{205A}f\u{205A}0\u{205A}smth1";
    let helper = "$\u{205A}f\u{205A}0\u{205A}arg0";
    assert_eq!(
        edge_polarity(&project, helper, module),
        LinkPolarity::Positive
    );
    assert_eq!(
        the_loop(&project),
        ("r1".to_string(), DetectedLoopPolarity::Reinforcing)
    );
}

#[test]
fn delay_time_port_of_smth1_follows_the_division_convention() {
    // `s -> tau -> SMTH1 at delay_time -> f -> s`.  The one pathway is
    // `delay_time -> flow -> output` with `flow = (input - Output)/delay_time`:
    // the analyzer's Div arm labels a non-constant divisor Negative under its
    // positive-numerator convention (`(a - b)/y` flips, the same reading
    // `gap / adjustment_time` gets), so the loop is reported balancing.  The
    // true sign depends on the sign of the gap `input - Output`; the runtime
    // series settles Rux/Bux/U, while the static label follows the convention
    // every other `x / y` link in the model follows.
    let project = stdlib_delay_port_project("SMTH1(inp, tau)");
    let module = "$\u{205A}f\u{205A}0\u{205A}smth1";
    assert_eq!(
        edge_polarity(&project, "tau", module),
        LinkPolarity::Negative
    );
    assert_eq!(
        the_loop(&project),
        ("b1".to_string(), DetectedLoopPolarity::Balancing)
    );
}

#[test]
fn pathways_of_one_port_that_disagree_make_the_edge_unknown() {
    // `input -> pos -> output` is Positive and `input -> neg -> output` is
    // Negative: the port's sign is not a sign.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("pos", "input", None),
            x_aux("neg", "0 - input", None),
            x_aux("output", "pos + neg", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
    assert_eq!(
        the_loop(&project),
        ("u1".to_string(), DetectedLoopPolarity::Undetermined)
    );
}

#[test]
fn two_read_output_ports_that_disagree_make_the_edge_unknown() {
    // The parent reads both `out_pos = input` and `out_neg = 0 - input`; the
    // edge's sign is a property of the edge, not of the port one loop exits
    // by, so two read ports of opposite sign leave it Unknown.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("out_pos", "input", None),
            x_aux("out_neg", "0 - input", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.out_pos + m.out_neg");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
    assert_eq!(
        the_loop(&project),
        ("u1".to_string(), DetectedLoopPolarity::Undetermined)
    );
}

#[test]
fn an_entry_port_with_no_pathway_to_a_read_output_is_unknown() {
    // `output` does not depend on `input` at all (only `sink` reads it).  The
    // causal graph still carries `x -> m -> f` (the parent feeds the port and
    // reads the output), so the loop is enumerated; its sign cannot be
    // composed from anything.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("sink", "input", None),
            x_aux("output", "5", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
    assert_eq!(
        the_loop(&project),
        ("u1".to_string(), DetectedLoopPolarity::Undetermined)
    );
}

#[test]
fn a_truncated_pathway_enumeration_makes_the_edge_unknown() {
    // A diamond (`input -> a`, `input -> b`, `output = a + b`) has two
    // Positive pathways.  Under a budget of one the enumeration keeps one
    // pathway and reports truncation; a dropped pathway could disagree, so
    // the edge is Unknown -- and with the full enumeration it is Positive.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("a", "input * 2", None),
            x_aux("b", "input * 3", None),
            x_aux("output", "a + b", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Positive);
    let _guard = ModulePathwayBudgetGuard::new(1);
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
}

#[test]
fn a_source_feeding_two_entry_ports_with_agreeing_pathways_keeps_the_sign() {
    // `x` is wired to both `in1` and `in2`; `output = in1 + in2` reaches the
    // output from each port with the same sign.
    let sub = x_model(
        "m",
        vec![
            x_aux("in1", "0", None),
            x_aux("in2", "0", None),
            x_aux("output", "in1 + in2", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.in1"), ("x", "m.in2")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Positive);
    assert_eq!(
        the_loop(&project),
        ("r1".to_string(), DetectedLoopPolarity::Reinforcing)
    );
}

#[test]
fn an_unknown_link_inside_the_pathway_makes_the_edge_unknown() {
    // `output = ABS(input)` is non-monotone, so the only pathway carries an
    // Unknown link.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("output", "ABS(input)", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
    assert_eq!(
        the_loop(&project),
        ("u1".to_string(), DetectedLoopPolarity::Undetermined)
    );
}

#[test]
fn a_module_the_parent_reads_nothing_from_uses_its_own_output_convention() {
    // Nothing in the parent reads `m`, so no loop runs through it; the
    // `x -> m` edge still has a sign, composed to the sub-model's own output
    // convention (its sinks): `output = 0 - input` is Negative.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("output", "0 - input", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("x", "3", None),
            x_module("m", &[("x", "m.input")], None),
        ],
    );
    let project = x_project(sim_specs_with_units("years"), &[main, sub]);
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Negative);
    assert!(detected(&project).loops.is_empty());
}

#[test]
fn a_nested_instance_composes_recursively() {
    // A user module wrapping a stdlib smooth: `m.output = SMTH1(m.input, 3)`.
    // The pathway `input -> $smth1 -> output` inside `m` crosses a nested
    // instance, whose hop is signed by the same rule one level down (the
    // smooth's `input -> flow -> output` is Positive), so the parent's
    // `x -> m` edge is Positive and the loop keeps its reinforcing id.
    let sub = x_model(
        "m",
        vec![
            x_aux("input", "0", None),
            x_aux("output", "SMTH1(input, 3)", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.input")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Positive);
    assert_eq!(
        the_loop(&project),
        ("r1".to_string(), DetectedLoopPolarity::Reinforcing)
    );
}

#[test]
fn a_source_feeding_two_entry_ports_with_disagreeing_pathways_is_unknown() {
    // `x` is wired to both `in1` and `in2`; `output = in1 - in2` reaches the
    // output Positive from one port and Negative from the other, so the
    // edge has no sign -- the second port is consulted, not just the first.
    let sub = x_model(
        "m",
        vec![
            x_aux("in1", "0", None),
            x_aux("in2", "0", None),
            x_aux("output", "in1 - in2", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.in1"), ("x", "m.in2")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Unknown);
    assert_eq!(
        the_loop(&project),
        ("u1".to_string(), DetectedLoopPolarity::Undetermined)
    );
}

#[test]
fn a_fed_port_that_reaches_no_read_output_is_ignored() {
    // `x` is wired to both `in1` and `in2`, but only `in1` reaches the read
    // output (`in2` feeds a dead-end `sink`).  A port that reaches no read
    // output cannot carry the loop, so it does not veto the sign the other
    // port gives.
    let sub = x_model(
        "m",
        vec![
            x_aux("in1", "0", None),
            x_aux("in2", "0", None),
            x_aux("sink", "in2", None),
            x_aux("output", "in1 * 2", None),
        ],
    );
    let project = user_module_project(sub, &[("x", "m.in1"), ("x", "m.in2")], "m.output");
    assert_eq!(edge_polarity(&project, "x", "m"), LinkPolarity::Positive);
    assert_eq!(
        the_loop(&project),
        ("r1".to_string(), DetectedLoopPolarity::Reinforcing)
    );
}
