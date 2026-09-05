// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Declining per-element partials of rank-like targets (GH #995 option C).
//!
//! A target whose equation applies an ORDER-STATISTIC, array-producing
//! builtin -- `VECTOR SORT ORDER`, `RANK`, `ALLOCATE AVAILABLE`,
//! `ALLOCATE BY PRIORITY` -- ranks a WHOLE array. A per-element SCALAR
//! link-score partial of such a target pins the builtin's argument down to
//! one element, and an order statistic of a single element is meaningless
//! (`vm_vector_sort_order` on a 1-element view yields rank 0 always). Today
//! those fragments fail codegen loudly (an array in a position that consumes
//! one value -- 21 VECTOR SORT ORDER fragments on C-LEARN, plus 84 VECTOR ELM
//! MAP ones on the sibling reason below); any change that made them compile
//! (e.g. widening the materializer, GH #995 option A) would convert the loud
//! drop into a silent constant-0 partial. Option C
//! therefore declines the edge at generation, with a warning naming the
//! shape, BEFORE any such widening can land.
//!
//! `VECTOR ELM MAP` joins the decline set for a DIFFERENT reason: pinning
//! its arg-1 element is semantically fine (required, even -- the
//! `vm_vector_elm_map` base semantics), but its RESULT is an array, which a
//! per-element scalar partial cannot hold either -- the same
//! "outside AssignTemp context" codegen rejection, without the semantic
//! trap. Declining both keeps the whole AssignTemp-required family out of
//! scalar partials.
//!
//! What is deliberately NOT declined:
//! * the A2A/whole-array-shaped score of the same target (`effective_target_
//!   year -> target_order` on C-LEARN): its arms keep the dimension-name
//!   spelling, the A2A hoisting evaluates the builtin over the full array,
//!   and the slot read selects this element's rank -- compilable and
//!   semantically right;
//! * `VECTOR SELECT`: its selection reduces to a scalar, so per-element
//!   pinning of the non-reduced axes is exactly right (the array-freeze
//!   tests are the compiling control);
//! * a rank-like call HOISTED into a synthetic agg: the check runs after
//!   reducer substitution, so the agg reference (which carries the
//!   correctly-slotted whole-array rank, GH #742/#771) is untouched --
//!   pinned by the pre-existing `ltm_array_agg` RANK integration tests.

use crate::db::{
    DiagnosticError, SimlinDb, collect_all_diagnostics, model_ltm_variables,
    sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

const LINK_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";

/// A scalar source feeding a VECTOR SORT ORDER target: the per-element
/// partials would rank one element each. `asc` depends on `s` so the edge is
/// loop-carrying and exhaustive mode emits (or here, declines) its scores.
fn rank_target_fixture() -> TestProject {
    TestProject::new("rank_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("asc", "1 + s - s", None)
        .array_aux("order[C]", "VECTOR SORT ORDER(vals[*], asc)")
        .flow("g", "(order[c1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None)
}

fn fragment_failures(db: &SimlinDb, project: crate::db::SourceProject) -> Vec<String> {
    collect_all_diagnostics(db, project, crate::db::LtmOverlay::Off)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("failed to compile") => {
                Some(format!("{:?}: {msg}", d.variable))
            }
            _ => None,
        })
        .collect()
}

/// The per-element scores for `asc -> order` are DECLINED (not emitted, not
/// left to fail codegen), with a warning naming the rank-like shape.
#[test]
fn per_element_partial_of_rank_like_target_declines_loudly() {
    let datamodel = rank_target_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("asc\u{2192}order"))
        .collect();
    assert!(
        emitted.is_empty(),
        "per-element partials of a rank-like target must be declined, not \
         emitted; got: {emitted:?}"
    );

    let rank_warnings: Vec<String> =
        collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::On)
            .iter()
            .filter_map(|d| match &d.error {
                DiagnosticError::Assembly(msg)
                    if msg.contains("order statistic") || msg.contains("ranks a whole array") =>
                {
                    Some(msg.clone())
                }
                _ => None,
            })
            .collect();
    assert!(
        !rank_warnings.is_empty(),
        "the decline must be loud, naming the rank-like shape; diagnostics: {:?}",
        collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::On)
            .iter()
            .map(|d| format!("{:?}", d.error))
            .collect::<Vec<_>>()
    );

    // The decline replaces what used to be attributed codegen FAILURES: with
    // the scores declined, nothing on this fixture should fail to compile.
    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "declining must leave no failing fragments; failures:\n{}",
        failures.join("\n")
    );
}

/// Loop scores traversing the declined edge are DROPPED (the GH #758/#780
/// unscoreable-edge contract), never stubbed to a warned constant 0.
#[test]
fn loops_through_the_declined_rank_edge_are_dropped() {
    let datamodel = rank_target_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // The only feedback loop runs s -> asc -> order -> g -> s (and
    // s -> vals -> order -> g -> s); the asc hop is declined, so any loop
    // score that references the never-emitted asc->order name would be a
    // stub. Assert no emitted loop score references the declined edge's
    // link-score name.
    let declined = format!("{LINK_PREFIX}asc\u{2192}order");
    for v in &ltm.vars {
        if v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}") {
            let text = v.equation.source_text();
            assert!(
                !text.contains(&declined),
                "loop score {} references the declined edge (a constant-0 stub): {text}",
                v.name
            );
        }
    }
}

/// The whole-array (A2A-shaped) score of the SAME rank-like target keeps
/// compiling: `vals -> order` holds the builtin's argument live at full
/// array width, which the A2A hoisting evaluates correctly.
#[test]
fn whole_array_score_of_the_rank_target_still_compiles() {
    let datamodel = rank_target_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let score = format!("{LINK_PREFIX}vals\u{2192}order");
    assert!(
        ltm.vars.iter().any(|v| v.name == score),
        "the whole-array vals->order score must still be emitted; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "the A2A-shaped score must compile; failures:\n{}",
        failures.join("\n")
    );
}

/// `VECTOR ELM MAP` declines on the per-element scalar paths too -- not for
/// the order-statistic trap (its arg-1 pin is required semantics) but
/// because its array RESULT cannot live in a scalar fragment; these used to
/// be emitted and fail codegen ("outside AssignTemp context", the C-LEARN
/// "84" residue).
#[test]
fn per_element_partial_of_elm_map_target_declines_loudly() {
    let project = TestProject::new("elm_map_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("off", "1 + s - s", None)
        .array_aux("mapped[C]", "VECTOR ELM MAP(vals[c1], off)")
        .flow("g", "(mapped[c1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("off\u{2192}mapped["))
        .collect();
    assert!(
        emitted.is_empty(),
        "per-element partials of an elm-map target must be declined; got: {emitted:?}"
    );
    // Scoped to the declined edge: the sibling A2A-shaped `vals[c1] -> mapped`
    // score still fails on a DIFFERENT, pre-existing class (a frozen
    // `PREVIOUS(off)` lands in ELM MAP's view-position argument -- the same
    // shape as C-LEARN's residual `target_order -> sorted_target_*` four),
    // which this decline deliberately does not touch.
    let failures: Vec<String> = fragment_failures(&db, sync.project)
        .into_iter()
        .filter(|f| f.contains("off\u{2192}mapped"))
        .collect();
    assert!(
        failures.is_empty(),
        "declining must leave no failing off->mapped fragments; failures:\n{}",
        failures.join("\n")
    );
}

/// The predicate RECURSES: a rank-like call nested inside other arithmetic
/// (`MIN(VECTOR SORT ORDER(...), 5)`) declines too. Deleting the recursion
/// (checking only the top-level App) survived every prior test while
/// regressing this shape to emitted-and-failing fragments.
#[test]
fn nested_rank_like_call_declines_too() {
    let project = TestProject::new("nested_rank_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("asc", "1 + s - s", None)
        .array_aux("order[C]", "MIN(VECTOR SORT ORDER(vals[*], asc), 5)")
        .flow("g", "(order[c1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("asc\u{2192}order["))
        .collect();
    assert!(
        emitted.is_empty(),
        "a NESTED rank-like call must decline the per-element partials; got: {emitted:?}"
    );
    let failures: Vec<String> = fragment_failures(&db, sync.project)
        .into_iter()
        .filter(|f| f.contains("asc\u{2192}order"))
        .collect();
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

/// The ALLOCATE arms of the decline set are live: a market-clearing
/// allocation is an order statistic over the whole request array, so its
/// per-element partials decline.
#[test]
fn allocate_target_per_element_partials_decline() {
    let project = TestProject::new("allocate_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D", &["d1", "d2"])
        .array_with_ranges("request[D]", vec![("d1", "s"), ("d2", "s * 2")])
        .array_aux("pp[D,2]", "1")
        .aux("supply", "s * 0.5", None)
        .array_aux(
            "result[D]",
            "ALLOCATE AVAILABLE(request[*], pp[*,1], supply)",
        )
        .flow("g", "(result[d1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("supply\u{2192}result["))
        .collect();
    assert!(
        emitted.is_empty(),
        "ALLOCATE per-element partials must decline; got: {emitted:?}"
    );
    let failures: Vec<String> = fragment_failures(&db, sync.project)
        .into_iter()
        .filter(|f| f.contains("supply\u{2192}result"))
        .collect();
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

/// The RANK arm matters most: before the decline, a RANK-bearing target's
/// per-element scores COMPILED while the `PREVIOUS(RANK(...))` capture
/// helpers under them failed -- a present-and-wrong score over a constant-0
/// helper, the silent-degradation class. The decline replaces that with a
/// loud drop.
#[test]
fn rank_target_per_element_partials_decline() {
    let project = TestProject::new("rank_builtin_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("k", "s * 0.1", None)
        .array_aux("ranked[C]", "RANK(vals, 1) * k")
        .flow("g", "(ranked[c1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("k\u{2192}ranked["))
        .collect();
    assert!(
        emitted.is_empty(),
        "RANK per-element partials must decline; got: {emitted:?}"
    );
    let failures: Vec<String> = fragment_failures(&db, sync.project)
        .into_iter()
        .filter(|f| f.contains("k\u{2192}ranked"))
        .collect();
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

/// The `generate_per_element_link_equation` guard site, pinned directly (it
/// was previously covered only by the C-LEARN var-count pin): a mixed
/// Iterated+Pinned (`PerElement`-shaped) source read inside a rank-bearing
/// target routes through the per-(row, element) emitter, whose scalar
/// partials must decline identically.
#[test]
fn per_element_shaped_site_in_rank_target_declines() {
    let project = TestProject::new("per_element_rank_decline")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("R", &["r1", "r2"])
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("w[R]", vec![("r1", "s"), ("r2", "s * 2")])
        .array_aux("pop[R,C]", "s * 2")
        // `pop[R, c1]` is an Iterated+Pinned (PerElement) site; the equation
        // also ranks w, so every scalar partial of `out` must decline.
        .array_aux("out[R]", "VECTOR SORT ORDER(w[*], 1) + pop[R, c1]")
        .flow("g", "(out[r1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.contains("pop[") && n.contains("\u{2192}out["))
        .collect();
    assert!(
        emitted.is_empty(),
        "PerElement-shaped partials of a rank-bearing target must decline; got: {emitted:?}"
    );
    let failures: Vec<String> = fragment_failures(&db, sync.project)
        .into_iter()
        .filter(|f| f.contains("\u{2192}out"))
        .collect();
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}
