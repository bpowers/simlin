// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;

/// The scalar -> arrayed sibling (GH #780): a forced `PartialEquationError`
/// on a scalar-source -> arrayed-target edge routed through
/// `try_scalar_to_arrayed_link_scores` (the direct
/// `ltm_partial_equation_warning` call site) must ALSO record the edge
/// and drop the loop through it -- the third shaped/per-element call site
/// this fix touches.
#[test]
fn gh780_forced_partial_equation_scalar_to_arrayed_drops_loop() {
    use crate::db::{DiagnosticSeverity, ForcePartialEquationErrorGuard};
    use salsa::Setter;

    // driver (scalar stock) -> grid[d1] (arrayed) -> feedback (scalar) -> driver.
    let project = crate::test_common::TestProject::new("gh780_scalar_to_arrayed")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["a", "b"])
        .stock("driver", "1", &["bump"], &[], None)
        .flow("bump", "feedback", None)
        .array_aux("grid[d1]", "driver * 0.1")
        .aux("feedback", "grid[a] * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let _force = ForcePartialEquationErrorGuard::new("driver", "grid");

    let ltm = model_ltm_variables(&db, model, source_project);

    // No `driver -> grid[*]` per-element link scores.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("driver\u{2192}grid")),
        "the doomed scalar->arrayed edge must emit no link score; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );
    // The single loop traverses driver -> grid, so it is dropped.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}loop_score\u{205A}")),
        "loops through the doomed scalar->arrayed edge must be dropped; got: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    compile_project_incremental(&db, source_project, "main")
        .expect("the scalar->arrayed doomed-edge model still compiles");
    let diags = collect_model_diagnostics(&db, model, source_project);
    let assembly: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Warning && d.assembly_reason().is_some())
        .collect();
    // EXACTLY one warning -- the first doomed per-element partial; the edge
    // is recorded on that first doom (the rest of the elements never run),
    // so the loop drops with no fragment-failure cascade and no per-element
    // warning spam.
    assert_eq!(
        assembly.len(),
        1,
        "the doomed scalar->arrayed edge must warn exactly once (the first \
         doomed per-element partial), NOT a cascade; got: {assembly:?}"
    );
    assert!(
        assembly[0]
            .variable
            .as_deref()
            .is_some_and(|v| v.contains("driver\u{2192}grid")),
        "the one warning names the doomed per-element link score; got: {assembly:?}"
    );
}

/// A pinned-pass RE-VISIT of a doomed edge must not warn twice (round-2
/// review MINOR): the pinned pass dedups only edges that EMITTED a var
/// (`emitted_edges`), so a doomed edge -- which emits none -- is re-visited
/// and its emitter re-dooms deterministically. The new #780 doom sites must
/// gate the warning on `unscoreable_edges.insert(..)` returning true (the
/// pre-existing #758 convention), so the re-visit is silent. Repro:
/// discovery mode (the causal-edge pass visits `driver -> grid` first) plus
/// a pin through the same edge (the pinned pass re-visits it).
#[test]
fn gh780_doomed_edge_pinned_revisit_warns_once() {
    use crate::db::DiagnosticSeverity;
    use crate::db::ForcePartialEquationErrorGuard;
    use salsa::Setter;

    let mut project = crate::test_common::TestProject::new("gh780_revisit")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("d1", &["a", "b"])
        .stock("driver", "1", &["bump"], &[], None)
        .flow("bump", "feedback", None)
        .array_aux("grid[d1]", "driver * 0.1")
        .aux("feedback", "grid[a] * 0.01", None)
        .build_datamodel();
    // Pin the loop through the doomed edge (UIDs + LoopMetadata, the
    // `SetLoopName` shape) so the pinned pass re-visits `driver -> grid`.
    {
        let model = &mut project.models[0];
        let mut uid_of = HashMap::new();
        for (i, var) in model.variables.iter_mut().enumerate() {
            let uid = (i as i32) + 1;
            uid_of.insert(crate::canonicalize(var.get_ident()).into_owned(), uid);
            match var {
                datamodel::Variable::Stock(s) => s.uid = Some(uid),
                datamodel::Variable::Flow(f) => f.uid = Some(uid),
                datamodel::Variable::Aux(a) => a.uid = Some(uid),
                datamodel::Variable::Module(m) => m.uid = Some(uid),
            }
        }
        model.loop_metadata.push(datamodel::LoopMetadata {
            uids: ["driver", "grid", "feedback", "bump"]
                .iter()
                .map(|v| uid_of[*v])
                .collect(),
            deleted: false,
            name: "doomed loop".to_string(),
            description: String::new(),
        });
    }

    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let _force = ForcePartialEquationErrorGuard::new("driver", "grid");

    let _ = model_ltm_variables(&db, model, source_project);
    let diags = collect_model_diagnostics(&db, model, source_project);
    let partial_warnings: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && d.variable
                    .as_deref()
                    .is_some_and(|v| v.contains("driver\u{2192}grid"))
                && d.assembly_reason().is_some()
        })
        .collect();
    assert_eq!(
        partial_warnings.len(),
        1,
        "a pinned-pass re-visit of the doomed edge must NOT duplicate the \
         partial-equation warning; got: {partial_warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// Conveyor + LTM degradation warning (docs/design/conveyors.md §9.6)
// ---------------------------------------------------------------------------

/// Parse the minimal conveyor fixture into a datamodel. The `students`
/// stock carries a `<conveyor>` block, so its `compat.conveyor` marker is
/// present on the salsa diagnostic path (which never expands it).
#[cfg(test)]
fn minimal_conveyor_datamodel() -> datamodel::Project {
    use std::io::BufReader;
    let xml = include_str!("../../../../../test/conveyors/minimal_conveyor.xmile");
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile")
}

/// Predicate: a diagnostic is the §9.6 conveyor-LTM-degraded `Warning`
/// naming `conveyor_name`.
#[cfg(test)]
fn is_conveyor_ltm_degraded(d: &crate::db::Diagnostic, conveyor_name: &str) -> bool {
    use crate::common::ErrorCode;
    use crate::db::DiagnosticSeverity;
    d.severity == DiagnosticSeverity::Warning
        && d.variable.as_deref() == Some(conveyor_name)
        && d.is(DiagnosticCategory::Model, ErrorCode::ConveyorLtmDegraded)
        && d.details
            .as_deref()
            .is_some_and(|message| message.contains(conveyor_name))
}

/// With LTM enabled, a model containing a conveyor stock must emit exactly
/// one `ConveyorLtmDegraded` `Warning` naming the conveyor, reaching
/// `collect_model_diagnostics` (the exact entry point libsimlin/simlin-mcp
/// drive, and -- via the transient `ltm_enabled` re-enable --
/// `simlin_project_get_errors`). §9.6.
#[test]
fn test_conveyor_ltm_degraded_warning_surfaces_under_ltm() {
    use salsa::Setter;

    let project = minimal_conveyor_datamodel();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let degraded: Vec<_> = diags
        .iter()
        .filter(|d| is_conveyor_ltm_degraded(d, "Students"))
        .collect();
    assert_eq!(
        degraded.len(),
        1,
        "expected exactly one ConveyorLtmDegraded warning naming 'students'; got: {diags:?}"
    );
}

/// The `ltm_enabled` gate scopes the warning to LTM callers: a project that
/// never requested LTM must NOT pay LTM synthesis cost, so the conveyor
/// degradation warning is absent. Mirrors
/// `test_ltm_disabled_gate_suppresses_auto_flip_warning`.
#[test]
fn test_conveyor_ltm_degraded_warning_absent_without_ltm() {
    let project = minimal_conveyor_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    assert!(
        !sync.project.ltm_enabled(&db),
        "baseline: ltm_enabled must default to false"
    );

    let diags = collect_model_diagnostics(&db, source_model, sync.project);

    let has_degraded = diags
        .iter()
        .any(|d| is_conveyor_ltm_degraded(d, "Students"));
    assert!(
        !has_degraded,
        "LTM-disabled project must not emit the conveyor degradation warning; got: {diags:?}"
    );
}

/// A conveyor living in a sub-model that a PARENT model references as a MODULE
/// must surface EXACTLY ONE `ConveyorLtmDegraded` warning over the whole
/// project -- not two.
///
/// `model_ltm_variables(parent)` reaches `model_ltm_variables(child)`
/// transitively because the module-output read drives the parent's
/// port/composite discovery into the child. LTM derivations must therefore
/// return pure facts, and only the non-recursive `model_all_diagnostics`
/// trigger may emit them. The whole-project drain reports one row regardless
/// of nesting.
#[test]
fn test_conveyor_ltm_degraded_warning_emitted_once_across_module_boundary() {
    use crate::db::collect_all_diagnostics;
    use salsa::Setter;

    // Child = the minimal conveyor model, named to match the module ident
    // (`x_module` sets `model_name == ident`). The parent reads a child output
    // (`belt·graduating`), which is what pulls the parent's LTM pass into the
    // child's `model_ltm_variables`.
    let fixture = minimal_conveyor_datamodel();
    let sim_specs = fixture.sim_specs.clone();
    let mut child = fixture.models.into_iter().next().expect("one model");
    child.name = "belt".to_string();

    let parent = x_model(
        "main",
        vec![
            x_module("belt", &[], None),
            x_aux("reader", "belt·graduating", None),
        ],
    );

    let project = datamodel::Project {
        name: "conveyor_module".to_string(),
        sim_specs,
        dimensions: vec![],
        units: vec![],
        models: vec![parent, child],
        source: Default::default(),
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_all_diagnostics(&db, source_project);

    let degraded: Vec<_> = diags
        .iter()
        .filter(|d| is_conveyor_ltm_degraded(d, "Students"))
        .collect();
    assert_eq!(
        degraded.len(),
        1,
        "a conveyor in a module-referenced sub-model must warn exactly once across the whole \
         project; got: {diags:?}"
    );
}

/// The warning is advisory, not a hard error: the same conveyor model still
/// compiles and simulates through the special-stock build path (which expands
/// the belt and clears the marker), independent of the LTM diagnostic overlay.
#[test]
fn test_conveyor_still_simulates_despite_ltm_degraded_warning() {
    let project = minimal_conveyor_datamodel();
    let main = project.models[0].name.clone();
    let mut vm = crate::queue_compile::build_vm(&project, &main).expect("build conveyor vm");
    vm.run_to_end().expect("run conveyor sim");

    let students = vm
        .get_series(&crate::common::Ident::new("students"))
        .expect("students series");
    // Steady state: init 1000 == inflow(250) * transit(4), so it holds flat.
    assert!(students.len() > 40, "should have many saved steps");
    assert!(
        (students[students.len() - 1] - 1000.0).abs() < 1e-6,
        "conveyor holds steady state; final students {}",
        students[students.len() - 1]
    );
}

/// Parse the queue-drain fixture into a datamodel. The `waiting` stock carries
/// a `<queue/>` marker, so its `compat.queue` is present on the salsa
/// diagnostic path (which never expands it).
#[cfg(test)]
fn queue_drain_datamodel() -> datamodel::Project {
    use std::io::BufReader;
    let xml = include_str!("../../../../../test/queues/queue_drain.xmile");
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile")
}

/// Predicate: a diagnostic is the §10.5 queue-LTM-degraded `Warning` naming
/// `queue_name`.
#[cfg(test)]
fn is_queue_ltm_degraded(d: &crate::db::Diagnostic, queue_name: &str) -> bool {
    use crate::common::ErrorCode;
    use crate::db::{DiagnosticCategory, DiagnosticSeverity};
    d.severity == DiagnosticSeverity::Warning
        && d.variable.as_deref() == Some(queue_name)
        && d.is(DiagnosticCategory::Model, ErrorCode::QueueLtmDegraded)
        && d.details
            .as_deref()
            .is_some_and(|message| message.contains(queue_name))
}

/// With LTM enabled, a model containing a queue stock must emit exactly one
/// `QueueLtmDegraded` `Warning` naming the queue (§10.5), mirroring the
/// conveyor twin.
#[test]
fn test_queue_ltm_degraded_warning_surfaces_under_ltm() {
    use salsa::Setter;

    let project = queue_drain_datamodel();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let degraded: Vec<_> = diags
        .iter()
        .filter(|d| is_queue_ltm_degraded(d, "waiting"))
        .collect();
    assert_eq!(
        degraded.len(),
        1,
        "expected exactly one QueueLtmDegraded warning naming 'waiting'; got: {diags:?}"
    );
}

/// The `ltm_enabled` gate scopes the queue warning to LTM callers: a project
/// that never requested LTM must not emit it.
#[test]
fn test_queue_ltm_degraded_warning_absent_without_ltm() {
    let project = queue_drain_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    let diags = collect_model_diagnostics(&db, source_model, sync.project);

    assert!(
        !diags.iter().any(|d| is_queue_ltm_degraded(d, "waiting")),
        "LTM-disabled project must not emit the queue degradation warning; got: {diags:?}"
    );
}

/// A queue in a sub-model referenced as a MODULE by a parent must surface
/// EXACTLY ONE `QueueLtmDegraded` warning over the whole project -- the same
/// cross-module double-drain regression the conveyor twin guards, closed by
/// emitting from the per-model `model_all_diagnostics` trigger.
#[test]
fn test_queue_ltm_degraded_warning_emitted_once_across_module_boundary() {
    use crate::db::collect_all_diagnostics;
    use salsa::Setter;

    let fixture = queue_drain_datamodel();
    let sim_specs = fixture.sim_specs.clone();
    let mut child = fixture.models.into_iter().next().expect("one model");
    child.name = "q".to_string();

    let parent = x_model(
        "main",
        vec![x_module("q", &[], None), x_aux("reader", "q·served", None)],
    );

    let project = datamodel::Project {
        name: "queue_module".to_string(),
        sim_specs,
        dimensions: vec![],
        units: vec![],
        models: vec![parent, child],
        source: Default::default(),
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_all_diagnostics(&db, source_project);

    let degraded: Vec<_> = diags
        .iter()
        .filter(|d| is_queue_ltm_degraded(d, "waiting"))
        .collect();
    assert_eq!(
        degraded.len(),
        1,
        "a queue in a module-referenced sub-model must warn exactly once across the whole \
         project; got: {diags:?}"
    );
}
