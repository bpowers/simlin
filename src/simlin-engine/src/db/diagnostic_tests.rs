// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Error accumulator consolidation tests (Phase 3, Tasks 5-6).
//!
//! These tests verify the acceptance criteria for salsa-consolidation.AC2:
//! that all compilation error types are surfaced through the salsa
//! accumulator with specific error codes and correct severity levels.

use super::*;
use crate::datamodel;

// ---- Task 5: model_all_diagnostics triggers all sources ----

/// Task 5 verification: model_all_diagnostics triggers all accumulation
/// sources (parse errors, compilation errors, unit warnings). After calling
/// collect_all_diagnostics, we should see diagnostics from parse errors,
/// bad-table compilation errors, and unit check warnings -- all without
/// invoking compile_project_incremental.
#[test]
fn test_model_all_diagnostics_triggers_all_sources() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "all_sources".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("months".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // 1) Syntax error -> Equation diagnostic (Error severity)
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "broken_syntax".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // 2) Bad table: x_points length != y_points length
                //    -> compilation-level error accumulated by compile_var_fragment
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "bad_table_var".to_string(),
                    equation: datamodel::Equation::Scalar("bad_table_var".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 1.0]),
                        y_points: vec![0.0, 1.0, 2.0],
                        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    }),
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // 3) Unit mismatch: adding "people" + "months" -> Unit warning
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "pop".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: Some("people".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "dur".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: Some("months".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "unit_mismatch".to_string(),
                    equation: datamodel::Equation::Scalar("pop + dur".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    // Check for equation error from syntax error
    let has_equation_error = diags.iter().any(|d| {
        d.variable.as_deref() == Some("broken_syntax")
            && matches!(&d.error, DiagnosticError::Equation(_))
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_equation_error,
        "should have an equation error for 'broken_syntax'; got: {diags:?}"
    );

    // Check for BadTable compilation error from mismatched x/y points
    let has_bad_table = diags.iter().any(|d| {
        d.variable.as_deref() == Some("bad_table_var")
            && matches!(
                &d.error,
                DiagnosticError::Model(crate::common::Error {
                    code: crate::common::ErrorCode::BadTable,
                    ..
                })
            )
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_bad_table,
        "should have a BadTable error for 'bad_table_var'; got: {diags:?}"
    );

    // Check for unit-related warning. The unit inference failure surfaces
    // as a DiagnosticError::Model with ErrorCode::UnitMismatch at Warning
    // severity (model-level inference error). Per-variable unit checking
    // errors would surface as DiagnosticError::Unit.
    let has_unit_warning = diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Model(crate::common::Error {
                    code: crate::common::ErrorCode::UnitMismatch,
                    ..
                }) | DiagnosticError::Unit(_)
            )
    });
    assert!(
        has_unit_warning,
        "should have a unit warning for the unit mismatch; got: {diags:?}"
    );
}

// ---- Task 6: AC2 verification tests ----

/// AC2.1: Salsa accumulator error collection produces the expected
/// error codes for a project with various error types.
#[test]
fn test_ac2_1_accumulator_parity_with_old_path() {
    use crate::common::ErrorCode;
    use std::collections::HashSet;

    let project = datamodel::Project {
        name: "parity".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("months".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // Valid variable
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "good".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Syntax error
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "bad_syntax".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Empty equation
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "empty".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);
    let mut error_codes: HashSet<ErrorCode> = HashSet::new();
    for d in &diags {
        if d.severity == DiagnosticSeverity::Error {
            match &d.error {
                DiagnosticError::Equation(err) => {
                    error_codes.insert(err.code);
                }
                DiagnosticError::Model(err) => {
                    error_codes.insert(err.code);
                }
                _ => {}
            }
        }
    }

    // "bad_syntax" should produce a parse error and "empty" should produce
    // an empty equation error. Both are equation-level errors.
    assert!(
        !error_codes.is_empty(),
        "accumulator should produce errors for bad_syntax and empty variables"
    );
    assert!(
        error_codes.contains(&ErrorCode::EmptyEquation),
        "should contain EmptyEquation; got: {error_codes:?}"
    );
}

/// AC2.2: BadTable error from mismatched x/y table lengths surfaces
/// as a specific error code, not generic NotSimulatable.
#[test]
fn test_ac2_2_bad_table_specific_error() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "bad_table".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Scalar("lookup_var".to_string()),
                documentation: String::new(),
                units: None,
                gf: Some(datamodel::GraphicalFunction {
                    kind: datamodel::GraphicalFunctionKind::Continuous,
                    // Deliberately mismatched: 2 x-points but 3 y-points
                    x_points: Some(vec![0.0, 1.0]),
                    y_points: vec![0.0, 1.0, 2.0],
                    x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                }),
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let has_bad_table = diags.iter().any(|d| {
        d.variable.as_deref() == Some("lookup_var")
            && matches!(
                &d.error,
                DiagnosticError::Model(crate::common::Error {
                    code: crate::common::ErrorCode::BadTable,
                    ..
                })
            )
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_bad_table,
        "expected specific BadTable error code for mismatched x/y lengths, \
         not generic NotSimulatable; got: {diags:?}"
    );
}

/// AC2.3: EmptyEquation error for a stock with no equation surfaces
/// through the accumulator.
#[test]
fn test_ac2_3_empty_equation() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "empty_eq".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Stock(datamodel::Stock {
                ident: "my_stock".to_string(),
                equation: datamodel::Equation::Scalar(String::new()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let has_empty_equation = diags.iter().any(|d| {
        d.variable.as_deref() == Some("my_stock")
            && matches!(
                &d.error,
                DiagnosticError::Equation(crate::common::EquationError {
                    code: crate::common::ErrorCode::EmptyEquation,
                    ..
                })
            )
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_empty_equation,
        "expected EmptyEquation error code for stock with no equation; got: {diags:?}"
    );
}

/// AC2.4: MismatchedDimensions error for array variables with
/// incompatible dimensions surfaces through the accumulator.
#[test]
fn test_ac2_4_mismatched_dimensions() {
    let db = SimlinDb::default();
    // Two dimensions with different named elements but the same size.
    // Adding arrays subscripted to different dimensions should fail.
    let project = datamodel::Project {
        name: "dim_mismatch".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "Cities".to_string(),
                vec!["Boston".to_string(), "Seattle".to_string()],
            ),
            datamodel::Dimension::named(
                "Products".to_string(),
                vec!["Widgets".to_string(), "Gadgets".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "sales".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["Cities".to_string()],
                        "1".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "prices".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["Products".to_string()],
                        "1".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Adding Cities-dimensioned + Products-dimensioned should fail
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "bad".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["Cities".to_string()],
                        "sales + prices".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    // MismatchedDimensions can surface as either an EquationError
    // (from AST lowering in compile_var_fragment) or a Model error
    // (from the compiler context).
    let has_mismatch = diags.iter().any(|d| {
        d.model == "main"
            && d.severity == DiagnosticSeverity::Error
            && matches!(
                &d.error,
                DiagnosticError::Equation(crate::common::EquationError {
                    code: crate::common::ErrorCode::MismatchedDimensions,
                    ..
                }) | DiagnosticError::Model(crate::common::Error {
                    code: crate::common::ErrorCode::MismatchedDimensions,
                    ..
                })
            )
    });
    assert!(
        has_mismatch,
        "expected MismatchedDimensions error for incompatible array dimensions; got: {diags:?}"
    );
}

/// AC2.5: Unit warnings are accumulated with Warning severity, not
/// blocking Error severity.
#[test]
fn test_ac2_5_unit_warnings_severity() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "unit_warn".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("months".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // Two variables with incompatible units
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "people_count".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: Some("people".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "time_period".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: Some("months".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Adding people + months is a unit mismatch
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "unit_conflict".to_string(),
                    equation: datamodel::Equation::Scalar("people_count + time_period".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    // Unit issues should be present as warnings, not errors
    let unit_warnings: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Unit(_)
                        | DiagnosticError::Model(crate::common::Error {
                            code: crate::common::ErrorCode::UnitMismatch,
                            ..
                        })
                )
        })
        .collect();

    assert!(
        !unit_warnings.is_empty(),
        "expected at least one unit warning for people + months mismatch; got: {diags:?}"
    );

    // Verify none of the unit diagnostics have Error severity
    let unit_errors: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Error && matches!(&d.error, DiagnosticError::Unit(_))
        })
        .collect();

    assert!(
        unit_errors.is_empty(),
        "unit diagnostics should have Warning severity, not Error; got errors: {unit_errors:?}"
    );
}

/// AC2.7: VM bytecode validation errors (Vm::new failures) are
/// detected during compilation. This verifies that the error path
/// exists and is exercised when sim spec validation fails.
#[test]
fn test_ac2_7_vm_validation_errors() {
    // Construct a project with invalid sim specs (stop < start).
    // compile_project_incremental should succeed (it produces bytecode),
    // but Vm::new should reject it with BadSimSpecs.
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "bad_specs".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 10.0,
            stop: 0.0, // stop < start -> Vm::new rejects
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);

    // compile_project_incremental should produce valid compiled output
    let compiled = compile_project_incremental(&db, sync.project, "main");
    assert!(
        compiled.is_ok(),
        "compilation should succeed even with bad sim specs; \
         error: {compiled:?}"
    );

    // But Vm::new should fail with BadSimSpecs
    let compiled = compiled.unwrap();
    let vm_result = crate::vm::Vm::new(compiled);
    assert!(
        vm_result.is_err(),
        "Vm::new should reject simulation with stop < start"
    );

    let err = vm_result.unwrap_err();
    assert_eq!(
        err.code,
        crate::common::ErrorCode::BadSimSpecs,
        "Vm::new should report BadSimSpecs, got: {err:?}"
    );

    // Verify that apply_patch-level consumers would detect this:
    // The error from Vm::new is detectable and would cause patch rejection.
    // This proves the error path exists for AC2.7.
}

/// AC2.7 supplemental: Verify that assembly-level errors from
/// compile_project_incremental (circular deps, missing models) are
/// both returned as Err and accumulated as diagnostics.
#[test]
fn test_ac2_7_assembly_errors_accumulated() {
    let db = SimlinDb::default();
    // Create a project with circular dependencies between auxiliaries
    let project = datamodel::Project {
        name: "circular".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("a".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);

    // compile_project_incremental should fail due to circular deps
    let result = compile_project_incremental(&db, sync.project, "main");
    assert!(
        result.is_err(),
        "compilation should fail for circular dependencies"
    );

    // The assembly-stage `Assembly` error surfaces only via `assemble_module`'s
    // `Result::Err` (the aggregate string), not the diagnostic accumulator.
    // The per-variable/model diagnostics from `model_all_diagnostics` DO capture
    // the circular dependency detected by `model_dependency_graph`, so that is
    // what `collect_all_diagnostics` returns here.
    let diags = collect_all_diagnostics(&db, sync.project);
    let has_circular = diags.iter().any(|d| {
        matches!(
            &d.error,
            DiagnosticError::Model(crate::common::Error {
                code: crate::common::ErrorCode::CircularDependency,
                ..
            })
        )
    });
    assert!(
        has_circular,
        "accumulator should contain CircularDependency diagnostic; got: {diags:?}"
    );
}

// ---- compile_var_fragment per-site diagnostic behavior pins ----
//
// `compile_var_fragment` accumulates diagnostics at six distinct sites
// while lowering a single variable to per-phase bytecode. Three of those
// sites already have dedicated coverage elsewhere in this file
// (parse-error via `broken_syntax` in
// `test_model_all_diagnostics_triggers_all_sources`; lowering-error via
// `test_ac2_4_mismatched_dimensions`; table-build-error via
// `test_ac2_2_bad_table_specific_error`). The fixtures below pin the
// remaining sites so that any refactor of the lowering path is provably
// diagnostic-behavior-preserving: the malformed-unit-string site (a
// non-fatal accumulate that does NOT early-return), the
// unknown-dependency site reached from the dependency walk (a fatal
// accumulate-then-return), and the per-phase `Var::new` failure site (a
// per-phase failure where the function still returns a fragment for the
// phases that did compile).

/// A syntactically malformed unit string surfaces as a `Unit`
/// diagnostic at Error severity. This is a *unit-string parse* failure
/// (stored on the parsed variable's `unit_errors`), distinct from the
/// unit-*checking* dimensional mismatches in
/// `test_ac2_5_unit_warnings_severity`, which are Warnings. The variable
/// is otherwise well-formed, so this site accumulates without aborting
/// the rest of compilation for that variable.
#[test]
fn test_compile_var_fragment_malformed_unit_string() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "malformed_unit".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "bad_unit_var".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: Some("bad units here!!!".to_string()),
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let has_unit_error = diags.iter().any(|d| {
        d.variable.as_deref() == Some("bad_unit_var")
            && matches!(&d.error, DiagnosticError::Unit(_))
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_unit_error,
        "expected a Unit syntax error at Error severity for 'bad_unit_var'; got: {diags:?}"
    );
}

/// A reference to a name that is neither a declared variable nor an
/// implicit/module variable surfaces as an `UnknownDependency`
/// equation error. This site is reached from the dependency walk and
/// aborts compilation of the referencing variable.
#[test]
fn test_compile_var_fragment_unknown_dependency() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "unknown_dep".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("undefined_var".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let has_unknown_dep = diags.iter().any(|d| {
        d.variable.as_deref() == Some("x")
            && matches!(
                &d.error,
                DiagnosticError::Equation(crate::common::EquationError {
                    code: crate::common::ErrorCode::UnknownDependency,
                    ..
                })
            )
    });
    assert!(
        has_unknown_dep,
        "expected UnknownDependency for 'x' referencing undefined_var; got: {diags:?}"
    );
}

/// A scalar variable wildcard-subscripted as if it were an array
/// (`SUM(x[*])` where `x` is scalar) parses, lowers, resolves its
/// dependency (`x`), and builds no tables -- all the whole-variable
/// fatal gates pass -- yet `Var::new` fails while resolving the
/// subscript for the phase being compiled. That per-phase failure is
/// recorded as an `Equation` diagnostic and the failing phase's bytecode
/// is dropped, but the function still returns a fragment (it does not
/// abort the whole variable the way the parse / lowering /
/// unknown-dependency / table-build sites do). The per-phase accumulate
/// site stamps `start = end = 0` (it has no AST span for the failure),
/// which distinguishes it from the span-carrying lowering and
/// unknown-dependency diagnostics.
#[test]
fn test_compile_var_fragment_per_phase_var_new_failure() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "per_phase_failure".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("SUM(x[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let y_var = sync.models["main"].variables["y"].source;

    // The function still returns a fragment: the per-phase `Var::new`
    // failure drops only the failing phase's bytecode, it does not abort
    // the whole variable (unlike the parse / lowering / unknown-dependency
    // / table-build sites, which return `None`).
    let frag = compile_var_fragment(&db, y_var, model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        frag.is_some(),
        "per-phase Var::new failure must still return a fragment (not whole-variable None)"
    );

    let diags = collect_all_diagnostics(&db, sync.project);
    let has_per_phase_failure = diags.iter().any(|d| {
        d.variable.as_deref() == Some("y")
            && d.severity == DiagnosticSeverity::Error
            && matches!(
                &d.error,
                DiagnosticError::Equation(crate::common::EquationError {
                    start: 0,
                    end: 0,
                    ..
                })
            )
    });
    assert!(
        has_per_phase_failure,
        "expected a per-phase Var::new Equation diagnostic (span 0..0) for 'y'; got: {diags:?}"
    );
}

// ---- diagnostics stable across unrelated salsa input changes ----
//
// `collect_all_diagnostics` / `collect_model_diagnostics` drain the salsa
// `CompilationDiagnostic` accumulator via
// `model_all_diagnostics::accumulated::<_>(..)`. salsa 0.26's
// `accumulated_by` does a DFS that prunes any subtree whose root memo's
// `accumulated_inputs` flag is `Empty`. When `model_all_diagnostics` is
// validated-but-not-re-executed after an unrelated salsa revision bump, the
// deep-verify path (`deep_verify_edges`) recomputes that pruning flag from
// each input's `maybe_changed_after` result -- and a self-accumulating input
// (`check_model_units`) reports `Empty` there because `accumulated_inputs`
// only reflects an input's *inputs*, never whether the input itself
// accumulated. The flag therefore collapses to `Empty`, the DFS prunes the
// whole subtree, and previously-collected diagnostics silently vanish on the
// next collection. These tests pin the desired behavior: the collected set
// must be byte-stable across changes to unrelated inputs.

/// Build a one-model project whose `unit_conflict` aux adds `people` to
/// `months`, producing at least one Warning-severity unit diagnostic. The
/// model carries no pinned loops, so the regression tests can flip
/// `pinned_loops` (an input that does not feed unit checking) without
/// touching any unit-relevant input.
fn unit_warning_fixture() -> datamodel::Project {
    datamodel::Project {
        name: "stable_diag".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("months".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "people_count".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: Some("people".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "time_period".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: Some("months".to_string()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "unit_conflict".to_string(),
                    equation: datamodel::Equation::Scalar("people_count + time_period".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Count the Warning-severity unit diagnostics in a collected set, the
/// quantity the libsimlin patch-validation baseline keys on.
fn count_unit_warnings(diags: &[Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Unit(_)
                        | DiagnosticError::Model(crate::common::Error {
                            code: crate::common::ErrorCode::UnitMismatch,
                            ..
                        })
                )
        })
        .count()
}

/// Flipping `pinned_loops` (an input read by the LTM pipeline, never by unit
/// checking) bumps the salsa revision. `model_all_diagnostics` then validates
/// without re-executing, and the unit warnings must survive the next
/// `collect_all_diagnostics`. Before the fix the second collection returned 0.
#[test]
fn test_diagnostics_stable_across_unrelated_input_change() {
    use crate::db::PinnedLoopSpec;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = unit_warning_fixture();
    let sync = sync_from_datamodel(&db, &project);

    let before = collect_all_diagnostics(&db, sync.project);
    let n_before = count_unit_warnings(&before);
    assert!(
        n_before > 0,
        "fixture must produce at least one unit warning; got: {before:?}"
    );

    // Mutate an input that has nothing to do with unit checking: set a pinned
    // loop on the model. This bumps the salsa revision without touching any
    // input `check_model_units` reads.
    let source_model = sync.models["main"].source;
    source_model
        .set_pinned_loops(&mut db)
        .to(vec![PinnedLoopSpec {
            name: "dummy_loop".to_string(),
            variables: vec![],
            description: String::new(),
        }]);

    let after = collect_all_diagnostics(&db, sync.project);
    let n_after = count_unit_warnings(&after);
    assert_eq!(
        n_after, n_before,
        "unit warnings must be stable across an unrelated salsa input change; \
         before={n_before}, after={n_after}; after diags: {after:?}"
    );
    assert_eq!(
        before, after,
        "the full diagnostic set must be identical across an unrelated input change"
    );
}

// ---- F15: pass-driven flows are permitted an empty equation ----
//
// A conveyor stock's primary/leak outflow and any queue stock's outflow are
// DRIVEN by the native expansion pass (conveyor_compile / queue_compile),
// which writes their slot each step (the expansion gives each such flow a
// placeholder `0` equation). By XMILE design such a flow carries no <eqn>. The
// salsa diagnostic path runs over the UN-expanded datamodel, so without a
// marker-aware guard every driven flow was reported as an EmptyEquation Error
// on a model that simulates correctly (F15). These tests pin that the guard
// (a) suppresses EmptyEquation for a conveyor/queue driven flow, (b) leaves it
// intact for an ordinary empty-equation variable, and (c) re-emits it when the
// special-stock marker is removed (salsa invalidation of the flow's fragment,
// which read the owning stock's compat).

/// A minimal `<conveyor>` block (transit time only) for the F15 fixtures.
fn f15_conveyor_block() -> datamodel::Conveyor {
    datamodel::Conveyor {
        transit_time: "4".to_string(),
        capacity: None,
        inflow_limit: None,
        sample: None,
        arrest: None,
        discrete: false,
        batch_integrity: false,
        one_at_a_time: true,
        exponential_leak: false,
        ignore_earlier_zone_losses: false,
    }
}

/// Whether the collected set carries an `EmptyEquation` Error for `name`.
fn has_empty_equation(diags: &[Diagnostic], name: &str) -> bool {
    diags.iter().any(|d| {
        d.variable.as_deref() == Some(name)
            && d.severity == DiagnosticSeverity::Error
            && matches!(
                &d.error,
                DiagnosticError::Equation(crate::common::EquationError {
                    code: crate::common::ErrorCode::EmptyEquation,
                    ..
                })
            )
    })
}

/// A one-model project with a conveyor stock `belt` whose primary outflow
/// `out_f` and leak outflow `leak_f` both carry NO equation (the conveyor pass
/// drives them), plus an inflow `in_f` with a real equation and an ordinary
/// empty-equation aux `orphan` as the non-driven control.
fn f15_conveyor_project() -> datamodel::Project {
    datamodel::Project {
        name: "conveyor_driven".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "belt".to_string(),
                    equation: datamodel::Equation::Scalar("1000".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["in_f".to_string()],
                    outflows: vec!["out_f".to_string(), "leak_f".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        conveyor: Some(f15_conveyor_block()),
                        ..Default::default()
                    },
                }),
                // Primary outflow: NO equation (the conveyor pass drives it).
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "out_f".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Leak outflow: NO equation, leak-marked (also pass-driven).
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "leak_f".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        leakage: Some(datamodel::Leakage {
                            fraction: Some("0.1".to_string()),
                            integers: false,
                            zone_start: None,
                            zone_end: None,
                        }),
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "in_f".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // PRECISION CONTROL: a PLAIN INTEG stock coexisting in the same
                // model, whose empty-equation outflow `tank_out` is NOT
                // pass-driven. It must still be an EmptyEquation Error -- the
                // guard against a future over-broad "the model contains any
                // special stock" rewrite of `flow_is_special_stock_driven` (the
                // `orphan` aux only exercises the kind==Flow gate, not the
                // owning-stock-is-special gate).
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "tank".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["tank_out".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "tank_out".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // CONTROL: an ordinary aux with an empty equation is NOT
                // pass-driven, so it must still be an EmptyEquation Error.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "orphan".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// A conveyor stock's primary outflow and leak outflow are pass-driven, so an
/// empty equation on them is spec-sanctioned and must NOT surface as an
/// EmptyEquation error -- while an ordinary empty-equation aux in the same
/// model still does.
#[test]
fn test_conveyor_driven_flow_empty_equation_suppressed() {
    let db = SimlinDb::default();
    let project = f15_conveyor_project();

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        !has_empty_equation(&diags, "out_f"),
        "conveyor primary outflow must not get a phantom empty_equation; got: {diags:?}"
    );
    assert!(
        !has_empty_equation(&diags, "leak_f"),
        "conveyor leak outflow must not get a phantom empty_equation; got: {diags:?}"
    );
    assert!(
        has_empty_equation(&diags, "tank_out"),
        "an empty-equation outflow of a PLAIN stock coexisting in the same conveyor \
         model must still be an EmptyEquation error; got: {diags:?}"
    );
    assert!(
        has_empty_equation(&diags, "orphan"),
        "an ordinary empty-equation aux must still be an EmptyEquation error; got: {diags:?}"
    );
}

/// Scope guard: the suppression is for the empty-equation code ONLY. A
/// pass-driven flow that carries a NON-empty but malformed equation (`1 +`)
/// still surfaces its parse error -- the expansion pass overwrites the slot,
/// but a syntactically broken equation is a genuine modeling error the
/// diagnostics must report, not silently swallow. (Correct by construction --
/// only `EmptyEquation` is filtered -- but unpinned before this.)
#[test]
fn test_conveyor_driven_flow_malformed_equation_still_errors() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "conveyor_malformed".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "belt".to_string(),
                    equation: datamodel::Equation::Scalar("1000".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["in_f".to_string()],
                    outflows: vec!["out_f".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        conveyor: Some(f15_conveyor_block()),
                        ..Default::default()
                    },
                }),
                // Driven outflow with a NON-empty, malformed equation.
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "out_f".to_string(),
                    equation: datamodel::Equation::Scalar("1 +".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "in_f".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    // The malformed equation must NOT be mislabeled as -- or swallowed like --
    // an empty equation: it surfaces as a non-EmptyEquation parse error.
    assert!(
        !has_empty_equation(&diags, "out_f"),
        "a malformed non-empty equation is not an empty equation; got: {diags:?}"
    );
    let has_parse_error = diags.iter().any(|d| {
        d.variable.as_deref() == Some("out_f")
            && d.severity == DiagnosticSeverity::Error
            && matches!(
                &d.error,
                DiagnosticError::Equation(e) if e.code != crate::common::ErrorCode::EmptyEquation
            )
    });
    assert!(
        has_parse_error,
        "a conveyor-driven outflow with a malformed equation must still report its \
         parse error; got: {diags:?}"
    );
}

/// Every outflow of a queue stock is pass-driven (queue_compile writes each
/// served rate), so an empty-equation queue outflow is spec-sanctioned. The
/// committed queue fixtures happen to use `<eqn>0</eqn>` placeholders, so this
/// fixture uses genuinely empty outflow equations to exercise the queue branch
/// of the guard.
#[test]
fn test_queue_driven_outflow_empty_equation_suppressed() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "queue_driven".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "waiting".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["arrivals".to_string()],
                    outflows: vec!["into_service".to_string(), "balk".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        queue: Some(datamodel::Queue {}),
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "arrivals".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Primary outflow: NO equation (the queue pass drives it).
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "into_service".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Overflow outflow: NO equation, also pass-driven.
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "balk".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        overflow: true,
                        ..Default::default()
                    },
                }),
                // CONTROL: an ordinary empty-equation aux still errors.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "orphan".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        !has_empty_equation(&diags, "into_service"),
        "queue primary outflow must not get a phantom empty_equation; got: {diags:?}"
    );
    assert!(
        !has_empty_equation(&diags, "balk"),
        "queue overflow outflow must not get a phantom empty_equation; got: {diags:?}"
    );
    assert!(
        has_empty_equation(&diags, "orphan"),
        "an ordinary empty-equation aux must still be an EmptyEquation error; got: {diags:?}"
    );
}

/// The suppression is marker-driven: removing the `<conveyor>` block makes
/// `belt` an ordinary INTEG stock, so its outflow `out_f` is no longer
/// pass-driven and its missing equation becomes a genuine EmptyEquation error.
/// Salsa must invalidate `out_f`'s fragment (it read `belt.compat`) and re-emit
/// the diagnostic on the incremental re-sync.
#[test]
fn test_conveyor_marker_removal_reinstates_empty_equation() {
    let mut db = SimlinDb::default();
    let project = f15_conveyor_project();

    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let before = collect_all_diagnostics(&db, state.project);
    assert!(
        !has_empty_equation(&before, "out_f"),
        "with the <conveyor> marker present, the driven outflow has no empty_equation; \
         got: {before:?}"
    );

    let mut changed = project.clone();
    if let datamodel::Variable::Stock(s) = &mut changed.models[0].variables[0] {
        s.compat.conveyor = None;
    } else {
        panic!("fixture's first variable must be the conveyor stock");
    }
    let state = sync_from_datamodel_incremental(&mut db, &changed, Some(&state));
    let after = collect_all_diagnostics(&db, state.project);
    assert!(
        has_empty_equation(&after, "out_f"),
        "removing the <conveyor> marker must reinstate the empty_equation error on out_f; \
         got: {after:?}"
    );
}

/// The same invariant via the production incremental-sync path, driving the
/// exact input the `SetLoopName` patch touches: add a `loop_metadata` entry
/// (which re-syncs `pinned_loops` and bumps the revision) and re-collect. The
/// unit-check subtree reads no pinned-loop input, so `model_all_diagnostics`
/// validates without re-executing -- the precise scenario where the salsa
/// pruning bug made the pre-existing unit warnings vanish (libsimlin symptom:
/// a `SetLoopName` patch silently zeroed `get_errors`). The pre-existing
/// warnings must still be reported afterward.
#[test]
fn test_diagnostics_stable_across_incremental_loop_metadata_change() {
    let mut db = SimlinDb::default();
    let project = unit_warning_fixture();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);

    let before = collect_all_diagnostics(&db, state.project);
    let n_before = count_unit_warnings(&before);
    assert!(
        n_before > 0,
        "fixture must produce at least one unit warning; got: {before:?}"
    );

    // Pin a loop on the model. `pinned_loops_from_datamodel` resolves the
    // loop_metadata's variable UIDs at sync time; an empty stock-free loop is
    // fine here -- the LTM pipeline ignores it and we only need the unrelated
    // input write to bump the salsa revision (the production `SetLoopName`
    // path writes the same `SourceModel.pinned_loops` input).
    let mut changed = project.clone();
    changed.models[0]
        .loop_metadata
        .push(datamodel::LoopMetadata {
            name: "dummy_loop".to_string(),
            uids: vec![],
            deleted: false,
            description: String::new(),
        });
    let state = sync_from_datamodel_incremental(&mut db, &changed, Some(&state));

    let after = collect_all_diagnostics(&db, state.project);
    let n_after = count_unit_warnings(&after);
    assert_eq!(
        n_after, n_before,
        "unit warnings must survive an incremental loop_metadata re-sync; \
         before={n_before}, after={n_after}; after diags: {after:?}"
    );
}

// ---- Conveyor compile-time spec advisories (conveyors.md §4.1 / §5.1) ----
//
// `ConveyorTransitNotDtMultiple` and `ConveyorLeakFractionsExceedOne` are the
// two Warning-level compile diagnostics docs/design/conveyors.md mandates
// (§9.8 table). The special conveyor/queue build path
// (`queue_compile::build_compiled`) returns a single hard `Err` with no
// warnings channel, so -- like the LTM-degraded twins -- they are emitted from
// the per-model `model_all_diagnostics` trigger, which sees the UN-expanded
// conveyor via `compat.conveyor`, and reach `collect_all_diagnostics` /
// `simlin_project_get_errors`. Unlike the LTM twins they are NOT gated on
// `ltm_enabled`: they describe the simulation itself (GH #873).

/// A one-model project with a conveyor stock `belt` (transit expression
/// `transit`, linear or exponential leakage per `exponential_leak`) and one
/// leak-marked outflow per entry of `leak_fractions`, under project dt `dt`.
/// `extra_aux` adds a plain constant aux for the non-constant-expression
/// fixtures to reference.
fn conveyor_spec_project(
    transit: &str,
    dt: datamodel::Dt,
    exponential_leak: bool,
    leak_fractions: &[&str],
    extra_aux: Option<(&str, &str)>,
) -> datamodel::Project {
    let empty_flow = |ident: &str, eqn: &str, compat: datamodel::Compat| {
        datamodel::Variable::Flow(datamodel::Flow {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat,
        })
    };

    let mut outflows = vec!["out_f".to_string()];
    for i in 0..leak_fractions.len() {
        outflows.push(format!("leak_{i}"));
    }

    let mut variables = vec![
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "belt".to_string(),
            equation: datamodel::Equation::Scalar("1000".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec!["in_f".to_string()],
            outflows,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                conveyor: Some(datamodel::Conveyor {
                    transit_time: transit.to_string(),
                    capacity: None,
                    inflow_limit: None,
                    sample: None,
                    arrest: None,
                    discrete: false,
                    batch_integrity: false,
                    one_at_a_time: true,
                    exponential_leak,
                    ignore_earlier_zone_losses: false,
                }),
                ..Default::default()
            },
        }),
        // Primary outflow: pass-driven, no equation.
        empty_flow("out_f", "", datamodel::Compat::default()),
        empty_flow("in_f", "10", datamodel::Compat::default()),
    ];
    for (i, frac) in leak_fractions.iter().enumerate() {
        variables.push(empty_flow(
            &format!("leak_{i}"),
            "",
            datamodel::Compat {
                leakage: Some(datamodel::Leakage {
                    fraction: Some(frac.to_string()),
                    integers: false,
                    zone_start: None,
                    zone_end: None,
                }),
                ..Default::default()
            },
        ));
    }
    if let Some((name, eqn)) = extra_aux {
        variables.push(datamodel::Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: datamodel::Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    }

    datamodel::Project {
        name: "conveyor_spec".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 12.0,
            dt,
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// The Warning-severity `Model` diagnostics carrying `code`, in accumulation
/// order.
fn conveyor_spec_warnings(diags: &[Diagnostic], code: crate::common::ErrorCode) -> Vec<Diagnostic> {
    diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(&d.error, DiagnosticError::Model(err) if err.code == code)
        })
        .cloned()
        .collect()
}

/// The bare details string of a `Model` diagnostic.
fn model_diag_details(d: &Diagnostic) -> String {
    match &d.error {
        DiagnosticError::Model(err) => err.get_details().unwrap_or_default(),
        other => panic!("expected a Model diagnostic, got {other:?}"),
    }
}

/// §4.1: a compile-time-constant transit time that is not an integer multiple
/// of dt warns once, naming the conveyor and reporting the effective
/// (DT-quantized) transit time. 1.3 at dt 0.25 quantizes to 5 slats
/// (round-half-away), an effective transit of 1.25.
#[test]
fn test_conveyor_transit_not_dt_multiple_warns() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project("1.3", datamodel::Dt::Dt(0.25), false, &[], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorTransitNotDtMultiple);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one transit-quantization warning; got: {diags:?}"
    );
    let w = &warnings[0];
    assert_eq!(
        w.variable.as_deref(),
        Some("belt"),
        "the warning must be attributed to the conveyor stock"
    );
    let details = model_diag_details(w);
    assert!(
        details.contains("belt"),
        "the message must name the conveyor: {details}"
    );
    assert!(
        details.contains("1.25"),
        "the message must report the effective transit time 1.25: {details}"
    );
}

/// §4.1 negative: an exact integer multiple (4 at dt 0.25 -> 16 slats) emits
/// no quantization warning.
#[test]
fn test_conveyor_transit_dt_multiple_no_warning() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project("4", datamodel::Dt::Dt(0.25), false, &[], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        conveyor_spec_warnings(&diags, ErrorCode::ConveyorTransitNotDtMultiple).is_empty(),
        "an exact dt multiple must not warn; got: {diags:?}"
    );
}

/// §4.1 non-constant: a `<len>` expression that is not a compile-time
/// constant (here a variable reference) gets no warning -- its value is only
/// known at runtime, and a false positive is worse than silence.
#[test]
fn test_conveyor_transit_non_constant_no_warning() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project(
        "t_len",
        datamodel::Dt::Dt(0.25),
        false,
        &[],
        Some(("t_len", "1.3")),
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        conveyor_spec_warnings(&diags, ErrorCode::ConveyorTransitNotDtMultiple).is_empty(),
        "a non-constant transit expression must not warn; got: {diags:?}"
    );
}

/// §4.1 dt resolution: a model-level `sim_specs` override wins over the
/// project specs, mirroring `assemble_simulation`'s root rule. Transit 1.5 is
/// an exact multiple of the project dt 0.25 (no warning there) but not of the
/// model-level dt 1 -- and 1.5 rounds half-away UP to 2 slats, an effective
/// transit of 2.
#[test]
fn test_conveyor_transit_warning_uses_model_sim_specs_override() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let mut project = conveyor_spec_project("1.5", datamodel::Dt::Dt(0.25), false, &[], None);
    project.models[0].sim_specs = Some(datamodel::SimSpecs {
        start: 0.0,
        stop: 12.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    });
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorTransitNotDtMultiple);
    assert_eq!(
        warnings.len(),
        1,
        "the model-level dt override must drive the check; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("effective transit time of 2"),
        "1.5 at dt 1 must quantize half-away to 2 slats (effective 2): {details}"
    );
}

/// §5.1: constant LINEAR leak fractions summing above 1 (0.7 + 0.5 = 1.2)
/// warn once per conveyor, naming the stock and reporting the sum.
#[test]
fn test_conveyor_leak_fractions_exceed_one_warns() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project("4", datamodel::Dt::Dt(0.25), false, &["0.7", "0.5"], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one leak-fraction-sum warning; got: {diags:?}"
    );
    let w = &warnings[0];
    assert_eq!(
        w.variable.as_deref(),
        Some("belt"),
        "the warning must be attributed to the conveyor stock"
    );
    let details = model_diag_details(w);
    assert!(
        details.contains("belt") && details.contains("1.2"),
        "the message must name the conveyor and report the sum 1.2: {details}"
    );
}

/// §3.3's OTHER leak encoding -- the one real Stella files use: a bare
/// `<leak/>` marker with the fraction in the flow's own `<eqn>` (the XMILE
/// reader leaves `Leakage.fraction` None; the runtime's
/// `leak_fraction_equation` falls back to the flow equation). The §5.1 sum
/// must resolve fractions in the same order the runtime does, or this
/// encoding silently escapes the check.
#[test]
fn test_conveyor_leak_marker_eqn_form_warns() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let mut project =
        conveyor_spec_project("4", datamodel::Dt::Dt(0.25), false, &["0.7", "0.5"], None);
    // Re-encode both leaks in the marker+<eqn> form: move each fraction into
    // the flow's own equation and leave the `<leak/>` marker bare.
    for var in &mut project.models[0].variables {
        if let datamodel::Variable::Flow(f) = var
            && let Some(leak) = &mut f.compat.leakage
        {
            f.equation =
                datamodel::Equation::Scalar(leak.fraction.take().expect("fixture fraction"));
        }
    }
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne);
    assert_eq!(
        warnings.len(),
        1,
        "the marker+eqn leak encoding must be summed exactly like the explicit \
         <leak> fraction form; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("belt") && details.contains("1.2"),
        "the message must name the conveyor and report the sum 1.2: {details}"
    );
}

/// §5.1 runtime mirror: the runtime clamps each LINEAR fraction to [0, 1]
/// (`clamp_fraction`), so a negative constant contributes 0 leakage -- it
/// must not be allowed to cancel other fractions out of the compile-time
/// sum. 0.7 + 0.5 + (-0.5) sums raw to 0.7 (silent) but leaks 1.2 at
/// runtime: the per-term clamp must warn.
#[test]
fn test_conveyor_leak_negative_fraction_clamped_like_runtime() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project(
        "4",
        datamodel::Dt::Dt(0.25),
        false,
        &["0.7", "0.5", "-0.5"],
        None,
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne);
    assert_eq!(
        warnings.len(),
        1,
        "a negative constant fraction must clamp to 0 (runtime behavior), not \
         cancel the others out of the sum; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("1.2"),
        "the reported sum must be the runtime-clamped 1.2: {details}"
    );
}

/// §5.1 NaN hygiene: `nan` is a reserved lexer keyword parsing to a NaN
/// constant, and `f64::clamp` PROPAGATES NaN -- an unguarded per-term clamp
/// would turn the whole sum NaN and suppress the warning for the entire
/// conveyor (`NaN > 1 + tol` is false), while the runtime's `clamp_fraction`
/// maps NaN to 0 and genuinely leaks the other 1.2. The NaN term must
/// contribute 0, exactly like the runtime.
#[test]
fn test_conveyor_leak_nan_fraction_contributes_zero() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project(
        "4",
        datamodel::Dt::Dt(0.25),
        false,
        &["0.7", "0.5", "nan"],
        None,
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne);
    assert_eq!(
        warnings.len(),
        1,
        "a NaN constant fraction must contribute 0 (runtime clamp_fraction \
         behavior), not poison the sum into silence; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("1.2"),
        "the reported sum must be the NaN-excluded 1.2: {details}"
    );
}

/// Display disambiguation: for a transit within ~5e-5 of dt, the trimmed
/// 4-decimal display renders transit, dt, and effective transit as the SAME
/// string ("transit time 0.3333 is not an integer multiple of dt 0.3333 ...
/// effective transit time of 0.3333" -- self-contradictory). When the trimmed
/// transit collides with the trimmed dt or effective value, all three fall
/// back to the full round-trip form so the reader can see why the warning
/// fired. Also pins the singular "1 slat" grammar.
#[test]
fn test_conveyor_transit_warning_display_disambiguates_near_dt() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project =
        conveyor_spec_project("0.33333", datamodel::Dt::Reciprocal(3.0), false, &[], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorTransitNotDtMultiple);
    assert_eq!(
        warnings.len(),
        1,
        "0.33333 at dt 1/3 (ratio 0.99999) must warn; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("transit time 0.33333 ") && details.contains("dt 0.3333333333333333"),
        "colliding trimmed displays must fall back to full round-trip forms \
         so transit and dt are distinguishable: {details}"
    );
    assert!(
        details.contains("1 slat,"),
        "a single-slat belt must read '1 slat', not '1 slats': {details}"
    );
}

/// Cosmetic: the reported sum is trimmed to a sensible precision. The f64
/// sequential sum 0.7 + 0.2 + 0.4 is 1.2999999999999998; the message must
/// read "1.3", not the full shortest-round-trip tail.
#[test]
fn test_conveyor_leak_fraction_sum_display_is_trimmed() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project(
        "4",
        datamodel::Dt::Dt(0.25),
        false,
        &["0.7", "0.2", "0.4"],
        None,
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne);
    assert_eq!(
        warnings.len(),
        1,
        "0.7 + 0.2 + 0.4 sums above 1 and must warn; got: {diags:?}"
    );
    let details = model_diag_details(&warnings[0]);
    assert!(
        details.contains("sum to 1.3,") && !details.contains("1.2999999999999998"),
        "the sum must display trimmed (1.3), not with the f64 round-trip tail: {details}"
    );
}

/// §5.1 negative: fractions summing to exactly 1 are legal (isee: "with a
/// leak fraction of 1, there will be no outflow") -- no warning.
#[test]
fn test_conveyor_leak_fractions_at_most_one_no_warning() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project("4", datamodel::Dt::Dt(0.25), false, &["0.5", "0.5"], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne).is_empty(),
        "fractions summing to exactly 1 must not warn; got: {diags:?}"
    );
}

/// §5.2: exponential leaks are per-time RATES, not fractions of the cohort --
/// overlapping rates ADD by design, so a sum above 1 is legal and the §5.1
/// check must skip an exponential conveyor entirely.
#[test]
fn test_conveyor_leak_fractions_exponential_no_warning() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project("4", datamodel::Dt::Dt(0.25), true, &["0.7", "0.5"], None);
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne).is_empty(),
        "exponential leak rates must not trip the linear-fraction-sum warning; got: {diags:?}"
    );
}

/// §5.1 non-constant: a runtime leak-fraction expression is excluded from the
/// sum (fractions clamp to [0, 1] at runtime, so the constant subset is a
/// lower bound and skipping the runtime one can never hide a warning that the
/// constants alone justify). Here the constant subset is 0.7 <= 1: no warning.
#[test]
fn test_conveyor_leak_fraction_non_constant_excluded() {
    use crate::common::ErrorCode;
    let db = SimlinDb::default();
    let project = conveyor_spec_project(
        "4",
        datamodel::Dt::Dt(0.25),
        false,
        &["0.7", "frac_x"],
        Some(("frac_x", "0.6")),
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        conveyor_spec_warnings(&diags, ErrorCode::ConveyorLeakFractionsExceedOne).is_empty(),
        "a non-constant fraction must be excluded from the compile-time sum; got: {diags:?}"
    );
}
