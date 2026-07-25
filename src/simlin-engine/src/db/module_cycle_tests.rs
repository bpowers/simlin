// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the module-reference cycle guard.
//!
//! A cyclic or self-referential module graph makes the recursive
//! `model_module_map` / `compute_layout` salsa queries loop, which salsa turns
//! into an unrecoverable dependency-graph panic. The empty-`model_name` sibling
//! of this class was fixed in c1c4c954; this is the reachable cousin tracked as
//! GH #806. Every production entry point -- compile, diagnostic collection, and
//! analysis -- must surface the cycle as a clean `CircularDependency` error
//! instead of aborting (a WASM panic plus the recursive-mutex cascade).

use crate::analysis::analyze_model;
use crate::common::ErrorCode;
use crate::datamodel::{self, Equation, Variable};
use crate::db::{
    DiagnosticError, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel,
};
use crate::test_common::TestProject;

fn module_var(ident: &str, target_model: &str) -> Variable {
    Variable::Module(datamodel::Module {
        ident: ident.to_string(),
        model_name: target_model.to_string(),
        documentation: String::new(),
        units: None,
        references: vec![],
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn aux_var(ident: &str, equation: &str) -> Variable {
    Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn model(name: &str, variables: Vec<Variable>) -> datamodel::Model {
    datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

fn has_circular_diagnostic(diags: &[crate::db::Diagnostic]) -> bool {
    diags.iter().any(|d| {
        matches!(&d.error, DiagnosticError::Model(e) if e.code == ErrorCode::CircularDependency)
    })
}

/// A module that instantiates its own enclosing model: `main` contains a module
/// whose `model_name` is `main`.
#[test]
fn self_referential_module_errors_without_panicking() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models[0].variables.push(module_var("m", "main"));
    project.models[0].variables.push(aux_var("x", "1"));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    // Compile must reject cleanly rather than panic.
    assert!(
        compile_project_incremental(&db, sp, "main").is_err(),
        "a self-referential module must not compile"
    );

    // Diagnostic collection must surface the cycle, not panic.
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );

    // Analysis (the MCP read_model path) must degrade to an analysis_error.
    let mut db = db;
    let analysis = analyze_model(&project, &mut db, sp, "main", None)
        .expect("analyze_model must not panic on a module cycle");
    assert!(
        analysis.analysis_error.is_some(),
        "expected an analysis_error for a cyclic module graph"
    );
}

/// Two models that instantiate each other: `a` contains a module targeting `b`
/// and `b` contains a module targeting `a`.
#[test]
fn mutually_recursive_modules_error_without_panicking() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("a", vec![module_var("to_b", "b"), aux_var("x", "1")]),
        model("b", vec![module_var("to_a", "a"), aux_var("y", "1")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "a").is_err(),
        "mutually recursive modules must not compile"
    );

    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );

    let mut db = db;
    let analysis = analyze_model(&project, &mut db, sp, "a", None)
        .expect("analyze_model must not panic on a module cycle");
    assert!(analysis.analysis_error.is_some());
}

/// The PER-MODEL diagnostic collector needs the same cycle gate the
/// whole-project one has. `collect_all_diagnostics` consults
/// `project_module_graph` before driving each model's passes;
/// `collect_model_diagnostics` -- equally `pub`, and the shape a future caller
/// would reach for to diagnose one model -- did not, so on a cyclic project it
/// drove `model_all_diagnostics` straight into the recursive `model_module_map`
/// query and salsa raised an unrecoverable dependency-graph cycle panic. Milder
/// than a stack overflow (this one unwinds, so a non-`panic=abort` host could
/// catch it) but the same class: the caller-side gate was the only thing
/// holding it, and only on one of the two callers.
#[test]
fn per_model_diagnostics_report_a_cycle_instead_of_panicking() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("a", vec![module_var("to_b", "b"), aux_var("x", "1")]),
        model("b", vec![module_var("to_a", "a"), aux_var("y", "1")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_model_diagnostics(&db, sync.models["a"].source, sync.project);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );
}

/// The gate must not swallow a valid model's diagnostics just because some
/// OTHER model in the project is cyclic -- the same reachability scoping
/// `collect_all_diagnostics` uses. `main` here reaches no cycle, so its own
/// passes still run and its own equation error is still reported.
#[test]
fn per_model_diagnostics_survive_an_unrelated_draft_cycle() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("main", vec![aux_var("x", "undefined_thing")]),
        model("a", vec![module_var("to_b", "b")]),
        model("b", vec![module_var("to_a", "a")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_model_diagnostics(&db, sync.models["main"].source, sync.project);
    assert!(
        !has_circular_diagnostic(&diags),
        "a model that reaches no cycle must not be reported as cyclic: {diags:?}"
    );
    assert!(
        !diags.is_empty(),
        "main's own diagnostics must still be collected despite the unrelated \
         draft cycle, got: {diags:?}"
    );
}

/// The cycle diagnostic must carry the model's DISPLAY name, not its canonical
/// map key -- and that is load-bearing, not cosmetic.
///
/// `simlin-mcp-core`'s `read_model` (and `edit_model`) filter the collected
/// diagnostics with `e.model_name.as_ref().is_none_or(|name| name ==
/// model_name)`, where `model_name` comes from `resolve_model_name` and is the
/// DATAMODEL spelling (`&m.name`). So a diagnostic reporting a model as
/// `sub_a` when the project spells it `Sub A` does not match and is dropped
/// from what the MCP tool returns -- silently, since the filter has no
/// fallback. Every other diagnostic in the crate already used
/// `model.name(db)`; the module-cycle gate was the sole outlier, so a
/// `CircularDependency` on a non-canonically-named model was the one error that
/// vanished on the way out.
///
/// Every OTHER fixture in this file uses names that are already canonical
/// (`a`, `b`, `main`), where the two spellings coincide and nothing can
/// detect a regression. This one does not, so "tidying" the gate back to the
/// canonical key reds here.
#[test]
fn cycle_diagnostic_carries_the_display_name_not_the_canonical_key() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model(
            "Sub A",
            vec![module_var("to_b", "Sub B"), aux_var("x", "1")],
        ),
        model(
            "Sub B",
            vec![module_var("to_a", "Sub A"), aux_var("y", "1")],
        ),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let circular_models = |diags: &[crate::db::Diagnostic]| -> Vec<String> {
        diags
            .iter()
            .filter(|d| {
                matches!(&d.error, DiagnosticError::Model(e) if e.code == ErrorCode::CircularDependency)
            })
            .map(|d| d.model.clone())
            .collect()
    };

    // The per-model entry point (the one that gained the gate).
    let per_model = collect_model_diagnostics(&db, sync.models["sub_a"].source, sync.project);
    assert_eq!(
        circular_models(&per_model),
        vec!["Sub A".to_string()],
        "the per-model collector must report the display spelling"
    );

    // ...and the whole-project one, which reaches the same helper by delegating.
    let all = collect_all_diagnostics(&db, sync.project);
    let mut names = circular_models(&all);
    names.sort();
    assert_eq!(
        names,
        vec!["Sub A".to_string(), "Sub B".to_string()],
        "the whole-project collector must report display spellings too"
    );
}

/// An unused draft cycle (`a <-> b`) that the requested `main` model does not
/// reach must NOT block compiling or analyzing `main` -- the recursive queries
/// only loop for modules under the requested root. The cycle is still surfaced
/// as a diagnostic for the affected models. Regression for the Codex review
/// finding on PR #807 (the project-wide guard over-rejected valid roots).
///
/// `main` carries a dimensional problem of its OWN so that the "does not
/// block" half is actually pinned. With a clean `main` this test could not
/// tell "main's passes ran and found nothing" apart from "main's passes were
/// skipped" -- the exact failure a project-wide reject would produce, and the
/// one the gate's reachability scoping exists to prevent. A unit mismatch is
/// the right canary: it is a Warning, so it flows through `check_model_units`
/// (a pass the gate would skip) without disturbing the compile assertion
/// above.
#[test]
fn unused_draft_cycle_does_not_block_valid_main() {
    use crate::testutils::x_aux;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model(
            "main",
            vec![
                x_aux("x", "1", Some("widget")),
                // widget + time: a genuine dimensional mismatch, reported as a
                // Warning by `check_model_units`.
                aux_var("bad", "x + TIME"),
            ],
        ),
        model("a", vec![module_var("to_b", "b")]),
        model("b", vec![module_var("to_a", "a")]),
    ];

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    // `main` cannot reach the a<->b cycle, so it compiles cleanly.
    assert!(
        compile_project_incremental(&db, sp, "main").is_ok(),
        "a valid main must compile despite an unrelated draft cycle"
    );

    // Diagnostics still surface the draft cycle (for the affected models)...
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "the unrelated draft cycle should still be reported: {diags:?}"
    );
    // ...and main's own passes still ran, so its own problem is still reported.
    assert!(
        diags.iter().any(|d| d.model == "main"
            && matches!(&d.error, DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch)),
        "a valid model's own diagnostics must not be hidden by an unrelated \
         draft cycle: {diags:?}"
    );

    // Analyzing main succeeds (no analysis_error); analyzing a cyclic model degrades.
    let main_analysis =
        analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model must not panic");
    assert!(
        main_analysis.analysis_error.is_none(),
        "analyzing a valid main must not error on an unrelated draft cycle"
    );
    let a_analysis =
        analyze_model(&project, &mut db, sp, "a", None).expect("analyze_model must not panic");
    assert!(
        a_analysis.analysis_error.is_some(),
        "analyzing a model that reaches a cycle must surface an analysis_error"
    );
}

/// A valid nested-module project (no cycle) must still compile and produce no
/// spurious cycle diagnostic -- the guard must not false-positive on legitimate
/// acyclic nesting.
#[test]
fn acyclic_nested_modules_compile_clean() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("main", vec![module_var("mid", "middle"), aux_var("x", "1")]),
        model(
            "middle",
            vec![module_var("leaf_mod", "leaf"), aux_var("y", "2")],
        ),
        model("leaf", vec![aux_var("z", "3")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "main").is_ok(),
        "an acyclic nested-module project must compile"
    );
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        !has_circular_diagnostic(&diags),
        "acyclic nesting must not report a module cycle: {diags:?}"
    );
}
