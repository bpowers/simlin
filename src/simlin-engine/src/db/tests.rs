// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use salsa::plumbing::AsId;

/// Parse with an empty module-ident context (test convenience).
fn parse_var_no_module_ctx(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> &ParsedVariableResult {
    parse_source_variable_with_module_context(db, var, project, ModuleIdentContext::new(db, vec![]))
}

/// Direct dependencies with an empty module-ident context and no module
/// inputs -- the default (input-agnostic) path the old no-arg
/// `variable_direct_dependencies` took. A test convenience.
fn deps_no_inputs(db: &dyn Db, var: SourceVariable, project: SourceProject) -> &VariableDeps {
    variable_direct_dependencies(
        db,
        var,
        project,
        ModuleIdentContext::new(db, vec![]),
        ModuleInputSet::empty(db),
    )
}

pub(crate) fn simple_project() -> datamodel::Project {
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

#[test]
fn test_create_db() {
    let _db = SimlinDb::default();
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
    // 1 user model + the remaining stdlib models (PREVIOUS/INIT are intrinsic).
    assert_eq!(
        result.project.model_names(&db).len(),
        1 + crate::stdlib::MODEL_NAMES.len()
    );

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
    assert_eq!(pop_var.source.kind(&db), SourceVariableKind::Aux);
    assert_eq!(pop_var.source.units(&db), &Some("people".to_string()));
    assert_eq!(
        pop_var.source.equation(&db),
        &datamodel::Equation::Scalar("100".to_string())
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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    assert_eq!(result.models.len(), 2 + crate::stdlib::MODEL_NAMES.len(),);
    assert!(result.models.contains_key("main"));
    assert!(result.models.contains_key("submodel"));

    // Different models get distinct SourceModel input handles
    let main_source = result.models["main"].source;
    let sub_source = result.models["submodel"].source;
    assert_ne!(main_source.as_id(), sub_source.as_id());
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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        non_negative: true,
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "flow_var".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        non_negative: true,
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "aux_var".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
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
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: None,
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

    let result = sync_from_datamodel(&db, &project);
    let var = &result.models["main"].variables["lookup_var"];
    let gf = var.source.gf(&db);
    assert!(gf.is_some());
    let gf = gf.as_ref().unwrap();
    assert_eq!(gf.kind, datamodel::GraphicalFunctionKind::Continuous);
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
            macro_spec: None,
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
        datamodel::DimensionElements::Named(vec!["North".to_string(), "South".to_string()])
    );

    assert_eq!(dims[1].name, "Periods");
    assert_eq!(dims[1].elements, datamodel::DimensionElements::Indexed(5));
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
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
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
        &datamodel::Equation::Scalar("100".to_string())
    );

    // Modify the equation and re-sync
    project.models[0].variables[0].set_scalar_equation("200");
    let result2 = sync_from_datamodel(&db, &project);

    let pop2 = &result2.models["main"].variables["population"];
    assert_eq!(
        pop2.source.equation(&db),
        &datamodel::Equation::Scalar("200".to_string())
    );
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
    assert_eq!(specs.dt, datamodel::Dt::Reciprocal(4.0));
    assert_eq!(specs.save_step, Some(datamodel::Dt::Dt(0.5)));
    assert_eq!(specs.sim_method, datamodel::SimMethod::RungeKutta4);
}

#[test]
fn test_parse_source_variable_scalar() {
    use crate::ast::Expr0;
    use crate::variable::Variable;

    let db = SimlinDb::default();
    let project = simple_project();
    let result = sync_from_datamodel(&db, &project);

    let pop_var = result.models["main"].variables["population"].source;
    let parsed = parse_var_no_module_ctx(&db, pop_var, result.project);

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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        non_negative: true,
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "production".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
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

    let result = sync_from_datamodel(&db, &project);

    // Parse the stock variable
    let stock_var = result.models["main"].variables["inventory"].source;
    let parsed = parse_var_no_module_ctx(&db, stock_var, result.project);
    assert!(matches!(&parsed.variable, Variable::Stock { .. }));
    assert_eq!(parsed.variable.ident(), "inventory");

    // Parse a flow variable
    let flow_var = result.models["main"].variables["production"].source;
    let parsed = parse_var_no_module_ctx(&db, flow_var, result.project);
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
    let tracked_result = parse_var_no_module_ctx(&db, pop_var, result.project);

    // Parse directly via parse_var for comparison
    let dm_var = &project.models[0].variables[0];
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
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
        let alpha_result = parse_var_no_module_ctx(&db, alpha_src, source_project);
        let beta_result = parse_var_no_module_ctx(&db, beta_src, source_project);
        assert_eq!(alpha_result.variable.ident(), "alpha");
        assert_eq!(beta_result.variable.ident(), "beta");
        beta_result as *const ParsedVariableResult
    };

    // Modify only alpha's equation; beta is unchanged
    alpha_src
        .set_equation(&mut db)
        .to(datamodel::Equation::Scalar("42".to_string()));

    // Re-parse both: alpha should have new result, beta should be cached
    let alpha_result_2 = parse_var_no_module_ctx(&db, alpha_src, source_project);
    let beta_result_2 = parse_var_no_module_ctx(&db, beta_src, source_project);

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
    let deps = deps_no_inputs(&db, pop_var, result.project);

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

    let result = sync_from_datamodel(&db, &project);

    let births_var = result.models["main"].variables["births"].source;
    let deps = deps_no_inputs(&db, births_var, result.project);

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

    let result = sync_from_datamodel(&db, &project);
    let stock_var = result.models["main"].variables["inventory"].source;
    let deps = deps_no_inputs(&db, stock_var, result.project);

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
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let mod_var = result.models["main"].variables["submodel"].source;
    let deps = deps_no_inputs(&db, mod_var, result.project);

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
        let deps = deps_no_inputs(&db, beta_src, source_project);
        assert_eq!(
            deps.dt_deps,
            ["alpha", "gamma"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        (deps.dt_deps.clone(), deps.initial_deps.clone())
    };

    let graph_before = model_dependency_graph(
        &db,
        source_model,
        source_project,
        ModuleInputSet::empty(&db),
    );
    let graph_ptr_before = graph_before as *const ModelDepGraphResult;

    // Change beta's equation from "alpha + gamma" to "alpha * gamma"
    // Same deps, different equation
    beta_src
        .set_equation(&mut db)
        .to(datamodel::Equation::Scalar("alpha * gamma".to_string()));

    // Beta's deps should be the same (alpha, gamma)
    let beta_deps_after = deps_no_inputs(&db, beta_src, source_project);
    assert_eq!(beta_dt_before, beta_deps_after.dt_deps);
    assert_eq!(beta_init_before, beta_deps_after.initial_deps);

    // The dep graph should be returned from cache (pointer-equal)
    let graph_after = model_dependency_graph(
        &db,
        source_model,
        source_project,
        ModuleInputSet::empty(&db),
    );
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

    let (source_project, beta_src, source_model) = {
        let result = sync_from_datamodel(&db, &project);
        (
            result.project,
            result.models["main"].variables["beta"].source,
            result.models["main"].source,
        )
    };

    // Prime the cache
    let graph_before = model_dependency_graph(
        &db,
        source_model,
        source_project,
        ModuleInputSet::empty(&db),
    );
    let graph_ptr_before = graph_before as *const ModelDepGraphResult;

    // Change beta's equation from "alpha" to "gamma" -- different deps
    beta_src
        .set_equation(&mut db)
        .to(datamodel::Equation::Scalar("gamma".to_string()));

    // The dep graph should be recomputed (different pointer)
    let graph_after = model_dependency_graph(
        &db,
        source_model,
        source_project,
        ModuleInputSet::empty(&db),
    );
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

    let result = sync_from_datamodel(&db, &project);
    let graph = model_dependency_graph(
        &db,
        result.models["main"].source,
        result.project,
        ModuleInputSet::empty(&db),
    );

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

    let result = sync_from_datamodel(&db, &project);
    let graph = model_dependency_graph(
        &db,
        result.models["main"].source,
        result.project,
        ModuleInputSet::empty(&db),
    );

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

    let result = sync_from_datamodel(&db, &project);
    let _graph = model_dependency_graph(
        &db,
        result.models["main"].source,
        result.project,
        ModuleInputSet::empty(&db),
    );

    // Collect diagnostics emitted by model_dependency_graph
    let diags = model_dependency_graph::accumulated::<CompilationDiagnostic>(
        &db,
        result.models["main"].source,
        result.project,
        ModuleInputSet::empty(&db),
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

use crate::testutils::feedback_loop_project;

#[test]
fn test_normalize_module_ref_str() {
    assert_eq!(normalize_module_ref_str("foo\u{00B7}output"), "foo");
    assert_eq!(normalize_module_ref_str("plain_name"), "plain_name");
    assert_eq!(normalize_module_ref_str(""), "");
}

#[test]
fn test_generate_max_abs_selection_small_counts() {
    // 0 pathways: constant zero, no helpers.
    let (eq, helpers) = generate_max_abs_selection("port", &[]);
    assert_eq!(eq, "0");
    assert!(helpers.is_empty());

    // 1 pathway: a bare quoted reference, no helpers.
    let (eq, helpers) = generate_max_abs_selection("port", &["p0".into()]);
    assert_eq!(eq, "\"p0\"");
    assert!(helpers.is_empty());

    // 2 pathways: a single selection step, no helpers.
    let (eq, helpers) = generate_max_abs_selection("port", &["p0".into(), "p1".into()]);
    assert!(eq.contains("ABS"));
    assert!(eq.contains("p0"));
    assert!(eq.contains("p1"));
    assert!(helpers.is_empty());
}

#[test]
fn test_generate_max_abs_selection_folds_through_helpers() {
    // 3+ pathways: n-2 accumulator helpers, each O(1); the composite equation
    // references the final accumulator and the last pathway only.
    let names: Vec<String> = (0..5).map(|i| format!("p{i}")).collect();
    let (eq, helpers) = generate_max_abs_selection("port", &names);

    assert_eq!(helpers.len(), 3, "5 pathways need 3 accumulators");
    // acc_0 selects between p0 and p1.
    assert!(helpers[0].equation.source_text().contains("p0"));
    assert!(helpers[0].equation.source_text().contains("p1"));
    // acc_i (i > 0) selects between acc_{i-1} and p_{i+1}.
    for i in 1..helpers.len() {
        let text = helpers[i].equation.source_text();
        assert!(
            text.contains(helpers[i - 1].name.as_str()),
            "acc_{i} must reference acc_{}: {text}",
            i - 1
        );
        assert!(text.contains(&format!("p{}", i + 1)), "acc_{i}: {text}");
    }
    // The composite references the final accumulator and the last pathway.
    assert!(eq.contains(helpers.last().unwrap().name.as_str()));
    assert!(eq.contains("p4"));

    // Accumulator names sort in fold order, and after the (digit-suffixed)
    // pathway names they reference -- this lexical order IS the runlist
    // evaluation order within the LTM "path" category.
    let mut sorted = helpers.iter().map(|h| h.name.clone()).collect::<Vec<_>>();
    sorted.sort();
    assert_eq!(
        sorted,
        helpers.iter().map(|h| h.name.clone()).collect::<Vec<_>>(),
        "accumulators must already be in sorted (= evaluation) order"
    );
    for h in &helpers {
        assert!(
            "$\u{205A}ltm\u{205A}path\u{205A}port\u{205A}9" < h.name.as_str(),
            "every digit-suffixed pathway name must sort before accumulator {}",
            h.name
        );
    }
}

#[test]
fn test_generate_max_abs_selection_total_size_is_linear() {
    // The regression that motivated the fold: equation text must scale
    // linearly with pathway count, never exponentially. 200 pathways with the
    // old nested form would be ~2^198 bytes; linear form stays in the tens of
    // kilobytes.
    let names: Vec<String> = (0..200)
        .map(|i| format!("$\u{205A}ltm\u{205A}path\u{205A}in\u{205A}{i}"))
        .collect();
    let (eq, helpers) = generate_max_abs_selection("in", &names);
    let total: usize = eq.len()
        + helpers
            .iter()
            .map(|h| h.name.len() + h.equation.source_text().len())
            .sum::<usize>();
    assert_eq!(helpers.len(), 198);
    assert!(
        total < 100_000,
        "200 pathways must produce <100KB of equation text, got {total}"
    );
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
fn test_model_causal_edges_normalizes_inter_module_output_refs() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "inter_module_edges".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "source".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "a".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "source".to_string(),
                            dst: "a.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "b".to_string(),
                        model_name: "consumer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "a.output".to_string(),
                            dst: "b.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("input".to_string()),
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
            },
            datamodel::Model {
                name: "consumer".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("input".to_string()),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let edges = model_causal_edges(&db, model, result.project);

    assert!(
        edges.edges.get("a").is_some_and(|tos| tos.contains("b")),
        "inter-module edge should be normalized to module node 'a' -> 'b'"
    );
    assert!(
        !edges.edges.contains_key("a\u{00B7}output"),
        "phantom module output node should not appear in causal graph"
    );
}

#[test]
fn test_model_causal_edges_skips_internal_module_refs() {
    // Stella-imported models can include output refs in the module references
    // list where src starts with the module's own prefix (e.g. src="a.output").
    // These internal/output refs should not create false self-loops like a -> a.
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "internal_refs".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "source".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "a".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![
                            // Normal input ref
                            datamodel::ModuleReference {
                                src: "source".to_string(),
                                dst: "a.input".to_string(),
                            },
                            // Internal/output ref (Stella-style): src starts
                            // with the module prefix
                            datamodel::ModuleReference {
                                src: "a.output".to_string(),
                                dst: "consumer_target".to_string(),
                            },
                        ],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "consumer_target".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
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
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("input".to_string()),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let edges = model_causal_edges(&db, model, result.project);

    // Internal ref "a.output" should NOT create a self-loop edge a -> a
    let has_self_loop = edges.edges.get("a").is_some_and(|tos| tos.contains("a"));
    assert!(
        !has_self_loop,
        "internal module output ref should not create false self-loop; edges: {:?}",
        edges.edges
    );

    // The normal input ref source -> a should still be present
    assert!(
        edges
            .edges
            .get("source")
            .is_some_and(|tos| tos.contains("a")),
        "normal input ref edge should still exist"
    );
}

#[test]
fn test_model_causal_edges_normalizes_leading_middot_parent_refs() {
    // A submodel's module instance can reference parent-scope variables via
    // leading-dot syntax (e.g. ".area"), which canonicalizes to a leading
    // middot ("·area").  normalize_module_ref_str must strip the leading
    // middot before truncating at the module qualifier, otherwise "·area"
    // yields an empty-string key.
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "parent_ref_edges".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "area".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "area".to_string(),
                            dst: "sub.area_input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "area_input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "nested".to_string(),
                        model_name: "nested_model".to_string(),
                        documentation: String::new(),
                        units: None,
                        // Use leading-dot parent ref: ".area_input" canonicalizes
                        // to "·area_input"
                        references: vec![datamodel::ModuleReference {
                            src: ".area_input".to_string(),
                            dst: "nested.val".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "result".to_string(),
                        equation: datamodel::Equation::Scalar("area_input * 2".to_string()),
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
            },
            datamodel::Model {
                name: "nested_model".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "val".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        can_be_module_input: true,
                        ..datamodel::Compat::default()
                    },
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let result = sync_from_datamodel(&db, &project);
    let sub_model = result.models["submodel"].source;
    let edges = model_causal_edges(&db, sub_model, result.project);

    // The "nested" module's src ".area_input" (canonicalized "·area_input")
    // should normalize to "area_input", creating an edge area_input -> nested.
    assert!(
        edges
            .edges
            .get("area_input")
            .is_some_and(|tos| tos.contains("nested")),
        "leading-dot parent ref should normalize to bare variable name; edges: {:?}",
        edges.edges
    );
    assert!(
        !edges.edges.contains_key(""),
        "empty-string key should not appear from leading-middot truncation"
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
    assert!(!circuits.is_empty(), "should find at least one circuit");
    let has_pop_births_loop = (0..circuits.len()).any(|i| {
        let names: Vec<&str> = circuits.circuit_names(i).collect();
        names.contains(&"population") && names.contains(&"births")
    });
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
    use super::model_ltm_variables;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

    // Should generate link scores and loop scores for the feedback loop
    assert!(!ltm.vars.is_empty(), "should generate LTM variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(has_loop_score, "should have loop score variables");

    // All vars should have non-empty equations
    for var in &ltm.vars {
        assert!(
            !var.equation.source_text().is_empty(),
            "var {} should have non-empty equation",
            var.name
        );
    }
}

#[test]
fn test_model_ltm_all_link_synthetic_variables_discovery_mode() {
    use super::model_ltm_variables;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

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
    use super::model_ltm_variables;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    // Simple project has just a constant -- no loops
    let project = simple_project();
    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);
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
    births_src
        .set_equation(&mut db)
        .to(datamodel::Equation::Scalar(
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
        !circuits_before.is_empty(),
        "should have circuits initially"
    );

    // Change births to a constant -- breaks the feedback loop
    births_src
        .set_equation(&mut db)
        .to(datamodel::Equation::Scalar("10".to_string()));

    let circuits_after = model_loop_circuits(&db, source_model, source_project);
    assert!(
        circuits_after.is_empty(),
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
        .to(datamodel::Equation::Scalar(
            "stock_a * rate_a * 2".to_string(),
        ));

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
    source_project.set_ltm_enabled(&mut db).to(true);
    let ltm_before = model_ltm_variables(&db, source_model, source_project);
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
        .to(datamodel::Equation::Scalar(
            "stock_a * rate_a * 2".to_string(),
        ));

    // Model-level result should still produce valid results
    let ltm_after = model_ltm_variables(&db, source_model, source_project);
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

    let diags = collect_all_diagnostics(&db, sync.project);
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

    // Verify struct field path also shows an error
    let parsed = parse_var_no_module_ctx(
        &db,
        sync.models["main"].variables["broken"].source,
        sync.project,
    );
    assert!(
        parsed.variable.equation_errors().is_some(),
        "struct fields should show equation errors for 'if then'"
    );

    let diags = collect_all_diagnostics(&db, sync.project);
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

    // Collect from accumulator
    let accum_diags = collect_all_diagnostics(&db, sync.project);

    // Collect from struct fields (parse results)
    let mut field_equation_errors: HashSet<(String, crate::common::EquationError)> = HashSet::new();
    for (var_name, synced_var) in &sync.models["main"].variables {
        let parsed = parse_var_no_module_ctx(&db, synced_var.source, sync.project);
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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
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
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);

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

    // Extract the salsa IDs we need (they are Copy) before dropping sync
    let (alpha_src, source_model, source_project) = {
        let sync = sync_from_datamodel(&db, &project);
        let alpha_src = sync.models["main"].variables["alpha"].source;
        let source_model = sync.models["main"].source;
        let source_project = sync.project;

        // Initially: alpha has errors, beta does not
        let diags1 = collect_all_diagnostics(&db, sync.project);
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
        .to(datamodel::Equation::Scalar("42".to_string()));

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

    // Initial sync
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Prime the cache by parsing both variables
    let alpha_src = state1.models["main"].variables["alpha"].source_var;
    let beta_src = state1.models["main"].variables["beta"].source_var;
    let beta_ptr_before = {
        let _alpha_result = parse_var_no_module_ctx(&db, alpha_src, state1.project);
        let beta_result = parse_var_no_module_ctx(&db, beta_src, state1.project);
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
    let beta_result_after = parse_var_no_module_ctx(&db, beta_src2, state2.project);
    let beta_ptr_after = beta_result_after as *const ParsedVariableResult;
    assert_eq!(
        beta_ptr_before, beta_ptr_after,
        "beta's parse result should be cached since it was not modified"
    );

    // Alpha's parse result should reflect the new equation
    let alpha_src2 = state2.models["main"].variables["alpha"].source_var;
    let alpha_result = parse_var_no_module_ctx(&db, alpha_src2, state2.project);
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
    }

    // Verify the reconstituted SyncResult works for diagnostic collection
    let diags = collect_all_diagnostics(&db, sync.project);
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
    let _ = parse_var_no_module_ctx(&db, pop_src, state0.project);

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
    let result = parse_var_no_module_ctx(&db, pop_src2, state2.project);
    if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
        result.variable.ast()
    {
        assert_eq!(*val, 999.0);
    } else {
        panic!("Expected Const(999.0), got {:?}", result.variable.ast());
    }
}

#[test]
fn test_sync_preserves_module_visibility_from_datamodel() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "visibility".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "sub".to_string(),
                    model_name: "submodel".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let module_var = sync.models["main"].variables["sub"].source;
    assert_eq!(
        module_var.compat(&db).visibility,
        datamodel::Visibility::Public,
        "module visibility should be preserved in SourceVariable compat"
    );
}

#[test]
fn test_incremental_sync_updates_module_visibility() {
    let mut db = SimlinDb::default();
    let mut project = datamodel::Project {
        name: "visibility_update".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "sub".to_string(),
                    model_name: "submodel".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    if let datamodel::Variable::Module(m) = &mut project.models[0].variables[0] {
        m.compat.visibility = datamodel::Visibility::Public;
    } else {
        panic!("expected module variable in test fixture");
    }

    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    let module_var = state2.models["main"].variables["sub"].source_var;
    assert_eq!(
        module_var.compat(&db).visibility,
        datamodel::Visibility::Public,
        "incremental sync should propagate module visibility changes"
    );
}
