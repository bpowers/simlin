// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an element-scoped helper is to the layers downstream of the parse:
//! one element of its parent's apply-to-all body, and nothing else's.
//!
//! A per-element capture or hoisted argument (`CaptureShape::Element`,
//! `HoistedArg::scope`) is scalar storage with an edge into ONE element of its
//! arrayed parent. The LTM scorers read that off the parent's reference sites,
//! not off the helper's shape: `endpoint_dimensions` answers "scalar" for it,
//! which is also what a genuine scalar source answers, and a scalar source
//! feeds every element. The first test is the asymmetric fixture that tells
//! the two apart -- with equal per-element values a score broadcast into the
//! wrong element is invisible.
//!
//! The rest pin the EXCEPT-default rules of `instantiate_implicit_modules`:
//! an explicit `Ast::Arrayed` slot keeps its own element context, a
//! snapshot-only default is captured once and its capture read only in the
//! slots no explicit element claims, and a module-bearing default is
//! materialized once per missing slot. Every fixture has a plain twin, and the
//! VM is the oracle for both.

use std::collections::BTreeSet;

use crate::ast::print_eqn;
use crate::capture::ImplicitVar;
use crate::db::{
    SimlinDb, model_element_causal_edges, model_ltm_variables, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use crate::test_common::{TestProject, implicit_vars_of};

/// A per-element capture scores into its own target element and no other.
///
/// `growth[Region]`'s body is module-bearing, so it is expanded per element
/// and `PREVIOUS(stock[idx], 0)` is captured once per element as scalar
/// storage (`$⁚growth⁚1⁚arg0⁚north`, `⁚south`). The stocks start at 10 and 40
/// so the two elements' scores differ: a scalar-source emitter that admitted
/// the helper would broadcast each helper into both elements, and the phantom
/// `north -> growth[south]` score would carry south's value. The element graph
/// agrees: the north helper is fed by `stock[north]` alone.
#[test]
fn a_per_element_capture_scores_its_own_element_only() {
    let project = TestProject::new("scoped_capture_scores")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .aux("idx", "1", None)
        .array_with_ranges("init[Region]", vec![("north", "10"), ("south", "40")])
        .array_stock("stock[Region]", "init[Region]", &["growth"], &[], None)
        .array_flow(
            "growth[Region]",
            "SMTH1(stock[Region], 1) * 0.1 + PREVIOUS(stock[idx], 0) * 0.001",
            None,
        );
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let model = sync.models["main"].source_model;

    let helper = "$\u{205A}growth\u{205A}1\u{205A}arg0";
    let prefix = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{helper}");
    let ltm = model_ltm_variables(&db, model, sync.project);
    let helper_scores: BTreeSet<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    let north = format!("{prefix}\u{205A}north\u{2192}growth[north]");
    let south = format!("{prefix}\u{205A}south\u{2192}growth[south]");
    assert_eq!(
        helper_scores,
        [north.as_str(), south.as_str()].into_iter().collect(),
        "one score per element, into the element that reads the helper"
    );

    let edges = model_element_causal_edges(&db, model, sync.project);
    let feeds = |from: &str| -> BTreeSet<&str> {
        edges
            .edges
            .get(from)
            .map(|ts| ts.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    let arg0 = "$\u{205A}growth\u{205A}0\u{205A}arg0";
    assert!(
        feeds("stock[north]").contains(format!("{arg0}\u{205A}north").as_str())
            && !feeds("stock[south]").contains(format!("{arg0}\u{205A}north").as_str()),
        "the north SMTH1 argument reads stock[north] alone; edges from stock[north]: {:?}, \
         from stock[south]: {:?}",
        feeds("stock[north]"),
        feeds("stock[south]")
    );

    // The values, from the run: the north helper's score is north's, the
    // south helper's south's (the base tree's numbers, which the asymmetric
    // stocks keep apart).
    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
    .expect("the LTM-enabled fixture compiles");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    let series = |name: &str| -> Vec<f64> {
        let off = compiled.offsets[&crate::common::Ident::new(name)];
        (0..4)
            .map(|step| results.data[step * results.step_size + off])
            .collect()
    };
    for (name, want) in [
        (&north, [0.0, 1.0, 0.009900990099, 0.009900990099]),
        (&south, [0.0, 1.0, 0.002493765586, 0.002512375314]),
    ] {
        let got = series(name);
        for (step, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            assert!(
                (g - w).abs() < 1e-9,
                "{name} at step {step}: got {g}, want {w}; series {got:?}"
            );
        }
    }
}

/// The elements of `d` are `e1, e2, e3` with `vals` 30, 10, 20.
fn base(name: &str) -> TestProject {
    TestProject::new(name)
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("d", &["e1", "e2", "e3"])
        .array_with_ranges("vals[d]", vec![("e1", "30"), ("e2", "10"), ("e3", "20")])
}

/// A helper as what a consumer reads off it: ident, kind, body and the
/// element it is scoped to.
fn describe(v: &ImplicitVar) -> String {
    let scope = v
        .element_scope()
        .map(|s| {
            let pairs: Vec<String> = s
                .dims
                .iter()
                .zip(&s.element)
                .map(|(d, e)| format!("{}={}", d.as_str(), e.as_str()))
                .collect();
            format!(" in {}", pairs.join(","))
        })
        .unwrap_or_default();
    match v {
        ImplicitVar::Capture(c) => format!(
            "{} = capture[{}] {}{scope}",
            c.ident(),
            c.dims().join(","),
            print_eqn(c.arg())
        ),
        ImplicitVar::HoistedArg(a) => format!("{} = aux {}{scope}", a.ident(), print_eqn(a.arg())),
        ImplicitVar::Module(m) => format!("{} = module {}", m.ident, m.model_name),
    }
}

/// The helpers `out` synthesizes, sorted.
fn helpers_of(tp: &TestProject) -> Vec<String> {
    let dm = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let mut described: Vec<String> = implicit_vars_of(&db, &sync, "main", "out")
        .iter()
        .map(describe)
        .collect();
    described.sort();
    described
}

/// `out[e]` at the last step equals `plain[e]` at the last step, for every
/// element -- the twin is the same shape without the module call or
/// snapshot, so the values the rules produce are the plain equation's.
fn assert_matches_plain_twin(tp: &TestProject) {
    let run = tp.run_vm_expecting_success();
    for element in ["e1", "e2", "e3"] {
        assert_eq!(
            run[&format!("out[{element}]")].last(),
            run[&format!("plain[{element}]")].last(),
            "out[{element}] reads what plain[{element}] reads"
        );
    }
}

/// An explicit `Ast::Arrayed` slot keeps its own element context beside an
/// EXCEPT default: `e2`'s own body is hoisted for `e2`, the default's body
/// for the other elements, each helper scoped to the slot it was minted for.
#[test]
fn an_explicit_slot_keeps_its_element_context_beside_a_module_bearing_default() {
    let tp = base("explicit_slot")
        .array_with_default_and_overrides(
            "out[d]",
            "SMTH1(vals[d] * 2, 1)",
            vec![("e2", "SMTH1(vals[d] * 3, 1)")],
        )
        .array_with_default_and_overrides("plain[d]", "vals[d] * 2", vec![("e2", "vals[d] * 3")]);
    let module = |e: &str| format!("$⁚out⁚0⁚smth1⁚{e} = module stdlib⁚smth1");
    assert_eq!(
        helpers_of(&tp),
        [
            "$⁚out⁚0⁚arg0⁚e1 = aux vals[d] * 2 in d=e1".to_string(),
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d] * 3 in d=e2".to_string(),
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d] * 2 in d=e3".to_string(),
            "$⁚out⁚0⁚arg1⁚e1 = aux 1 in d=e1".to_string(),
            "$⁚out⁚0⁚arg1⁚e2 = aux 1 in d=e2".to_string(),
            "$⁚out⁚0⁚arg1⁚e3 = aux 1 in d=e3".to_string(),
            module("e1"),
            module("e2"),
            module("e3"),
        ]
    );
    assert_matches_plain_twin(&tp);
}

/// A snapshot-only EXCEPT default is captured ONCE, structurally, over the
/// parent's dimensions; the capture is evaluated at every element but read
/// only in the slots no explicit element claims.
#[test]
fn a_snapshot_only_default_is_captured_once_and_read_in_the_missing_slots() {
    let tp = base("snapshot_default")
        .array_with_default_and_overrides(
            "out[d]",
            "PREVIOUS(vals[d] * 2 + 0, 0)",
            vec![("e2", "3")],
        )
        .array_with_default_and_overrides("plain[d]", "vals[d] * 2", vec![("e2", "3")]);
    assert_eq!(
        helpers_of(&tp),
        ["$⁚out⁚0⁚arg0 = capture[d] vals[d] * 2 + 0".to_string()],
        "one structural capture, no per-element one"
    );
    assert_matches_plain_twin(&tp);
}

/// A module-bearing EXCEPT default is materialized once per MISSING slot:
/// the explicit `e2` gets no instance and no helper.
#[test]
fn a_module_bearing_default_is_materialized_per_missing_slot() {
    let tp = base("module_default")
        .array_with_default_and_overrides("out[d]", "SMTH1(vals[d] * 2, 1)", vec![("e2", "3")])
        .array_with_default_and_overrides("plain[d]", "vals[d] * 2", vec![("e2", "3")]);
    let helpers = helpers_of(&tp);
    assert!(
        !helpers.iter().any(|h| h.contains("\u{205A}e2")),
        "the explicit slot gets no helper: {helpers:?}"
    );
    assert_eq!(
        helpers.len(),
        6,
        "an argument, a delay time and an instance for each of e1 and e3: {helpers:?}"
    );
    assert_matches_plain_twin(&tp);
}

/// A hoisted read of a PROPER SUBDIMENSION is scored at the element the
/// helper reads, inside a loop.
///
/// `agg[Sub] = SMTH1(stock[Sub], 1)` with `Sub = {a2, a3}` a subdimension of
/// `Dim = {a1, a2, a3}`: the `a2` helper's fragment reads `stock[a2]` (the
/// element's own name on `stock`'s axis), so the element edge, the link score
/// and the loop through `agg[a2]` all name `a2`. A describer that folded the
/// read to `a2`'s ORDINAL in `Sub` (0, applied to `Dim`) would name
/// `stock[a1]`, score a link the helper does not make (a plausible non-zero
/// number) and report the loop through the wrong element; the stocks 10, 20,
/// 40 keep the three elements apart.
#[test]
fn a_hoisted_read_of_a_proper_subdimension_is_scored_at_its_own_element() {
    let project = TestProject::new("subdim_in_loop")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Dim", &["a1", "a2", "a3"])
        .named_dimension("Sub", &["a2", "a3"])
        .array_with_ranges("init[Dim]", vec![("a1", "10"), ("a2", "20"), ("a3", "40")])
        .array_stock("stock[Dim]", "init[Dim]", &["growth"], &[], None)
        .array_flow(
            "growth[Dim]",
            "stock[Dim] * 0.1 + SUM(agg[*]) * 0.001",
            None,
        )
        .array_aux("agg[Sub]", "SMTH1(stock[Sub], 1)");
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let model = sync.models["main"].source_model;

    // The helper's fragment reads stock[a2]: 20 at t0, stock[a2]'s series after.
    let run = project.run_vm_expecting_success();
    assert_eq!(run["$⁚agg⁚0⁚arg0⁚a2"], run["stock[a2]"]);
    assert_eq!(run["$⁚agg⁚0⁚arg0⁚a3"], run["stock[a3]"]);

    let edges = model_element_causal_edges(&db, model, sync.project);
    let feeds = |from: &str| -> BTreeSet<&str> {
        edges
            .edges
            .get(from)
            .map(|ts| ts.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    let helper_a2 = "$\u{205A}agg\u{205A}0\u{205A}arg0\u{205A}a2";
    assert!(
        feeds("stock[a2]").contains(helper_a2) && !feeds("stock[a1]").contains(helper_a2),
        "the a2 helper is fed by stock[a2] alone; from stock[a1]: {:?}, from stock[a2]: {:?}",
        feeds("stock[a1]"),
        feeds("stock[a2]")
    );

    let ltm = model_ltm_variables(&db, model, sync.project);
    let own = format!("$\u{205A}ltm\u{205A}link_score\u{205A}stock[a2]\u{2192}{helper_a2}");
    let wrong = format!("$\u{205A}ltm\u{205A}link_score\u{205A}stock[a1]\u{2192}{helper_a2}");
    let names: BTreeSet<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
    assert!(
        names.contains(own.as_str()) && !names.contains(wrong.as_str()),
        "the score names the read element; emitted: {names:?}"
    );
    let through_a2 = ltm.vars.iter().any(|v| {
        v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
            && v.equation.source_text().contains(&format!("\"{own}\""))
    });
    assert!(through_a2, "a loop score composes the a2 link: {names:?}");

    // The score itself: the helper is the read, so the link scores 1 once the
    // initial-step guard clears.
    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
    .expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    let off = compiled.offsets[&crate::common::Ident::new(&own)];
    let series: Vec<f64> = (0..4)
        .map(|step| results.data[step * results.step_size + off])
        .collect();
    assert_eq!(series[0], 0.0);
    assert!(
        series[1..].iter().all(|v| (v - 1.0).abs() < 1e-9),
        "stock[a2] -> {helper_a2} scores 1: {series:?}"
    );
}

/// A snapshot nested in a snapshot under an apply-to-all body: both capture
/// structurally, the outer reading the inner's capture, and the value is the
/// plain spelling's from the first step on.
#[test]
fn a_nested_snapshot_under_an_apply_to_all_body_captures_structurally() {
    let tp = base("nested_snapshot")
        .array_aux("out[d]", "PREVIOUS(INIT(vals[d] * 2) + 1, 0)")
        .array_aux("plain[d]", "INIT(vals[d] * 2) + 1");
    let helpers = helpers_of(&tp);
    assert_eq!(helpers.len(), 2, "{helpers:?}");
    assert!(
        helpers
            .iter()
            .all(|h| h.contains("= capture[d] ") && !h.contains(" in ")),
        "both captures are structural, over d: {helpers:?}"
    );
    assert_matches_plain_twin(&tp);
}
