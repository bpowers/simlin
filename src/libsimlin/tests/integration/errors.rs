// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::CStr;
use std::ptr;

use simlin::*;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use crate::common::open_project_from_datamodel;

#[allow(unused_imports)]
use simlin::errors::{format_diagnostic, FormattedErrorKind, UnitErrorKind};

#[test]
fn test_format_diagnostic_equation_error() {
    use engine::common::{EquationError, ErrorCode};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind};

    let diag = Diagnostic {
        model: "test_model".to_string(),
        variable: Some("bad_var".to_string()),
        error: DiagnosticError::Equation(EquationError {
            start: 4,
            end: 9,
            code: ErrorCode::UnknownDependency,
        }),
        severity: DiagnosticSeverity::Error,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.code, ErrorCode::UnknownDependency);
    assert_eq!(formatted.kind, FormattedErrorKind::Variable);
    assert_eq!(formatted.model_name, Some("test_model".to_string()));
    assert_eq!(formatted.variable_name, Some("bad_var".to_string()));
    assert_eq!(formatted.start_offset, 4);
    assert_eq!(formatted.end_offset, 9);
    assert!(formatted.unit_error_kind.is_none());
    let msg = formatted.message.expect("should have message");
    assert!(
        msg.contains("test_model"),
        "message should include model: {msg}"
    );
    assert!(msg.contains("bad_var"), "message should include var: {msg}");
    assert!(
        msg.contains("unknown_dependency"),
        "message should include error code: {msg}"
    );
}

#[test]
fn test_format_diagnostic_equation_error_no_variable() {
    use engine::common::{EquationError, ErrorCode};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind};

    let diag = Diagnostic {
        model: "m".to_string(),
        variable: None,
        error: DiagnosticError::Equation(EquationError {
            start: 0,
            end: 5,
            code: ErrorCode::EmptyEquation,
        }),
        severity: DiagnosticSeverity::Error,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.kind, FormattedErrorKind::Variable);
    assert_eq!(formatted.variable_name, None);
    let msg = formatted.message.expect("should have message");
    assert!(
        msg.contains("<unknown>"),
        "missing variable should show <unknown>: {msg}"
    );
}

#[test]
fn test_format_diagnostic_model_error_non_unit() {
    use engine::common::{Error, ErrorCode, ErrorKind};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind};

    let diag = Diagnostic {
        model: "broken_model".to_string(),
        variable: None,
        error: DiagnosticError::Model(Error {
            kind: ErrorKind::Model,
            code: ErrorCode::CircularDependency,
            details: Some("a -> b -> a".to_string()),
        }),
        severity: DiagnosticSeverity::Error,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.code, ErrorCode::CircularDependency);
    assert_eq!(formatted.kind, FormattedErrorKind::Model);
    assert!(formatted.unit_error_kind.is_none());
    assert_eq!(formatted.model_name, Some("broken_model".to_string()));
    assert_eq!(formatted.start_offset, 0);
    assert_eq!(formatted.end_offset, 0);
}

#[test]
fn test_format_diagnostic_model_error_unit_mismatch() {
    use engine::common::{Error, ErrorCode, ErrorKind};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind, UnitErrorKind};

    let diag = Diagnostic {
        model: "unit_model".to_string(),
        variable: Some("x".to_string()),
        error: DiagnosticError::Model(Error {
            kind: ErrorKind::Model,
            code: ErrorCode::UnitMismatch,
            details: None,
        }),
        severity: DiagnosticSeverity::Error,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.code, ErrorCode::UnitMismatch);
    assert_eq!(formatted.kind, FormattedErrorKind::Units);
    assert!(
        matches!(formatted.unit_error_kind, Some(UnitErrorKind::Inference)),
        "expected Inference unit error kind"
    );
    assert_eq!(formatted.variable_name, Some("x".to_string()));
}

#[test]
fn test_format_diagnostic_unit_error() {
    use engine::common::{ErrorCode, UnitError};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind, UnitErrorKind};

    let diag = Diagnostic {
        model: "unit_model".to_string(),
        variable: Some("measured".to_string()),
        error: DiagnosticError::Unit(UnitError::ConsistencyError(
            ErrorCode::UnitMismatch,
            engine::builtins::Loc::new(2, 8),
            Some("kg vs m".to_string()),
        )),
        severity: DiagnosticSeverity::Warning,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.code, ErrorCode::UnitMismatch);
    assert_eq!(formatted.kind, FormattedErrorKind::Units);
    assert!(
        matches!(formatted.unit_error_kind, Some(UnitErrorKind::Consistency)),
        "expected Consistency unit error kind"
    );
    assert_eq!(formatted.start_offset, 2);
    assert_eq!(formatted.end_offset, 8);
    assert_eq!(formatted.model_name, Some("unit_model".to_string()));
    assert_eq!(formatted.variable_name, Some("measured".to_string()));
    let msg = formatted.message.expect("should have message");
    assert!(msg.contains("kg vs m"), "should contain details: {msg}");
}

#[test]
fn test_format_diagnostic_unit_definition_error() {
    use engine::common::{EquationError, ErrorCode, UnitError};
    use engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};
    use simlin::errors::{format_diagnostic, FormattedErrorKind, UnitErrorKind};

    let diag = Diagnostic {
        model: "unit_def_model".to_string(),
        variable: Some("y".to_string()),
        error: DiagnosticError::Unit(UnitError::DefinitionError(
            EquationError {
                start: 0,
                end: 3,
                code: ErrorCode::UnitDefinitionErrors,
            },
            Some("parse error".to_string()),
        )),
        severity: DiagnosticSeverity::Warning,
    };

    let formatted = format_diagnostic(&diag);
    assert_eq!(formatted.code, ErrorCode::UnitDefinitionErrors);
    assert_eq!(formatted.kind, FormattedErrorKind::Units);
    assert!(
        matches!(formatted.unit_error_kind, Some(UnitErrorKind::Definition)),
        "expected Definition unit error kind"
    );
    let msg = formatted.message.expect("should have message");
    assert!(msg.contains("parse error"), "should contain details: {msg}");
}

#[test]
fn test_error_kind_equation_error() {
    let datamodel = TestProject::new("kind_test")
        .aux("bad", "1 + unknown", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let equation_error = error_slice
            .iter()
            .find(|e| e.code == SimlinErrorCode::UnknownDependency)
            .expect("should have unknown dependency error");

        assert_eq!(
            equation_error.kind,
            SimlinErrorKind::Variable,
            "equation errors should have Variable kind"
        );
        assert_eq!(
            equation_error.unit_error_kind,
            SimlinUnitErrorKind::NotApplicable,
            "non-unit errors should have NotApplicable unit_error_kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_consistency_error() {
    let datamodel = TestProject::new("unit_kind_test")
        .unit("Person", None)
        .unit("Dollar", None)
        .aux("x", "1", Some("Person"))
        .aux("y", "x", Some("Dollar"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);

        // A unit mismatch produces both an inference-level diagnostic (from
        // the constraint solver) and a per-variable consistency diagnostic.
        // Verify both are present and correctly classified.
        let has_inference = error_slice.iter().any(|e| {
            e.kind == SimlinErrorKind::Units && e.unit_error_kind == SimlinUnitErrorKind::Inference
        });
        let has_consistency = error_slice.iter().any(|e| {
            e.kind == SimlinErrorKind::Units
                && e.unit_error_kind == SimlinUnitErrorKind::Consistency
        });

        assert!(
            has_consistency,
            "unit mismatch should produce a Consistency error"
        );
        assert!(
            has_inference,
            "unit mismatch should also produce an Inference error from the constraint solver"
        );

        // The consistency detail must arrive with the wire code UnitMismatch
        // (not an engine-numbered discriminant) and carry the bare reason in
        // `details`, separate from the snippet-formatted `message`.
        let consistency = error_slice
            .iter()
            .find(|e| {
                e.kind == SimlinErrorKind::Units
                    && e.unit_error_kind == SimlinUnitErrorKind::Consistency
            })
            .unwrap();
        assert_eq!(consistency.code, SimlinErrorCode::UnitMismatch);
        assert!(
            !consistency.details.is_null(),
            "consistency error should carry bare details"
        );
        let details = CStr::from_ptr(consistency.details).to_str().unwrap();
        assert!(
            details.starts_with("computed units"),
            "details should be the bare reason: {details}"
        );
        assert!(
            !details.contains('~') && !details.contains("units error in model"),
            "details must not carry snippet/summary formatting: {details}"
        );
        let message = CStr::from_ptr(consistency.message).to_str().unwrap();
        assert!(
            message.contains('~'),
            "message keeps the terminal-formatted snippet: {message}"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_all_error_kinds_mapped() {
    let datamodel = TestProject::new("all_kinds_test")
        .unit("A", None)
        .unit("B", None)
        .aux("eq_error", "1 + bogus", None)
        .aux("src", "1", Some("A"))
        .aux("unit_error", "src", Some("B"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count >= 2, "should have at least 2 errors");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);

        let has_variable_kind = error_slice
            .iter()
            .any(|e| e.kind == SimlinErrorKind::Variable);
        let has_units_kind = error_slice.iter().any(|e| e.kind == SimlinErrorKind::Units);

        assert!(has_variable_kind, "should have Variable kind error");
        assert!(has_units_kind, "should have Units kind error");

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_definition_error() {
    // Create a project with an invalid unit syntax to trigger a Definition error
    let datamodel = TestProject::new("def_error_test")
        .unit("BadUnit", Some("1///invalid"))
        .aux("x", "1", Some("BadUnit"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0, "should have at least one error");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let unit_def_error = error_slice
            .iter()
            .find(|e| e.unit_error_kind == SimlinUnitErrorKind::Definition);

        assert!(
            unit_def_error.is_some(),
            "should have a Definition unit error kind, got: {:?}",
            error_slice
                .iter()
                .map(|e| (e.code, e.kind, e.unit_error_kind))
                .collect::<Vec<_>>()
        );

        let def_error = unit_def_error.unwrap();
        assert_eq!(
            def_error.kind,
            SimlinErrorKind::Units,
            "definition errors should have Units kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_inference_error() {
    // Create a project with conflicting inferred units to trigger an Inference error
    // Adding Widget + Month (time units) causes inference to fail
    let datamodel = TestProject::new("infer_error_test")
        .with_time_units("Month")
        .unit("Widget", None)
        .aux("input", "1", Some("Widget"))
        .aux("bad", "input + TIME", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0, "should have at least one error");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let unit_infer_error = error_slice
            .iter()
            .find(|e| e.unit_error_kind == SimlinUnitErrorKind::Inference);

        assert!(
            unit_infer_error.is_some(),
            "should have an Inference unit error kind, got: {:?}",
            error_slice
                .iter()
                .map(|e| (e.code, e.kind, e.unit_error_kind))
                .collect::<Vec<_>>()
        );

        let infer_error = unit_infer_error.unwrap();
        assert_eq!(
            infer_error.kind,
            SimlinErrorKind::Units,
            "inference errors should have Units kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_str() {
    unsafe {
        let err_str = simlin_error_str(SimlinErrorCode::NoError as u32);
        assert!(!err_str.is_null());
        let s = CStr::from_ptr(err_str);
        assert_eq!(s.to_str().unwrap(), "no_error");

        // Test unknown error code returns "unknown_error"
        let unknown_str = simlin_error_str(9999);
        assert!(!unknown_str.is_null());
        let s = CStr::from_ptr(unknown_str);
        assert_eq!(s.to_str().unwrap(), "unknown_error");
    }
}

// ---------------------------------------------------------------------------
// GH #466: LTM diagnostics reachable through simlin_project_get_errors after
// a sim was created with enable_ltm=true.
// ---------------------------------------------------------------------------

/// Build a scalar project whose element-level causal graph is a single cycle
/// of `total_nodes` nodes (1 stock + 1 flow + (total_nodes - 2) auxiliaries).
/// Mirrors the engine's `build_chain_scc_project` auto-flip fixture: at 51
/// nodes the largest SCC exceeds `MAX_LTM_SCC_NODES` (50), so an LTM compile
/// auto-flips from exhaustive enumeration to discovery mode and accumulates a
/// "discovery mode" Warning diagnostic.
///
/// Chain: `cap_stock -> aux_{N-3} -> ... -> aux_0 -> cap_flow -> cap_stock`.
fn build_chain_scc_datamodel(name: &str, total_nodes: usize) -> engine::datamodel::Project {
    assert!(total_nodes >= 3, "chain SCC needs >= 3 nodes");
    let aux_count = total_nodes - 2;
    let mut builder = TestProject::new(name);
    for i in 0..aux_count {
        let var = format!("aux_{i}");
        let eq = if i + 1 == aux_count {
            "cap_stock".to_string()
        } else {
            format!("aux_{}", i + 1)
        };
        builder = builder.scalar_aux(&var, &eq);
    }
    builder = builder.flow("cap_flow", "aux_0", None);
    builder = builder.stock("cap_stock", "0", &["cap_flow"], &[], None);
    builder.build_datamodel()
}

/// Returns true if any error detail's message contains `needle`.
///
/// # Safety
/// `all_errors` must be a valid non-null `SimlinError` pointer.
unsafe fn any_detail_message_contains(all_errors: *const SimlinError, needle: &str) -> bool {
    let count = simlin_error_get_detail_count(all_errors);
    if count == 0 {
        return false;
    }
    let details = simlin_error_get_details(all_errors);
    let slice = std::slice::from_raw_parts(details, count);
    slice.iter().any(|d| {
        if d.message.is_null() {
            return false;
        }
        CStr::from_ptr(d.message)
            .to_str()
            .map(|m| m.contains(needle))
            .unwrap_or(false)
    })
}

/// F2 regression: `simlin_project_get_errors` must NOT report the
/// `QueueNotExpanded` guard error for a valid queue model. The queue fixtures
/// emit no Error-severity diagnostic, so `get_errors` returns NULL (no errors).
/// Before the `build_sim` dispatch, `get_errors` computed `vm_error` via the
/// ordinary `compile_project_incremental`, which hit the guard and produced a
/// spurious error for a model that simulates fine.
#[test]
fn test_get_errors_queue_model_reports_none() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "out_error must be null");
        assert!(
            all_errors.is_null(),
            "valid queue model must report no errors"
        );

        if !all_errors.is_null() {
            simlin_error_free(all_errors);
        }
        simlin_project_unref(proj);
    }
}

/// F2 + F15 regression: `simlin_project_get_errors` must report NO errors for a
/// valid conveyor model. F2 stopped the `ConveyorNotExpanded` guard error; F15
/// stopped the phantom `empty_equation` diagnostics that the salsa diagnostic
/// path emitted for each conveyor-driven outflow (the primary + leak flows carry
/// no `<eqn>` by XMILE design). The minimal_conveyor fixture requests no LTM, so
/// there is no LTM-degraded Warning either -- `get_errors` returns NULL.
#[test]
fn test_get_errors_conveyor_model_has_no_not_expanded() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "out_error must be null");
        if !all_errors.is_null() {
            let count = simlin_error_get_detail_count(all_errors);
            let details = simlin_error_get_details(all_errors);
            let messages: Vec<String> = std::slice::from_raw_parts(details, count)
                .iter()
                .filter_map(|d| {
                    (!d.message.is_null())
                        .then(|| CStr::from_ptr(d.message).to_str().ok().map(String::from))
                        .flatten()
                })
                .collect();
            simlin_error_free(all_errors);
            panic!("valid conveyor model must report no errors; got: {messages:?}");
        }
        simlin_project_unref(proj);
    }
}

/// A two-model XMILE whose main model instantiates `sub` as a module and whose
/// `sub` model holds a `<conveyor>` stock. Conveyor/queue support is main-model
/// only for now (docs/design/conveyors.md §9.3), so this must be rejected with a
/// clear, user-facing limitation naming the stock and its model -- NOT the
/// engine-internal "reached the ordinary compile path un-expanded" guard text.
const CONVEYOR_IN_SUBMODEL_XMILE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model>
    <variables>
      <module name="sub"/>
    </variables>
  </model>
  <model name="sub">
    <variables>
      <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
        <conveyor><len>4</len></conveyor></stock>
      <flow name="in_f"><eqn>10</eqn></flow>
      <flow name="out_f"><eqn>0</eqn></flow>
    </variables>
  </model>
</xmile>"#;

/// F3: a conveyor stock in a NON-main (module-referenced) model is permanently
/// un-simulatable (support is main-model only), and every FFI surface must
/// surface a CLEAR, user-facing limitation rather than the engine-internal
/// `ConveyorNotExpanded` guard text. Before the fix the all-models guard tripped
/// with "reached the ordinary compile path un-expanded", which reads like an
/// engine bug. This pins both `simlin_project_get_errors` (a sensible message
/// naming the stock + model) and `simlin_sim_new` + run (surfaces the same error,
/// never a panic or a silent success).
#[test]
fn test_submodel_conveyor_surfaces_clear_error() {
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(
        CONVEYOR_IN_SUBMODEL_XMILE.as_bytes(),
    ))
    .expect("parse conveyor-in-submodel xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // get_errors: a real error, with a message naming the stock and its
        // model, and NOT the internal un-expanded guard text.
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "out_error must be null");
        assert!(
            !all_errors.is_null(),
            "a submodel conveyor must report an error"
        );
        assert!(
            any_detail_message_contains(all_errors, "belt"),
            "the error must name the offending stock"
        );
        assert!(
            any_detail_message_contains(all_errors, "main model"),
            "the error must state the main-model-only limitation"
        );
        assert!(
            !any_detail_message_contains(all_errors, "un-expanded"),
            "the error must NOT be the engine-internal un-expanded guard text"
        );
        simlin_error_free(all_errors);

        // simlin_sim_new + run: the sim is created (a latent vm_error), and
        // running it surfaces the clear error -- never a panic or silent success.
        let mut merr: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut merr as *mut *mut SimlinError);
        assert!(merr.is_null(), "get_model must succeed");
        let mut serr: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut serr as *mut *mut SimlinError);
        assert!(!sim.is_null(), "sim handle is created");

        let mut rerr: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut rerr as *mut *mut SimlinError);
        assert!(
            !rerr.is_null(),
            "running a submodel-conveyor sim must surface an error"
        );
        let msg = CStr::from_ptr(simlin_error_get_message(rerr))
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            !msg.contains("un-expanded"),
            "run error must be the clear limitation, not the internal guard: {msg}"
        );
        simlin_error_free(rerr);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// After creating an LTM-enabled simulation on a model that auto-flips to
/// discovery mode, `simlin_project_get_errors` must surface the auto-flip
/// Warning. Before GH #466 the warning was unreachable: `simlin_sim_new`
/// resets `ltm_enabled` to false right after compile, so `get_errors`
/// collected diagnostics with LTM synthesis gated off.
#[test]
fn test_get_errors_surfaces_ltm_auto_flip_warning_after_ltm_sim() {
    let datamodel = build_chain_scc_datamodel("get_errors_auto_flip", 51);
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Sanity: before any LTM sim, get_errors must NOT surface the LTM
        // warning -- the project hasn't requested LTM, so it pays no LTM cost.
        let mut err: *mut SimlinError = ptr::null_mut();
        let pre = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        if !pre.is_null() {
            assert!(
                !any_detail_message_contains(pre, "discovery mode"),
                "LTM warning must be absent before any LTM sim is created"
            );
            simlin_error_free(pre);
        }

        let mut model_err: *mut SimlinError = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut model_err as *mut *mut SimlinError);
        assert!(model_err.is_null());
        assert!(!model.is_null());

        // Create an LTM-enabled sim; this is the point at which the project
        // records that LTM was requested.
        let mut sim_err: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut sim_err as *mut *mut SimlinError);
        assert!(sim_err.is_null(), "LTM sim creation should succeed");
        assert!(!sim.is_null());

        // Now the auto-flip warning must be reachable through get_errors.
        let mut err2: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err2 as *mut *mut SimlinError);
        assert!(err2.is_null());
        assert!(
            !all_errors.is_null(),
            "get_errors must return the auto-flip warning"
        );
        assert!(
            any_detail_message_contains(all_errors, "discovery mode"),
            "auto-flip warning ('discovery mode') must be reachable via get_errors after an LTM sim"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A project that never created an LTM-enabled simulation must not surface
/// any LTM diagnostics through `get_errors` -- and must not pay the LTM
/// synthesis cost. Preserves the original intent of
/// `test_ltm_disabled_gate_suppresses_auto_flip_warning` at the FFI level.
#[test]
fn test_get_errors_no_ltm_diagnostics_when_ltm_never_requested() {
    let datamodel = build_chain_scc_datamodel("get_errors_no_ltm", 51);
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Create a NON-LTM sim, then a plain get_errors.
        let mut model_err: *mut SimlinError = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut model_err as *mut *mut SimlinError);
        assert!(model_err.is_null());
        assert!(!model.is_null());
        let mut sim_err: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut sim_err as *mut *mut SimlinError);
        assert!(sim_err.is_null());
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // A well-formed model with no errors should return NULL; if anything
        // is returned it must not be an LTM diagnostic.
        if !all_errors.is_null() {
            assert!(
                !any_detail_message_contains(all_errors, "discovery mode"),
                "no LTM diagnostics when LTM was never requested"
            );
            simlin_error_free(all_errors);
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// GH #741: an LTM *fragment-compile-failure* warning must flow through the
/// same `LtmEnabledGuard` harvest as the auto-flip warning above -- it rides
/// the identical `model_all_diagnostics` -> `model_ltm_fragment_diagnostics`
/// accumulator, so `simlin_project_get_errors` surfaces it after an
/// LTM-enabled sim was created.
///
/// The fixture is a NON-PROJECTION-feeder co-source closure
/// (`growth[D1] = SUM(matrix[D1,*] * w[D1, c1])` with the feedback loop
/// closed through the wildcard-read co-source `matrix`): the Pinned-axis
/// no-`Reduced` source `w[D1, c1]` falls outside the GH #767 / T5
/// projection-feeder acceptance, so the reducer is not hoisted, the element
/// graph keeps the conservative cross-product, and the cross-row circuits'
/// loop scores reference per-`(row, slot)` link-score names the cartesian
/// emitters never produce. The engine catches that before fragment
/// compilation now, warning that the loop score was not emitted rather than
/// letting the fragment compiler create a zero stub (pinned engine-side by
/// `non_projection_feeder_co_source_closure_stays_loud`). The previous
/// fixtures here were organic fragment-failure shapes that have since been
/// fixed; this one keeps covering the FFI diagnostic harvest channel for the
/// current loud-safe degradation.
#[test]
fn test_get_errors_surfaces_ltm_fragment_failure_after_ltm_sim() {
    let datamodel = TestProject::new("get_errors_fragment_fail")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_stock("pop[D1]", "100", &["growth"], &[], None)
        .array_aux_direct(
            "matrix",
            vec!["D1".into(), "D2".into()],
            "pop[D1] * 0.05",
            None,
        )
        .array_aux_direct("w", vec!["D1".into(), "D2".into()], "0.5", None)
        .array_flow("growth[D1]", "SUM(matrix[D1, *] * w[D1, c1])", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut model_err: *mut SimlinError = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut model_err as *mut *mut SimlinError);
        assert!(model_err.is_null());
        assert!(!model.is_null());

        // Create an LTM-enabled sim so the project records the LTM request.
        let mut sim_err: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut sim_err as *mut *mut SimlinError);
        assert!(sim_err.is_null(), "LTM sim creation should succeed");
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(
            !all_errors.is_null(),
            "get_errors must return the fragment-failure warning"
        );
        assert!(
            any_detail_message_contains(all_errors, "was not emitted"),
            "the LTM missing-name loop-score warning must reach get_errors after an LTM sim"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The LTM-requested flag must survive subsequent non-LTM operations on the
/// project: after an LTM sim and then a plain non-LTM sim, the auto-flip
/// warning must still be reachable through `get_errors`.
#[test]
fn test_get_errors_ltm_warning_survives_subsequent_non_ltm_sim() {
    let datamodel = build_chain_scc_datamodel("get_errors_survives", 51);
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut model_err: *mut SimlinError = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut model_err as *mut *mut SimlinError);
        assert!(model_err.is_null());
        assert!(!model.is_null());

        // LTM sim first, sets the flag.
        let mut e1: *mut SimlinError = ptr::null_mut();
        let ltm_sim = simlin_sim_new(model, true, &mut e1 as *mut *mut SimlinError);
        assert!(e1.is_null());
        assert!(!ltm_sim.is_null());

        // Then a non-LTM sim, which resets the salsa flag to false as before.
        let mut e2: *mut SimlinError = ptr::null_mut();
        let plain_sim = simlin_sim_new(model, false, &mut e2 as *mut *mut SimlinError);
        assert!(e2.is_null());
        assert!(!plain_sim.is_null());

        // The warning must still be reachable: the project remembers that LTM
        // was requested at least once.
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(
            !all_errors.is_null(),
            "get_errors must still surface the LTM warning"
        );
        assert!(
            any_detail_message_contains(all_errors, "discovery mode"),
            "LTM warning must survive a subsequent non-LTM sim"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(plain_sim);
        simlin_sim_unref(ltm_sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Build a single-stock feedback-loop project (population/births/birth_rate)
/// with the requested integration method. Under RK4 an LTM-enabled compile is
/// rejected (the flow-to-stock link-score formula assumes Euler -- GH #486),
/// but the project itself simulates fine without LTM.
fn build_feedback_loop_datamodel(
    name: &str,
    method: engine::datamodel::SimMethod,
) -> engine::datamodel::Project {
    TestProject::new(name)
        .with_sim_method(method)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * birth_rate", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel()
}

/// GH #466 follow-up regression: a project that uses RK4 is intrinsically
/// valid -- it simulates fine without LTM. Creating an LTM sim on it (which the
/// GH #486 guard rejects at compile time) must NOT make `get_errors` report the
/// non-Euler rejection as a project error. LTM is an analysis overlay, not part
/// of the project's intrinsic compilability: `get_errors` assesses the
/// compile/vm_error channel with LTM OFF, and uses the latched re-enable only to
/// harvest the additional LTM diagnostics (which here are none, since the model
/// does not auto-flip and has no failing fragments).
#[test]
fn test_get_errors_rk4_ltm_compile_failure_is_not_a_project_error() {
    let datamodel =
        build_feedback_loop_datamodel("rk4_ltm_overlay", engine::datamodel::SimMethod::RungeKutta4);
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Baseline: a fresh RK4 model has no errors.
        let mut e0: *mut SimlinError = ptr::null_mut();
        let pre = simlin_project_get_errors(proj, &mut e0 as *mut *mut SimlinError);
        assert!(e0.is_null());
        assert!(
            pre.is_null(),
            "RK4 model must have no errors before any sim"
        );

        let mut me: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
        assert!(me.is_null());
        assert!(!model.is_null());

        // Create an LTM-enabled sim; the GH #486 rejection rides the compile/VM
        // path. The sim object is still created (the error defers to run time),
        // and the project's ltm_requested latch is now set.
        let mut se: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut se as *mut *mut SimlinError);
        if !se.is_null() {
            simlin_error_free(se);
        }

        // The regression: get_errors must still report the RK4 model as clean.
        // The non-Euler rejection is an LTM-overlay concern, not a project error.
        let mut e1: *mut SimlinError = ptr::null_mut();
        let post = simlin_project_get_errors(proj, &mut e1 as *mut *mut SimlinError);
        assert!(e1.is_null());
        assert!(
            post.is_null(),
            "RK4 model that simulates fine without LTM must not report errors \
             after a latched LTM sim; the non-Euler rejection is an analysis overlay"
        );

        if !sim.is_null() {
            simlin_sim_unref(sim);
        }
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// GH #466 follow-up (severity): the auto-flip advisory reaches `get_errors`
/// (the GH #466 fix), and it must carry `Warning` severity through the FFI, not
/// `Error`. A latched auto-flip project surfaces exactly one LTM diagnostic and
/// it must be a warning.
#[test]
fn test_get_errors_auto_flip_warning_has_warning_severity() {
    let datamodel = build_chain_scc_datamodel("get_errors_severity", 51);
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut me: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
        assert!(me.is_null());
        assert!(!model.is_null());

        let mut se: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut se as *mut *mut SimlinError);
        assert!(se.is_null());
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());

        let count = simlin_error_get_detail_count(all_errors);
        let details = simlin_error_get_details(all_errors);
        let slice = std::slice::from_raw_parts(details, count);
        let auto_flip = slice
            .iter()
            .find(|d| {
                !d.message.is_null()
                    && CStr::from_ptr(d.message)
                        .to_str()
                        .map(|m| m.contains("discovery mode"))
                        .unwrap_or(false)
            })
            .expect("auto-flip advisory must be present");
        assert_eq!(
            auto_flip.severity,
            SimlinErrorSeverity::Warning,
            "the auto-flip advisory must carry Warning severity through the FFI"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

// ---------------------------------------------------------------------------
// F13: enable_ltm on a conveyor/queue model must latch ltm_requested so the
// ConveyorLtmDegraded / QueueLtmDegraded warnings (docs/design/conveyors.md
// §9.6, queues.md §10.5) become reachable through simlin_project_get_errors.
//
// A conveyor/queue model compiles through the dedicated special-stock build
// path, which supplies the VM without LTM instrumentation -- LTM over a
// non-INTEG stock is a documented degradation. Before the fix that branch
// short-circuited the incremental-compile block that holds the sole
// ltm_requested latch, so enable_ltm=true was silently honored-as-disabled:
// the caller asked for analysis, got a sim with no LTM data, AND the degraded
// warning that exists precisely for these models never reached get_errors
// (ltm_requested stayed false, so the LtmEnabledGuard harvest never ran).
// ---------------------------------------------------------------------------

/// After an LTM-enabled sim is created on a conveyor model, `get_errors` must
/// surface the `ConveyorLtmDegraded` warning naming the conveyor stock. RED
/// before the latch fix: no diagnostic is reachable at all.
#[test]
fn test_get_errors_surfaces_conveyor_ltm_degraded_warning_after_ltm_sim() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Sanity: before any LTM sim, the degraded warning must be absent (the
        // project has not requested LTM, so it pays no LTM cost).
        let mut e0: *mut SimlinError = ptr::null_mut();
        let pre = simlin_project_get_errors(proj, &mut e0 as *mut *mut SimlinError);
        assert!(e0.is_null());
        if !pre.is_null() {
            assert!(
                !any_detail_message_contains(pre, "is degraded"),
                "the degraded warning must be absent before any LTM sim is created"
            );
            simlin_error_free(pre);
        }

        let mut me: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
        assert!(me.is_null());
        assert!(!model.is_null());

        // Create an LTM-enabled sim; this must latch the LTM request even
        // though the special build supplies a non-LTM VM.
        let mut se: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut se as *mut *mut SimlinError);
        assert!(se.is_null(), "LTM conveyor sim creation should succeed");
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(
            !all_errors.is_null(),
            "get_errors must return the conveyor LTM-degraded warning"
        );
        assert!(
            any_detail_message_contains(all_errors, "conveyor stock")
                && any_detail_message_contains(all_errors, "is degraded"),
            "the ConveyorLtmDegraded warning must be reachable via get_errors after an LTM sim"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The queue twin of the conveyor test above: after an LTM-enabled sim on a
/// queue model, `get_errors` must surface the `QueueLtmDegraded` warning.
#[test]
fn test_get_errors_surfaces_queue_ltm_degraded_warning_after_ltm_sim() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut me: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
        assert!(me.is_null());
        assert!(!model.is_null());

        let mut se: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut se as *mut *mut SimlinError);
        assert!(se.is_null(), "LTM queue sim creation should succeed");
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(
            !all_errors.is_null(),
            "get_errors must return the queue LTM-degraded warning"
        );
        assert!(
            any_detail_message_contains(all_errors, "queue stock")
                && any_detail_message_contains(all_errors, "is degraded"),
            "the QueueLtmDegraded warning must be reachable via get_errors after an LTM sim"
        );

        simlin_error_free(all_errors);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The latch must not fire spuriously: a conveyor sim created with
/// `enable_ltm = false` must NOT surface the degraded warning through
/// `get_errors` (the project never requested LTM, so it pays no LTM cost).
#[test]
fn test_get_errors_no_conveyor_ltm_degraded_when_ltm_not_requested() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut me: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
        assert!(me.is_null());
        assert!(!model.is_null());

        // A NON-LTM sim must leave ltm_requested false.
        let mut se: *mut SimlinError = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut se as *mut *mut SimlinError);
        assert!(se.is_null());
        assert!(!sim.is_null());

        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        if !all_errors.is_null() {
            assert!(
                !any_detail_message_contains(all_errors, "is degraded"),
                "the degraded warning must not surface when LTM was never requested"
            );
            simlin_error_free(all_errors);
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The honest LTM contract for conveyor/queue models: even with
/// `enable_ltm = true`, the sim is created WITHOUT LTM instrumentation (the
/// documented degradation), so `simlin_sim_get_ltm_mode` reports `Disabled`.
/// The latch fix makes the degraded *warning* reachable via get_errors, but it
/// must not fabricate an LTM mode. Pins the mode contract for both enable_ltm
/// values and both special-stock types; this holds before and after the fix.
#[test]
fn test_conveyor_and_queue_ltm_sim_report_disabled_ltm_mode() {
    for (xml, label) in [
        (
            include_str!("../../../../test/conveyors/minimal_conveyor.xmile") as &str,
            "conveyor",
        ),
        (
            include_str!("../../../../test/queues/queue_drain.xmile") as &str,
            "queue",
        ),
    ] {
        let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
            .unwrap_or_else(|_| panic!("parse {label} fixture"));
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut me: *mut SimlinError = ptr::null_mut();
            let model =
                simlin_project_get_model(proj, ptr::null(), &mut me as *mut *mut SimlinError);
            assert!(me.is_null());
            assert!(!model.is_null());

            for enable_ltm in [false, true] {
                let mut se: *mut SimlinError = ptr::null_mut();
                let sim = simlin_sim_new(model, enable_ltm, &mut se as *mut *mut SimlinError);
                assert!(se.is_null(), "{label} sim creation should succeed");
                assert!(!sim.is_null());

                let mut mode_err: *mut SimlinError = ptr::null_mut();
                let mode = simlin_sim_get_ltm_mode(sim, &mut mode_err as *mut *mut SimlinError);
                assert!(mode_err.is_null());
                assert_eq!(
                    mode,
                    SimlinLtmMode::Disabled,
                    "{label} model must report Disabled LTM mode (enable_ltm={enable_ltm}): LTM \
                     over a non-INTEG stock is a documented degradation, not an active mode"
                );

                simlin_sim_unref(sim);
            }

            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }
}
