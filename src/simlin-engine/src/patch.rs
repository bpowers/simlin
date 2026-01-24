// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::{Ast, Expr0, Expr2, IndexExpr2};
use crate::builtins::{BuiltinFn, UntypedBuiltinFn};
use crate::canonicalize;
use crate::common::{
    Canonical, CanonicalElementName, Error, ErrorCode, ErrorKind, Ident, RawIdent, Result,
};
use crate::datamodel::{self, Variable};
use crate::project::Project as CompiledProject;
use crate::project_io::{self, model_operation, project_operation};
use crate::serde;
use std::collections::HashMap;

macro_rules! require_field {
    ($field:expr, $msg:expr) => {{
        let Some(value) = $field else {
            return Err(Error::new(
                ErrorKind::Model,
                ErrorCode::ProtobufDecode,
                Some($msg.to_string()),
            ));
        };
        value
    }};
}

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
        for op in &model_patch.ops {
            let Some(kind) = &op.op else {
                return Err(Error::new(
                    ErrorKind::Model,
                    ErrorCode::Generic,
                    Some("missing model operation".to_string()),
                ));
            };

            match kind {
                model_operation::Op::RenameVariable(op) => {
                    apply_rename_variable(&mut staged, &model_patch.name, op)?
                }
                _ => {
                    let model = get_model_mut(&mut staged, &model_patch.name)?;
                    match kind {
                        model_operation::Op::UpsertStock(op) => apply_upsert_stock(model, op)?,
                        model_operation::Op::UpsertFlow(op) => apply_upsert_flow(model, op)?,
                        model_operation::Op::UpsertAux(op) => apply_upsert_aux(model, op)?,
                        model_operation::Op::UpsertModule(op) => apply_upsert_module(model, op)?,
                        model_operation::Op::DeleteVariable(op) => {
                            apply_delete_variable(model, op)?
                        }
                        model_operation::Op::UpsertView(op) => apply_upsert_view(model, op)?,
                        model_operation::Op::DeleteView(op) => apply_delete_view(model, op)?,
                        model_operation::Op::UpdateStockFlows(op) => {
                            apply_update_stock_flows(model, op)?
                        }
                        model_operation::Op::RenameVariable(_) => unreachable!(),
                    }
                }
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
    let sim_specs = require_field!(&op.sim_specs, "missing sim_specs payload");

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
    let stock_pb = require_field!(&op.stock, "missing stock payload");
    let mut stock = datamodel::Stock::from(stock_pb.clone());
    canonicalize_stock(&mut stock);
    upsert_variable(model, Variable::Stock(stock));
    Ok(())
}

fn apply_upsert_flow(model: &mut datamodel::Model, op: &project_io::UpsertFlowOp) -> Result<()> {
    let flow_pb = require_field!(&op.flow, "missing flow payload");
    let mut flow = datamodel::Flow::from(flow_pb.clone());
    canonicalize_flow(&mut flow);
    upsert_variable(model, Variable::Flow(flow));
    Ok(())
}

fn apply_upsert_aux(model: &mut datamodel::Model, op: &project_io::UpsertAuxOp) -> Result<()> {
    let aux_pb = require_field!(&op.aux, "missing auxiliary payload");
    let mut aux = datamodel::Aux::from(aux_pb.clone());
    canonicalize_aux(&mut aux);
    upsert_variable(model, Variable::Aux(aux));
    Ok(())
}

fn apply_upsert_module(
    model: &mut datamodel::Model,
    op: &project_io::UpsertModuleOp,
) -> Result<()> {
    let module_pb = require_field!(&op.module, "missing module payload");
    let mut module = datamodel::Module::from(module_pb.clone());
    canonicalize_module(&mut module);
    upsert_variable(model, Variable::Module(module));
    Ok(())
}

fn apply_update_stock_flows(
    model: &mut datamodel::Model,
    op: &project_io::UpdateStockFlowsOp,
) -> Result<()> {
    let ident = canonicalize(op.ident.as_str());

    let stock = model
        .variables
        .iter_mut()
        .find_map(|var| {
            if let Variable::Stock(stock) = var
                && canonicalize(stock.ident.as_str()) == ident
            {
                return Some(stock);
            }
            None
        })
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::DoesNotExist,
                Some(format!("stock '{}' not found", op.ident)),
            )
        })?;

    stock.inflows = op
        .inflows
        .iter()
        .map(|s| canonicalize(s).into_string())
        .collect();
    stock.outflows = op
        .outflows
        .iter()
        .map(|s| canonicalize(s).into_string())
        .collect();
    stock.inflows.sort_unstable();
    stock.outflows.sort_unstable();

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
    project: &mut datamodel::Project,
    model_name: &str,
    op: &project_io::RenameVariableOp,
) -> Result<()> {
    let old_ident = canonicalize(op.from.as_str());
    let new_ident = canonicalize(op.to.as_str());

    if old_ident == new_ident {
        return Ok(());
    }

    let compiled_project = CompiledProject::from(project.clone());
    let canonical_model_name = canonicalize(model_name);
    let compiled_model = compiled_project
        .models
        .get(&canonical_model_name)
        .ok_or_else(|| Error::new(ErrorKind::Model, ErrorCode::BadModelName, None))?
        .clone();

    let model = get_model_mut(project, model_name)?;

    if model.get_variable(new_ident.as_str()).is_some() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            None,
        ));
    }

    let (target_index, is_flow) = model
        .variables
        .iter()
        .enumerate()
        .find_map(|(idx, var)| {
            if canonicalize(var.get_ident()) == old_ident {
                Some((idx, matches!(var, Variable::Flow(_))))
            } else {
                None
            }
        })
        .ok_or_else(|| Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None))?;

    let compiled_vars = &compiled_model.variables;
    rename_model_equations(model, compiled_vars, &old_ident, &new_ident);

    if is_flow {
        update_stock_flow_references(model, &old_ident, &new_ident);
    }

    rename_module_references(model, &old_ident, &new_ident);

    if let Some(var) = model.variables.get_mut(target_index) {
        var.set_ident(new_ident.as_str().to_string());
    }

    Ok(())
}

fn rename_model_equations(
    model: &mut datamodel::Model,
    compiled_vars: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for datamodel_var in model.variables.iter_mut() {
        let canonical_var_ident = canonicalize(datamodel_var.get_ident());
        let Some(compiled_var) = compiled_vars.get(&canonical_var_ident) else {
            continue;
        };

        match datamodel_var {
            Variable::Stock(stock) => {
                rewrite_equation(
                    &mut stock.equation,
                    compiled_var.ast(),
                    compiled_var.init_ast(),
                    old_ident,
                    new_ident,
                );
            }
            Variable::Flow(flow) => {
                rewrite_equation(
                    &mut flow.equation,
                    compiled_var.ast(),
                    compiled_var.init_ast(),
                    old_ident,
                    new_ident,
                );
            }
            Variable::Aux(aux) => {
                rewrite_equation(
                    &mut aux.equation,
                    compiled_var.ast(),
                    compiled_var.init_ast(),
                    old_ident,
                    new_ident,
                );
            }
            Variable::Module(_module) => {}
        }
    }
}

fn rewrite_equation(
    equation: &mut datamodel::Equation,
    ast: Option<&Ast<Expr2>>,
    init_ast: Option<&Ast<Expr2>>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    if let Some(ast) = ast {
        let renamed = rename_ast(ast, old_ident, new_ident);
        apply_ast_to_equation_main(equation, &renamed);
    }

    if let Some(init_ast) = init_ast {
        let renamed = rename_ast(init_ast, old_ident, new_ident);
        apply_ast_to_equation_initial(equation, &renamed);
    }
}

fn apply_ast_to_equation_main(equation: &mut datamodel::Equation, ast: &Ast<Expr2>) {
    match (equation, ast) {
        (datamodel::Equation::Scalar(main, _), Ast::Scalar(expr)) => {
            *main = expr2_to_string(expr);
        }
        (datamodel::Equation::ApplyToAll(_, main, _), Ast::ApplyToAll(_, expr)) => {
            *main = expr2_to_string(expr);
        }
        (datamodel::Equation::Arrayed(_, elements), Ast::Arrayed(_, exprs)) => {
            for (element_name, equation, _, _) in elements.iter_mut() {
                let canonical_element = CanonicalElementName::from_raw(element_name.as_str());
                if let Some(expr) = exprs.get(&canonical_element) {
                    *equation = expr2_to_string(expr);
                }
            }
        }
        _ => {}
    }
}

fn apply_ast_to_equation_initial(equation: &mut datamodel::Equation, ast: &Ast<Expr2>) {
    match (equation, ast) {
        (datamodel::Equation::Scalar(_, initial @ Some(_)), Ast::Scalar(expr)) => {
            *initial = Some(expr2_to_string(expr));
        }
        (datamodel::Equation::ApplyToAll(_, _, initial @ Some(_)), Ast::ApplyToAll(_, expr)) => {
            *initial = Some(expr2_to_string(expr));
        }
        (datamodel::Equation::Arrayed(_, elements), Ast::Arrayed(_, exprs)) => {
            for (element_name, _, initial, _) in elements.iter_mut() {
                if let Some(initial_value) = initial.as_mut() {
                    let canonical_element = CanonicalElementName::from_raw(element_name.as_str());
                    if let Some(expr) = exprs.get(&canonical_element) {
                        *initial_value = expr2_to_string(expr);
                    }
                }
            }
        }
        _ => {}
    }
}

fn rename_ast(
    ast: &Ast<Expr2>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> Ast<Expr2> {
    match ast {
        Ast::Scalar(expr) => Ast::Scalar(rename_expr(expr, old_ident, new_ident)),
        Ast::ApplyToAll(dims, expr) => {
            Ast::ApplyToAll(dims.clone(), rename_expr(expr, old_ident, new_ident))
        }
        Ast::Arrayed(dims, elements) => {
            let rewritten = elements
                .iter()
                .map(|(name, expr)| (name.clone(), rename_expr(expr, old_ident, new_ident)))
                .collect();
            Ast::Arrayed(dims.clone(), rewritten)
        }
    }
}

fn rename_expr(expr: &Expr2, old_ident: &Ident<Canonical>, new_ident: &Ident<Canonical>) -> Expr2 {
    match expr {
        Expr2::Const(text, value, loc) => Expr2::Const(text.clone(), *value, *loc),
        Expr2::Var(ident, bounds, loc) => Expr2::Var(
            rename_canonical_ident(ident, old_ident, new_ident),
            bounds.clone(),
            *loc,
        ),
        Expr2::App(builtin, bounds, loc) => Expr2::App(
            rename_builtin(builtin, old_ident, new_ident),
            bounds.clone(),
            *loc,
        ),
        Expr2::Subscript(ident, indexes, bounds, loc) => Expr2::Subscript(
            rename_canonical_ident(ident, old_ident, new_ident),
            indexes
                .iter()
                .map(|idx| rename_index_expr(idx, old_ident, new_ident))
                .collect(),
            bounds.clone(),
            *loc,
        ),
        Expr2::Op1(op, rhs, bounds, loc) => Expr2::Op1(
            *op,
            Box::new(rename_expr(rhs, old_ident, new_ident)),
            bounds.clone(),
            *loc,
        ),
        Expr2::Op2(op, lhs, rhs, bounds, loc) => Expr2::Op2(
            *op,
            Box::new(rename_expr(lhs, old_ident, new_ident)),
            Box::new(rename_expr(rhs, old_ident, new_ident)),
            bounds.clone(),
            *loc,
        ),
        Expr2::If(cond, then_branch, else_branch, bounds, loc) => Expr2::If(
            Box::new(rename_expr(cond, old_ident, new_ident)),
            Box::new(rename_expr(then_branch, old_ident, new_ident)),
            Box::new(rename_expr(else_branch, old_ident, new_ident)),
            bounds.clone(),
            *loc,
        ),
    }
}

fn rename_builtin(
    builtin: &BuiltinFn<Expr2>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> BuiltinFn<Expr2> {
    match builtin {
        BuiltinFn::Lookup(table_expr, index_expr, loc) => BuiltinFn::Lookup(
            Box::new(rename_expr(table_expr, old_ident, new_ident)),
            Box::new(rename_expr(index_expr, old_ident, new_ident)),
            *loc,
        ),
        BuiltinFn::LookupForward(table_expr, index_expr, loc) => BuiltinFn::LookupForward(
            Box::new(rename_expr(table_expr, old_ident, new_ident)),
            Box::new(rename_expr(index_expr, old_ident, new_ident)),
            *loc,
        ),
        BuiltinFn::LookupBackward(table_expr, index_expr, loc) => BuiltinFn::LookupBackward(
            Box::new(rename_expr(table_expr, old_ident, new_ident)),
            Box::new(rename_expr(index_expr, old_ident, new_ident)),
            *loc,
        ),
        BuiltinFn::IsModuleInput(ident, loc) => {
            BuiltinFn::IsModuleInput(rename_identifier_string(ident, old_ident, new_ident), *loc)
        }
        BuiltinFn::Abs(expr) => BuiltinFn::Abs(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Arccos(expr) => {
            BuiltinFn::Arccos(Box::new(rename_expr(expr, old_ident, new_ident)))
        }
        BuiltinFn::Arcsin(expr) => {
            BuiltinFn::Arcsin(Box::new(rename_expr(expr, old_ident, new_ident)))
        }
        BuiltinFn::Arctan(expr) => {
            BuiltinFn::Arctan(Box::new(rename_expr(expr, old_ident, new_ident)))
        }
        BuiltinFn::Cos(expr) => BuiltinFn::Cos(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Exp(expr) => BuiltinFn::Exp(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Inf => BuiltinFn::Inf,
        BuiltinFn::Int(expr) => BuiltinFn::Int(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Ln(expr) => BuiltinFn::Ln(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Log10(expr) => {
            BuiltinFn::Log10(Box::new(rename_expr(expr, old_ident, new_ident)))
        }
        BuiltinFn::Max(lhs, rhs) => BuiltinFn::Max(
            Box::new(rename_expr(lhs, old_ident, new_ident)),
            rhs.as_ref()
                .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
        ),
        BuiltinFn::Mean(args) => BuiltinFn::Mean(
            args.iter()
                .map(|expr| rename_expr(expr, old_ident, new_ident))
                .collect(),
        ),
        BuiltinFn::Min(lhs, rhs) => BuiltinFn::Min(
            Box::new(rename_expr(lhs, old_ident, new_ident)),
            rhs.as_ref()
                .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
        ),
        BuiltinFn::Pi => BuiltinFn::Pi,
        BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
            Box::new(rename_expr(a, old_ident, new_ident)),
            Box::new(rename_expr(b, old_ident, new_ident)),
            c.as_ref()
                .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
        ),
        BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
            Box::new(rename_expr(a, old_ident, new_ident)),
            Box::new(rename_expr(b, old_ident, new_ident)),
            c.as_ref()
                .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
        ),
        BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
            Box::new(rename_expr(a, old_ident, new_ident)),
            Box::new(rename_expr(b, old_ident, new_ident)),
            c.as_ref()
                .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
        ),
        BuiltinFn::Sign(expr) => BuiltinFn::Sign(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Sin(expr) => BuiltinFn::Sin(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Sqrt(expr) => BuiltinFn::Sqrt(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Step(a, b) => BuiltinFn::Step(
            Box::new(rename_expr(a, old_ident, new_ident)),
            Box::new(rename_expr(b, old_ident, new_ident)),
        ),
        BuiltinFn::Tan(expr) => BuiltinFn::Tan(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Time => BuiltinFn::Time,
        BuiltinFn::TimeStep => BuiltinFn::TimeStep,
        BuiltinFn::StartTime => BuiltinFn::StartTime,
        BuiltinFn::FinalTime => BuiltinFn::FinalTime,
        BuiltinFn::Rank(expr, opts) => BuiltinFn::Rank(
            Box::new(rename_expr(expr, old_ident, new_ident)),
            opts.as_ref().map(|(a, b)| {
                (
                    Box::new(rename_expr(a, old_ident, new_ident)),
                    b.as_ref()
                        .map(|expr| Box::new(rename_expr(expr, old_ident, new_ident))),
                )
            }),
        ),
        BuiltinFn::Size(expr) => BuiltinFn::Size(Box::new(rename_expr(expr, old_ident, new_ident))),
        BuiltinFn::Stddev(expr) => {
            BuiltinFn::Stddev(Box::new(rename_expr(expr, old_ident, new_ident)))
        }
        BuiltinFn::Sum(expr) => BuiltinFn::Sum(Box::new(rename_expr(expr, old_ident, new_ident))),
    }
}

fn rename_index_expr(
    index: &IndexExpr2,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> IndexExpr2 {
    match index {
        IndexExpr2::Wildcard(loc) => IndexExpr2::Wildcard(*loc),
        IndexExpr2::StarRange(dim, loc) => IndexExpr2::StarRange(dim.clone(), *loc),
        IndexExpr2::Range(lhs, rhs, loc) => IndexExpr2::Range(
            rename_expr(lhs, old_ident, new_ident),
            rename_expr(rhs, old_ident, new_ident),
            *loc,
        ),
        IndexExpr2::DimPosition(pos, loc) => IndexExpr2::DimPosition(*pos, *loc),
        IndexExpr2::Expr(expr) => IndexExpr2::Expr(rename_expr(expr, old_ident, new_ident)),
    }
}

fn expr2_to_string(expr: &Expr2) -> String {
    let expr0 = expr2_to_expr0(expr);
    crate::ast::print_eqn(&expr0)
}

fn expr2_to_expr0(expr: &Expr2) -> Expr0 {
    match expr {
        Expr2::Const(text, value, loc) => Expr0::Const(text.clone(), *value, *loc),
        Expr2::Var(ident, _, loc) => Expr0::Var(RawIdent::new(ident.to_source_repr()), *loc),
        Expr2::App(builtin, _, loc) => {
            let untyped = builtin_to_untyped(builtin);
            Expr0::App(untyped, *loc)
        }
        Expr2::Subscript(ident, indexes, _, loc) => Expr0::Subscript(
            RawIdent::new(ident.to_source_repr()),
            indexes.iter().map(index_expr2_to_index_expr0).collect(),
            *loc,
        ),
        Expr2::Op1(op, rhs, _, loc) => Expr0::Op1(*op, Box::new(expr2_to_expr0(rhs)), *loc),
        Expr2::Op2(op, lhs, rhs, _, loc) => Expr0::Op2(
            *op,
            Box::new(expr2_to_expr0(lhs)),
            Box::new(expr2_to_expr0(rhs)),
            *loc,
        ),
        Expr2::If(cond, then_branch, else_branch, _, loc) => Expr0::If(
            Box::new(expr2_to_expr0(cond)),
            Box::new(expr2_to_expr0(then_branch)),
            Box::new(expr2_to_expr0(else_branch)),
            *loc,
        ),
    }
}

fn index_expr2_to_index_expr0(index: &IndexExpr2) -> crate::ast::IndexExpr0 {
    use crate::ast::IndexExpr0;
    match index {
        IndexExpr2::Wildcard(loc) => IndexExpr0::Wildcard(*loc),
        IndexExpr2::StarRange(dim, loc) => {
            IndexExpr0::StarRange(RawIdent::new(dim.as_str().to_string()), *loc)
        }
        IndexExpr2::Range(lhs, rhs, loc) => {
            IndexExpr0::Range(expr2_to_expr0(lhs), expr2_to_expr0(rhs), *loc)
        }
        IndexExpr2::DimPosition(pos, loc) => IndexExpr0::DimPosition(*pos, *loc),
        IndexExpr2::Expr(expr) => IndexExpr0::Expr(expr2_to_expr0(expr)),
    }
}

fn builtin_to_untyped(builtin: &BuiltinFn<Expr2>) -> UntypedBuiltinFn<Expr0> {
    use crate::builtins::BuiltinFn;
    match builtin {
        BuiltinFn::Lookup(table_expr, index_expr, _) => UntypedBuiltinFn(
            "lookup".to_string(),
            vec![expr2_to_expr0(table_expr), expr2_to_expr0(index_expr)],
        ),
        BuiltinFn::LookupForward(table_expr, index_expr, _) => UntypedBuiltinFn(
            "lookup_forward".to_string(),
            vec![expr2_to_expr0(table_expr), expr2_to_expr0(index_expr)],
        ),
        BuiltinFn::LookupBackward(table_expr, index_expr, _) => UntypedBuiltinFn(
            "lookup_backward".to_string(),
            vec![expr2_to_expr0(table_expr), expr2_to_expr0(index_expr)],
        ),
        BuiltinFn::IsModuleInput(ident, _) => UntypedBuiltinFn(
            "ismoduleinput".to_string(),
            vec![Expr0::Var(RawIdent::new(ident.clone()), Default::default())],
        ),
        BuiltinFn::Abs(expr) => UntypedBuiltinFn("abs".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Arccos(expr) => {
            UntypedBuiltinFn("arccos".to_string(), vec![expr2_to_expr0(expr)])
        }
        BuiltinFn::Arcsin(expr) => {
            UntypedBuiltinFn("arcsin".to_string(), vec![expr2_to_expr0(expr)])
        }
        BuiltinFn::Arctan(expr) => {
            UntypedBuiltinFn("arctan".to_string(), vec![expr2_to_expr0(expr)])
        }
        BuiltinFn::Cos(expr) => UntypedBuiltinFn("cos".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Exp(expr) => UntypedBuiltinFn("exp".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Inf => UntypedBuiltinFn("inf".to_string(), vec![]),
        BuiltinFn::Int(expr) => UntypedBuiltinFn("int".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Ln(expr) => UntypedBuiltinFn("ln".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Log10(expr) => UntypedBuiltinFn("log10".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Max(lhs, rhs) => {
            let mut args = vec![expr2_to_expr0(lhs)];
            if let Some(rhs) = rhs {
                args.push(expr2_to_expr0(rhs));
            }
            UntypedBuiltinFn("max".to_string(), args)
        }
        BuiltinFn::Mean(args) => UntypedBuiltinFn(
            "mean".to_string(),
            args.iter().map(expr2_to_expr0).collect(),
        ),
        BuiltinFn::Min(lhs, rhs) => {
            let mut args = vec![expr2_to_expr0(lhs)];
            if let Some(rhs) = rhs {
                args.push(expr2_to_expr0(rhs));
            }
            UntypedBuiltinFn("min".to_string(), args)
        }
        BuiltinFn::Pi => UntypedBuiltinFn("pi".to_string(), vec![]),
        BuiltinFn::Pulse(a, b, c) => {
            let mut args = vec![expr2_to_expr0(a), expr2_to_expr0(b)];
            if let Some(c) = c {
                args.push(expr2_to_expr0(c));
            }
            UntypedBuiltinFn("pulse".to_string(), args)
        }
        BuiltinFn::Ramp(a, b, c) => {
            let mut args = vec![expr2_to_expr0(a), expr2_to_expr0(b)];
            if let Some(c) = c {
                args.push(expr2_to_expr0(c));
            }
            UntypedBuiltinFn("ramp".to_string(), args)
        }
        BuiltinFn::SafeDiv(a, b, c) => {
            let mut args = vec![expr2_to_expr0(a), expr2_to_expr0(b)];
            if let Some(c) = c {
                args.push(expr2_to_expr0(c));
            }
            UntypedBuiltinFn("safediv".to_string(), args)
        }
        BuiltinFn::Sign(expr) => UntypedBuiltinFn("sign".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Sin(expr) => UntypedBuiltinFn("sin".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Sqrt(expr) => UntypedBuiltinFn("sqrt".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Step(a, b) => UntypedBuiltinFn(
            "step".to_string(),
            vec![expr2_to_expr0(a), expr2_to_expr0(b)],
        ),
        BuiltinFn::Tan(expr) => UntypedBuiltinFn("tan".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Time => UntypedBuiltinFn("time".to_string(), vec![]),
        BuiltinFn::TimeStep => UntypedBuiltinFn("time_step".to_string(), vec![]),
        BuiltinFn::StartTime => UntypedBuiltinFn("initial_time".to_string(), vec![]),
        BuiltinFn::FinalTime => UntypedBuiltinFn("final_time".to_string(), vec![]),
        BuiltinFn::Rank(expr, opts) => {
            let mut args = vec![expr2_to_expr0(expr)];
            if let Some((a, b)) = opts {
                args.push(expr2_to_expr0(a));
                if let Some(b) = b {
                    args.push(expr2_to_expr0(b));
                }
            }
            UntypedBuiltinFn("rank".to_string(), args)
        }
        BuiltinFn::Size(expr) => UntypedBuiltinFn("size".to_string(), vec![expr2_to_expr0(expr)]),
        BuiltinFn::Stddev(expr) => {
            UntypedBuiltinFn("stddev".to_string(), vec![expr2_to_expr0(expr)])
        }
        BuiltinFn::Sum(expr) => UntypedBuiltinFn("sum".to_string(), vec![expr2_to_expr0(expr)]),
    }
}

fn rename_canonical_ident(
    ident: &Ident<Canonical>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> Ident<Canonical> {
    if ident == old_ident {
        return new_ident.clone();
    }

    let ident_str = ident.as_str();
    if let Some(pos) = ident_str.rfind('·') {
        let prefix = &ident_str[..pos];
        let suffix = &ident_str[pos + '·'.len_utf8()..];

        // Only rename self-qualified references (self·variable)
        // Don't rename other module-qualified references as they refer to different variables
        if suffix == old_ident.as_str() && prefix == "self" {
            return Ident::from_unchecked(format!("self·{}", new_ident.as_str()));
        }
    }

    ident.clone()
}

fn rename_identifier_string(
    ident: &str,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> String {
    let canonical = canonicalize(ident);
    let renamed = rename_canonical_ident(&canonical, old_ident, new_ident);
    renamed.as_str().to_string()
}

fn rename_module_references(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for var in model.variables.iter_mut() {
        if let Variable::Module(module) = var {
            for reference in module.references.iter_mut() {
                rename_module_reference_string(&mut reference.src, old_ident, new_ident);
                rename_module_reference_string(&mut reference.dst, old_ident, new_ident);
            }
        }
    }
}

fn rename_module_reference_string(
    value: &mut String,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    let canonical = canonicalize(value.as_str());
    let renamed = rename_canonical_ident(&canonical, old_ident, new_ident);
    if renamed != canonical {
        *value = renamed.to_source_repr();
    }
}

fn update_stock_flow_references(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for var in model.variables.iter_mut() {
        if let Variable::Stock(stock) = var {
            for inflow in stock.inflows.iter_mut() {
                if canonicalize(inflow.as_str()) == *old_ident {
                    *inflow = new_ident.to_source_repr();
                }
            }
            for outflow in stock.outflows.iter_mut() {
                if canonicalize(outflow.as_str()) == *old_ident {
                    *outflow = new_ident.to_source_repr();
                }
            }
            stock.inflows.sort_unstable();
            stock.outflows.sort_unstable();
        }
    }
}

fn apply_upsert_view(model: &mut datamodel::Model, op: &project_io::UpsertViewOp) -> Result<()> {
    let view_pb = require_field!(&op.view, "missing view payload");
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
    let source = require_field!(&op.source, "missing source payload");
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

    #[test]
    fn rename_aux_updates_equations() {
        let mut project = TestProject::new("test")
            .aux("foo", "bar + 1", None)
            .aux("bar", "foo + 2", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "bar".to_string(),
                            to: "baz".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("foo").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => assert_eq!(eqn, "baz + 1"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        match model.get_variable("baz").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => assert_eq!(eqn, "foo + 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected renamed auxiliary"),
        }

        assert!(model.get_variable("bar").is_none());
    }

    #[test]
    fn rename_updates_module_references() {
        let mut project = TestProject::new("test")
            .aux("input", "1", None)
            .aux("consumer", "input * 2", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .expect("main model");

        model
            .variables
            .push(datamodel::Variable::Module(datamodel::Module {
                ident: "child".to_string(),
                model_name: "child".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "input".to_string(),
                    dst: "self.target".to_string(),
                }],
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            }));

        project.models.push(datamodel::Model {
            name: "child".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "target".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string(), None),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "input".to_string(),
                            to: "new_input".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => assert_eq!(eqn, "new_input * 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        let module = model
            .variables
            .iter()
            .find_map(|var| match var {
                Variable::Module(module) => Some(module),
                _ => None,
            })
            .expect("module variable");

        assert_eq!(module.references.len(), 1);
        assert_eq!(module.references[0].src, "new_input");
        assert_eq!(module.references[0].dst, "self.target");
    }

    #[test]
    fn rename_does_not_affect_unrelated_module_variables() {
        let mut project = TestProject::new("test")
            .aux("foo", "1", None)
            .aux("bar", "2", None)
            .aux("consumer", "foo + child·foo + bar", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .expect("main model");

        model
            .variables
            .push(datamodel::Variable::Module(datamodel::Module {
                ident: "child".to_string(),
                model_name: "child_model".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "bar".to_string(),
                    dst: "child·foo".to_string(),
                }],
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            }));

        project.models.push(datamodel::Model {
            name: "child_model".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "foo".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string(), None),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "foo".to_string(),
                            to: "renamed_foo".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => {
                    assert_eq!(eqn, "renamed_foo + child·foo + bar");
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        match model.get_variable("renamed_foo").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => assert_eq!(eqn, "1"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected renamed auxiliary"),
        }

        assert!(model.get_variable("foo").is_none());
    }

    #[test]
    fn rename_self_qualified_references() {
        let mut project = TestProject::new("test")
            .aux("foo", "1", None)
            .aux("consumer", "foo + self·foo", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "foo".to_string(),
                            to: "bar".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn, _) => {
                    assert_eq!(eqn, "bar + self·bar");
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_arrayed_equation() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
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
                        ident: "base_value".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string(), None),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "regional_growth".to_string(),
                        equation: datamodel::Equation::Arrayed(
                            vec!["Region".to_string()],
                            vec![
                                (
                                    "North".to_string(),
                                    "base_value * 1.5".to_string(),
                                    None,
                                    None,
                                ),
                                (
                                    "South".to_string(),
                                    "base_value * 2".to_string(),
                                    None,
                                    None,
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
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "base_value".to_string(),
                            to: "initial_value".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("regional_growth").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &vec!["Region".to_string()]);
                    assert_eq!(elements[0].0, "North");
                    assert_eq!(elements[0].1, "initial_value * 1.5");
                    assert_eq!(elements[1].0, "South");
                    assert_eq!(elements[1].1, "initial_value * 2");
                }
                _ => panic!("expected arrayed equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_apply_to_all_equation() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![datamodel::Dimension::named(
                "Product".to_string(),
                vec!["A".to_string(), "B".to_string(), "C".to_string()],
            )],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "price".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string(), None),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "revenue".to_string(),
                        equation: datamodel::Equation::ApplyToAll(
                            vec!["Product".to_string()],
                            "price * quantity".to_string(),
                            None,
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "quantity".to_string(),
                        equation: datamodel::Equation::Scalar("5".to_string(), None),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "price".to_string(),
                            to: "unit_price".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("revenue").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::ApplyToAll(dims, eqn, _) => {
                    assert_eq!(dims, &vec!["Product".to_string()]);
                    assert_eq!(eqn, "unit_price * quantity");
                }
                _ => panic!("expected apply-to-all equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_stock_with_initial_value() {
        let mut project = TestProject::new("test")
            .aux("initial_stock", "100", None)
            .stock("inventory", "initial_stock * 2", &[], &[], None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::RenameVariable(
                        project_io::RenameVariableOp {
                            from: "initial_stock".to_string(),
                            to: "starting_inventory".to_string(),
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("inventory").unwrap() {
            Variable::Stock(stock) => match &stock.equation {
                datamodel::Equation::Scalar(main, initial) => {
                    assert_eq!(main, "starting_inventory * 2");
                    assert_eq!(initial, &None);
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected stock variable"),
        }
    }

    #[test]
    fn upsert_stock_to_model_with_empty_name() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "".to_string(),
                sim_specs: None,
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let stock = datamodel::Stock {
            ident: "inventory".to_string(),
            equation: Equation::Scalar("100".to_string(), None),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
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
        let var = model.get_variable("inventory").unwrap();
        match var {
            Variable::Stock(actual) => assert_eq!(actual.equation, stock.equation),
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn update_stock_flows_preserves_equation() {
        let mut project = TestProject::new("test")
            .flow("inflow", "10", None)
            .stock("inventory", "100", &["inflow"], &[], None)
            .build_datamodel();

        // Verify initial state
        let model = project.get_model("main").unwrap();
        match model.get_variable("inventory").unwrap() {
            Variable::Stock(stock) => {
                assert_eq!(stock.inflows, vec!["inflow".to_string()]);
                assert_eq!(stock.equation, Equation::Scalar("100".to_string(), None));
            }
            _ => panic!("expected stock"),
        }

        // Apply updateStockFlows to remove the inflow
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpdateStockFlows(
                        project_io::UpdateStockFlowsOp {
                            ident: "inventory".to_string(),
                            inflows: vec![],
                            outflows: vec![],
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();

        let model = project.get_model("main").unwrap();
        match model.get_variable("inventory").unwrap() {
            Variable::Stock(stock) => {
                // Flows should be updated
                assert!(stock.inflows.is_empty());
                assert!(stock.outflows.is_empty());
                // Equation should be preserved
                assert_eq!(stock.equation, Equation::Scalar("100".to_string(), None));
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn update_stock_flows_preserves_all_fields() {
        let mut project = TestProject::new("test")
            .flow("birth_rate", "10", None)
            .stock_with_options(
                "population",
                "1000",
                &["birth_rate"],
                &[],
                Some("people"),
                "Total population",
                true, // non_negative
                true, // can_be_module_input
                Visibility::Public,
                Some(42),
            )
            .build_datamodel();

        // Disconnect the flow
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpdateStockFlows(
                        project_io::UpdateStockFlowsOp {
                            ident: "population".to_string(),
                            inflows: vec![],
                            outflows: vec![],
                        },
                    )),
                }],
            }],
        };

        apply_patch(&mut project, &patch).unwrap();

        let model = project.get_model("main").unwrap();
        match model.get_variable("population").unwrap() {
            Variable::Stock(stock) => {
                // Flows updated
                assert!(stock.inflows.is_empty());
                assert!(stock.outflows.is_empty());
                // All other fields preserved
                assert_eq!(stock.equation, Equation::Scalar("1000".to_string(), None));
                assert_eq!(stock.documentation, "Total population");
                assert_eq!(stock.units, Some("people".to_string()));
                assert!(stock.non_negative);
                assert!(stock.can_be_module_input);
                assert_eq!(stock.visibility, Visibility::Public);
                assert_eq!(stock.uid, Some(42));
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn update_stock_flows_nonexistent_stock_returns_error() {
        let mut project = TestProject::new("test").build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation {
                    op: Some(model_operation::Op::UpdateStockFlows(
                        project_io::UpdateStockFlowsOp {
                            ident: "nonexistent".to_string(),
                            inflows: vec![],
                            outflows: vec![],
                        },
                    )),
                }],
            }],
        };

        let err = apply_patch(&mut project, &patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DoesNotExist);
    }
}
