// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::CStr;
use std::ptr;

use simlin::*;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

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
