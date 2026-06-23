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
