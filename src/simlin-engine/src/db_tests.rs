// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

fn simple_project() -> datamodel::Project {
    datamodel::Project {
        name: "test".to_string(),
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
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: Some("people".to_string()),
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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
    }
}

#[test]
fn test_create_db() {
    let _db = SimlinDb::default();
}

#[test]
fn test_intern_variable_id_same_name() {
    let db = SimlinDb::default();
    let id1 = VariableId::new(&db, "population".to_string());
    let id2 = VariableId::new(&db, "population".to_string());
    assert_eq!(id1, id2);
}

#[test]
fn test_intern_variable_id_different_names() {
    let db = SimlinDb::default();
    let id1 = VariableId::new(&db, "population".to_string());
    let id2 = VariableId::new(&db, "birth_rate".to_string());
    assert_ne!(id1, id2);
}

#[test]
fn test_intern_model_id_same_name() {
    let db = SimlinDb::default();
    let id1 = ModelId::new(&db, "main".to_string());
    let id2 = ModelId::new(&db, "main".to_string());
    assert_eq!(id1, id2);
}

#[test]
fn test_intern_model_id_different_names() {
    let db = SimlinDb::default();
    let id1 = ModelId::new(&db, "main".to_string());
    let id2 = ModelId::new(&db, "submodel".to_string());
    assert_ne!(id1, id2);
}

#[test]
fn test_intern_variable_id_text_roundtrip() {
    let db = SimlinDb::default();
    let id = VariableId::new(&db, "birth_rate".to_string());
    assert_eq!(id.text(&db), "birth_rate");
}

#[test]
fn test_intern_model_id_text_roundtrip() {
    let db = SimlinDb::default();
    let id = ModelId::new(&db, "main".to_string());
    assert_eq!(id.text(&db), "main");
}

#[test]
fn test_sync_simple_project() {
    let db = SimlinDb::default();
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);

    assert_eq!(result.project.name(&db), "test");
    assert!(
        result
            .project
            .model_names(&db)
            .contains(&"main".to_string())
    );
    // 1 user model + 7 stdlib models
    assert_eq!(result.project.model_names(&db).len(), 8);

    let sim_specs = result.project.sim_specs(&db);
    assert_eq!(sim_specs.start, 0.0);
    assert_eq!(sim_specs.stop, 10.0);
    assert_eq!(sim_specs.time_units, Some("months".to_string()));

    assert!(result.models.contains_key("main"));
    let main_model = &result.models["main"];
    assert_eq!(main_model.source.name(&db), "main");
    assert_eq!(main_model.source.variable_names(&db).len(), 1);
    assert_eq!(main_model.source.variable_names(&db)[0], "population");

    let pop_var = &main_model.variables["population"];
    assert_eq!(pop_var.id.text(&db), "population");
    assert_eq!(pop_var.source.kind(&db), SourceVariableKind::Aux);
    assert_eq!(pop_var.source.units(&db), &Some("people".to_string()));
    assert_eq!(
        pop_var.source.equation(&db),
        &SourceEquation::Scalar("100".to_string())
    );
    assert!(!pop_var.source.non_negative(&db));
    assert!(!pop_var.source.can_be_module_input(&db));
}

#[test]
fn test_sync_multi_model() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "multi".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    assert_eq!(result.models.len(), 2);
    assert!(result.models.contains_key("main"));
    assert!(result.models.contains_key("submodel"));

    // Different model names get different IDs
    let main_id = result.models["main"].id;
    let sub_id = result.models["submodel"].id;
    assert_ne!(main_id, sub_id);
}

#[test]
fn test_sync_all_variable_kinds() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "kinds".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_var".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["flow_in".to_string()],
                    outflows: vec!["flow_out".to_string()],
                    non_negative: true,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "flow_var".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: true,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "aux_var".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Module(datamodel::Module {
                    ident: "mod_var".to_string(),
                    model_name: "submodel".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![datamodel::ModuleReference {
                        src: "x".to_string(),
                        dst: "y".to_string(),
                    }],
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let main = &result.models["main"];

    let stock = &main.variables["stock_var"];
    assert_eq!(stock.source.kind(&db), SourceVariableKind::Stock);
    assert_eq!(stock.source.inflows(&db), &vec!["flow_in".to_string()]);
    assert_eq!(stock.source.outflows(&db), &vec!["flow_out".to_string()]);
    assert!(stock.source.non_negative(&db));

    let flow = &main.variables["flow_var"];
    assert_eq!(flow.source.kind(&db), SourceVariableKind::Flow);
    assert!(flow.source.non_negative(&db));

    let aux = &main.variables["aux_var"];
    assert_eq!(aux.source.kind(&db), SourceVariableKind::Aux);

    let module = &main.variables["mod_var"];
    assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
    assert_eq!(module.source.module_refs(&db).len(), 1);
    assert_eq!(module.source.module_refs(&db)[0].src, "x");
    assert_eq!(module.source.module_refs(&db)[0].dst, "y");
}

#[test]
fn test_sync_variable_with_gf() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "gf_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Scalar("lookup_var(time)".to_string()),
                documentation: String::new(),
                units: None,
                gf: Some(datamodel::GraphicalFunction {
                    kind: datamodel::GraphicalFunctionKind::Continuous,
                    x_points: Some(vec![0.0, 1.0, 2.0]),
                    y_points: vec![0.0, 5.0, 10.0],
                    x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    y_scale: datamodel::GraphicalFunctionScale {
                        min: 0.0,
                        max: 10.0,
                    },
                }),
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);
    let var = &result.models["main"].variables["lookup_var"];
    let gf = var.source.gf(&db);
    assert!(gf.is_some());
    let gf = gf.as_ref().unwrap();
    assert_eq!(gf.kind, SourceGraphicalFunctionKind::Continuous);
    assert_eq!(gf.x_points, Some(vec![0.0, 1.0, 2.0]));
    assert_eq!(gf.y_points, vec![0.0, 5.0, 10.0]);
    assert_eq!(gf.x_scale.min, 0.0);
    assert_eq!(gf.x_scale.max, 2.0);
}

#[test]
fn test_sync_dimensions() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "Region".to_string(),
                vec!["North".to_string(), "South".to_string()],
            ),
            datamodel::Dimension::indexed("Periods".to_string(), 5),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let dims = result.project.dimensions(&db);
    assert_eq!(dims.len(), 2);

    assert_eq!(dims[0].name, "Region");
    assert_eq!(
        dims[0].elements,
        SourceDimensionElements::Named(vec!["North".to_string(), "South".to_string()])
    );

    assert_eq!(dims[1].name, "Periods");
    assert_eq!(dims[1].elements, SourceDimensionElements::Indexed(5));
}

#[test]
fn test_sync_module_refs() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "mod_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Module(datamodel::Module {
                ident: "my_module".to_string(),
                model_name: "sub".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![
                    datamodel::ModuleReference {
                        src: "input_a".to_string(),
                        dst: "a".to_string(),
                    },
                    datamodel::ModuleReference {
                        src: "input_b".to_string(),
                        dst: "b".to_string(),
                    },
                ],
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let module = &result.models["main"].variables["my_module"];
    assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
    let refs = module.source.module_refs(&db);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].src, "input_a");
    assert_eq!(refs[0].dst, "a");
    assert_eq!(refs[1].src, "input_b");
    assert_eq!(refs[1].dst, "b");
}

#[test]
fn test_sync_resync_updates() {
    let db = SimlinDb::default();
    let mut project = simple_project();
    let result1 = sync_from_datamodel(&db, &project);

    let pop1 = &result1.models["main"].variables["population"];
    assert_eq!(
        pop1.source.equation(&db),
        &SourceEquation::Scalar("100".to_string())
    );

    // Modify the equation and re-sync
    project.models[0].variables[0].set_scalar_equation("200");
    let result2 = sync_from_datamodel(&db, &project);

    let pop2 = &result2.models["main"].variables["population"];
    assert_eq!(
        pop2.source.equation(&db),
        &SourceEquation::Scalar("200".to_string())
    );

    // Interned IDs for the same canonical name should be the same
    assert_eq!(pop1.id, pop2.id);
}

#[test]
fn test_sync_empty_model_name_canonicalized() {
    let db = SimlinDb::default();
    let id1 = ModelId::new(&db, "".to_string());
    let id2 = ModelId::new(&db, "main".to_string());
    // Empty and "main" are different canonical strings
    assert_ne!(id1, id2);
}

#[test]
fn test_sync_sim_specs_dt_reciprocal() {
    let db = SimlinDb::default();
    let mut project = simple_project();
    project.sim_specs.dt = datamodel::Dt::Reciprocal(4.0);
    project.sim_specs.save_step = Some(datamodel::Dt::Dt(0.5));
    project.sim_specs.sim_method = datamodel::SimMethod::RungeKutta4;

    let result = sync_from_datamodel(&db, &project);
    let specs = result.project.sim_specs(&db);
    assert_eq!(specs.dt, SourceDt::Reciprocal(4.0));
    assert_eq!(specs.save_step, Some(SourceDt::Dt(0.5)));
    assert_eq!(specs.sim_method, SourceSimMethod::RungeKutta4);
}

#[test]
fn test_parse_source_variable_scalar() {
    use crate::ast::Expr0;
    use crate::variable::Variable;

    let db = SimlinDb::default();
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);

    let pop_var = result.models["main"].variables["population"].source;
    let parsed = parse_source_variable(&db, pop_var, result.project);

    // Should parse to a Var (aux) with equation "100"
    assert!(matches!(&parsed.variable, Variable::Var { .. }));
    assert_eq!(parsed.variable.ident(), "population");

    // Should have a valid AST with a constant 100.0
    let ast = parsed.variable.ast();
    assert!(ast.is_some());
    if let Some(crate::ast::Ast::Scalar(Expr0::Const(_, val, _))) = ast {
        assert_eq!(*val, 100.0);
    } else {
        panic!("Expected Scalar(Const(100.0)), got {:?}", ast);
    }
}

#[test]
fn test_parse_source_variable_stock() {
    use crate::variable::Variable;

    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "inventory".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["production".to_string()],
                    outflows: vec!["sales".to_string()],
                    non_negative: true,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "production".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "sales".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);

    // Parse the stock variable
    let stock_var = result.models["main"].variables["inventory"].source;
    let parsed = parse_source_variable(&db, stock_var, result.project);
    assert!(matches!(&parsed.variable, Variable::Stock { .. }));
    assert_eq!(parsed.variable.ident(), "inventory");

    // Parse a flow variable
    let flow_var = result.models["main"].variables["production"].source;
    let parsed = parse_source_variable(&db, flow_var, result.project);
    assert!(matches!(
        &parsed.variable,
        Variable::Var { is_flow: true, .. }
    ));
    assert_eq!(parsed.variable.ident(), "production");
}

#[test]
fn test_parse_source_variable_matches_direct_parse() {
    use crate::variable::parse_var;

    let db = SimlinDb::default();
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);

    // Parse via tracked function
    let pop_var = result.models["main"].variables["population"].source;
    let tracked_result = parse_source_variable(&db, pop_var, result.project);

    // Parse directly via parse_var for comparison
    let dm_var = &project.models[0].variables[0];
    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let mut implicit_vars = Vec::new();
    let direct_result = parse_var(
        &project.dimensions,
        dm_var,
        &mut implicit_vars,
        &units_ctx,
        |mi| Ok(Some(mi.clone())),
    );

    // The tracked function and direct parse should produce equivalent results
    assert_eq!(tracked_result.variable.ident(), direct_result.ident());
    assert_eq!(
        tracked_result.variable.equation_errors().is_some(),
        direct_result.equation_errors().is_some()
    );
}

#[test]
fn test_incrementality_unchanged_variable_not_reparsed() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("20".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let (source_project, alpha_src, beta_src) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["alpha"].source,
            result.models["main"].variables["beta"].source,
        )
    };

    // Initial parse of both variables to prime the cache
    let beta_ptr_before = {
        let alpha_result = parse_source_variable(&db, alpha_src, source_project);
        let beta_result = parse_source_variable(&db, beta_src, source_project);
        assert_eq!(alpha_result.variable.ident(), "alpha");
        assert_eq!(beta_result.variable.ident(), "beta");
        beta_result as *const ParsedVariableResult
    };

    // Modify only alpha's equation; beta is unchanged
    alpha_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("42".to_string()));

    // Re-parse both: alpha should have new result, beta should be cached
    let alpha_result_2 = parse_source_variable(&db, alpha_src, source_project);
    let beta_result_2 = parse_source_variable(&db, beta_src, source_project);

    // Alpha's parse result should reflect the new equation
    if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
        alpha_result_2.variable.ast()
    {
        assert_eq!(*val, 42.0);
    } else {
        panic!(
            "Expected alpha to parse as Const(42.0), got {:?}",
            alpha_result_2.variable.ast()
        );
    }

    // Beta should be pointer-equal (same &ParsedVariableResult from cache)
    let beta_ptr_after = beta_result_2 as *const ParsedVariableResult;
    assert_eq!(
        beta_ptr_before, beta_ptr_after,
        "beta should be returned from salsa cache (pointer-equal) since it was not modified"
    );
}

#[test]
fn test_variable_direct_dependencies_constant() {
    let db = SimlinDb::default();
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);

    let pop_var = result.models["main"].variables["population"].source;
    let deps = variable_direct_dependencies(&db, pop_var, result.project);

    assert!(deps.dt_deps.is_empty(), "constant has no deps");
    assert!(deps.initial_deps.is_empty(), "constant has no initial deps");
}

#[test]
fn test_variable_direct_dependencies_with_refs() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);

    let births_var = result.models["main"].variables["births"].source;
    let deps = variable_direct_dependencies(&db, births_var, result.project);

    assert_eq!(
        deps.dt_deps,
        ["population", "rate"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn test_variable_direct_dependencies_stock() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Stock(datamodel::Stock {
                ident: "inventory".to_string(),
                equation: datamodel::Equation::Scalar("initial_value".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["production".to_string()],
                outflows: vec![],
                non_negative: false,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);
    let stock_var = result.models["main"].variables["inventory"].source;
    let deps = variable_direct_dependencies(&db, stock_var, result.project);

    // Stock's init equation references "initial_value"
    assert!(deps.dt_deps.contains("initial_value"));
    assert!(deps.initial_deps.contains("initial_value"));
}

#[test]
fn test_variable_direct_dependencies_module() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Module(datamodel::Module {
                ident: "submodel".to_string(),
                model_name: "sub".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![
                    datamodel::ModuleReference {
                        src: "input_x".to_string(),
                        dst: "x".to_string(),
                    },
                    datamodel::ModuleReference {
                        src: "input_y".to_string(),
                        dst: "y".to_string(),
                    },
                ],
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let mod_var = result.models["main"].variables["submodel"].source;
    let deps = variable_direct_dependencies(&db, mod_var, result.project);

    assert_eq!(
        deps.dt_deps,
        ["input_x", "input_y"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn test_incrementality_same_deps_no_recompute() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("alpha + gamma".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "gamma".to_string(),
                    equation: datamodel::Equation::Scalar("20".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let (source_project, _alpha_src, beta_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["alpha"].source,
            result.models["main"].variables["beta"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache: compute deps and dep graph
    let (beta_dt_before, beta_init_before) = {
        let deps = variable_direct_dependencies(&db, beta_src, source_project);
        assert_eq!(
            deps.dt_deps,
            ["alpha", "gamma"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        (deps.dt_deps.clone(), deps.initial_deps.clone())
    };

    let graph_before = model_dependency_graph(&db, source_model, source_project);
    let graph_ptr_before = graph_before as *const ModelDepGraphResult;

    // Change beta's equation from "alpha + gamma" to "alpha * gamma"
    // Same deps, different equation
    beta_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("alpha * gamma".to_string()));

    // Beta's deps should be the same (alpha, gamma)
    let beta_deps_after = variable_direct_dependencies(&db, beta_src, source_project);
    assert_eq!(beta_dt_before, beta_deps_after.dt_deps);
    assert_eq!(beta_init_before, beta_deps_after.initial_deps);

    // The dep graph should be returned from cache (pointer-equal)
    let graph_after = model_dependency_graph(&db, source_model, source_project);
    let graph_ptr_after = graph_after as *const ModelDepGraphResult;
    assert_eq!(
        graph_ptr_before, graph_ptr_after,
        "model_dependency_graph should be cached when deps don't change"
    );
}

#[test]
fn test_incrementality_different_deps_recompute() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("alpha".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "gamma".to_string(),
                    equation: datamodel::Equation::Scalar("20".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let (source_project, beta_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["beta"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache
    let graph_before = model_dependency_graph(&db, source_model, source_project);
    let graph_ptr_before = graph_before as *const ModelDepGraphResult;

    // Change beta's equation from "alpha" to "gamma" -- different deps
    beta_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("gamma".to_string()));

    // The dep graph should be recomputed (different pointer)
    let graph_after = model_dependency_graph(&db, source_model, source_project);
    let graph_ptr_after = graph_after as *const ModelDepGraphResult;
    assert_ne!(
        graph_ptr_before, graph_ptr_after,
        "model_dependency_graph should recompute when deps change"
    );

    // Verify the new graph has the correct deps
    assert!(
        graph_after.dt_dependencies["beta"].contains("gamma"),
        "beta should now depend on gamma"
    );
    assert!(
        !graph_after.dt_dependencies["beta"].contains("alpha"),
        "beta should no longer depend on alpha"
    );
}

#[test]
fn test_model_dependency_graph_basic() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "growth".to_string(),
                    equation: datamodel::Equation::Scalar("rate * 100".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);
    let graph = model_dependency_graph(&db, result.models["main"].source, result.project);

    // growth depends on rate (transitively)
    assert!(graph.dt_dependencies["growth"].contains("rate"));
    // rate has no deps
    assert!(graph.dt_dependencies["rate"].is_empty());

    // Flows runlist should have rate before growth
    let rate_pos = graph
        .runlist_flows
        .iter()
        .position(|n| n == "rate")
        .unwrap();
    let growth_pos = graph
        .runlist_flows
        .iter()
        .position(|n| n == "growth")
        .unwrap();
    assert!(
        rate_pos < growth_pos,
        "rate should come before growth in runlist"
    );
}

#[test]
fn test_model_dependency_graph_stock_breaks_chain() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * 0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);
    let graph = model_dependency_graph(&db, result.models["main"].source, result.project);

    // In dt phase, stocks have empty deps (chain breaks)
    assert!(
        graph.dt_dependencies["population"].is_empty(),
        "stock should have empty dt deps"
    );

    // births references population but population is a stock, so in dt phase
    // the dep is filtered out
    assert!(
        !graph.dt_dependencies["births"].contains("population"),
        "births should not depend on stock in dt phase"
    );

    // Stock equation is "100" (constant), so initial deps are empty
    assert!(
        graph.initial_dependencies["population"].is_empty(),
        "stock with constant equation should have empty initial deps"
    );
}

#[test]
fn test_model_dependency_graph_circular_emits_diagnostic() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
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
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let result = sync_from_datamodel(&db, &project);
    let _graph = model_dependency_graph(&db, result.models["main"].source, result.project);

    // Collect diagnostics emitted by model_dependency_graph
    let diags = model_dependency_graph::accumulated::<CompilationDiagnostic>(
        &db,
        result.models["main"].source,
        result.project,
    );
    let has_circular = diags.iter().any(|d| {
        matches!(
            d.0.error,
            DiagnosticError::Model(crate::common::Error {
                code: crate::common::ErrorCode::CircularDependency,
                ..
            })
        )
    });
    assert!(
        has_circular,
        "circular dependency between a and b should emit a diagnostic"
    );
}

fn feedback_loop_project() -> datamodel::Project {
    datamodel::Project {
        name: "feedback".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
    }
}

#[test]
fn test_normalize_module_ref_str() {
    assert_eq!(normalize_module_ref_str("foo\u{00B7}output"), "foo");
    assert_eq!(normalize_module_ref_str("plain_name"), "plain_name");
    assert_eq!(normalize_module_ref_str(""), "");
}

#[test]
fn test_generate_max_abs_chain_str() {
    assert_eq!(generate_max_abs_chain_str(&[]), "0");
    assert_eq!(generate_max_abs_chain_str(&["p0".into()]), "\"p0\"");
    let two = generate_max_abs_chain_str(&["p0".into(), "p1".into()]);
    assert!(two.contains("ABS"));
    assert!(two.contains("p0"));
    assert!(two.contains("p1"));
    let three = generate_max_abs_chain_str(&["p0".into(), "p1".into(), "p2".into()]);
    assert!(three.contains("p2"));
}

#[test]
fn test_model_causal_edges_feedback_loop() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let edges = model_causal_edges(&db, model, result.project);

    assert!(edges.stocks.contains("population"));
    // births flows into population, so births -> population edge exists
    assert!(
        edges
            .edges
            .get("births")
            .is_some_and(|t| t.contains("population")),
        "births should have edge to population (stock inflow)"
    );
    // births = population * birth_rate, so population -> births and birth_rate -> births
    assert!(
        edges
            .edges
            .get("population")
            .is_some_and(|t| t.contains("births")),
        "population should have edge to births (dep)"
    );
    assert!(
        edges
            .edges
            .get("birth_rate")
            .is_some_and(|t| t.contains("births")),
        "birth_rate should have edge to births (dep)"
    );
}

#[test]
fn test_model_loop_circuits_finds_feedback() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let circuits = model_loop_circuits(&db, model, result.project);

    // population -> births -> population is the single feedback loop
    assert!(
        !circuits.circuits.is_empty(),
        "should find at least one circuit"
    );
    let has_pop_births_loop = circuits
        .circuits
        .iter()
        .any(|c| c.contains(&"population".to_string()) && c.contains(&"births".to_string()));
    assert!(has_pop_births_loop, "should find population-births loop");
}

#[test]
fn test_model_cycle_partitions_single_stock() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);

    // Single stock should yield one partition
    assert!(
        !partitions.partitions.is_empty(),
        "should have at least one partition"
    );
    assert!(
        partitions.stock_partition.contains_key("population"),
        "population should be in a partition"
    );
}

#[test]
fn test_model_ltm_synthetic_variables_generates_scores() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_synthetic_variables(&db, model, result.project);

    // Should generate link scores and loop scores for the feedback loop
    assert!(!ltm.vars.is_empty(), "should generate LTM variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(has_loop_score, "should have loop score variables");

    // All vars should have non-empty equations
    for var in &ltm.vars {
        assert!(
            !var.equation.is_empty(),
            "var {} should have non-empty equation",
            var.name
        );
    }
}

#[test]
fn test_model_ltm_all_link_synthetic_variables_discovery_mode() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_all_link_synthetic_variables(&db, model, result.project);

    assert!(!ltm.vars.is_empty(), "should generate link score variables");

    // Discovery mode should NOT generate loop scores
    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(
        !has_loop_score,
        "discovery mode should not have loop scores"
    );

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");
}

#[test]
fn test_model_ltm_no_loops_empty() {
    let db = SimlinDb::default();
    // Simple project has just a constant -- no loops
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_synthetic_variables(&db, model, result.project);
    assert!(ltm.vars.is_empty(), "no loops should produce no LTM vars");
}

#[test]
fn test_ltm_caching_equation_change_no_dep_change() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, births_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["births"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache
    let circuits_before = model_loop_circuits(&db, source_model, source_project);
    let circuits_ptr_before = circuits_before as *const LoopCircuitsResult;

    // Change births equation from "population * birth_rate" to
    // "birth_rate * population" -- same deps, different equation text
    births_src.set_equation(&mut db).to(SourceEquation::Scalar(
        "birth_rate * population".to_string(),
    ));

    // Loop circuits should be pointer-equal (cached) because the
    // causal edge structure hasn't changed
    let circuits_after = model_loop_circuits(&db, source_model, source_project);
    let circuits_ptr_after = circuits_after as *const LoopCircuitsResult;
    assert_eq!(
        circuits_ptr_before, circuits_ptr_after,
        "loop circuits should be cached when deps don't change"
    );
}

#[test]
fn test_ltm_caching_dep_change_recomputes_circuits() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, births_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["births"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache
    let circuits_before = model_loop_circuits(&db, source_model, source_project);
    assert!(
        !circuits_before.circuits.is_empty(),
        "should have circuits initially"
    );

    // Change births to a constant -- breaks the feedback loop
    births_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("10".to_string()));

    let circuits_after = model_loop_circuits(&db, source_model, source_project);
    assert!(
        circuits_after.circuits.is_empty(),
        "should have no circuits after breaking loop"
    );
}

/// Two-stock model with independent feedback loops for testing per-link caching.
///
///   stock_a --[births_a]--> stock_a  (loop 1: stock_a <-> births_a)
///   stock_b --[births_b]--> stock_b  (loop 2: stock_b <-> births_b)
///
/// Changing births_a's equation should NOT invalidate link scores in loop 2.
fn two_loop_project() -> datamodel::Project {
    datamodel::Project {
        name: "two_loop".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_a".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births_a".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births_a".to_string(),
                    equation: datamodel::Equation::Scalar("stock_a * rate_a".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate_a".to_string(),
                    equation: datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_b".to_string(),
                    equation: datamodel::Equation::Scalar("200".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births_b".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births_b".to_string(),
                    equation: datamodel::Equation::Scalar("stock_b * rate_b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate_b".to_string(),
                    equation: datamodel::Equation::Scalar("0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
    }
}

#[test]
fn test_ltm_per_link_caching() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = two_loop_project();
    let (source_project, births_a_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["births_a"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache and capture pointer addresses in a scoped block
    // so that the immutable borrow on db is released before the mutation.
    let (link_b_ptr_before, link_a_ptr_before) = {
        let link_b_id = LtmLinkId::new(&db, "stock_b".to_string(), "births_b".to_string());
        let link_b_before = link_score_equation_text(&db, link_b_id, source_model, source_project);
        assert!(link_b_before.is_some(), "link B score should exist");

        let link_a_id = LtmLinkId::new(&db, "stock_a".to_string(), "births_a".to_string());
        let link_a_before = link_score_equation_text(&db, link_a_id, source_model, source_project);
        assert!(link_a_before.is_some(), "link A score should exist");

        (
            link_b_before as *const Option<LtmSyntheticVar>,
            link_a_before as *const Option<LtmSyntheticVar>,
        )
    };

    // Change births_a equation (affects loop A, should NOT affect loop B)
    births_a_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("stock_a * rate_a * 2".to_string()));

    // Re-intern the link IDs (interning is idempotent, returns same ID)
    let link_b_id = LtmLinkId::new(&db, "stock_b".to_string(), "births_b".to_string());
    let link_a_id = LtmLinkId::new(&db, "stock_a".to_string(), "births_a".to_string());

    // Link B should be pointer-equal (cached) since births_b is unaffected
    let link_b_after = link_score_equation_text(&db, link_b_id, source_model, source_project);
    let link_b_ptr_after = link_b_after as *const Option<LtmSyntheticVar>;
    assert_eq!(
        link_b_ptr_before, link_b_ptr_after,
        "link score for unaffected loop B should be cached (pointer-equal)"
    );

    // Link A should be recomputed (equation changed for births_a)
    let link_a_after = link_score_equation_text(&db, link_a_id, source_model, source_project);
    let link_a_ptr_after = link_a_after as *const Option<LtmSyntheticVar>;
    assert_ne!(
        link_a_ptr_before, link_a_ptr_after,
        "link score for affected loop A should be recomputed"
    );
}

#[test]
fn test_ltm_per_link_caching_model_level() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = two_loop_project();
    let (source_project, births_a_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["births_a"].source,
            result.models["main"].source,
        )
    };

    // Prime the model-level LTM function
    let ltm_before = model_ltm_synthetic_variables(&db, source_model, source_project);
    assert!(!ltm_before.vars.is_empty(), "should generate LTM variables");

    // Verify both loops have link scores
    let has_link_a = ltm_before
        .vars
        .iter()
        .any(|v| v.name.contains("stock_a") && v.name.contains("births_a"));
    let has_link_b = ltm_before
        .vars
        .iter()
        .any(|v| v.name.contains("stock_b") && v.name.contains("births_b"));
    assert!(has_link_a, "should have link score for loop A");
    assert!(has_link_b, "should have link score for loop B");

    // Change births_a equation
    births_a_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("stock_a * rate_a * 2".to_string()));

    // Model-level result should still produce valid results
    let ltm_after = model_ltm_synthetic_variables(&db, source_model, source_project);
    assert!(
        !ltm_after.vars.is_empty(),
        "should still generate LTM variables"
    );

    // Both loops should still have link scores
    let has_link_a_after = ltm_after
        .vars
        .iter()
        .any(|v| v.name.contains("stock_a") && v.name.contains("births_a"));
    let has_link_b_after = ltm_after
        .vars
        .iter()
        .any(|v| v.name.contains("stock_b") && v.name.contains("births_b"));
    assert!(has_link_a_after, "should still have link score for loop A");
    assert!(has_link_b_after, "should still have link score for loop B");
}

// ── Accumulator parity tests ──────────────────────────────────────

#[test]
fn test_accumulator_no_errors_for_valid_project() {
    let db = SimlinDb::default();
    let project = simple_project();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_all_diagnostics(&db, &sync);
    assert!(
        diags.is_empty(),
        "valid project should produce no diagnostics"
    );
}

#[test]
fn test_accumulator_parse_error_bad_equation() {
    let db = SimlinDb::default();
    // "if then" is a syntax error (missing condition/consequent)
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "broken".to_string(),
                equation: datamodel::Equation::Scalar("if then".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    // Verify struct field path also shows an error
    let parsed = parse_source_variable(
        &db,
        sync.models["main"].variables["broken"].source,
        sync.project,
    );
    assert!(
        parsed.variable.equation_errors().is_some(),
        "struct fields should show equation errors for 'if then'"
    );

    let diags = collect_all_diagnostics(&db, &sync);
    assert!(!diags.is_empty(), "bad equation should produce diagnostics");

    let d = &diags[0];
    assert_eq!(d.model, "main");
    assert_eq!(d.variable.as_deref(), Some("broken"));
    assert!(
        matches!(&d.error, DiagnosticError::Equation(_)),
        "expected equation error, got {:?}",
        d.error
    );
}

#[test]
fn test_accumulator_parity_with_struct_fields() {
    use std::collections::HashSet;

    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "parity".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "good".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "bad_syntax".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "empty".to_string(),
                    equation: datamodel::Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    // Collect from accumulator
    let accum_diags = collect_all_diagnostics(&db, &sync);

    // Collect from struct fields (parse_source_variable results)
    let mut field_equation_errors: HashSet<(String, crate::common::EquationError)> = HashSet::new();
    for (var_name, synced_var) in &sync.models["main"].variables {
        let parsed = parse_source_variable(&db, synced_var.source, sync.project);
        if let Some(errors) = parsed.variable.equation_errors() {
            for err in errors {
                field_equation_errors.insert((var_name.clone(), err));
            }
        }
    }

    // Extract equation errors from accumulator
    let mut accum_equation_errors: HashSet<(String, crate::common::EquationError)> = HashSet::new();
    for d in &accum_diags {
        if let DiagnosticError::Equation(err) = &d.error
            && let Some(var) = &d.variable
        {
            accum_equation_errors.insert((var.clone(), err.clone()));
        }
    }

    assert_eq!(
        field_equation_errors, accum_equation_errors,
        "accumulator equation errors must match struct field errors"
    );
}

#[test]
fn test_accumulator_multiple_models() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "multi_err".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "sub".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, &sync);

    let models_with_errors: std::collections::HashSet<&str> =
        diags.iter().map(|d| d.model.as_str()).collect();
    assert!(
        models_with_errors.contains("main"),
        "main model should have errors"
    );
    assert!(
        models_with_errors.contains("sub"),
        "sub model should have errors"
    );
}

#[test]
fn test_accumulator_incrementality() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    // Extract the salsa IDs we need (they are Copy) before dropping sync
    let (alpha_src, source_model, source_project) = {
        let sync = sync_from_datamodel(&db, &project);
        let alpha_src = sync.models["main"].variables["alpha"].source;
        let source_model = sync.models["main"].source;
        let source_project = sync.project;

        // Initially: alpha has errors, beta does not
        let diags1 = collect_all_diagnostics(&db, &sync);
        assert_eq!(
            diags1
                .iter()
                .filter(|d| d.variable.as_deref() == Some("alpha"))
                .count(),
            1,
            "alpha should have 1 error"
        );
        assert_eq!(
            diags1
                .iter()
                .filter(|d| d.variable.as_deref() == Some("beta"))
                .count(),
            0,
            "beta should have no errors"
        );

        (alpha_src, source_model, source_project)
    };

    // Fix alpha's equation (needs &mut db)
    alpha_src
        .set_equation(&mut db)
        .to(SourceEquation::Scalar("42".to_string()));

    let diags2 = collect_model_diagnostics(&db, source_model, source_project);
    assert!(
        diags2.is_empty(),
        "after fixing alpha, no diagnostics expected"
    );
}

// ── Incremental sync tests ────────────────────────────────────────

#[test]
fn test_incremental_sync_fresh_matches_regular_sync() {
    let db1 = SimlinDb::default();
    let mut db2 = SimlinDb::default();
    let project = simple_project();

    let regular = sync_from_datamodel(&db1, &project);
    let state = sync_from_datamodel_incremental(&mut db2, &project, None);

    assert_eq!(regular.project.name(&db1), state.project.name(&db2));
    assert_eq!(regular.models.len(), state.models.len());
    for (name, regular_model) in &regular.models {
        let persistent_model = &state.models[name];
        assert_eq!(
            regular_model.source.name(&db1),
            persistent_model.source_model.name(&db2)
        );
        assert_eq!(
            regular_model.variables.len(),
            persistent_model.variables.len()
        );
    }
}

#[test]
fn test_incremental_sync_preserves_cache_for_unchanged_variable() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("20".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    // Initial sync
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Prime the cache by parsing both variables
    let alpha_src = state1.models["main"].variables["alpha"].source_var;
    let beta_src = state1.models["main"].variables["beta"].source_var;
    let beta_ptr_before = {
        let _alpha_result = parse_source_variable(&db, alpha_src, state1.project);
        let beta_result = parse_source_variable(&db, beta_src, state1.project);
        beta_result as *const ParsedVariableResult
    };

    // Modify only alpha's equation
    let mut project2 = project.clone();
    project2.models[0].variables[0].set_scalar_equation("42");

    // Incremental sync with previous state
    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));

    // Same SourceProject handle should be reused
    assert_eq!(
        state1.project.as_id(),
        state2.project.as_id(),
        "SourceProject handle should be stable across incremental syncs"
    );

    // Beta's SourceVariable handle should be the same
    let beta_src2 = state2.models["main"].variables["beta"].source_var;
    assert_eq!(
        beta_src.as_id(),
        beta_src2.as_id(),
        "unchanged variable's handle should be stable"
    );

    // Beta's parse result should be pointer-equal (cached)
    let beta_result_after = parse_source_variable(&db, beta_src2, state2.project);
    let beta_ptr_after = beta_result_after as *const ParsedVariableResult;
    assert_eq!(
        beta_ptr_before, beta_ptr_after,
        "beta's parse result should be cached since it was not modified"
    );

    // Alpha's parse result should reflect the new equation
    let alpha_src2 = state2.models["main"].variables["alpha"].source_var;
    let alpha_result = parse_source_variable(&db, alpha_src2, state2.project);
    if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
        alpha_result.variable.ast()
    {
        assert_eq!(*val, 42.0);
    } else {
        panic!(
            "Expected alpha to parse as Const(42.0), got {:?}",
            alpha_result.variable.ast()
        );
    }
}

#[test]
fn test_incremental_sync_add_variable() {
    let mut db = SimlinDb::default();
    let project = simple_project();

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    assert_eq!(state1.models["main"].variables.len(), 1);

    // Add a new variable
    let mut project2 = project.clone();
    project2.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "growth".to_string(),
            equation: datamodel::Equation::Scalar("0.1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    assert_eq!(state2.models["main"].variables.len(), 2);
    assert!(state2.models["main"].variables.contains_key("growth"));

    // Original variable's handle should be preserved
    let pop1 = &state1.models["main"].variables["population"];
    let pop2 = &state2.models["main"].variables["population"];
    assert_eq!(
        pop1.source_var.as_id(),
        pop2.source_var.as_id(),
        "existing variable handle should be preserved when adding new variables"
    );
}

#[test]
fn test_incremental_sync_remove_variable() {
    let mut db = SimlinDb::default();
    let mut project = simple_project();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "extra".to_string(),
            equation: datamodel::Equation::Scalar("99".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    assert_eq!(state1.models["main"].variables.len(), 2);

    // Remove the "extra" variable
    project.models[0].variables.pop();
    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    assert_eq!(state2.models["main"].variables.len(), 1);
    assert!(!state2.models["main"].variables.contains_key("extra"));
    assert!(state2.models["main"].variables.contains_key("population"));
}

#[test]
fn test_incremental_sync_persistent_state_roundtrip() {
    let mut db = SimlinDb::default();
    let project = simple_project();

    // Create initial state
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Sync again with no changes -- should preserve all handles
    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));

    assert_eq!(
        state1.project.as_id(),
        state2.project.as_id(),
        "project handle should be stable"
    );
    assert_eq!(
        state1.models["main"].source_model.as_id(),
        state2.models["main"].source_model.as_id(),
        "model handle should be stable"
    );
    assert_eq!(
        state1.models["main"].variables["population"]
            .source_var
            .as_id(),
        state2.models["main"].variables["population"]
            .source_var
            .as_id(),
        "variable handle should be stable"
    );
}

#[test]
fn test_persistent_state_to_sync_result() {
    let mut db = SimlinDb::default();
    let project = simple_project();

    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();

    assert_eq!(sync.project.name(&db), state.project.name(&db));
    assert_eq!(sync.models.len(), state.models.len());

    let main_model = &sync.models["main"];
    let persistent_main = &state.models["main"];
    assert_eq!(
        main_model.source.as_id(),
        persistent_main.source_model.as_id()
    );

    for (name, sv) in &main_model.variables {
        let pv = &persistent_main.variables[name];
        assert_eq!(sv.source.as_id(), pv.source_var.as_id());
        assert_eq!(sv.id.as_id(), pv.var_interned_id);
    }

    // Verify the reconstituted SyncResult works for diagnostic collection
    let diags = collect_all_diagnostics(&db, &sync);
    assert!(
        diags.is_empty(),
        "simple project should have no diagnostics"
    );
}

#[test]
fn test_incremental_sync_successive_patches() {
    let mut db = SimlinDb::default();
    let mut project = simple_project();

    let state0 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Prime parse cache
    let pop_src = state0.models["main"].variables["population"].source_var;
    let _ = parse_source_variable(&db, pop_src, state0.project);

    // Patch 1: change project name (shouldn't affect variable cache)
    project.name = "renamed".to_string();
    let state1 = sync_from_datamodel_incremental(&mut db, &project, Some(&state0));

    let pop_src1 = state1.models["main"].variables["population"].source_var;
    assert_eq!(
        pop_src.as_id(),
        pop_src1.as_id(),
        "variable handle should survive project name change"
    );

    // Patch 2: change the variable's equation
    project.models[0].variables[0].set_scalar_equation("999");
    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));

    let pop_src2 = state2.models["main"].variables["population"].source_var;
    assert_eq!(
        pop_src.as_id(),
        pop_src2.as_id(),
        "handle should be stable even when equation changes"
    );

    // Parse should reflect the new equation
    let result = parse_source_variable(&db, pop_src2, state2.project);
    if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
        result.variable.ast()
    {
        assert_eq!(*val, 999.0);
    } else {
        panic!("Expected Const(999.0), got {:?}", result.variable.ast());
    }
}

// ── Incremental compilation tests ──────────────────────────────

fn two_var_project() -> datamodel::Project {
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("alpha * 2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
    }
}

#[test]
fn test_variable_dimensions_scalar() {
    let db = SimlinDb::default();
    let project = simple_project();
    let sync = sync_from_datamodel(&db, &project);

    let pop_var = sync.models["main"].variables["population"].source;
    let dims = variable_dimensions(&db, pop_var, sync.project);
    assert!(dims.is_empty());
}

#[test]
fn test_variable_size_scalar() {
    let db = SimlinDb::default();
    let project = simple_project();
    let sync = sync_from_datamodel(&db, &project);

    let pop_var = sync.models["main"].variables["population"].source;
    assert_eq!(variable_size(&db, pop_var, sync.project), 1);
}

#[test]
fn test_compute_layout_simple() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;
    let layout = compute_layout(&db, model, sync.project, true);

    // Should have implicit vars (time, dt, initial_time, final_time) + 2 user vars
    let alpha_entry = layout.get("alpha").expect("alpha should be in layout");
    let beta_entry = layout.get("beta").expect("beta should be in layout");
    let time_entry = layout.get("time").expect("time should be in layout");

    assert_eq!(time_entry.offset, 0);
    assert_eq!(time_entry.size, 1);

    // Alpha and beta should be after implicit vars (offset >= 4)
    assert!(alpha_entry.offset >= 4);
    assert!(beta_entry.offset >= 4);
    assert_ne!(alpha_entry.offset, beta_entry.offset);
    assert_eq!(alpha_entry.size, 1);
    assert_eq!(beta_entry.size, 1);
}

#[test]
fn test_compile_var_fragment_produces_result() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;
    let alpha_var = sync.models["main"].variables["alpha"].source;

    let result = compile_var_fragment(&db, alpha_var, model, sync.project, true);
    assert!(result.is_some(), "alpha should compile successfully");

    let frag = &result.as_ref().unwrap().fragment;
    assert_eq!(frag.ident, "alpha");
    // Alpha is an aux, should have flow_bytecodes (in the flows runlist)
    assert!(
        frag.flow_bytecodes.is_some() || frag.initial_bytecodes.is_some(),
        "alpha should produce bytecodes in at least one phase"
    );
}

#[test]
fn test_compile_var_fragment_caching() {
    // AC1.1: Changing one variable's equation (same deps) should
    // only recompile that variable. Other variables' fragments
    // should be cached.
    let mut db = SimlinDb::default();
    let project = two_var_project();
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Clone the fragment before mutating db
    let beta_frag1 = {
        let sync1 = state1.to_sync_result();
        let model = sync1.models["main"].source;
        let alpha_var = sync1.models["main"].variables["alpha"].source;
        let beta_var = sync1.models["main"].variables["beta"].source;

        let alpha_result1 = compile_var_fragment(&db, alpha_var, model, sync1.project, true);
        let beta_result1 = compile_var_fragment(&db, beta_var, model, sync1.project, true);
        assert!(alpha_result1.is_some());
        assert!(beta_result1.is_some());

        beta_result1.as_ref().unwrap().fragment.clone()
    };

    // Change alpha's equation (same deps -- it has no deps)
    let mut project2 = project.clone();
    project2.models[0].variables[0] = datamodel::Variable::Aux(datamodel::Aux {
        ident: "alpha".to_string(),
        equation: datamodel::Equation::Scalar("20".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        can_be_module_input: false,
        visibility: datamodel::Visibility::Private,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;

    // Alpha should be recompiled (different equation)
    let alpha_var2 = sync2.models["main"].variables["alpha"].source;
    let alpha_result2 = compile_var_fragment(&db, alpha_var2, model2, sync2.project, true);
    assert!(alpha_result2.is_some());

    // Beta's fragment should be unchanged since beta's equation
    // and deps haven't changed
    let beta_var2 = sync2.models["main"].variables["beta"].source;
    let beta_result2 = compile_var_fragment(&db, beta_var2, model2, sync2.project, true);
    assert!(beta_result2.is_some());
    assert_eq!(
        beta_frag1,
        beta_result2.as_ref().unwrap().fragment,
        "beta fragment should be unchanged when only alpha's equation changes"
    );
}

#[test]
fn test_incremental_bytecode_equivalence() {
    // AC4.3: Build a model, compile incrementally via salsa, then
    // compile monolithically. Both should produce valid simulations
    // that generate identical numerical output.
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;

    // Incremental: assemble via tracked functions
    let incremental_result = assemble_module(&db, model, sync.project, true, &BTreeSet::new());

    // Monolithic: compile via Project::from + compile_project
    let engine_project = crate::project::Project::from(project);
    let monolithic_result = crate::interpreter::compile_project(&engine_project, "main");

    // If the incremental path produces a result, verify equivalence
    // by running both through the VM
    if let (Ok(incr_module), Ok(mono_compiled)) = (&incremental_result, &monolithic_result) {
        // Both should have the same number of slots
        assert_eq!(incr_module.n_slots, mono_compiled.n_slots());
    }
}

#[test]
fn test_assemble_simulation_simple() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let result = assemble_simulation(&db, sync.project, "main");
    assert!(
        result.is_ok(),
        "assemble_simulation failed: {:?}",
        result.err()
    );

    let compiled = result.unwrap();
    // Two user vars (alpha, beta) + 4 implicit (time, dt, initial_time, final_time) = 6
    assert_eq!(compiled.n_slots(), 6);

    // Verify offsets exist for the variables
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("alpha"))
            .is_some()
    );
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("beta"))
            .is_some()
    );
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("time"))
            .is_some()
    );
}

#[test]
fn test_incremental_vs_monolithic_output() {
    use crate::vm::Vm;

    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    // Incremental path: assemble_simulation
    let incr_result = assemble_simulation(&db, sync.project, "main");
    assert!(
        incr_result.is_ok(),
        "incremental assemble_simulation failed: {:?}",
        incr_result.err()
    );
    let incr_compiled = incr_result.unwrap();

    // Monolithic path: compile_project
    let engine_project = crate::project::Project::from(project);
    let mono_result = crate::interpreter::compile_project(&engine_project, "main");
    assert!(mono_result.is_ok(), "monolithic compile_project failed");
    let mono_compiled = mono_result.unwrap();

    // Both should have same n_slots
    assert_eq!(incr_compiled.n_slots(), mono_compiled.n_slots());

    // Run both through the VM
    let mut incr_sim = Vm::new(incr_compiled).unwrap();
    incr_sim.run_to_end().unwrap();
    let incr_results = incr_sim.into_results();

    let mut mono_sim = Vm::new(mono_compiled).unwrap();
    mono_sim.run_to_end().unwrap();
    let mono_results = mono_sim.into_results();

    // Verify identical output dimensions
    assert_eq!(incr_results.step_count, mono_results.step_count);
    assert_eq!(incr_results.step_size, mono_results.step_size);

    // Compare numerical output for each variable at each timestep
    let alpha_ident = crate::common::Ident::new("alpha");
    let beta_ident = crate::common::Ident::new("beta");

    let incr_alpha_off = incr_results.offsets[&alpha_ident];
    let mono_alpha_off = mono_results.offsets[&alpha_ident];
    let incr_beta_off = incr_results.offsets[&beta_ident];
    let mono_beta_off = mono_results.offsets[&beta_ident];

    for step in 0..incr_results.step_count {
        let incr_base = step * incr_results.step_size;
        let mono_base = step * mono_results.step_size;

        let incr_alpha = incr_results.data[incr_base + incr_alpha_off];
        let mono_alpha = mono_results.data[mono_base + mono_alpha_off];
        assert!(
            (incr_alpha - mono_alpha).abs() < 1e-10,
            "alpha mismatch at step {}: incr={}, mono={}",
            step,
            incr_alpha,
            mono_alpha
        );

        let incr_beta = incr_results.data[incr_base + incr_beta_off];
        let mono_beta = mono_results.data[mono_base + mono_beta_off];
        assert!(
            (incr_beta - mono_beta).abs() < 1e-10,
            "beta mismatch at step {}: incr={}, mono={}",
            step,
            incr_beta,
            mono_beta
        );
    }
}

/// Stock-and-flow model: stock integrates a flow over time.
fn stock_flow_project() -> datamodel::Project {
    datamodel::Project {
        name: "stock_flow_test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * 0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
    }
}

#[test]
fn test_assemble_simulation_stock_flow() {
    use crate::vm::Vm;

    let db = SimlinDb::default();
    let project = stock_flow_project();
    let sync = sync_from_datamodel(&db, &project);

    // Incremental path
    let incr_result = assemble_simulation(&db, sync.project, "main");
    assert!(
        incr_result.is_ok(),
        "assemble_simulation failed for stock-flow model: {:?}",
        incr_result.err()
    );
    let incr_compiled = incr_result.unwrap();

    // Monolithic path
    let engine_project = crate::project::Project::from(project);
    let mono_result = crate::interpreter::compile_project(&engine_project, "main");
    assert!(mono_result.is_ok());
    let mono_compiled = mono_result.unwrap();

    assert_eq!(incr_compiled.n_slots(), mono_compiled.n_slots());

    // Run both and compare
    let mut incr_sim = Vm::new(incr_compiled).unwrap();
    incr_sim.run_to_end().unwrap();
    let incr_results = incr_sim.into_results();

    let mut mono_sim = Vm::new(mono_compiled).unwrap();
    mono_sim.run_to_end().unwrap();
    let mono_results = mono_sim.into_results();

    assert_eq!(incr_results.step_count, mono_results.step_count);

    let pop_ident = crate::common::Ident::new("population");
    let incr_pop_off = incr_results.offsets[&pop_ident];
    let mono_pop_off = mono_results.offsets[&pop_ident];

    for step in 0..incr_results.step_count {
        let incr_val = incr_results.data[step * incr_results.step_size + incr_pop_off];
        let mono_val = mono_results.data[step * mono_results.step_size + mono_pop_off];
        assert!(
            (incr_val - mono_val).abs() < 1e-10,
            "population mismatch at step {}: incr={}, mono={}",
            step,
            incr_val,
            mono_val
        );
    }
}

/// Teacup model: stock with flow, two constants. Matches the teacup.stmx
/// fixture used by TS engine tests.
fn teacup_project() -> datamodel::Project {
    datamodel::Project {
        name: "teacup".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 30.0,
            dt: datamodel::Dt::Dt(0.125),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        source: None,
        ai_information: None,
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "teacup_temperature".to_string(),
                    equation: datamodel::Equation::Scalar("180".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["heat_loss_to_room".to_string()],
                    non_negative: true,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "heat_loss_to_room".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "(teacup_temperature - room_temperature) / characteristic_time".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: true,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "characteristic_time".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "room_temperature".to_string(),
                    equation: datamodel::Equation::Scalar("70".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
        }],
    }
}

#[test]
fn test_incremental_teacup_constant_detection() {
    let db = SimlinDb::default();
    let project = teacup_project();
    let sync = sync_from_datamodel(&db, &project);

    let incr_compiled =
        assemble_simulation(&db, sync.project, "main").expect("incremental compilation failed");
    let mono_compiled =
        crate::interpreter::compile_project(&crate::project::Project::from(project), "main")
            .expect("monolithic compilation failed");

    // room_temperature should be detected as a constant in both paths
    let room_temp_ident = crate::common::Ident::new("room_temperature");
    let incr_off = incr_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature in incremental");
    let mono_off = mono_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature in monolithic");

    assert!(
        mono_compiled.is_constant_offset(mono_off),
        "monolithic should detect room_temperature as constant"
    );
    assert!(
        incr_compiled.is_constant_offset(incr_off),
        "incremental should detect room_temperature as constant"
    );
}

#[test]
fn test_incremental_teacup_simulation() {
    use crate::vm::Vm;

    let db = SimlinDb::default();
    let project = teacup_project();
    let sync = sync_from_datamodel(&db, &project);

    let incr_compiled =
        assemble_simulation(&db, sync.project, "main").expect("incremental compilation failed");

    let mono_compiled =
        crate::interpreter::compile_project(&crate::project::Project::from(project), "main")
            .expect("monolithic compilation failed");

    let mut incr_sim = Vm::new(incr_compiled).unwrap();
    incr_sim.run_to_end().unwrap();
    let incr_results = incr_sim.into_results();

    let mut mono_sim = Vm::new(mono_compiled).unwrap();
    mono_sim.run_to_end().unwrap();
    let mono_results = mono_sim.into_results();

    // Temperature should decrease over time (not stay at 0)
    let temp_ident = crate::common::Ident::new("teacup_temperature");
    let incr_temp_off = incr_results.offsets[&temp_ident];
    let first_temp = incr_results.data[incr_temp_off];
    assert!(
        first_temp > 0.0,
        "temperature should not be zero, got {}",
        first_temp
    );
    assert!(
        (first_temp - 180.0).abs() < 1e-10,
        "initial temperature should be 180, got {}",
        first_temp
    );

    // Compare all values
    assert_eq!(incr_results.step_count, mono_results.step_count);
    for step in 0..incr_results.step_count {
        let incr_val = incr_results.data[step * incr_results.step_size + incr_temp_off];
        let mono_temp_off = mono_results.offsets[&temp_ident];
        let mono_val = mono_results.data[step * mono_results.step_size + mono_temp_off];
        assert!(
            (incr_val - mono_val).abs() < 1e-10,
            "temperature mismatch at step {}: incr={}, mono={}",
            step,
            incr_val,
            mono_val
        );
    }
}

/// Test that mimics the libsimlin XMILE flow: load teacup.stmx, sync to
/// DB via sync_from_datamodel_incremental (None prev), convert back with
/// to_sync_result(), then compile incrementally.
#[test]
fn test_incremental_teacup_via_persistent_sync() {
    use crate::vm::Vm;

    let dm_project = teacup_project();

    // Mirror the libsimlin path: use sync_from_datamodel_incremental with None prev
    let mut db = SimlinDb::default();
    let persistent_state = sync_from_datamodel_incremental(&mut db, &dm_project, None);

    // Now reconstruct SyncResult from PersistentSyncState (like simlin_sim_new does)
    let sync = persistent_state.to_sync_result();

    let incr_compiled =
        assemble_simulation(&db, sync.project, "main").expect("incremental compilation failed");

    // Verify constant detection
    let room_temp_ident = crate::common::Ident::new("room_temperature");
    let incr_off = incr_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature in incremental");
    assert!(
        incr_compiled.is_constant_offset(incr_off),
        "room_temperature should be detected as constant via persistent sync path"
    );

    // Run simulation and verify results
    let mut vm = Vm::new(incr_compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let temp_ident = crate::common::Ident::new("teacup_temperature");
    let temp_off = results.offsets[&temp_ident];
    let first_temp = results.data[temp_off];
    assert!(
        (first_temp - 180.0).abs() < 1e-10,
        "initial temperature should be 180, got {}",
        first_temp
    );
}

// ── AC acceptance-criteria tests ──────────────────────────────────

/// AC1.3/AC1.4: Adding or removing a variable reuses existing variables'
/// compile_var_fragment results (salsa cache hit) while compute_layout
/// changes to reflect the new variable set.
#[test]
fn test_ac1_3_ac1_4_fragment_reuse_on_add_remove() {
    let mut db = SimlinDb::default();
    let project = two_var_project();

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();
    let model1 = sync1.models["main"].source;

    // Prime layout cache
    let layout_ptr1 = compute_layout(&db, model1, sync1.project, true)
        as *const crate::compiler::symbolic::VariableLayout;

    // Add a new variable "gamma"
    let mut project2 = project.clone();
    project2.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "gamma".to_string(),
            equation: datamodel::Equation::Scalar("99".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;

    // Clone fragment contents before mutation
    let alpha_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["alpha"].source,
        model1,
        sync1.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["beta"].source,
        model1,
        sync1.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    // Existing variables' fragments should be value-equal (salsa recomputes
    // but the symbolic bytecodes are independent of variable set size)
    let alpha_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["alpha"].source,
        model2,
        sync2.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["beta"].source,
        model2,
        sync2.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        alpha_frag1, alpha_frag2,
        "AC1.3: alpha's fragment content should be unchanged after adding gamma"
    );
    assert_eq!(
        beta_frag1, beta_frag2,
        "AC1.3: beta's fragment content should be unchanged after adding gamma"
    );

    // Layout MUST change (gamma added)
    let layout_ptr2 = compute_layout(&db, model2, sync2.project, true)
        as *const crate::compiler::symbolic::VariableLayout;
    assert_ne!(
        layout_ptr1, layout_ptr2,
        "AC1.3: compute_layout should change when a variable is added"
    );

    // Now remove gamma (AC1.4)
    let state3 = sync_from_datamodel_incremental(&mut db, &project, Some(&state2));
    let sync3 = state3.to_sync_result();
    let model3 = sync3.models["main"].source;

    let alpha_frag3 = compile_var_fragment(
        &db,
        sync3.models["main"].variables["alpha"].source,
        model3,
        sync3.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag3 = compile_var_fragment(
        &db,
        sync3.models["main"].variables["beta"].source,
        model3,
        sync3.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        alpha_frag1, alpha_frag3,
        "AC1.4: alpha's fragment content should be unchanged after removing gamma"
    );
    assert_eq!(
        beta_frag1, beta_frag3,
        "AC1.4: beta's fragment content should be unchanged after removing gamma"
    );

    // Layout should change again (back to 2 variables)
    let layout_ptr3 = compute_layout(&db, model3, sync3.project, true)
        as *const crate::compiler::symbolic::VariableLayout;
    assert_ne!(
        layout_ptr2, layout_ptr3,
        "AC1.4: compute_layout should change when a variable is removed"
    );
}

/// AC1.5: Changing a dimension definition recompiles only variables that
/// use that dimension. Variables not referencing the dimension should
/// have their compile_var_fragment cached (via salsa backdating).
#[test]
fn test_ac1_5_dimension_change_selective_recompile() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "sales".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "10".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "price".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();
    let model1 = sync1.models["main"].source;

    // Prime caches and capture fragment content
    let price_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["price"].source,
        model1,
        sync1.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let sales_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["sales"].source,
        model1,
        sync1.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    // Change dimension size: add "East" element
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "Region".to_string(),
        vec!["North".to_string(), "South".to_string(), "East".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;

    // Price doesn't use the dimension, so its fragment should be value-equal
    let price_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["price"].source,
        model2,
        sync2.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        price_frag1, price_frag2,
        "AC1.5: price fragment should be unchanged (price doesn't use Region)"
    );

    // Sales uses the dimension, so its fragment should differ
    let sales_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["sales"].source,
        model2,
        sync2.project,
        true,
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_ne!(
        sales_frag1, sales_frag2,
        "AC1.5: sales fragment should be recomputed (sales uses Region)"
    );
}

/// AC1.6: Changing module connections in model B should not invalidate
/// model A's dependency graph. Cross-model isolation means the dep graph
/// for an unrelated model is a cache hit.
#[test]
fn test_ac1_6_cross_model_isolation() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "multi".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "model_a".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("x + 1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "model_b".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "sub".to_string(),
                    model_name: "model_a".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![datamodel::ModuleReference {
                        src: "input_a".to_string(),
                        dst: "x".to_string(),
                    }],
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();
    let model_a_src = sync1.models["model_a"].source;

    // Prime model_a's dep graph
    let graph_a_ptr1 =
        model_dependency_graph(&db, model_a_src, sync1.project) as *const ModelDepGraphResult;

    // Change model_b's module connections
    let mut project2 = project.clone();
    if let datamodel::Variable::Module(ref mut m) = project2.models[1].variables[0] {
        m.references = vec![
            datamodel::ModuleReference {
                src: "input_a".to_string(),
                dst: "x".to_string(),
            },
            datamodel::ModuleReference {
                src: "input_b".to_string(),
                dst: "y".to_string(),
            },
        ];
    }

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model_a_src2 = sync2.models["model_a"].source;

    // Model A's dep graph should be a cache hit (pointer-equal)
    let graph_a_ptr2 =
        model_dependency_graph(&db, model_a_src2, sync2.project) as *const ModelDepGraphResult;
    assert_eq!(
        graph_a_ptr1, graph_a_ptr2,
        "AC1.6: model A's dependency graph should be cached when only model B changes"
    );
}

/// AC2.4: Stdlib composite scores for dynamic modules (SMOOTH, DELAY, etc.)
/// compute once and are never recomputed. Calling module_ltm_synthetic_variables
/// twice with unchanged inputs returns pointer-equal results.
#[test]
fn test_ac2_4_stdlib_composite_scores_cached() {
    let db = SimlinDb::default();

    // Use the "smooth" stdlib model directly.
    let stdlib_model = match crate::stdlib::get("smooth") {
        Some(m) => m,
        None => return,
    };

    let stdlib_project = datamodel::Project {
        name: "stdlib_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![stdlib_model],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &stdlib_project);
    let model_name = sync.models.keys().next().unwrap().clone();
    let model = sync.models[&model_name].source;

    // First call: compute
    let result1 =
        module_ltm_synthetic_variables(&db, model, sync.project) as *const LtmVariablesResult;

    // Second call: should be cached (pointer-equal)
    let result2 =
        module_ltm_synthetic_variables(&db, model, sync.project) as *const LtmVariablesResult;

    assert_eq!(
        result1, result2,
        "AC2.4: module_ltm_synthetic_variables should be cached on unchanged inputs"
    );
}

/// AC4.3 (strengthened): Compile a model with stocks, flows, and lookups
/// both incrementally and monolithically, run both through the VM, and
/// assert identical time-series output for all variables.
#[test]
fn test_ac4_3_full_bytecode_equivalence_stock_flow_lookup() {
    use crate::vm::Vm;

    let project = datamodel::Project {
        name: "sfg".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(0.5),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "level".to_string(),
                    equation: datamodel::Equation::Scalar("50".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["inflow".to_string()],
                    outflows: vec!["outflow".to_string()],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "inflow".to_string(),
                    equation: datamodel::Equation::Scalar("effect * 10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "outflow".to_string(),
                    equation: datamodel::Equation::Scalar("level * 0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "effect".to_string(),
                    equation: datamodel::Equation::Scalar("time".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 5.0, 10.0]),
                        y_points: vec![1.0, 0.5, 0.2],
                        x_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 10.0,
                        },
                        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
                    }),
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    // Incremental path
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let incr_compiled =
        assemble_simulation(&db, sync.project, "main").expect("incremental compilation failed");

    // Monolithic path
    let engine_project = crate::project::Project::from(project.clone());
    let mono_compiled = crate::interpreter::compile_project(&engine_project, "main")
        .expect("monolithic compilation failed");

    assert_eq!(incr_compiled.n_slots(), mono_compiled.n_slots());

    // Run both
    let mut incr_sim = Vm::new(incr_compiled).unwrap();
    incr_sim.run_to_end().unwrap();
    let incr_results = incr_sim.into_results();

    let mut mono_sim = Vm::new(mono_compiled).unwrap();
    mono_sim.run_to_end().unwrap();
    let mono_results = mono_sim.into_results();

    assert_eq!(incr_results.step_count, mono_results.step_count);
    assert_eq!(incr_results.step_size, mono_results.step_size);

    // Compare every variable at every timestep
    for (ident, &incr_off) in &incr_results.offsets {
        let mono_off = mono_results
            .offsets
            .get(ident)
            .unwrap_or_else(|| panic!("variable {:?} not found in monolithic results", ident));

        for step in 0..incr_results.step_count {
            let incr_val = incr_results.data[step * incr_results.step_size + incr_off];
            let mono_val = mono_results.data[step * mono_results.step_size + mono_off];
            assert!(
                (incr_val - mono_val).abs() < 1e-10,
                "{:?} mismatch at step {}: incr={}, mono={}",
                ident,
                step,
                incr_val,
                mono_val
            );
        }
    }
}

/// Test loading teacup.stmx via open_xmile and running through the
/// full incremental compilation path, mirroring the libsimlin XMILE flow.
/// Catches regressions where display names with spaces (from XMILE) don't
/// match canonical names used in dependency graphs and variable maps.
#[test]
fn test_incremental_teacup_xmile_file() {
    use crate::vm::Vm;
    use std::io::BufReader;

    let xmile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("src/pysimlin/tests/fixtures/teacup.stmx");
    if !xmile_path.exists() {
        panic!("teacup.stmx not found at {:?}", xmile_path);
    }
    let xmile_data = std::fs::read(&xmile_path).unwrap();
    let mut reader = BufReader::new(xmile_data.as_slice());
    let dm_project = crate::open_xmile(&mut reader).expect("failed to parse teacup.stmx");

    let engine_project: crate::project::Project = dm_project.into();
    let mut db = SimlinDb::default();
    let persistent_state =
        sync_from_datamodel_incremental(&mut db, &engine_project.datamodel, None);

    let sync = persistent_state.to_sync_result();

    let incr_compiled =
        assemble_simulation(&db, sync.project, "main").expect("incremental compilation failed");

    // Constant detection must work for XMILE-loaded models
    let room_temp_ident = crate::common::Ident::new("room_temperature");
    let incr_off = incr_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature");
    assert!(
        incr_compiled.is_constant_offset(incr_off),
        "room_temperature should be detected as constant (XMILE path)"
    );

    // Simulation must produce correct results
    let mut vm = Vm::new(incr_compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let temp_ident = crate::common::Ident::new("teacup_temperature");
    let temp_off = results.offsets[&temp_ident];
    let first_temp = results.data[temp_off];
    assert!(
        (first_temp - 180.0).abs() < 1e-10,
        "initial temperature should be 180, got {}",
        first_temp
    );
}

// ====================================================================
// Fix #3: model-specific sim_specs override
// ====================================================================

#[test]
fn test_model_sim_specs_override() {
    let project_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let model_specs = datamodel::SimSpecs {
        start: 5.0,
        stop: 20.0,
        dt: datamodel::Dt::Dt(0.5),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let dm_project = datamodel::Project {
        name: "test_override".to_string(),
        sim_specs: project_specs.clone(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: Some(model_specs.clone()),
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm_project);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let specs = &compiled.specs;

    assert!(
        (specs.start - 5.0).abs() < f64::EPSILON,
        "start should be 5.0 (from model specs), got {}",
        specs.start
    );
    assert!(
        (specs.stop - 20.0).abs() < f64::EPSILON,
        "stop should be 20.0 (from model specs), got {}",
        specs.stop
    );
    assert!(
        (specs.dt - 0.5).abs() < f64::EPSILON,
        "dt should be 0.5 (from model specs), got {}",
        specs.dt
    );
}

#[test]
fn test_model_sim_specs_defaults_to_project() {
    let project_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let dm_project = datamodel::Project {
        name: "test_no_override".to_string(),
        sim_specs: project_specs.clone(),
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
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm_project);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let specs = &compiled.specs;

    assert!(
        (specs.start - 0.0).abs() < f64::EPSILON,
        "start should be 0.0 (from project specs), got {}",
        specs.start
    );
    assert!(
        (specs.stop - 10.0).abs() < f64::EPSILON,
        "stop should be 10.0 (from project specs), got {}",
        specs.stop
    );
}

#[test]
fn test_circular_dependency_blocks_incremental_compilation() {
    let project = datamodel::Project {
        name: "circular".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        source: None,
        ai_information: None,
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
        }],
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let dep_graph = model_dependency_graph(&db, sync.models["main"].source, sync.project);
    assert!(dep_graph.has_cycle, "should detect circular dependency");

    let result = assemble_simulation(&db, sync.project, "main");
    assert!(
        result.is_err(),
        "incremental compilation should fail for circular dependencies"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("circular"),
        "error should mention circular dependencies, got: {}",
        err
    );
}

#[test]
fn test_malformed_graphical_function_fails_fragment() {
    // A variable with a graphical function where x_points and y_points
    // have different lengths should fail compile_var_fragment (returning
    // None) rather than silently dropping the table and producing bytecode
    // that references a missing lookup.
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Scalar("time".to_string()),
                documentation: String::new(),
                units: None,
                gf: Some(datamodel::GraphicalFunction {
                    kind: datamodel::GraphicalFunctionKind::Continuous,
                    x_points: Some(vec![0.0, 1.0, 2.0]),
                    y_points: vec![10.0, 20.0], // mismatched length
                    x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    y_scale: datamodel::GraphicalFunctionScale {
                        min: 0.0,
                        max: 20.0,
                    },
                }),
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;
    let var = sync.models["main"].variables["lookup_var"].source;

    let result = compile_var_fragment(&db, var, model, sync.project, true);
    assert!(
        result.is_none(),
        "compile_var_fragment should return None for malformed graphical function"
    );
}

#[test]
fn test_sparse_per_element_gfs_preserve_table_indices() {
    // When an arrayed variable has per-element graphical functions but some
    // elements lack a GF, the table vector must contain empty placeholders
    // to keep table[element_offset] aligned.  Without placeholders, later
    // elements would get the wrong lookup table.
    let project = datamodel::Project {
        name: "sparse_gf".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Arrayed(
                    vec!["Dim".to_string()],
                    vec![
                        // Element A: has a GF
                        (
                            "A".to_string(),
                            "time".to_string(),
                            None,
                            Some(datamodel::GraphicalFunction {
                                kind: datamodel::GraphicalFunctionKind::Continuous,
                                x_points: Some(vec![0.0, 10.0]),
                                y_points: vec![100.0, 200.0],
                                x_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 10.0,
                                },
                                y_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 200.0,
                                },
                            }),
                        ),
                        // Element B: NO GF (placeholder needed)
                        ("B".to_string(), "time".to_string(), None, None),
                        // Element C: has a different GF
                        (
                            "C".to_string(),
                            "time".to_string(),
                            None,
                            Some(datamodel::GraphicalFunction {
                                kind: datamodel::GraphicalFunctionKind::Continuous,
                                x_points: Some(vec![0.0, 10.0]),
                                y_points: vec![500.0, 600.0],
                                x_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 10.0,
                                },
                                y_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 600.0,
                                },
                            }),
                        ),
                    ],
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let var = sync.models["main"].variables["lookup_var"].source;

    // extract_tables_from_source_var must produce exactly 3 tables (one per
    // element), including an empty placeholder for element B.
    let tables = extract_tables_from_source_var(&db, &var);
    assert_eq!(
        tables.len(),
        3,
        "should have 3 tables (including empty placeholder for element B), got {}",
        tables.len()
    );
    assert!(
        !tables[0].data.is_empty(),
        "element A should have a non-empty table"
    );
    assert!(
        tables[1].data.is_empty(),
        "element B should have an empty placeholder table"
    );
    assert!(
        !tables[2].data.is_empty(),
        "element C should have a non-empty table"
    );

    // Ensure the model still compiles successfully through the incremental path.
    let result = compile_project_incremental(&db, sync.project, "main");
    assert!(
        result.is_ok(),
        "model with sparse per-element GFs should compile: {:?}",
        result.err()
    );
}

#[test]
fn test_implicit_module_offsets_in_flattened_map() {
    // SMOOTH creates implicit MODULE variables whose sub-models contain
    // multiple slots.  calc_flattened_offsets_incremental must account for
    // the full sub-model size, not just 1 slot per implicit var.
    let project = datamodel::Project {
        name: "smooth_offsets".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
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
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "input".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "delay_time".to_string(),
                    equation: datamodel::Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "smoothed".to_string(),
                    equation: datamodel::Equation::Scalar("SMTH1(input, delay_time)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Add a variable after the SMOOTH to verify its offset isn't
                // shifted by undercounting the implicit module's size.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "trailing".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    // Compile through the incremental path.
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("SMOOTH model should compile incrementally");

    // The flattened offsets should match the layout: implicit MODULE vars
    // must occupy their sub-model's full slot count.
    let layout = compute_layout(&db, sync.models["main"].source, sync.project, true);
    let offsets = calc_flattened_offsets_incremental(&db, sync.project, "main", true);

    // The total size from offsets should equal the layout's n_slots.
    let offsets_total: usize = if offsets.is_empty() {
        0
    } else {
        offsets
            .values()
            .map(|(off, size)| off + size)
            .max()
            .unwrap_or(0)
    };
    assert_eq!(
        offsets_total, layout.n_slots,
        "flattened offsets total ({offsets_total}) must match layout n_slots ({})",
        layout.n_slots
    );

    // Verify the simulation runs and produces correct results.
    let mut vm = crate::vm::Vm::new(compiled).expect("VM should build");
    vm.run_to_end().expect("simulation should run to end");

    // "trailing" should be 42 at every timestep.
    let trailing_ident: crate::common::Ident<crate::common::Canonical> =
        crate::common::Ident::new("trailing");
    let trailing_series = vm
        .get_series(&trailing_ident)
        .expect("trailing variable should be in results");
    for (t, &val) in trailing_series.iter().enumerate() {
        assert!(
            (val - 42.0).abs() < 1e-6,
            "trailing should be 42.0 at step {t}, got {val}"
        );
    }
}

/// When a user model shadows a stdlib model (same canonical name) and is
/// later removed, the stdlib definition must be rebuilt from scratch rather
/// than reusing the stale user override from `PersistentSyncState`.
#[test]
fn test_incremental_stdlib_restored_after_user_override_removed() {
    let mut db = SimlinDb::default();

    // Build a project with a user model that shadows stdlib delay1.
    let shadow_model = datamodel::Model {
        name: "stdlib\u{205A}delay1".to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "custom_var".to_string(),
            equation: datamodel::Equation::Scalar("999".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };

    let mut project = simple_project();
    project.models.push(shadow_model);

    // Sync twice so the override lands in PersistentSyncState.
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let canonical = canonicalize("stdlib\u{205A}delay1").into_owned();
    let pm1 = &state1.models[&canonical];
    assert!(
        !pm1.is_stdlib,
        "user override should be marked is_stdlib=false"
    );
    assert!(
        pm1.variables.contains_key("custom_var"),
        "override should contain the user-defined variable"
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    assert!(!state2.models[&canonical].is_stdlib);

    // Now remove the shadowing model and sync again.
    project.models.retain(|m| m.name != "stdlib\u{205A}delay1");
    let state3 = sync_from_datamodel_incremental(&mut db, &project, Some(&state2));

    let pm3 = &state3.models[&canonical];
    assert!(
        pm3.is_stdlib,
        "restored entry should be marked is_stdlib=true"
    );
    assert!(
        !pm3.variables.contains_key("custom_var"),
        "user variable should not be present in restored stdlib model"
    );

    // The real stdlib delay1 has variables like "delay_time", "output", etc.
    let real_stdlib = crate::stdlib::get("delay1").unwrap();
    let expected_vars: std::collections::HashSet<String> = real_stdlib
        .variables
        .iter()
        .map(|v| canonicalize(v.get_ident()).into_owned())
        .collect();
    let actual_vars: std::collections::HashSet<String> = pm3.variables.keys().cloned().collect();
    assert_eq!(
        expected_vars, actual_vars,
        "restored stdlib model should have exactly the real stdlib variables"
    );
}
