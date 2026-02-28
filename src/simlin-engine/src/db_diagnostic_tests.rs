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
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

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

/// AC2.1: Parity between old struct-field error collection and new
/// salsa accumulator error collection. Both paths should produce
/// the same set of error codes for a project with various error types.
#[test]
fn test_ac2_1_accumulator_parity_with_old_path() {
    use crate::common::ErrorCode;
    use crate::project::Project as CompiledProject;
    use std::collections::HashSet;

    // Build a project with a mix of valid and invalid variables
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
        }],
        source: None,
        ai_information: None,
    };

    // Old path: struct-field error collection
    let compiled = CompiledProject::from(project.clone());
    let mut old_error_codes: HashSet<ErrorCode> = HashSet::new();
    for model in compiled.models.values() {
        for (_var_name, var_errors) in model.get_variable_errors() {
            for err in var_errors {
                old_error_codes.insert(err.code);
            }
        }
    }

    // New path: salsa accumulator
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);
    let mut new_error_codes: HashSet<ErrorCode> = HashSet::new();
    for d in &diags {
        if d.severity == DiagnosticSeverity::Error {
            match &d.error {
                DiagnosticError::Equation(err) => {
                    new_error_codes.insert(err.code);
                }
                DiagnosticError::Model(err) => {
                    new_error_codes.insert(err.code);
                }
                _ => {}
            }
        }
    }

    // The accumulator path should be a superset of the struct-field path
    // for equation-level errors. The old path may include additional
    // model/simulation-level errors that the accumulator doesn't cover yet.
    for code in &old_error_codes {
        assert!(
            new_error_codes.contains(code),
            "old path has error code {code:?} that accumulator path is missing; \
             old: {old_error_codes:?}, new: {new_error_codes:?}"
        );
    }
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
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

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
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

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
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

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
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

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

    // The diagnostics accumulator should also have the circular dep error.
    // Since compile_project_incremental is not a tracked function, assembly
    // errors are accumulated via try_accumulate_diagnostic (which silently
    // drops when no tracked context is active). However, per-variable
    // diagnostics from model_all_diagnostics SHOULD capture the circular
    // dependency detected by model_dependency_graph.
    let diags = collect_all_diagnostics(&db, &sync);
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
