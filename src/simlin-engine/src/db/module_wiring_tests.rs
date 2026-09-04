// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the module input-wiring diagnostic
//! (`model_module_wiring_diagnostics`).
//!
//! `build_module_inputs` runs at lowering and binds only what `bound_port`
//! returns, raising no error for what it does not: a module reference whose
//! `dst` is not this instance's `{module}·{port}`, or whose `src` is inside the
//! instance's namespace, is dropped there silently, and a `dst` naming a port
//! the target model does not declare binds a slot nothing reads. Either way the
//! port reads its default and the simulation is quietly wrong. The diagnostic
//! pass is the one place that wiring is validated
//! (`BadModuleInputDst`/`BadModuleInputSrc`); these tests pin that a mis-wired
//! input surfaces a Warning while a correct (module-qualified) wiring and empty
//! placeholder rows stay clean.

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

/// A reference whose `src` is inside the instance's own namespace binds no
/// port (`db::assemble::bound_port`), so it warns as a bad source -- for a
/// lone internal reference and beside a correctly bound port alike. Both
/// fixtures are read through `open_xmile`, the spelling a writer emits.
#[test]
fn internal_src_warns_that_it_binds_nothing() {
    let cases = [
        // r3: the instance's only reference is internal.
        (
            r#"<module name="bridge" model_name="leaf"><connect to="bridge.input" from="bridge.output"/></module>"#,
            r#"<aux name="input"><eqn>2</eqn></aux><aux name="output"><eqn>input + 1</eqn></aux>"#,
        ),
        // r3b: an internal reference beside a bound port.
        (
            r#"<aux name="source"><eqn>5</eqn></aux>
<module name="bridge" model_name="leaf"><connect to="bridge.input" from="bridge.output"/><connect to="bridge.gain" from="source"/></module>"#,
            r#"<aux name="input"><eqn>2</eqn></aux><aux name="gain"><eqn>1</eqn></aux><aux name="output"><eqn>input * gain + 1</eqn></aux>"#,
        ),
    ];
    for (main_vars, leaf_vars) in cases {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
<header><name>r3</name><vendor>simlin</vendor><product version="1.0">simlin</product></header>
<sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
<model name="main"><variables>{main_vars}<aux name="reader"><eqn>bridge.output</eqn></aux></variables></model>
<model name="leaf"><variables>{leaf_vars}</variables></model>
</xmile>"#
        );
        let project = crate::compat::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
            .expect("the fixture is well-formed XMILE");
        let diags = diagnostics(&project);
        let internal: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| {
                d.severity == DiagnosticSeverity::Warning
                    && matches!(&d.error, DiagnosticError::Model(Error { code: ErrorCode::BadModuleInputSrc, details: Some(m), .. }) if m.contains("own namespace") && m.contains("bridge\u{00B7}output"))
            })
            .collect();
        assert_eq!(
            internal.len(),
            1,
            "one warning names the internal source as binding nothing: {diags:?}"
        );
        assert!(
            !has_warning(&diags, ErrorCode::BadModuleInputDst),
            "the internal reference's dst names a real port: {diags:?}"
        );
    }
}

/// A connection between two instances recorded on the SOURCE instance (a
/// `src` inside its namespace, a `dst` in another instance's -- the shape
/// Stella writes for a module-to-module connect) is the `dst` arm's report,
/// not an internal reference: it warns `BadModuleInputDst` only.
#[test]
fn a_cross_instance_reference_on_the_source_instance_is_a_dst_report_only() {
    let diags = diagnostics(&project_with_reference("m·output", "other·port"));
    assert!(
        has_warning(&diags, ErrorCode::BadModuleInputDst),
        "the dst names no port of this instance: {diags:?}"
    );
    assert!(
        !has_warning(&diags, ErrorCode::BadModuleInputSrc),
        "a src inside the namespace is reported as internal only when the dst is this \
         instance's port: {diags:?}"
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
