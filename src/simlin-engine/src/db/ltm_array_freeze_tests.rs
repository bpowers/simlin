// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The LTM array-freeze helper (GH #995 option B, wrap-side).
//!
//! A ceteris-paribus partial that must freeze an ARRAY SLICE -- an other-dep
//! (or, changed-last, the live source) referenced as `arr[pin, *]` or
//! `arr[pin, *:Sub]` inside a vector builtin -- did not spell the freeze
//! inline: `PREVIOUS(<slice>)` had no codegen path (an array-valued operand
//! must be a view over storage), so the wrap used to either decline the score
//! loudly (`UnfreezablePartial`, the changed-first/changed-last doom) or emit
//! a fragment that failed to compile (the per-target-element path, which
//! never doom-checked). GH #995 phase C3 has since given the inline form a
//! path of its own -- a view over `prev_values` -- so the helper is no longer
//! the only way to spell the freeze; it is retained for the name-correct row
//! rule below, and collapsing the two is tracked as follow-on work.
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
    sync_from_datamodel_incremental,
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
///
/// Collected under `On`: a link score's or a freeze helper's "failed to
/// compile" warning is a fact of the overlay's derivation, which
/// `model_all_diagnostics` emits only when the overlay is assembled. Under
/// `Off` this list is empty whatever the LTM fragments did.
fn fragment_failures(db: &SimlinDb, project: crate::db::SourceProject) -> Vec<String> {
    collect_all_diagnostics(db, project, crate::db::LtmOverlay::On)
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
    collect_all_diagnostics(db, project, crate::db::LtmOverlay::On)
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

    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
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

    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
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
/// failed codegen ("an array operand here must be a variable ..."); they
/// must now compile via freeze helpers.
fn scalar_to_arrayed_fixture() -> TestProject {
    TestProject::new("scalar_to_arrayed_freeze")
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
        .stock("s", "10", &["g"], &[], None)
}

#[test]
fn scalar_to_arrayed_frozen_wildcard_slice_scores() {
    let datamodel = scalar_to_arrayed_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
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
    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
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
///
/// The fixture must genuinely PRODUCE duplicates for this to constrain the
/// dedup: the scalar->arrayed fixture's per-element partials each freeze the
/// same `vals[*]` slice (one `$⁚ltm⁚freeze⁚vals[*]` per target element before
/// dedup), where the single-helper select_slice fixture never had two to
/// collapse and passed with the dedup disabled.
#[test]
fn identical_freezes_share_one_helper() {
    let datamodel = scalar_to_arrayed_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let vals_helpers: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.starts_with(FREEZE_PREFIX) && n.contains("vals"))
        .collect();
    assert_eq!(
        vals_helpers,
        vec![format!("{FREEZE_PREFIX}vals[*]").as_str()],
        "both per-element partials freeze vals[*]; exactly one helper must survive"
    );

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
///
/// DISCLOSED: the base model itself does not simulate -- a dynamic-pinned
/// slice (`m[*, idx]`) is an array operand codegen rejects with or without
/// LTM, and no compiling spelling of the shape was found -- so the decline
/// arm this pins is defense-in-depth reachable only from already-broken
/// models. What production supplies here is the EMISSION path
/// (`model_ltm_variables` runs before, and independent of, simulation
/// compile), which is exactly what the assertions read.
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

/// THE review-driven regression pin: an A2A-emitted link score whose target
/// reduces over ITS OWN dimension. The `year -> sel` score is
/// `ApplyToAll([C], ... sum("$:ltm:freeze:active[*]"[*] * year[*]) ...)`, and
/// a BARE helper reference (a variable declared over C, inside an equation
/// iterating C) resolves to the CURRENT element -- turning the whole-row
/// frozen read into a per-element broadcast, a silent wrong score. The
/// wildcard-subscripted reference reads the full array in every context, and
/// this VM oracle computes the correct value from the recorded series, so the
/// broadcast reading fails on numbers rather than on shape.
#[test]
fn a2a_score_over_its_own_dimension_reads_the_whole_frozen_row() {
    let project = TestProject::new("a2a_same_dim")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges(
            "active[C]",
            vec![("c1", "2 + s / 1000"), ("c2", "3 + s / 1000")],
        )
        .array_with_ranges("year[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .array_aux("sel[C]", "SUM(active[*] * year[*])")
        .flow("g", "sel[c1] * 0.001", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let score = format!("{LINK_PREFIX}year\u{2192}sel");
    assert!(
        ltm.vars.iter().any(|v| v.name == score),
        "expected the A2A year->sel score to be emitted"
    );
    let failures = fragment_failures(&db, sync.project);
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));

    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
    .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    let at = |name: &str, step: usize| -> f64 {
        let off = *compiled
            .offsets
            .get(name)
            .or_else(|| compiled.offsets.get(format!("{name}[c1]").as_str()))
            .unwrap_or_else(|| panic!("no offset for {name}"));
        results.data[step * results.step_size + off]
    };
    let at_slot = |name: &str, slot: usize, step: usize| -> f64 {
        let off = *compiled
            .offsets
            .get(name)
            .or_else(|| compiled.offsets.get(format!("{name}[c1]").as_str()))
            .unwrap_or_else(|| panic!("no offset for {name}"));
        results.data[step * results.step_size + off + slot]
    };
    let score_off = *compiled
        .offsets
        .get(score.as_str())
        .unwrap_or_else(|| panic!("{score} has no results offset"));

    let mut checked = 0;
    for step in 1..results.step_count {
        for c in 0..2usize {
            // The changed-first partial for slot c: active frozen at the
            // PREVIOUS step (the WHOLE row), year live.
            let partial: f64 = (0..2)
                .map(|k| at_slot("active", k, step - 1) * at_slot("year", k, step))
                .sum();
            let d_sel = at_slot("sel", c, step) - at_slot("sel", c, step - 1);
            let d_year = at_slot("year", c, step) - at_slot("year", c, step - 1);
            let got = results.data[step * results.step_size + score_off + c];
            if d_sel == 0.0 || d_year == 0.0 {
                assert_eq!(got, 0.0, "guard arm at step {step} slot {c}");
                continue;
            }
            let want = (partial - at_slot("sel", c, step - 1)) / d_sel.abs() * d_year.signum();
            assert!(
                (got - want).abs() < 1e-9,
                "step {step} slot {c}: score {got} != whole-frozen-row value {want} \
                 (a per-element broadcast reading of the helper produces a \
                 different number here)"
            );
            checked += 1;
        }
    }
    assert!(checked >= 4, "the oracle must actually check live steps");
    let _ = at; // silence unused when only at_slot is exercised
}

/// The pin-before-materialize ordering (review finding): an A2A-shaped slot
/// body spells the frozen slice's kept axis as its bare DIMENSION name
/// (`def[*, R]`), which only the per-element pin resolves to a static row.
/// Materializing before the pin sees a dynamic-looking index and declines,
/// leaving the inline `PREVIOUS(<slice>)` to fail codegen.
#[test]
fn a2a_body_dimension_name_pin_resolves_before_materialization() {
    let project = TestProject::new("a2a_dim_name_pin")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("R", &["r1", "r2"])
        .named_dimension("C", &["c1", "c2"])
        .array_aux("def[C,R]", "1")
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("k", "s * 0.1", None)
        .array_aux("agg[R]", "VECTOR SELECT(def[*, R], vals[*] * k, 0, 0, 0)")
        .flow("g", "agg[r1] * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
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
        "the dimension-name-pinned frozen slices must materialize and compile; \
         failures:\n{}",
        failures.join("\n")
    );
}

/// No orphan helpers (review finding): an ABANDONED changed-first leg's
/// helpers must not be emitted. Here the `w -> out` changed-first partial
/// freezes THREE slices -- two materializable (`active[*:SubT]`,
/// `year[*:SubT]`) and one dynamic-pinned (`m[idx, *]`, unmaterializable) --
/// so that leg dooms after minting two helpers; the changed-last leg freezes
/// only `w[*]` and emits. Only the changed-last helper may survive.
///
/// DISCLOSED: as in `dynamic_pin_slice_keeps_loud_decline`, the base model
/// does not simulate (the dynamic-pinned slice is rejected by codegen with or
/// without LTM); the invariant pinned here is EMISSION bookkeeping, which
/// production computes before and independent of simulation compile.
#[test]
fn abandoned_leg_helpers_are_not_emitted() {
    let project = TestProject::new("abandoned_leg")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Target", &["t1", "t2", "t3"])
        .named_dimension("SubT", &["t1", "t2"])
        .named_dimension("C", &["c1", "c2"])
        .named_dimension("R", &["r1", "r2"])
        .array_aux("active[Target]", "1")
        .array_aux("year[Target]", "2")
        .array_aux("m[C,R]", "1")
        .aux("idx", "1", None)
        .array_aux("w[C]", "s * 1")
        .aux(
            "out",
            "VECTOR SELECT(active[*:SubT], year[*:SubT], 0, 3, 0) \
             + VECTOR SELECT(m[idx, *], w[*], 0, 3, 0)",
            None,
        )
        .flow("g", "out * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let helpers: Vec<&str> = ltm
        .vars
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| n.starts_with(FREEZE_PREFIX))
        .collect();
    // The loop edge is w -> out (active/year/m/idx are constants, hence
    // feed-forward and unscored in exhaustive mode). Its changed-first leg is
    // doomed by the m-slice, so the active/year helpers it minted on the way
    // must be discarded; its changed-last leg freezes w[*] and emits.
    assert!(
        helpers.contains(&format!("{FREEZE_PREFIX}w[*]").as_str()),
        "the changed-last leg's own helper must be emitted; helpers: {helpers:?}"
    );
    for orphan in ["active", "year"] {
        assert!(
            !helpers.iter().any(|h| h.contains(orphan)),
            "helper for {orphan} belongs to the ABANDONED changed-first leg and \
             must not be emitted; helpers: {helpers:?}"
        );
    }
}

/// ELM MAP's SOURCE position reads are bounded by the dep's WHOLE storage
/// (`codegen::full_source_len`), so an offset may legally cross a slice's
/// row boundary. A frozen SLICE there must therefore materialize as a
/// whole-dep mirror referenced with the ORIGINAL slice subscript -- a
/// row-sized helper turned those in-bounds reads into NaN in the partial
/// only (PR #1003 codex review). The source `dep[r1, *]` is row r1 of a
/// 2x2 dep; a constant offset of +2 reads row r2's elements -- outside the
/// slice, inside the dep. `offs` is loop-carrying AND time-varying (2 or
/// 1 as INT(s)'s parity flips, so the score goes numerically live) and the
/// A2A-shaped `offs -> mapped` score -- the one whole-array partial that
/// may hold an ELM MAP result -- is emitted; its changed-first branch
/// freezes `dep`.
#[test]
fn frozen_view_position_slice_keeps_full_storage_reads() {
    let project = TestProject::new("view_pos_slice_freeze")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("R", &["r1", "r2"])
        .named_dimension("K", &["k1", "k2"])
        .array_with_ranges_direct(
            "dep",
            vec!["R".to_string(), "K".to_string()],
            vec![
                ("r1,k1", "1 + s / 1000"),
                ("r1,k2", "2 + s / 1000"),
                ("r2,k1", "3 + s / 1000"),
                ("r2,k2", "4 + s / 1000"),
            ],
            None,
        )
        .array_aux("offs[K]", "2 - (INT(s) MOD 2)")
        .array_aux("mapped[K]", "VECTOR ELM MAP(dep[r1, *], offs[K])")
        .flow("g", "(mapped[k1] + 1) * 0.7", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    // The frozen dep in the A2A partial must be the WHOLE-DEP mirror read
    // with the ORIGINAL slice subscript (row pin + wildcard), never a row
    // helper (whose 2-slot storage the +2 offsets would overrun).
    let score = format!("{LINK_PREFIX}offs\u{2192}mapped");
    let score_var = ltm
        .vars
        .iter()
        .find(|v| v.name == score)
        .unwrap_or_else(|| {
            panic!(
                "the offs->mapped score must be emitted; vars: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            )
        });
    let score_text = score_var.equation.source_text();
    let mirror_ref_prefix = format!("\"{FREEZE_PREFIX}dep\"[r");
    assert!(
        score_text.contains(&mirror_ref_prefix),
        "the frozen dep must be the whole-dep mirror with the original pinned \
         row index; got:\n{score_text}"
    );
    let helper = ltm
        .vars
        .iter()
        .find(|v| v.name == format!("{FREEZE_PREFIX}dep"))
        .expect("a whole-dep freeze mirror over dep");
    assert_eq!(helper.dimensions, vec!["r".to_string(), "k".to_string()]);

    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "the view-position slice freeze must compile; failures:\n{}",
        failures.join("\n")
    );

    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
    .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    let at = |name: &str, first_elem: &str, slot: usize, step: usize| -> f64 {
        let off = *compiled
            .offsets
            .get(name)
            .or_else(|| {
                compiled
                    .offsets
                    .get(format!("{name}[{first_elem}]").as_str())
            })
            .unwrap_or_else(|| panic!("no offset for {name}"));
        results.data[step * results.step_size + off + slot]
    };

    // Premise oracle (LIVE semantics): a +2 offset crosses the row
    // boundary and reads row r2's element -- in bounds against the dep's
    // full 4-slot storage. If this fails the whole hazard is misdiagnosed.
    // The offset alternates 2/1 with INT(s)'s parity; require the
    // cross-row phase to actually occur so the pin is not vacuous.
    let mut crossed_row = false;
    for step in 0..results.step_count {
        for i in 0..2usize {
            let o = at("offs", "k1", i, step).round() as usize;
            let src_flat = i + o;
            if src_flat >= 2 {
                crossed_row = true;
            }
            let want = at("dep", "r1,k1", src_flat, step);
            let got = at("mapped", "k1", i, step);
            assert!(
                (got - want).abs() < 1e-9,
                "live ELM MAP must read the dep's full storage: mapped[{i}] \
                 step {step} got {got}, want dep flat {src_flat} = {want}"
            );
        }
    }
    assert!(
        crossed_row,
        "the fixture must exercise a cross-row read, or the regression pin is vacuous"
    );

    // THE regression pin: the A2A score is FINITE at every slot past the
    // first step. With a row-sized helper the frozen branch's +2 offsets
    // overran the 2-slot storage and the series went NaN.
    let score_off = *compiled
        .offsets
        .get(score.as_str())
        .unwrap_or_else(|| panic!("{score} has no results offset"));
    let mut nonzero = 0usize;
    for step in 1..results.step_count {
        for slot in 0..2usize {
            let val = results.data[step * results.step_size + score_off + slot];
            assert!(
                val.is_finite(),
                "{score}[slot {slot}] at step {step} must be finite, got {val}"
            );
            if val != 0.0 {
                nonzero += 1;
            }
        }
    }
    assert!(
        nonzero > 0,
        "the score must be live (non-zero somewhere), or the finiteness pin is vacuous"
    );
}

/// The frozen-VIEW-POSITION class (C-LEARN's last 4 failing fragments): a
/// frozen reference landing in a view-position argument of a vector builtin
/// (`VECTOR ELM MAP(PREVIOUS(base[c2]), offs[C])`) is an App where codegen
/// needs a view over storage. The freeze materializes as a WHOLE-DEP helper
/// (every element frozen), referenced with the ORIGINAL indices -- which
/// preserves ELM MAP's full-storage base semantics (`full_source_len` of the
/// helper equals the dep's, and the pinned element's flat position is the
/// base).
///
/// The fixture is sharpened so the reference FORM is pinned, not just the
/// helper's existence (a bare or wildcard-subscripted reference passed the
/// first draft): the pin is `c2` (flat base 1 -- a bare/wildcard reference
/// lands at base 0/current-element and computes a DIFFERENT number), the
/// offsets are loop-carrying AND time-varying (so the guard's `d_offs != 0`
/// arm goes live and the VM oracle actually executes), and the equation text
/// is asserted to reference the helper AT the original index.
#[test]
fn frozen_view_position_subscript_materializes() {
    let project = TestProject::new("view_pos_freeze")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges(
            "base[C]",
            vec![("c1", "2 + s / 1000"), ("c2", "3 + s / 1000")],
        )
        // Loop-carrying (depends on s) and time-varying (int(s) parity
        // flips as s grows), alternating the elm-map offset 0 / -1.
        .array_aux("offs[C]", "0 - (INT(s) MOD 2)")
        .array_aux("mapped[C]", "VECTOR ELM MAP(base[c2], offs[C])")
        .flow("g", "(mapped[c1] + 1) * 0.7", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let score = format!("{LINK_PREFIX}offs\u{2192}mapped");
    let score_var = ltm
        .vars
        .iter()
        .find(|v| v.name == score)
        .unwrap_or_else(|| {
            panic!(
                "the offs->mapped score must be emitted; vars: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            )
        });
    // THE reference-form pin: the helper must be read at the ORIGINAL pinned
    // index (base 1 in the dep's storage), never bare / wildcarded (base 0).
    let score_text = score_var.equation.source_text();
    assert!(
        score_text.contains("\"$\u{205A}ltm\u{205A}freeze\u{205A}base\"[c\u{B7}c2]")
            || score_text.contains("\"$\u{205A}ltm\u{205A}freeze\u{205A}base\"[c2]"),
        "the helper must be referenced at the original index; got:\n{score_text}"
    );

    let whole_dep_helper = format!("{FREEZE_PREFIX}base");
    let helper = ltm
        .vars
        .iter()
        .find(|v| v.name == whole_dep_helper)
        .expect("a whole-dep freeze helper over base");
    assert_eq!(helper.dimensions, vec!["c".to_string()]);
    let text = helper.equation.source_text().to_lowercase();
    for elem in ["c1", "c2"] {
        assert!(
            text.contains(&format!("base[c\u{B7}{elem}]")),
            "whole-dep helper freezes every element; got:\n{text}"
        );
    }

    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "the view-position freeze must compile via the helper; failures:\n{}",
        failures.join("\n")
    );

    // VM oracle: partial_c = elm_map(base_prev, offs_now)[c]
    //                      = base_prev[base(c2)=1 + offs_now[c]].
    let compiled = crate::db::compile_project_incremental(
        &db,
        sync.project,
        "main",
        crate::db::LtmOverlay::On,
    )
    .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    let at_slot = |name: &str, slot: usize, step: usize| -> f64 {
        let off = *compiled
            .offsets
            .get(name)
            .or_else(|| compiled.offsets.get(format!("{name}[c1]").as_str()))
            .unwrap_or_else(|| panic!("no offset for {name}"));
        results.data[step * results.step_size + off + slot]
    };
    let score_off = *compiled
        .offsets
        .get(score.as_str())
        .unwrap_or_else(|| panic!("{score} has no results offset"));
    let mut checked = 0;
    for step in 1..results.step_count {
        for c in 0..2usize {
            let flat = (1i64 + at_slot("offs", c, step).round() as i64).clamp(0, 1) as usize;
            let out_of_range = (1i64 + at_slot("offs", c, step).round() as i64) != flat as i64;
            let partial = if out_of_range {
                f64::NAN
            } else {
                at_slot("base", flat, step - 1)
            };
            let d_mapped = at_slot("mapped", c, step) - at_slot("mapped", c, step - 1);
            let d_offs = at_slot("offs", c, step) - at_slot("offs", c, step - 1);
            let got = results.data[step * results.step_size + score_off + c];
            if d_mapped == 0.0 || d_offs == 0.0 {
                assert_eq!(got, 0.0, "guard arm at step {step} slot {c}");
                continue;
            }
            let want =
                (partial - at_slot("mapped", c, step - 1)) / d_mapped.abs() * d_offs.signum();
            assert!(
                (got - want).abs() < 1e-9 || (got.is_nan() && want.is_nan()),
                "step {step} slot {c}: score {got} != lagged-base elm-map value {want} \
                 (a bare or wildcard helper reference reads the wrong base)"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the oracle's live branch must execute -- the fixture's offsets vary in time"
    );
}

/// A frozen SCALAR reference in a view-position argument (the offset arg of
/// `VECTOR ELM MAP(vals[c1], off)`) materializes as a scalar freeze helper:
/// `PREVIOUS(off)` is an App, not a view, but a scalar helper variable is.
#[test]
fn frozen_view_position_scalar_materializes() {
    let project = TestProject::new("view_pos_scalar_freeze")
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

    let score = format!("{LINK_PREFIX}vals[c1]\u{2192}mapped");
    assert!(
        ltm.vars.iter().any(|v| v.name == score),
        "the vals[c1]->mapped score must be emitted; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    let helper_name = format!("{FREEZE_PREFIX}off");
    let helper = ltm
        .vars
        .iter()
        .find(|v| v.name == helper_name)
        .expect("a scalar freeze helper over off");
    assert!(helper.dimensions.is_empty(), "scalar dep -> scalar helper");
    assert!(
        matches!(helper.equation, crate::db::LtmEquation::Scalar(_)),
        "a dims-less helper must carry the Scalar equation variant"
    );

    let failures = fragment_failures(&db, sync.project);
    assert!(
        failures.is_empty(),
        "every fragment must compile; failures:\n{}",
        failures.join("\n")
    );
}
