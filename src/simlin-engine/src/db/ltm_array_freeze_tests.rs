// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The LTM array-freeze helper (GH #995 option B, wrap-side).
//!
//! A ceteris-paribus partial that must freeze an ARRAY SLICE -- an other-dep
//! (or, changed-last, the live source) referenced as `arr[pin, *]` or
//! `arr[pin, *:Sub]` inside a vector builtin -- cannot spell the freeze
//! inline: `PREVIOUS(<slice>)` has no codegen path (an array-valued operand
//! must be a view over storage), so the wrap used to either decline the score
//! loudly (`UnfreezablePartial`, the changed-first/changed-last doom) or emit
//! a fragment that failed to compile (the per-target-element path, which
//! never doom-checked).
//!
//! The fix materializes the freeze as its own synthetic variable: an
//! `Equation::Arrayed` aux `$⁚ltm⁚freeze⁚…` with one arm per slice row, each
//! arm `PREVIOUS(arr[pin, axis·elem])` -- a static subscript that compiles to
//! `LoadPrev` today -- and the partial references the helper as a whole array
//! (a variable, hence a view over storage by construction). Because the arm
//! subscripts are qualified with the AXIS dimension's name, the positional
//! `dim·element` resolution (PR #1001) reads the name-correct row for ANY
//! subdimension alignment; the parse-time alternative (qualifying with the
//! subrange's own name) reads a wrong row whenever a named subdimension is
//! not a positional prefix of its parent, which is why the helper is
//! synthesized in the WRAP (which knows each dep's declared dims via
//! `dep_dims`) and not in `builtins_visitor` (which does not -- the blocker
//! PR #1001 recorded).
//!
//! These tests pin: the decline paths now score (and what the helper looks
//! like), the name-correct row rule on a NON-prefix subdimension (the arm a
//! positional-within-subrange spelling would get wrong), the per-target-
//! element emitter path (the C-LEARN "139" shape, which used to emit failing
//! fragments), evaluation order (a freeze helper must run before the link
//! scores that read its current-step value), and that a slice nobody can
//! materialize (a dynamic pin) still declines loudly.

use std::collections::HashSet;

use crate::db::{
    DiagnosticError, SimlinDb, collect_all_diagnostics, model_ltm_variables,
    set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

const FREEZE_PREFIX: &str = "$\u{205A}ltm\u{205A}freeze\u{205A}";
const LINK_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";

/// Fixture for the arrayed->scalar decline path (the C-LEARN "14" shape):
/// a scalar aux whose equation VECTOR-SELECTs over `*:SubT` slices of two
/// arrayed deps, closed into a feedback loop so the edges are loop-carrying.
///
/// `sel = VECTOR SELECT(active[*:SubT], year[*:SubT], 0, max, none)`:
/// the `year -> sel` partial freezes the `active` slice and the
/// `active -> sel` partial freezes the `year` slice; both used to be
/// `UnfreezablePartial` declines.
fn select_slice_fixture() -> TestProject {
    TestProject::new("select_slice")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("Target", &["t1", "t2", "t3"])
        .named_dimension("SubT", &["t1", "t2"])
        .array_with_ranges(
            "year[Target]",
            vec![("t1", "s"), ("t2", "s * 2"), ("t3", "s * 3")],
        )
        .array_aux("active[Target]", "1")
        // action 3 == max; sel = max(year[t1], year[t2]) = 2 * s
        .aux(
            "sel",
            "VECTOR SELECT(active[*:SubT], year[*:SubT], 0, 3, 0)",
            None,
        )
        .flow("g", "sel * 0.1", None)
        .stock("s", "10", &["g"], &[], None)
}

/// Every LTM fragment-compile failure message in the project's diagnostics.
fn fragment_failures(db: &SimlinDb, project: crate::db::SourceProject) -> Vec<String> {
    collect_all_diagnostics(db, project)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("failed to compile") => {
                Some(format!("{:?}: {msg}", d.variable))
            }
            _ => None,
        })
        .collect()
}

/// Every `UnfreezablePartial`-style decline warning (the "would freeze an
/// array slice inside PREVIOUS()" wording).
fn unfreezable_declines(db: &SimlinDb, project: crate::db::SourceProject) -> Vec<String> {
    collect_all_diagnostics(db, project)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("freeze an array slice") => {
                Some(msg.clone())
            }
            _ => None,
        })
        .collect()
}

/// The arrayed->scalar slice edges score instead of declining, via a freeze
/// helper, and every emitted fragment compiles.
#[test]
fn unfreezable_select_slice_edges_now_score() {
    let datamodel = select_slice_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let names: Vec<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
    // Exhaustive mode scores LOOP edges; `year -> sel` is on the fixture's
    // loop and used to be the UnfreezablePartial decline. (`active -> sel`
    // is feed-forward -- a constant source -- so exhaustive mode emits no
    // score for it regardless; the discovery-mode behavior of that edge is
    // covered by the C-LEARN harness, not here.)
    let score = format!("{LINK_PREFIX}year\u{2192}sel");
    assert!(
        names.contains(&score.as_str()),
        "expected link score {score} to be emitted (not declined); vars: {names:#?}"
    );

    let helpers: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| n.starts_with(FREEZE_PREFIX))
        .collect();
    assert!(
        !helpers.is_empty(),
        "expected at least one $⁚ltm⁚freeze⁚ helper var; vars: {names:#?}"
    );

    // The year->sel partial freezes the `active` slice: its helper is arrayed
    // over SubT with one arm per subrange element, each a static PREVIOUS of
    // the AXIS-qualified row.
    let active_helper = ltm
        .vars
        .iter()
        .find(|v| v.name.starts_with(FREEZE_PREFIX) && v.name.contains("active"))
        .expect("a freeze helper over the active slice");
    assert_eq!(
        active_helper.dimensions,
        vec!["subt".to_string()],
        "the helper's axis is the subrange the slice reads"
    );
    let text = active_helper.equation.source_text().to_lowercase();
    for elem in ["t1", "t2"] {
        assert!(
            text.contains(&format!("active[target\u{B7}{elem}]")),
            "helper arm must read the AXIS-qualified row target·{elem}; got:\n{text}"
        );
    }

    assert_eq!(
        unfreezable_declines(&db, sync.project),
        Vec::<String>::new(),
        "no slice-freeze decline should remain on this fixture"
    );
    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "every fragment (scores + freeze helpers) must compile; failures:\n{}",
        failures.join("\n")
    );
}

/// End to end: the loop through `year -> sel -> g -> s -> year` is scored
/// and its link score is not identically zero.
#[test]
fn select_slice_loop_scores_nonzero() {
    let datamodel = select_slice_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    let score = format!("{LINK_PREFIX}year\u{2192}sel");
    let offset = *compiled
        .offsets
        .get(score.as_str())
        .unwrap_or_else(|| panic!("{score} has no results offset"));
    let series: Vec<f64> = (0..results.step_count)
        .map(|step| results.data[step * results.step_size + offset])
        .collect();
    assert!(
        series.iter().all(|v| v.is_finite()),
        "{score} must be finite everywhere: {series:?}"
    );
    assert!(
        series.iter().any(|v| *v != 0.0),
        "{score} is identically zero across the run, which is what a dropped \
         or stubbed fragment looks like: {series:?}"
    );
}

/// THE name-correct-row pin: a NON-prefix named subdimension, where
/// position-within-subrange and position-within-axis disagree.
///
/// `SubX = [t3, t1]` inside `Target = [t1, t2, t3]`. The `*:SubX` slice reads
/// axis rows 2 then 0 (name containment, SubX declared order). A helper arm
/// spelled `year[subx·t3]` would resolve POSITIONALLY within SubX -- position
/// 1 -> axis row 0 == t1: the WRONG row. The fix qualifies with the AXIS
/// dimension (`year[target·t3]` -> axis row 2), and the VM assertion below is
/// the oracle: helper slot k must equal the previous step's `year` at SubX's
/// k-th element.
#[test]
fn freeze_helper_reads_name_correct_rows_for_scattered_subdim() {
    // `active` varies per element AND per step (while staying truthy), so
    // the helper-vs-previous-value oracle below cannot pass by accident.
    let project = TestProject::new("scattered_subdim")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Target", &["t1", "t2", "t3"])
        .named_dimension("SubX", &["t3", "t1"])
        .array_with_ranges(
            "year[Target]",
            vec![("t1", "s + 1"), ("t2", "s + 2"), ("t3", "s + 3")],
        )
        .array_with_ranges(
            "active[Target]",
            vec![
                ("t1", "1 + s / 1000"),
                ("t2", "2 + s / 1000"),
                ("t3", "3 + s / 1000"),
            ],
        )
        .aux(
            "sel",
            "VECTOR SELECT(active[*:SubX], year[*:SubX], 0, 3, 0)",
            None,
        )
        .flow("g", "sel * 0.1", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // The loop edge is `year -> sel`, whose changed-first partial freezes
    // the OTHER slice -- `active[*:SubX]` -- into a helper.
    let active_helper = ltm
        .vars
        .iter()
        .find(|v| v.name.starts_with(FREEZE_PREFIX) && v.name.contains("active"))
        .expect("a freeze helper over the active slice");
    let text = active_helper.equation.source_text().to_lowercase();
    for elem in ["t3", "t1"] {
        assert!(
            text.contains(&format!("active[target\u{B7}{elem}]")),
            "arm must qualify against the AXIS dimension (target·{elem}), not the \
             subrange; got:\n{text}"
        );
    }

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    let helper_off = *compiled
        .offsets
        .get(active_helper.name.as_str())
        .unwrap_or_else(|| panic!("{} has no results offset", active_helper.name));
    let active_off = *compiled
        .offsets
        .get("active")
        .or_else(|| compiled.offsets.get("active[t1]"))
        .expect("an offset for active (bare or first-element key)");
    // active's axis order is Target = [t1, t2, t3]; SubX's rows are t3
    // (axis 2) then t1 (axis 0). A positional-within-SubX spelling would
    // read rows 0 and 1 instead -- distinct values here, so it would fail.
    let subx_axis_rows = [2usize, 0usize];
    for step in 1..results.step_count {
        for (slot, axis_row) in subx_axis_rows.iter().enumerate() {
            let got = results.data[step * results.step_size + helper_off + slot];
            let want = results.data[(step - 1) * results.step_size + active_off + axis_row];
            assert!(
                (got - want).abs() < 1e-12,
                "helper slot {slot} at step {step} must hold the PREVIOUS value \
                 of active's axis row {axis_row}: got {got}, want {want}"
            );
        }
    }
}

/// The per-target-element emitter path (the C-LEARN "139" shape): a scalar
/// source feeding an `Ast::Arrayed` target whose partials freeze a bare-`*`
/// slice and a mixed pinned+`*` slice. These used to EMIT fragments that
/// failed codegen ("Cannot push view ... expected array expression"); they
/// must now compile via freeze helpers.
#[test]
fn scalar_to_arrayed_frozen_wildcard_slice_scores() {
    let project = TestProject::new("scalar_to_arrayed_freeze")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("R", &["r1", "r2"])
        .named_dimension("C", &["c1", "c2"])
        .array_aux("def[C,R]", "1")
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("k", "s * 0.1", None)
        .array_with_ranges(
            "agg[R]",
            vec![
                ("r1", "VECTOR SELECT(def[*, r1], vals[*] * k, 0, 0, 0)"),
                ("r2", "VECTOR SELECT(def[*, r2], vals[*] * k, 0, 0, 0)"),
            ],
        )
        .flow("g", "agg[r1] * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let names: Vec<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
    for elem in ["r1", "r2"] {
        let score = format!("{LINK_PREFIX}k\u{2192}agg[{elem}]");
        assert!(
            names.contains(&score.as_str()),
            "expected per-target-element score {score}; vars: {names:#?}"
        );
    }

    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "the k->agg[e] scores and their freeze helpers must all compile; \
         failures:\n{}",
        failures.join("\n")
    );

    // End to end: the k->agg[r1] score participates in the loop and scores
    // nonzero once the loop turns.
    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    let score = format!("{LINK_PREFIX}k\u{2192}agg[r1]");
    let offset = *compiled
        .offsets
        .get(score.as_str())
        .unwrap_or_else(|| panic!("{score} has no results offset"));
    let series: Vec<f64> = (0..results.step_count)
        .map(|step| results.data[step * results.step_size + offset])
        .collect();
    assert!(
        series.iter().any(|v| v.is_finite() && *v != 0.0),
        "{score} is identically zero, the dropped-fragment signature: {series:?}"
    );
}

/// Evaluation order: a freeze helper's CURRENT-step value is read by the link
/// scores, so the helper must be ordered ahead of every link score in the
/// emitted vars (which is the flows-runlist order for LTM fragments).
#[test]
fn freeze_helpers_are_ordered_before_link_scores() {
    let datamodel = select_slice_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let first_link = ltm
        .vars
        .iter()
        .position(|v| v.name.starts_with(LINK_PREFIX))
        .expect("fixture emits link scores");
    let helper_positions: Vec<usize> = ltm
        .vars
        .iter()
        .enumerate()
        .filter(|(_, v)| v.name.starts_with(FREEZE_PREFIX))
        .map(|(i, _)| i)
        .collect();
    assert!(!helper_positions.is_empty(), "fixture emits freeze helpers");
    for pos in helper_positions {
        assert!(
            pos < first_link,
            "freeze helper at index {pos} must precede the first link score at \
             index {first_link} (flows-runlist order)"
        );
    }
}

/// A shared frozen slice yields ONE helper var, not one per referencing
/// score: helper names are content-derived, so identical freezes dedup.
#[test]
fn identical_freezes_share_one_helper() {
    let datamodel = select_slice_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let mut seen: HashSet<&str> = HashSet::new();
    for v in &ltm.vars {
        if v.name.starts_with(FREEZE_PREFIX) {
            assert!(
                seen.insert(v.name.as_str()),
                "duplicate freeze helper emitted: {}",
                v.name
            );
        }
    }
}

/// A slice whose pin is DYNAMIC (a runtime index variable) cannot be
/// materialized -- each arm would need a static subscript -- so both
/// conventions stay doomed and the edge keeps its loud decline.
#[test]
fn dynamic_pin_slice_keeps_loud_decline() {
    let project = TestProject::new("dynamic_pin_decline")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("R", &["r1", "r2"])
        .named_dimension("C", &["c1", "c2"])
        .array_aux("m[C,R]", "1")
        .array_aux("n[C,R]", "2")
        .aux("idx", "1 + (s - s)", None)
        // Both VECTOR SELECT args are dynamic-pinned slices: for either
        // source, the changed-first partial freezes the OTHER slice and the
        // changed-last partial freezes ITS OWN -- both unmaterializable.
        .aux("out", "VECTOR SELECT(m[*, idx], n[*, idx], 0, 3, 0)", None)
        .flow("g", "out * 0.1", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let names: Vec<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
    for edge in ["m\u{2192}out", "n\u{2192}out"] {
        let score = format!("{LINK_PREFIX}{edge}");
        assert!(
            !names.contains(&score.as_str()),
            "a dynamic-pinned slice must keep declining; unexpectedly emitted {score}"
        );
    }
    assert!(
        !unfreezable_declines(&db, sync.project).is_empty(),
        "the loud UnfreezablePartial decline must survive for dynamic-pinned slices"
    );
}
