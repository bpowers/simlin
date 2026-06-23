// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the module input-wiring diagnostic
//! (`model_module_wiring_diagnostics`).
//!
//! `build_module_inputs` silently drops a module reference whose `dst` does not
//! name an input of the target model, so the port reads its default and the
//! simulation is quietly wrong. The salsa compile path had lost the legacy
//! `BadModuleInputDst`/`BadModuleInputSrc` check; these tests pin that a
//! mis-wired input now surfaces a Warning while a correct (module-qualified)
//! wiring and empty placeholder rows stay clean.

use crate::common::{Error, ErrorCode};
use crate::datamodel::{self, Equation, Variable, Visibility};
use crate::db::{
    Diagnostic, DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    sync_from_datamodel,
};
use crate::test_common::TestProject;

/// `main` with `local_input`, a `submodel` exposing input port `input_var`, and
/// a module `m` in `main` whose single reference is `{ src, dst }`.
fn project_with_reference(src: &str, dst: &str) -> datamodel::Project {
    let mut project = TestProject::new("test")
        .aux("local_input", "10", None)
        .build_datamodel();

    project.models.push(datamodel::Model {
        name: "submodel".to_string(),
        sim_specs: None,
        variables: vec![Variable::Aux(datamodel::Aux {
            ident: "input_var".to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: true,
                visibility: Visibility::Public,
                ..datamodel::Compat::default()
            },
        })],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });

    project.models[0]
        .variables
        .push(Variable::Module(datamodel::Module {
            ident: "m".to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: src.to_string(),
                dst: dst.to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        }));

    project
}

fn diagnostics(project: &datamodel::Project) -> Vec<Diagnostic> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    collect_all_diagnostics(&db, sync.project)
}

fn has_warning(diags: &[Diagnostic], code: ErrorCode) -> bool {
    diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(&d.error, DiagnosticError::Model(Error { code: c, .. }) if *c == code)
    })
}

/// A correctly module-qualified `dst` (`m·input_var`) wiring a real input port
/// resolves cleanly -- no wiring diagnostic.
#[test]
fn qualified_dst_to_real_port_is_clean() {
    let diags = diagnostics(&project_with_reference("local_input", "m·input_var"));
    assert!(
        !has_warning(&diags, ErrorCode::BadModuleInputDst),
        "a correct module-qualified dst must not warn: {diags:?}"
    );
    assert!(!has_warning(&diags, ErrorCode::BadModuleInputSrc));
}

/// A BARE `dst` (the editor-bug shape: just the port name, missing the
/// `module·` qualifier) never matches an input and is silently dropped at
/// assembly -- it must warn.
#[test]
fn bare_dst_warns() {
    let diags = diagnostics(&project_with_reference("local_input", "input_var"));
    assert!(
        has_warning(&diags, ErrorCode::BadModuleInputDst),
        "a bare (unqualified) dst must surface a BadModuleInputDst warning: {diags:?}"
    );
}

/// A qualified `dst` naming a port that does not exist in the child model warns.
#[test]
fn dangling_dst_port_warns() {
    let diags = diagnostics(&project_with_reference("local_input", "m·nonexistent"));
    assert!(
        has_warning(&diags, ErrorCode::BadModuleInputDst),
        "a dst naming a non-existent child input must warn: {diags:?}"
    );
}

/// A bare `src` naming no variable in the enclosing model warns.
#[test]
fn dangling_src_warns() {
    let diags = diagnostics(&project_with_reference("missing_var", "m·input_var"));
    assert!(
        has_warning(&diags, ErrorCode::BadModuleInputSrc),
        "a src naming no parent variable must warn: {diags:?}"
    );
}

/// Empty placeholder endpoints (the new-row UI pattern) are not wiring errors.
#[test]
fn empty_placeholder_reference_is_clean() {
    let diags = diagnostics(&project_with_reference("", ""));
    assert!(
        !has_warning(&diags, ErrorCode::BadModuleInputDst),
        "{diags:?}"
    );
    assert!(
        !has_warning(&diags, ErrorCode::BadModuleInputSrc),
        "{diags:?}"
    );
}
