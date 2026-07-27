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

// ---- Duplicate canonical variable idents (GH #885) ----
//
// Canonicalization collapses case, whitespace, and underscores, so two
// variables like `Attrition`/`attrition` or `net flow`/`net_flow` are the
// same canonical ident. The canonical-keyed sync maps silently collapse such
// twins (last-in-document-order wins), so a model containing them would
// simulate a DIFFERENT model than the one the user wrote. These tests pin the
// loud rejection: a hard `DuplicateVariable` compile error naming both
// original spellings and the model, plus an Error-severity diagnostic on the
// accumulator path (so `collect_all_diagnostics` / `get_errors` surface it).

fn dup_sim_specs() -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    }
}

/// Compile `project`'s `main` model through the production incremental
/// pipeline, returning the result.
fn compile_main(project: &datamodel::Project) -> crate::Result<crate::vm::CompiledSimulation> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_project_incremental(&db, sync.project, "main")
}

/// All `DuplicateVariable` diagnostics in `diags`.
fn duplicate_var_diags(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| {
            matches!(&d.error, DiagnosticError::Model(e)
                if e.code == crate::common::ErrorCode::DuplicateVariable)
        })
        .collect()
}

#[test]
fn test_duplicate_case_variant_idents_fail_compile() {
    use crate::common::ErrorCode;
    use crate::testutils::{x_aux, x_model, x_project};

    let project = x_project(
        dup_sim_specs(),
        &[x_model(
            "main",
            vec![x_aux("Attrition", "1", None), x_aux("attrition", "2", None)],
        )],
    );
    let err = compile_main(&project).expect_err("case-variant duplicate pair must fail compile");
    assert_eq!(err.code, ErrorCode::DuplicateVariable);
    let details = err.details.expect("the error must carry a message");
    assert!(
        details.contains("'Attrition'") && details.contains("'attrition'"),
        "message must name both original spellings: {details}"
    );
    assert!(
        details.contains("main"),
        "message must name the model: {details}"
    );
}

#[test]
fn test_duplicate_whitespace_underscore_idents_fail_compile() {
    use crate::common::ErrorCode;
    use crate::testutils::{x_aux, x_model, x_project};

    let project = x_project(
        dup_sim_specs(),
        &[x_model(
            "main",
            vec![x_aux("net flow", "1", None), x_aux("net_flow", "2", None)],
        )],
    );
    let err =
        compile_main(&project).expect_err("whitespace/underscore variant pair must fail compile");
    assert_eq!(err.code, ErrorCode::DuplicateVariable);
    let details = err.details.expect("the error must carry a message");
    assert!(
        details.contains("'net flow'") && details.contains("'net_flow'"),
        "message must name both original spellings: {details}"
    );
}

#[test]
fn test_duplicate_idents_surface_error_diagnostic() {
    use crate::testutils::{x_aux, x_model, x_project};

    let project = x_project(
        dup_sim_specs(),
        &[x_model(
            "main",
            vec![x_aux("Attrition", "1", None), x_aux("attrition", "2", None)],
        )],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);
    let dups = duplicate_var_diags(&diags);
    assert_eq!(
        dups.len(),
        1,
        "exactly one diagnostic per colliding group; got: {diags:?}"
    );
    let d = dups[0];
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.model, "main");
    let DiagnosticError::Model(e) = &d.error else {
        unreachable!()
    };
    let msg = e.details.as_deref().unwrap_or_default();
    assert!(
        msg.contains("'Attrition'") && msg.contains("'attrition'"),
        "diagnostic must name both spellings: {msg}"
    );
}

#[test]
fn test_clean_model_unaffected_by_duplicate_check() {
    use crate::testutils::{x_aux, x_flow, x_model, x_project, x_stock};

    let project = x_project(
        dup_sim_specs(),
        &[x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &[], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "0.1", None),
            ],
        )],
    );
    compile_main(&project).expect("a clean model must still compile");

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);
    assert!(
        duplicate_var_diags(&diags).is_empty(),
        "no DuplicateVariable diagnostics for distinct idents; got: {diags:?}"
    );
}

#[test]
fn test_cross_model_same_canonical_idents_allowed() {
    use crate::testutils::{x_aux, x_model, x_project};

    // Models are namespaces: the same canonical ident in TWO DIFFERENT models
    // is not a collision.
    let project = x_project(
        dup_sim_specs(),
        &[
            x_model("main", vec![x_aux("attrition", "1", None)]),
            x_model("other", vec![x_aux("Attrition", "2", None)]),
        ],
    );
    compile_main(&project).expect("cross-model same-canonical idents must be allowed");
}

#[test]
fn test_incremental_sync_detects_newly_added_duplicate() {
    use crate::common::ErrorCode;
    use crate::testutils::{x_aux, x_model, x_project};

    // Pin the INCREMENTAL sync path (existing-model branch, salsa setters):
    // a re-sync that introduces a case twin must invalidate the duplicate
    // check and fail the next compile.
    let clean = x_project(
        dup_sim_specs(),
        &[x_model("main", vec![x_aux("attrition", "1", None)])],
    );
    let mut db = SimlinDb::default();
    let state1 = sync_from_datamodel_incremental(&mut db, &clean, None);
    compile_project_incremental(&db, state1.project, "main")
        .expect("the clean project must compile");

    let dup = x_project(
        dup_sim_specs(),
        &[x_model(
            "main",
            vec![x_aux("attrition", "1", None), x_aux("Attrition", "2", None)],
        )],
    );
    let state2 = sync_from_datamodel_incremental(&mut db, &dup, Some(&state1));
    let err = compile_project_incremental(&db, state2.project, "main")
        .expect_err("the re-synced duplicate must fail compile");
    assert_eq!(err.code, ErrorCode::DuplicateVariable);
}

#[test]
fn test_xmile_duplicate_pair_rejected_end_to_end() {
    use crate::common::ErrorCode;
    use std::io::BufReader;

    // The XMILE reader stable-sorts by canonical ident but never dedups; the
    // engine-side gate must reject the parsed project at compile time.
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <aux name="Attrition"><eqn>1</eqn></aux>
      <aux name="attrition"><eqn>2</eqn></aux>
    </variables>
  </model>
</xmile>"#;
    let project = crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes()))
        .expect("the reader itself tolerates duplicates");
    let main = project.models[0].name.clone();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let err = compile_project_incremental(&db, sync.project, &main)
        .expect_err("XMILE duplicate pair must fail compile");
    assert_eq!(err.code, ErrorCode::DuplicateVariable);
    let details = err.details.expect("the error must carry a message");
    assert!(
        details.contains("'Attrition'") && details.contains("'attrition'"),
        "message must name both original spellings: {details}"
    );
}

// ---- Unknown element subscripts on non-apply-to-all arrays (GH #905) ----

/// A project with one arrayed aux over the given dimensions, defined
/// element-by-element with the given `(subscript, equation)` entries.
fn arrayed_elements_project(
    dimensions: Vec<datamodel::Dimension>,
    dim_names: &[&str],
    entries: &[(&str, &str)],
) -> datamodel::Project {
    let elements = entries
        .iter()
        .map(|(sub, eqn)| (sub.to_string(), eqn.to_string(), None, None))
        .collect();
    datamodel::Project {
        name: "arrayed_elements".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions,
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "plain".to_string(),
                equation: datamodel::Equation::Arrayed(
                    dim_names.iter().map(|s| s.to_string()).collect(),
                    elements,
                    None,
                    false,
                ),
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
    }
}

fn unknown_element_warnings(diags: &[Diagnostic]) -> Vec<Diagnostic> {
    conveyor_spec_warnings(diags, crate::common::ErrorCode::UnknownElementSubscript)
}

/// The typo case from GH #905: an `<element subscript="c">` entry on a
/// variable over `board{a,b}` names no declared element. The equation is
/// silently dropped by every consumer; a Warning must surface it, naming the
/// variable and the subscript.
#[test]
fn test_unknown_element_subscript_warns() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::named(
            "board".to_string(),
            vec!["a".to_string(), "b".to_string()],
        )],
        &["board"],
        &[("a", "30"), ("c", "45")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = unknown_element_warnings(&diags);
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one unknown-element warning; got: {diags:?}"
    );
    let w = &warnings[0];
    assert_eq!(w.severity, DiagnosticSeverity::Warning);
    assert_eq!(w.model, "main");
    assert_eq!(
        w.variable.as_deref(),
        Some("plain"),
        "the warning must be attributed to the arrayed variable"
    );
    let details = model_diag_details(w);
    assert!(
        details.contains("plain") && details.contains("'c'"),
        "the message must name the variable and the unmatched subscript: {details}"
    );
}

/// Negative: element entries that match declared elements up to case and
/// surrounding whitespace (the canonical form) must not warn.
#[test]
fn test_unknown_element_subscript_no_warning_for_canonical_variants() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::named(
            "board".to_string(),
            vec!["alpha".to_string(), "beta".to_string()],
        )],
        &["board"],
        &[("Alpha", "1"), (" beta ", "2")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        unknown_element_warnings(&diags).is_empty(),
        "case/whitespace variants of declared elements must not warn; got: {diags:?}"
    );
}

/// Adversarial counterexample 1: an element NAME containing a comma. The
/// compiler matches the whole canonicalized subscript string against the
/// declared combination's comma-joined key, so an entry for the literal
/// element "a,b" of a one-dimensional variable RESOLVES and its equation is
/// used by the simulation. The per-part matcher alone would mis-split it
/// into two parts and flag it -- with a message falsely claiming the
/// equation is ignored. Must not warn.
#[test]
fn test_unknown_element_subscript_comma_element_name_not_flagged() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::named(
            "board".to_string(),
            vec!["a,b".to_string(), "c".to_string()],
        )],
        &["board"],
        &[("a,b", "7"), ("c", "1")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        unknown_element_warnings(&diags).is_empty(),
        "an entry naming a comma-containing element resolves whole-string and must not warn; \
         got: {diags:?}"
    );
}

/// Adversarial counterexample 2: a QUOTED whole subscript on a
/// two-dimensional variable. `canonicalize` strips balanced quotes, so the
/// whole-string key of `"a1,b1"` equals the declared combination `a1,b1`
/// and the compiler resolves the entry. The per-part split would leave
/// unbalanced quote characters on each half and flag it. Must not warn.
/// (Only reachable from API-built datamodels -- both file readers normalize
/// per-part on import.)
#[test]
fn test_unknown_element_subscript_quoted_whole_subscript_not_flagged() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![
            datamodel::Dimension::named(
                "dim_a".to_string(),
                vec!["a1".to_string(), "a2".to_string()],
            ),
            datamodel::Dimension::named(
                "dim_b".to_string(),
                vec!["b1".to_string(), "b2".to_string()],
            ),
        ],
        &["dim_a", "dim_b"],
        &[("\"a1,b1\"", "5"), ("a2, b2", "6")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        unknown_element_warnings(&diags).is_empty(),
        "a quoted whole subscript resolves whole-string and must not warn; got: {diags:?}"
    );
}

/// Multi-dimensional entries: a full match is accepted; a typo in one
/// position warns, and an entry with the wrong number of subscript parts
/// (one part for a two-dimensional variable) warns too.
#[test]
fn test_unknown_element_subscript_multi_dim() {
    let dims = vec![
        datamodel::Dimension::named(
            "dim_a".to_string(),
            vec!["a1".to_string(), "a2".to_string()],
        ),
        datamodel::Dimension::named(
            "dim_b".to_string(),
            vec!["b1".to_string(), "b2".to_string()],
        ),
    ];
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        dims,
        &["dim_a", "dim_b"],
        &[("a1, b1", "1"), ("a1, zz", "2"), ("a2", "3")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = unknown_element_warnings(&diags);
    assert_eq!(
        warnings.len(),
        2,
        "expected warnings for the typo'd and the partial entry only; got: {diags:?}"
    );
    let details: Vec<String> = warnings.iter().map(model_diag_details).collect();
    assert!(
        details.iter().any(|d| d.contains("'a1, zz'")),
        "must warn on the typo'd second position: {details:?}"
    );
    assert!(
        details.iter().any(|d| d.contains("'a2'")),
        "must warn on the wrong-arity entry: {details:?}"
    );
}

/// Indexed dimensions: numeric subscripts within `1..=size` match; an
/// out-of-range index warns.
#[test]
fn test_unknown_element_subscript_indexed_dimension() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::indexed("slots".to_string(), 2)],
        &["slots"],
        &[("1", "10"), ("2", "20"), ("3", "30")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    let warnings = unknown_element_warnings(&diags);
    assert_eq!(
        warnings.len(),
        1,
        "only the out-of-range index must warn; got: {diags:?}"
    );
    assert!(
        model_diag_details(&warnings[0]).contains("'3'"),
        "the message must name the out-of-range index"
    );
}

/// An unresolvable dimension NAME is a separate diagnostic's concern
/// (BadDimensionName on the equation); the element check must stay silent
/// rather than cascade a second warning per entry.
#[test]
fn test_unknown_element_subscript_skips_unresolved_dimension() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::named(
            "board".to_string(),
            vec!["a".to_string(), "b".to_string()],
        )],
        &["no_such_dim"],
        &[("a", "1"), ("zz", "2")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert!(
        unknown_element_warnings(&diags).is_empty(),
        "an unresolved dimension name must not cascade element warnings; got: {diags:?}"
    );
}

/// Duplicate entries with the same unknown subscript warn once, not once per
/// occurrence (real corpus files carry duplicated element lists).
#[test]
fn test_unknown_element_subscript_deduplicates() {
    let db = SimlinDb::default();
    let project = arrayed_elements_project(
        vec![datamodel::Dimension::named(
            "board".to_string(),
            vec!["a".to_string(), "b".to_string()],
        )],
        &["board"],
        &[("a", "1"), ("c", "2"), ("c", "3")],
    );
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

    assert_eq!(
        unknown_element_warnings(&diags).len(),
        1,
        "the same unknown subscript must warn once; got: {diags:?}"
    );
}

/// The conveyor per-element init lists (GH #889) inherit the same silence:
/// a typo'd per-element init-list subscript leaves that belt steady-filling
/// from 0. The sync path substitutes a constant placeholder equation for
/// list entries but preserves the subscript keys, so the generic check must
/// still see and flag the typo.
#[test]
fn test_unknown_element_subscript_warns_on_conveyor_init_list() {
    let db = SimlinDb::default();
    let dims = vec![datamodel::Dimension::named(
        "board".to_string(),
        vec!["a".to_string(), "b".to_string()],
    )];
    let project = datamodel::Project {
        name: "conveyor_init".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: dims,
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "belt".to_string(),
                    equation: datamodel::Equation::Arrayed(
                        vec!["board".to_string()],
                        vec![
                            ("a".to_string(), "5,3,2".to_string(), None, None),
                            ("c".to_string(), "1,1,1".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["in_f".to_string()],
                    outflows: vec!["out_f".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        conveyor: Some(datamodel::Conveyor {
                            transit_time: "3".to_string(),
                            capacity: None,
                            inflow_limit: None,
                            sample: None,
                            arrest: None,
                            discrete: false,
                            batch_integrity: false,
                            one_at_a_time: true,
                            exponential_leak: false,
                            ignore_earlier_zone_losses: false,
                        }),
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "out_f".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["board".to_string()],
                        String::new(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "in_f".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["board".to_string()],
                        "10".to_string(),
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

    let warnings = unknown_element_warnings(&diags);
    assert_eq!(
        warnings.len(),
        1,
        "the typo'd init-list subscript must warn; got: {diags:?}"
    );
    let w = &warnings[0];
    assert_eq!(w.variable.as_deref(), Some("belt"));
    assert!(
        model_diag_details(w).contains("'c'"),
        "the message must name the unmatched subscript"
    );
}

// ---- project-level macro-registry build error ----
//
// An invalid macro set (a recursion cycle, a duplicate macro name, a
// macro/model name collision) is a PROJECT-level failure: exactly one thing is
// wrong with the project, regardless of how many models it holds. It used to be
// accumulated from inside `project_macro_registry`'s body and discovered only by
// whatever accumulator DFS happened to reach that memo, which made it both
// over-reported (once per model, since every model's `model_all_diagnostics`
// subtree reaches the registry through `model_module_ident_context`) and
// FRAGILE (see the pruning hazard documented above `unit_warning_fixture`: after
// an unrelated revision bump the whole subtree is pruned and the diagnostic
// silently vanishes). It is now emitted once, directly, by
// `collect_all_diagnostics` from the memoized `build_error`.

/// A project with `n_extra` filler models plus two macros that share a name --
/// an AC5.3 `DuplicateMacroName` registry-build failure.
fn duplicate_macro_project(n_extra: usize) -> datamodel::Project {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

    let macro_model = |body: &str| {
        let mut model = x_model("foo", vec![x_aux("foo", body, None), x_aux("a", "0", None)]);
        model.macro_spec = Some(datamodel::MacroSpec {
            parameters: vec!["a".to_string()],
            primary_output: "foo".to_string(),
            additional_outputs: vec![],
        });
        model
    };

    let mut models = vec![x_model("main", vec![x_aux("x", "1", None)])];
    for i in 0..n_extra {
        models.push(x_model(&format!("filler_{i}"), vec![x_aux("y", "2", None)]));
    }
    models.push(macro_model("a"));
    models.push(macro_model("a + 1"));
    x_project(sim_specs_with_units("months"), &models)
}

/// The registry-build diagnostics in a collected set.
fn macro_build_diagnostics(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| {
            matches!(&d.error, DiagnosticError::Model(e)
                if e.code == crate::common::ErrorCode::DuplicateMacroName)
        })
        .collect()
}

/// One project-level failure produces exactly ONE diagnostic, however many
/// models the project holds.
#[test]
fn macro_registry_build_error_is_reported_exactly_once() {
    let db = SimlinDb::default();
    let project = duplicate_macro_project(4);
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_all_diagnostics(&db, sync.project);
    let found = macro_build_diagnostics(&diags);
    assert_eq!(
        found.len(),
        1,
        "a project-level macro-registry failure must be reported once, not once per model; \
         got {} copies: {found:?}",
        found.len()
    );
    let d = found[0];
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert!(
        d.model.is_empty() && d.variable.is_none(),
        "the registry failure is project-level, so it names no model or variable: {d:?}"
    );

    // It reaches the collection as a memoized VALUE read, never through the
    // accumulator: no model's per-model drain carries it. That is what makes it
    // exactly-once AND immune to the accumulator-DFS pruning below.
    for (name, source_model) in sync.project.models(&db) {
        let per_model = collect_model_diagnostics(&db, *source_model, sync.project);
        assert!(
            macro_build_diagnostics(&per_model).is_empty(),
            "the project-level registry error must not ride any model's accumulator \
             (model '{name}'): {per_model:?}"
        );
    }
}

/// The registry-build diagnostic survives an unrelated salsa revision bump.
///
/// This is the failure the accumulator-based emission actually had: bumping
/// `pinned_loops` (the same unrelated input
/// `test_diagnostics_stable_across_unrelated_input_change` uses) let salsa
/// validate `model_all_diagnostics` without re-executing it, the deep-verify
/// path recomputed the DFS pruning flag as `Empty`, and the diagnostic
/// disappeared from the next collection.
#[test]
fn macro_registry_build_error_survives_an_unrelated_input_change() {
    use crate::db::PinnedLoopSpec;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = duplicate_macro_project(2);
    let sync = sync_from_datamodel(&db, &project);

    let before = collect_all_diagnostics(&db, sync.project);
    assert_eq!(
        macro_build_diagnostics(&before).len(),
        1,
        "fixture must produce the registry error to begin with: {before:?}"
    );

    let source_model = sync.models["main"].source;
    source_model
        .set_pinned_loops(&mut db)
        .to(vec![PinnedLoopSpec {
            name: "dummy_loop".to_string(),
            variables: vec![],
            description: String::new(),
        }]);

    let after = collect_all_diagnostics(&db, sync.project);
    assert_eq!(
        macro_build_diagnostics(&after).len(),
        1,
        "the registry error must survive an unrelated input change: {after:?}"
    );
    assert_eq!(
        before, after,
        "the full diagnostic set must be identical across an unrelated input change"
    );
}

// ---- project-level unit-definition errors ----
//
// The same defect, in the same shape, as the macro-registry build error above:
// a project's `units` list belongs to no model, but the errors from parsing it
// were accumulated inside `project_units_context`'s body, so the DFS found them
// once per model and lost them completely after an unrelated revision bump.
// They now ride `UnitsContextResult::definition_errors` and are emitted once by
// `collect_all_diagnostics`.

/// A project whose unit declarations conflict: two units claim the same alias
/// `gadget` for different primary names. (Identical duplicate declarations are
/// deliberately tolerated -- Vensim MDL footers repeat `22:` lines -- so a
/// plain duplicate would not produce an error here.)
fn conflicting_unit_alias_project() -> datamodel::Project {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

    let mut dm = x_project(
        sim_specs_with_units("years"),
        &[
            x_model("main", vec![x_aux("x", "1", None)]),
            x_model("other", vec![x_aux("y", "2", None)]),
        ],
    );
    for name in ["widget", "doodad"] {
        dm.units.push(datamodel::Unit {
            name: name.to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["gadget".to_string()],
        });
    }
    dm
}

/// The unit-definition errors in a collected set.
fn unit_definition_diagnostics(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| {
            matches!(
                &d.error,
                DiagnosticError::Unit(crate::common::UnitError::DefinitionError(_, _))
            )
        })
        .collect()
}

/// One bad unit declaration produces one diagnostic, however many models the
/// project holds -- and it does not ride any model's accumulator.
#[test]
fn unit_definition_errors_are_reported_exactly_once() {
    let db = SimlinDb::default();
    let project = conflicting_unit_alias_project();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_all_diagnostics(&db, sync.project);
    let found = unit_definition_diagnostics(&diags);
    assert_eq!(
        found.len(),
        1,
        "a conflicting unit alias must be reported once, not once per model; \
         got {} copies: {found:?}",
        found.len()
    );
    assert!(
        found[0].model.is_empty(),
        "a unit declaration belongs to the project, not a model: {:?}",
        found[0]
    );

    for (name, source_model) in sync.project.models(&db) {
        let per_model = collect_model_diagnostics(&db, *source_model, sync.project);
        assert!(
            unit_definition_diagnostics(&per_model).is_empty(),
            "the project-level unit error must not ride model '{name}''s accumulator: \
             {per_model:?}"
        );
    }
}

/// The unit-definition error survives an unrelated salsa revision bump.
#[test]
fn unit_definition_errors_survive_an_unrelated_input_change() {
    use crate::db::PinnedLoopSpec;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = conflicting_unit_alias_project();
    let sync = sync_from_datamodel(&db, &project);

    let before = collect_all_diagnostics(&db, sync.project);
    assert_eq!(
        unit_definition_diagnostics(&before).len(),
        1,
        "fixture must produce the unit definition error to begin with: {before:?}"
    );

    let source_model = sync.models["main"].source;
    source_model
        .set_pinned_loops(&mut db)
        .to(vec![PinnedLoopSpec {
            name: "dummy_loop".to_string(),
            variables: vec![],
            description: String::new(),
        }]);

    let after = collect_all_diagnostics(&db, sync.project);
    assert_eq!(
        unit_definition_diagnostics(&after).len(),
        1,
        "the unit definition error must survive an unrelated input change: {after:?}"
    );
    assert_eq!(
        before, after,
        "the full diagnostic set must be identical across an unrelated input change"
    );
}

/// `Variable::errors` and `Variable::unit_errors` are the CHANNEL by which
/// parsing and lowering report a failure to the salsa path, not residue from
/// the monolithic compiler.
///
/// `docs/tech-debt.md` item 17 claimed all four embedded error fields were
/// "dead weight carried through the monolithic compilation path", redundant
/// with the salsa pipeline. For these two that is backwards: the salsa
/// pipeline's diagnostics are DOWNSTREAM of them --
/// `db::var_fragment::lower_var_fragment` reads
/// `parsed.variable.unit_errors()`, `parsed.variable.equation_errors()` and
/// `lowered.equation_errors()` and turns each entry into a `Diagnostic`. Acting
/// on the claim would silently drop those diagnostics, so it is pinned here
/// rather than left as prose: each half asserts BOTH that the stage's value
/// carries the error in the field AND that the matching diagnostic comes out of
/// `collect_all_diagnostics`.
///
/// Emptying the `unit_errors()` read or the `lowered.equation_errors()` read
/// reds THIS test. Emptying the `parsed.variable.equation_errors()` read does
/// NOT -- `lower_variable` clones parse errors forward in all three arms, so the
/// lowered read catches the same error and this test stays green. What that read
/// uniquely carries is the conveyor/queue driven-flow `EmptyEquation`
/// suppression, and dropping it reds
/// `test_conveyor_driven_flow_empty_equation_suppressed`,
/// `test_conveyor_marker_removal_reinstates_empty_equation` and
/// `test_queue_driven_outflow_empty_equation_suppressed` instead. Measured, not
/// assumed.
#[test]
fn variable_error_fields_are_the_lowering_channel() {
    use crate::test_common::TestProject;

    // ── parse-time: a malformed `<units>` string, and a syntax error ──
    let dm = TestProject::new("parse_channel")
        .aux("bad_unit_var", "1", Some("bad units here!!!"))
        .aux("bad_eqn_var", "1 +", None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let stage0 = crate::db::model_stage0(&db, sync.models["main"].source, sync.project);

    assert!(
        stage0.variables[&crate::common::Ident::new("bad_unit_var")]
            .unit_errors()
            .is_some(),
        "parsing must record the malformed unit string on the variable"
    );
    assert!(
        stage0.variables[&crate::common::Ident::new("bad_eqn_var")]
            .equation_errors()
            .is_some(),
        "parsing must record the equation syntax error on the variable"
    );

    let diags = collect_all_diagnostics(&db, sync.project);
    assert!(
        diags.iter().any(|d| {
            d.variable.as_deref() == Some("bad_unit_var")
                && matches!(&d.error, DiagnosticError::Unit(_))
        }),
        "the recorded unit error must reach collect_all_diagnostics; got: {diags:?}"
    );
    assert!(
        diags.iter().any(|d| {
            d.variable.as_deref() == Some("bad_eqn_var")
                && matches!(&d.error, DiagnosticError::Equation(_))
        }),
        "the recorded equation error must reach collect_all_diagnostics; got: {diags:?}"
    );

    // ── lowering-time: an error `lower_ast` raises, which the parsed
    // variable cannot carry because it does not exist yet ──
    let dm = TestProject::new("lower_channel")
        .named_dimension("Cities", &["Boston", "Seattle"])
        .named_dimension("Products", &["Widgets", "Gadgets"])
        .array_aux("sales[Cities]", "1")
        .array_aux("prices[Products]", "1")
        .array_aux("bad[Cities]", "sales + prices")
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let bad = crate::common::Ident::new("bad");

    assert!(
        crate::db::model_stage0(&db, sync.models["main"].source, sync.project).variables[&bad]
            .equation_errors()
            .is_none(),
        "the fixture must isolate a LOWERING error: parsing sees nothing wrong"
    );
    let lowered_errors = crate::db::model_stage1(&db, sync.models["main"].source, sync.project)
        .variables[&bad]
        .equation_errors()
        .expect("lowering must record the dimension mismatch on the variable");
    assert!(
        lowered_errors
            .iter()
            .any(|e| e.code == crate::common::ErrorCode::MismatchedDimensions),
        "expected MismatchedDimensions, got: {lowered_errors:?}"
    );

    let diags = collect_all_diagnostics(&db, sync.project);
    assert!(
        diags.iter().any(|d| {
            d.variable.as_deref() == Some("bad")
                && matches!(&d.error, DiagnosticError::Equation(e)
                    if e.code == crate::common::ErrorCode::MismatchedDimensions)
        }),
        "the recorded lowering error must reach collect_all_diagnostics; got: {diags:?}"
    );
}
