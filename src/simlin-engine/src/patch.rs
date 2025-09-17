// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::canonicalize;
use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::datamodel::{self, Variable};
use crate::project_io::{self, model_operation, project_operation};
use crate::serde;

pub fn apply_patch(
    project: &mut datamodel::Project,
    patch: &project_io::ProjectPatch,
) -> Result<()> {
    let mut staged = project.clone();

    // Apply project-level operations first
    for project_op in &patch.project_ops {
        let Some(kind) = &project_op.op else {
            return Err(Error::new(
                ErrorKind::Model,
                ErrorCode::Generic,
                Some("missing project operation".to_string()),
            ));
        };

        match kind {
            project_operation::Op::SetSimSpecs(specs) => apply_set_sim_specs(&mut staged, specs)?,
            project_operation::Op::SetSource(op) => apply_set_source(&mut staged, op)?,
        }
    }

    // Then apply model-level operations
    for model_patch in &patch.models {
        let model = get_model_mut(&mut staged, &model_patch.name)?;

        for op in &model_patch.ops {
            let Some(kind) = &op.op else {
                return Err(Error::new(
                    ErrorKind::Model,
                    ErrorCode::Generic,
                    Some("missing model operation".to_string()),
                ));
            };

            match kind {
                model_operation::Op::UpsertStock(op) => apply_upsert_stock(model, op)?,
                model_operation::Op::UpsertFlow(op) => apply_upsert_flow(model, op)?,
                model_operation::Op::UpsertAux(op) => apply_upsert_aux(model, op)?,
                model_operation::Op::UpsertModule(op) => apply_upsert_module(model, op)?,
                model_operation::Op::DeleteVariable(op) => apply_delete_variable(model, op)?,
                model_operation::Op::RenameVariable(op) => apply_rename_variable(model, op)?,
                model_operation::Op::UpsertView(op) => apply_upsert_view(model, op)?,
                model_operation::Op::DeleteView(op) => apply_delete_view(model, op)?,
            }
        }
    }

    *project = staged;
    Ok(())
}

fn canonicalize_ident(ident: &mut String) {
    let canonical = canonicalize(ident.as_str());
    *ident = canonical.as_str().to_string();
}

fn canonicalize_stock(stock: &mut datamodel::Stock) {
    canonicalize_ident(&mut stock.ident);
    for inflow in stock.inflows.iter_mut() {
        canonicalize_ident(inflow);
    }
    stock.inflows.sort_unstable();
    for outflow in stock.outflows.iter_mut() {
        canonicalize_ident(outflow);
    }
    stock.outflows.sort_unstable();
}

fn canonicalize_flow(flow: &mut datamodel::Flow) {
    canonicalize_ident(&mut flow.ident);
}

fn canonicalize_aux(aux: &mut datamodel::Aux) {
    canonicalize_ident(&mut aux.ident);
}

fn canonicalize_module(module: &mut datamodel::Module) {
    canonicalize_ident(&mut module.ident);
}

fn upsert_variable(model: &mut datamodel::Model, variable: Variable) {
    let ident = canonicalize(variable.get_ident());
    if let Some(existing) = model.get_variable_mut(ident.as_str()) {
        *existing = variable;
    } else {
        model.variables.push(variable);
    }
}

fn get_model_mut<'a>(
    project: &'a mut datamodel::Project,
    model_name: &str,
) -> Result<&'a mut datamodel::Model> {
    project.get_model_mut(model_name).ok_or_else(|| {
        Error::new(
            ErrorKind::Model,
            ErrorCode::BadModelName,
            Some(model_name.to_string()),
        )
    })
}

fn apply_set_sim_specs(
    project: &mut datamodel::Project,
    op: &project_io::SetSimSpecsOp,
) -> Result<()> {
    let Some(sim_specs) = &op.sim_specs else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing sim_specs payload".to_string()),
        ));
    };

    project.sim_specs.start = sim_specs.start;
    project.sim_specs.stop = sim_specs.stop;

    if let Some(dt) = &sim_specs.dt {
        project.sim_specs.dt = datamodel::Dt::from(*dt);
    }

    if let Some(save_step) = &sim_specs.save_step {
        project.sim_specs.save_step = Some(datamodel::Dt::from(*save_step));
    } else {
        project.sim_specs.save_step = None;
    }

    let sim_method = project_io::SimMethod::try_from(sim_specs.sim_method).unwrap_or_default();
    project.sim_specs.sim_method = datamodel::SimMethod::from(sim_method);

    if let Some(units) = &sim_specs.time_units {
        if units.is_empty() {
            project.sim_specs.time_units = None;
        } else {
            project.sim_specs.time_units = Some(units.clone());
        }
    } else {
        project.sim_specs.time_units = None;
    }

    Ok(())
}

fn apply_upsert_stock(model: &mut datamodel::Model, op: &project_io::UpsertStockOp) -> Result<()> {
    let Some(stock_pb) = &op.stock else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing stock payload".to_string()),
        ));
    };
    let mut stock = datamodel::Stock::from(stock_pb.clone());
    canonicalize_stock(&mut stock);
    upsert_variable(model, Variable::Stock(stock));
    Ok(())
}

fn apply_upsert_flow(model: &mut datamodel::Model, op: &project_io::UpsertFlowOp) -> Result<()> {
    let Some(flow_pb) = &op.flow else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing flow payload".to_string()),
        ));
    };
    let mut flow = datamodel::Flow::from(flow_pb.clone());
    canonicalize_flow(&mut flow);
    upsert_variable(model, Variable::Flow(flow));
    Ok(())
}

fn apply_upsert_aux(model: &mut datamodel::Model, op: &project_io::UpsertAuxOp) -> Result<()> {
    let Some(aux_pb) = &op.aux else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing auxiliary payload".to_string()),
        ));
    };
    let mut aux = datamodel::Aux::from(aux_pb.clone());
    canonicalize_aux(&mut aux);
    upsert_variable(model, Variable::Aux(aux));
    Ok(())
}

fn apply_upsert_module(
    model: &mut datamodel::Model,
    op: &project_io::UpsertModuleOp,
) -> Result<()> {
    let Some(module_pb) = &op.module else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing module payload".to_string()),
        ));
    };
    let mut module = datamodel::Module::from(module_pb.clone());
    canonicalize_module(&mut module);
    upsert_variable(model, Variable::Module(module));
    Ok(())
}

fn apply_delete_variable(
    model: &mut datamodel::Model,
    op: &project_io::DeleteVariableOp,
) -> Result<()> {
    let ident = canonicalize(op.ident.as_str());
    let Some(pos) = model
        .variables
        .iter()
        .position(|var| canonicalize(var.get_ident()) == ident)
    else {
        return Err(Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None));
    };

    let removed = model.variables.remove(pos);
    if let Variable::Flow(flow) = removed {
        let flow_ident = canonicalize(flow.ident.as_str());
        for var in model.variables.iter_mut() {
            if let Variable::Stock(stock) = var {
                stock
                    .inflows
                    .retain(|name| canonicalize(name.as_str()) != flow_ident);
                stock
                    .outflows
                    .retain(|name| canonicalize(name.as_str()) != flow_ident);
            }
        }
    }

    Ok(())
}

fn apply_rename_variable(
    model: &mut datamodel::Model,
    op: &project_io::RenameVariableOp,
) -> Result<()> {
    let old_ident = canonicalize(op.from.as_str());
    let new_ident = canonicalize(op.to.as_str());

    if old_ident == new_ident {
        return Ok(());
    }

    if model.get_variable_mut(new_ident.as_str()).is_some() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            None,
        ));
    }

    let is_flow = {
        let var = model
            .get_variable_mut(old_ident.as_str())
            .ok_or_else(|| Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None))?;
        let flow = matches!(var, Variable::Flow(_));
        var.set_ident(new_ident.as_str().to_string());
        flow
    };

    if is_flow {
        for var in model.variables.iter_mut() {
            if let Variable::Stock(stock) = var {
                for inflow in stock.inflows.iter_mut() {
                    if canonicalize(inflow.as_str()) == old_ident {
                        *inflow = new_ident.as_str().to_string();
                    }
                }
                for outflow in stock.outflows.iter_mut() {
                    if canonicalize(outflow.as_str()) == old_ident {
                        *outflow = new_ident.as_str().to_string();
                    }
                }
            }
        }
    }

    Ok(())
}

fn apply_upsert_view(model: &mut datamodel::Model, op: &project_io::UpsertViewOp) -> Result<()> {
    let Some(view_pb) = &op.view else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing view payload".to_string()),
        ));
    };
    let view = serde::deserialize_view(view_pb.clone());
    let index = op.index as usize;

    if index < model.views.len() {
        model.views[index] = view;
        Ok(())
    } else if index == model.views.len() {
        // Allow appending at the end
        model.views.push(view);
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DoesNotExist,
            Some(format!("view index {index} out of range")),
        ))
    }
}

fn apply_delete_view(model: &mut datamodel::Model, op: &project_io::DeleteViewOp) -> Result<()> {
    let index = op.index as usize;
    if index < model.views.len() {
        model.views.remove(index);
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DoesNotExist,
            Some(format!("view index {index} out of range")),
        ))
    }
}

fn apply_set_source(project: &mut datamodel::Project, op: &project_io::SetSourceOp) -> Result<()> {
    let Some(source) = &op.source else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::ProtobufDecode,
            Some("missing source payload".to_string()),
        ));
    };
    project.source = Some(datamodel::Source::from(source.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::{self, Equation, Visibility};
    use crate::project_io::variable::V;
    use crate::project_io::{
        self, ModelOperation, ModelPatch, ProjectOperation, ProjectPatch, model_operation,
        project_operation,
    };
    use crate::test_common::TestProject;

    fn stock_proto(stock: datamodel::Stock) -> project_io::variable::Stock {
        let variable = Variable::Stock(stock);
        match project_io::Variable::from(variable).v.unwrap() {
            V::Stock(stock) => stock,
            _ => unreachable!(),
        }
    }

    fn aux_proto(aux: datamodel::Aux) -> project_io::variable::Aux {
        let variable = Variable::Aux(aux);
        match project_io::Variable::from(variable).v.unwrap() {
            V::Aux(aux) => aux,
            _ => unreachable!(),
        }
    }

    #[test]
    fn upsert_aux_adds_variable() {
        let mut project = TestProject::new("test").build_datamodel();
        let aux = datamodel::Aux {
            ident: "new_aux".to_string(),
            equation: Equation::Scalar("1".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpsertAux(project_io::UpsertAuxOp {
                        aux: Some(aux_proto(aux.clone())),
                    })),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();
        let var = model.get_variable("new_aux").unwrap();
        match var {
            Variable::Aux(actual) => assert_eq!(actual.equation, aux.equation),
            _ => panic!("expected aux"),
        }
    }

    #[test]
    fn upsert_stock_replaces_existing() {
        let mut project = TestProject::new("test")
            .stock("stock", "1", &[], &[], None)
            .build_datamodel();
        let mut stock = datamodel::Stock {
            ident: "stock".to_string(),
            equation: Equation::Scalar("5".to_string(), None),
            documentation: "docs".to_string(),
            units: Some("people".to_string()),
            inflows: vec!["flow".to_string()],
            outflows: vec![],
            non_negative: true,
            can_be_module_input: true,
            visibility: Visibility::Public,
            ai_state: None,
            uid: Some(10),
        };
        stock.inflows.sort();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpsertStock(
                        project_io::UpsertStockOp {
                            stock: Some(stock_proto(stock.clone())),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();
        let var = model.get_variable("stock").unwrap();
        match var {
            Variable::Stock(actual) => {
                assert_eq!(actual.equation, stock.equation);
                assert_eq!(actual.inflows, stock.inflows);
                assert_eq!(actual.non_negative, stock.non_negative);
                assert_eq!(actual.visibility, stock.visibility);
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn delete_flow_removes_references() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .stock("stock", "stock", &["flow"], &["flow"], None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::DeleteVariable(
                        project_io::DeleteVariableOp {
                            ident: "flow".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.get_variable("flow").is_none());
        match model.get_variable("stock").unwrap() {
            Variable::Stock(stock) => {
                assert!(stock.inflows.is_empty());
                assert!(stock.outflows.is_empty());
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn rename_flow_updates_stock_references() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .stock("stock", "stock", &["flow"], &["flow"], None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "flow".to_string(),
                            to: "new_flow".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.get_variable("flow").is_none());
        match model.get_variable("new_flow").unwrap() {
            Variable::Flow(_) => {}
            _ => panic!("expected flow"),
        }
        match model.get_variable("stock").unwrap() {
            Variable::Stock(stock) => {
                assert_eq!(stock.inflows, vec!["new_flow".to_string()]);
                assert_eq!(stock.outflows, vec!["new_flow".to_string()]);
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn set_sim_specs_partial_update() {
        let mut project = TestProject::new("test").build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation {
                op: Some(project_operation::Op::SetSimSpecs(
                    project_io::SetSimSpecsOp {
                        sim_specs: Some(project_io::SimSpecs {
                            start: 5.0,
                            stop: project.sim_specs.stop,
                            dt: Some(project_io::Dt {
                                value: 0.5,
                                is_reciprocal: false,
                            }),
                            save_step: None,
                            sim_method: project_io::SimMethod::RungeKutta4 as i32,
                            time_units: Some("days".to_string()),
                        }),
                    },
                )),
            }],
            models: vec![],
        };

        apply_patch(&mut project, &patch).unwrap();
        assert_eq!(project.sim_specs.start, 5.0);
        assert_eq!(project.sim_specs.dt, datamodel::Dt::Dt(0.5));
        assert!(project.sim_specs.save_step.is_none());
        assert_eq!(
            project.sim_specs.sim_method,
            datamodel::SimMethod::RungeKutta4
        );
        assert_eq!(project.sim_specs.time_units, Some("days".to_string()));
    }

    #[test]
    fn upsert_view_and_delete() {
        let mut project = TestProject::new("test").build_datamodel();
        let view = project_io::View {
            kind: project_io::view::ViewType::StockFlow as i32,
            elements: vec![],
            view_box: None,
            zoom: 1.0,
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpsertView(project_io::UpsertViewOp {
                        index: 0,
                        view: Some(view.clone()),
                    })),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert_eq!(model.views.len(), 1);

        let delete_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::DeleteView(project_io::DeleteViewOp {
                        index: 0,
                    })),
                }],
            }],
        };

        apply_patch(&mut project, &delete_patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.views.is_empty());
    }

    #[test]
    fn set_source() {
        let mut project = TestProject::new("test").build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation {
                op: Some(project_operation::Op::SetSource(project_io::SetSourceOp {
                    source: Some(project_io::Source {
                        extension: project_io::source::Extension::Xmile as i32,
                        content: "hello".to_string(),
                    }),
                })),
            }],
            models: vec![],
        };

        apply_patch(&mut project, &patch).unwrap();
        assert!(project.source.is_some());
        assert_eq!(project.source.as_ref().unwrap().content, "hello");
    }

    #[test]
    fn rename_duplicate_returns_error() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .flow("flow2", "2", None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "flow".to_string(),
                            to: "flow2".to_string(),
                        },
                    )),
                }],
            }],
        };

        let err = apply_patch(&mut project, &patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
        assert_eq!(err.kind, ErrorKind::Model);
    }
}
