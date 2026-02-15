// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod codegen;
pub mod context;
pub mod dimensions;
pub mod expr;
pub mod pretty;
pub mod subscript;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use ordered_float::OrderedFloat;

use crate::ast::{Ast, Loc};
use crate::bytecode::CompiledModule;
use crate::common::{Canonical, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result};
use crate::dimensions::{Dimension, DimensionsContext, SubscriptIterator};
use crate::float::SimFloat;
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::Variable;
use crate::vm::{IMPLICIT_VAR_COUNT, ModuleKey};
use crate::{Error, sim_err};

// Re-exports for crate-internal API
pub(crate) use self::context::{Context, VariableMetadata};
pub(crate) use self::dimensions::UnaryOp;
pub(crate) use self::expr::{BuiltinFn, Expr, SubscriptIndex, Table};
pub(crate) use self::pretty::pretty;

use self::codegen::Compiler;

// Type alias to reduce complexity
type VariableOffsetMap = HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, (usize, usize)>>;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub struct Var<F: SimFloat> {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) ast: Vec<Expr<F>>,
}

#[test]
fn test_fold_flows() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    metadata.insert(
        Ident::new("a"),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: Variable::Var {
                ident: Ident::new(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        Ident::new("b"),
        VariableMetadata {
            offset: 2,
            size: 1,
            var: Variable::Var {
                ident: Ident::new(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        Ident::new("c"),
        VariableMetadata {
            offset: 3,
            size: 1,
            var: Variable::Var {
                ident: Ident::new(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        Ident::new("d"),
        VariableMetadata {
            offset: 4,
            size: 1,
            var: Variable::Var {
                ident: Ident::new(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context {
        dimensions: vec![],
        dimensions_ctx: &dims_ctx,
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
        preserve_wildcards_for_iteration: false,
    };

    assert_eq!(Ok(None), ctx.fold_flows::<f64>(&[]));
    assert_eq!(
        Ok(Some(Expr::Var(1, Loc::default()))),
        ctx.fold_flows::<f64>(&[Ident::new("a")])
    );
    assert_eq!(
        Ok(Some(Expr::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(Expr::Var(1, Loc::default())),
            Box::new(Expr::Var(4, Loc::default())),
            Loc::default(),
        ))),
        ctx.fold_flows::<f64>(&[Ident::new("a"), Ident::new("d")])
    );

    // Test that fold_flows returns an error for non-existent flows
    let result = ctx.fold_flows::<f64>(&[Ident::new("nonexistent")]);
    assert!(result.is_err(), "Expected error for non-existent flow");
}

impl<F: SimFloat> Var<F> {
    pub(crate) fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        // if this variable is overriden by a module input, our expression is easy
        let ast: Vec<Expr<F>> = if let Some((off, _ident)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| n.as_str() == var.ident())
        {
            vec![Expr::AssignCurr(
                ctx.get_offset(&Ident::new(var.ident()))?,
                Box::new(Expr::ModuleInput(off, Loc::default())),
            )]
        } else {
            match var {
                Variable::Module {
                    ident,
                    model_name,
                    inputs,
                    ..
                } => {
                    let mut inputs = inputs.clone();
                    inputs.sort_unstable_by(|a, b| a.dst.partial_cmp(&b.dst).unwrap());
                    // Create input set for module lookup key
                    let input_set: BTreeSet<Ident<Canonical>> =
                        inputs.iter().map(|mi| mi.dst.clone()).collect();
                    let inputs: Vec<Expr<F>> = inputs
                        .into_iter()
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap(), Loc::default()))
                        .collect();
                    vec![Expr::EvalModule(
                        ident.clone(),
                        model_name.clone(),
                        input_set,
                        inputs,
                    )]
                }
                Variable::Stock { init_ast: ast, .. } => {
                    let off = ctx.get_base_offset(&Ident::new(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                let mut exprs = ctx.lower(ast)?;
                                let main_expr = exprs.pop().unwrap();
                                exprs.push(Expr::AssignCurr(off, Box::new(main_expr)));
                                exprs
                            }
                            Ast::ApplyToAll(dims, ast) => {
                                let exprs: Result<Vec<Vec<Expr<F>>>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        ctx.lower(ast).map(|mut exprs| {
                                            let main_expr = exprs.pop().unwrap();
                                            exprs.push(Expr::AssignCurr(
                                                off + i,
                                                Box::new(main_expr),
                                            ));
                                            exprs
                                        })
                                    })
                                    .collect();
                                exprs?.into_iter().flatten().collect()
                            }
                            Ast::Arrayed(dims, elements) => {
                                let exprs: Result<Vec<Vec<Expr<F>>>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let subscript_str = subscripts.join(",");
                                        let canonical_key =
                                            CanonicalElementName::from_raw(&subscript_str);
                                        let ast = &elements[&canonical_key];
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        ctx.lower(ast).map(|mut exprs| {
                                            let main_expr = exprs.pop().unwrap();
                                            exprs.push(Expr::AssignCurr(
                                                off + i,
                                                Box::new(main_expr),
                                            ));
                                            exprs
                                        })
                                    })
                                    .collect();
                                exprs?.into_iter().flatten().collect()
                            }
                        }
                    } else {
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)?),
                            )],
                            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _) => {
                                let exprs: Result<Vec<Expr<F>>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        let update_expr = ctx.build_stock_update_expr(
                                            ctx.get_offset(&Ident::new(var.ident()))?,
                                            var,
                                        )?;
                                        Ok(Expr::AssignNext(off + i, Box::new(update_expr)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                Variable::Var { tables, .. } => {
                    let off = ctx.get_base_offset(&Ident::new(var.ident()))?;
                    let ast = if ctx.is_initial {
                        var.init_ast()
                    } else {
                        var.ast()
                    };
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().to_string());
                    }
                    match ast.as_ref().unwrap() {
                        Ast::Scalar(ast) => {
                            let mut exprs = ctx.lower(ast)?;
                            let main_expr = exprs.pop().unwrap();
                            let main_expr = if !tables.is_empty() {
                                let loc = main_expr.get_loc();
                                Expr::App(
                                    BuiltinFn::Lookup(
                                        Box::new(Expr::Var(off, loc)),
                                        Box::new(main_expr),
                                        loc,
                                    ),
                                    loc,
                                )
                            } else {
                                main_expr
                            };
                            exprs.push(Expr::AssignCurr(off, Box::new(main_expr)));
                            exprs
                        }
                        Ast::ApplyToAll(dims, ast) => {
                            let exprs: Result<Vec<Vec<Expr<F>>>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(
                                        subscripts
                                            .iter()
                                            .map(|s| CanonicalElementName::from_raw(s))
                                            .collect(),
                                    );
                                    ctx.lower(ast).map(|mut exprs| {
                                        let main_expr = exprs.pop().unwrap();
                                        exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                                        exprs
                                    })
                                })
                                .collect();
                            exprs?.into_iter().flatten().collect()
                        }
                        Ast::Arrayed(dims, elements) => {
                            let exprs: Result<Vec<Vec<Expr<F>>>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let subscript_str = subscripts.join(",");
                                    let canonical_key =
                                        CanonicalElementName::from_raw(&subscript_str);
                                    let ast = &elements[&canonical_key];
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(
                                        subscripts
                                            .iter()
                                            .map(|s| CanonicalElementName::from_raw(s))
                                            .collect(),
                                    );
                                    ctx.lower(ast).map(|mut exprs| {
                                        let main_expr = exprs.pop().unwrap();
                                        exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                                        exprs
                                    })
                                })
                                .collect();
                            exprs?.into_iter().flatten().collect()
                        }
                    }
                }
            }
        };
        Ok(Var {
            ident: Ident::new(var.ident()),
            ast,
        })
    }
}

/// Recursively extract temporary array sizes from an expression.
/// Populates the temp_sizes_map with (temp_id, max_size) entries.
/// Since temp IDs restart at 0 for each lower() call, the same ID may be
/// reused across different expressions with different sizes. We track the
/// maximum size per ID to ensure the temp buffer is large enough for all uses.
fn extract_temp_sizes<F: SimFloat>(expr: &Expr<F>, temp_sizes_map: &mut HashMap<u32, usize>) {
    match expr {
        Expr::AssignTemp(id, inner, view) => {
            let size = view.dims.iter().product::<usize>();
            // Preserve the maximum size for this temp ID across all expressions
            temp_sizes_map
                .entry(*id)
                .and_modify(|existing| *existing = (*existing).max(size))
                .or_insert(size);
            extract_temp_sizes(inner, temp_sizes_map);
        }
        Expr::TempArray(_, _, _) | Expr::TempArrayElement(_, _, _, _) => {
            // These reference temps, but don't define sizes - do nothing
        }
        Expr::Const(_, _) | Expr::Var(_, _) | Expr::Dt(_) => {}
        Expr::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    SubscriptIndex::Single(e) => extract_temp_sizes(e, temp_sizes_map),
                    SubscriptIndex::Range(start, end) => {
                        extract_temp_sizes(start, temp_sizes_map);
                        extract_temp_sizes(end, temp_sizes_map);
                    }
                }
            }
        }
        Expr::StaticSubscript(_, _, _) => {}
        Expr::App(builtin, _) => {
            extract_temp_sizes_from_builtin(builtin, temp_sizes_map);
        }
        Expr::EvalModule(_, _, _, args) => {
            for arg in args {
                extract_temp_sizes(arg, temp_sizes_map);
            }
        }
        Expr::ModuleInput(_, _) => {}
        Expr::Op2(_, left, right, _) => {
            extract_temp_sizes(left, temp_sizes_map);
            extract_temp_sizes(right, temp_sizes_map);
        }
        Expr::Op1(_, inner, _) => {
            extract_temp_sizes(inner, temp_sizes_map);
        }
        Expr::If(cond, t, f, _) => {
            extract_temp_sizes(cond, temp_sizes_map);
            extract_temp_sizes(t, temp_sizes_map);
            extract_temp_sizes(f, temp_sizes_map);
        }
        Expr::AssignCurr(_, inner) | Expr::AssignNext(_, inner) => {
            extract_temp_sizes(inner, temp_sizes_map);
        }
    }
}

/// Extract temp sizes from builtin function arguments.
fn extract_temp_sizes_from_builtin<F: SimFloat>(
    builtin: &BuiltinFn<F>,
    temp_sizes_map: &mut HashMap<u32, usize>,
) {
    match builtin {
        BuiltinFn::Lookup(_, expr, _)
        | BuiltinFn::LookupForward(_, expr, _)
        | BuiltinFn::LookupBackward(_, expr, _)
        | BuiltinFn::Abs(expr)
        | BuiltinFn::Arccos(expr)
        | BuiltinFn::Arcsin(expr)
        | BuiltinFn::Arctan(expr)
        | BuiltinFn::Cos(expr)
        | BuiltinFn::Exp(expr)
        | BuiltinFn::Int(expr)
        | BuiltinFn::Ln(expr)
        | BuiltinFn::Log10(expr)
        | BuiltinFn::Sign(expr)
        | BuiltinFn::Sin(expr)
        | BuiltinFn::Size(expr)
        | BuiltinFn::Sqrt(expr)
        | BuiltinFn::Stddev(expr)
        | BuiltinFn::Sum(expr)
        | BuiltinFn::Tan(expr) => {
            extract_temp_sizes(expr, temp_sizes_map);
        }
        BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            if let Some(b) = b {
                extract_temp_sizes(b, temp_sizes_map);
            }
        }
        BuiltinFn::Mean(args) => {
            for arg in args {
                extract_temp_sizes(arg, temp_sizes_map);
            }
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            if let Some(c) = c {
                extract_temp_sizes(c, temp_sizes_map);
            }
        }
        BuiltinFn::Rank(a, opt) => {
            extract_temp_sizes(a, temp_sizes_map);
            if let Some((b, c)) = opt {
                extract_temp_sizes(b, temp_sizes_map);
                if let Some(c) = c {
                    extract_temp_sizes(c, temp_sizes_map);
                }
            }
        }
        BuiltinFn::Step(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
        }
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime
        | BuiltinFn::IsModuleInput(_, _) => {}
    }
}

/// Per-variable initial expressions, kept alongside the flat runlist for
/// interpreter compatibility.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct VarInitial<F: SimFloat> {
    pub(crate) ident: Ident<Canonical>,
    /// Sorted, deduplicated offsets extracted from AssignCurr nodes.
    pub(crate) offsets: Vec<usize>,
    pub(crate) ast: Vec<Expr<F>>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct Module<F: SimFloat> {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) inputs: HashSet<Ident<Canonical>>,
    pub(crate) n_slots: usize,         // number of f64s we need storage for
    pub(crate) n_temps: usize,         // number of temporary arrays
    pub(crate) temp_sizes: Vec<usize>, // size of each temporary array
    pub(crate) runlist_initials: Vec<Expr<F>>,
    pub(crate) runlist_initials_by_var: Vec<VarInitial<F>>,
    pub(crate) runlist_flows: Vec<Expr<F>>,
    pub(crate) runlist_stocks: Vec<Expr<F>>,
    pub(crate) offsets: VariableOffsetMap,
    pub(crate) runlist_order: Vec<Ident<Canonical>>,
    pub(crate) tables: HashMap<Ident<Canonical>, Vec<Table<F>>>,
    /// All dimensions from the project, for bytecode compilation
    pub(crate) dimensions: Vec<Dimension>,
    /// DimensionsContext for subdimension relationship lookups
    pub(crate) dimensions_ctx: DimensionsContext,
    /// Maps module variable idents to their full ModuleKey (model_name, input_set).
    /// Used to correctly expand nested modules in runlist_order.
    pub(crate) module_refs: HashMap<Ident<Canonical>, ModuleKey>,
}

// calculate a mapping of module variable name -> module model name
pub(crate) fn calc_module_model_map(
    project: &Project,
    model_name: &Ident<Canonical>,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> {
    let mut all_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();

    let model = Arc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    let mut current_mapping: HashMap<Ident<Canonical>, Ident<Canonical>> = HashMap::new();

    for ident in var_names.iter() {
        let canonical_ident = Ident::new(ident);
        if let Variable::Module {
            model_name: module_model_name,
            ..
        } = &model.variables[&canonical_ident]
        {
            current_mapping.insert(canonical_ident.clone(), module_model_name.clone());
            let all_sub_models = calc_module_model_map(project, module_model_name);
            all_models.extend(all_sub_models);
        };
    }

    all_models.insert(model_name.clone(), current_mapping);

    all_models
}

// TODO: this should memoize
pub(crate) fn build_metadata(
    project: &Project,
    model_name: &Ident<Canonical>,
    is_root: bool,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>> {
    let mut all_offsets: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>> =
        HashMap::new();

    let mut offsets: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(
            Ident::new("time"),
            VariableMetadata {
                offset: 0,
                size: 1,
                var: Variable::Var {
                    ident: Ident::new("time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            Ident::new("dt"),
            VariableMetadata {
                offset: 1,
                size: 1,
                var: Variable::Var {
                    ident: Ident::new("dt"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            Ident::new("initial_time"),
            VariableMetadata {
                offset: 2,
                size: 1,
                var: Variable::Var {
                    ident: Ident::new("initial_time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            Ident::new("final_time"),
            VariableMetadata {
                offset: 3,
                size: 1,
                var: Variable::Var {
                    ident: Ident::new("final_time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Arc::clone(&project.models[model_name]);
    let var_names: Vec<&Ident<Canonical>> = {
        let mut var_names: Vec<_> = model.variables.keys().collect();
        var_names.sort_unstable();
        var_names
    };

    for canonical_ident in var_names {
        let size = if let Variable::Module { model_name, .. } = &model.variables[canonical_ident] {
            let all_sub_offsets = build_metadata(project, model_name, false);
            let sub_offsets = &all_sub_offsets[model_name];
            let sub_size: usize = sub_offsets.values().map(|metadata| metadata.size).sum();
            all_offsets.extend(all_sub_offsets);
            sub_size
        } else if let Some(Ast::ApplyToAll(dims, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else {
            1
        };
        offsets.insert(
            canonical_ident.clone(),
            VariableMetadata {
                offset: i,
                size,
                var: model.variables[canonical_ident].clone(),
            },
        );
        i += size;
    }

    all_offsets.insert(model_name.clone(), offsets);

    all_offsets
}

fn calc_n_slots(
    all_metadata: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>>,
    model_name: &Ident<Canonical>,
) -> usize {
    let metadata = &all_metadata[model_name];

    metadata.values().map(|v| v.size).sum()
}

impl<F: SimFloat> Module<F> {
    pub(crate) fn new(
        project: &Project,
        model: Arc<ModelStage1>,
        inputs: &BTreeSet<Ident<Canonical>>,
        is_root: bool,
    ) -> Result<Self> {
        let instantiation = model
            .instantiations
            .as_ref()
            .and_then(|instantiations| instantiations.get(inputs))
            .ok_or(Error {
                kind: ErrorKind::Simulation,
                code: ErrorCode::NotSimulatable,
                details: Some(model.name.to_string()),
            })?;

        // TODO: eventually we should try to simulate subsets of the model in the face of errors
        if model.errors.is_some() && !model.errors.as_ref().unwrap().is_empty() {
            return sim_err!(NotSimulatable, model.name.to_string());
        }

        let model_name: &Ident<Canonical> = &model.name;
        let metadata = build_metadata(project, model_name, is_root);

        let n_slots = calc_n_slots(&metadata, model_name);
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            var_names.sort_unstable();
            var_names
        };
        let module_models = calc_module_model_map(project, model_name);

        // Build module_refs: map from module variable ident to (model_name, input_set)
        let module_refs: HashMap<Ident<Canonical>, ModuleKey> = model
            .variables
            .iter()
            .filter_map(|(ident, var)| {
                if let Variable::Module {
                    model_name: module_model_name,
                    inputs,
                    ..
                } = var
                {
                    let input_set: BTreeSet<Ident<Canonical>> =
                        inputs.iter().map(|mi| mi.dst.clone()).collect();
                    Some((ident.clone(), (module_model_name.clone(), input_set)))
                } else {
                    None
                }
            })
            .collect();

        let converted_dims: Vec<Dimension> = project
            .datamodel
            .dimensions
            .iter()
            .map(Dimension::from)
            .collect();

        let build_var = |ident: &Ident<Canonical>, is_initial| {
            Var::new(
                &Context {
                    dimensions: converted_dims.clone(),
                    dimensions_ctx: &project.dimensions_ctx,
                    model_name,
                    ident,
                    active_dimension: None,
                    active_subscript: None,
                    metadata: &metadata,
                    module_models: &module_models,
                    is_initial,
                    inputs,
                    preserve_wildcards_for_iteration: false,
                },
                &model.variables[ident],
            )
        };

        let initial_vars = instantiation
            .runlist_initials
            .iter()
            .map(|ident| build_var(ident, true))
            .collect::<Result<Vec<Var<F>>>>()?;
        let flow_vars = instantiation
            .runlist_flows
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var<F>>>>()?;
        let stock_vars = instantiation
            .runlist_stocks
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var<F>>>>()?;

        let mut runlist_order = Vec::with_capacity(flow_vars.len() + stock_vars.len());
        runlist_order.extend(flow_vars.iter().map(|v| v.ident.clone()));
        runlist_order.extend(stock_vars.iter().map(|v| v.ident.clone()));

        // Build per-variable initials before flattening
        let runlist_initials_by_var: Vec<VarInitial<F>> = initial_vars
            .iter()
            .map(|v| {
                let mut offsets: Vec<usize> = v
                    .ast
                    .iter()
                    .filter_map(|expr| {
                        if let Expr::AssignCurr(off, _) = expr {
                            Some(*off)
                        } else {
                            None
                        }
                    })
                    .collect();
                offsets.sort_unstable();
                offsets.dedup();
                VarInitial {
                    ident: v.ident.clone(),
                    offsets,
                    ast: v.ast.clone(),
                }
            })
            .collect();

        // Flatten out the variables so that we're just dealing with lists of expressions
        let runlist_initials: Vec<Expr<F>> = initial_vars.into_iter().flat_map(|v| v.ast).collect();
        let runlist_flows: Vec<Expr<F>> = flow_vars.into_iter().flat_map(|v| v.ast).collect();
        let runlist_stocks: Vec<Expr<F>> = stock_vars.into_iter().flat_map(|v| v.ast).collect();

        // Extract temp array information from all runlists
        let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
        for expr in runlist_initials
            .iter()
            .chain(runlist_flows.iter())
            .chain(runlist_stocks.iter())
        {
            extract_temp_sizes(expr, &mut temp_sizes_map);
        }

        // Build temp_sizes vector, ordered by temp ID
        let n_temps = temp_sizes_map.len();
        let mut temp_sizes: Vec<usize> = vec![0; n_temps];
        for (id, size) in temp_sizes_map {
            temp_sizes[id as usize] = size;
        }

        let tables: Result<HashMap<Ident<Canonical>, Vec<Table<F>>>> = var_names
            .iter()
            .map(|id| {
                let canonical_id = Ident::new(id);
                (id, &model.variables[&canonical_id])
            })
            .filter(|(_, v)| !v.tables().is_empty())
            .map(|(id, v)| {
                let tables_result: Result<Vec<Table<F>>> =
                    v.tables().iter().map(|t| Table::new(id, t)).collect();
                (id, tables_result)
            })
            .map(|(id, tables_result)| match tables_result {
                Ok(tables) => Ok((Ident::new(id), tables)),
                Err(err) => Err(err),
            })
            .collect();
        let tables = tables?;

        let offsets = metadata
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.iter()
                        .map(|(k, v)| (k.clone(), (v.offset, v.size)))
                        .collect(),
                )
            })
            .collect();

        Ok(Module {
            ident: model_name.clone(),
            inputs: inputs.iter().cloned().collect(),
            n_slots,
            n_temps,
            temp_sizes,
            runlist_initials,
            runlist_initials_by_var,
            runlist_flows,
            runlist_stocks,
            offsets,
            runlist_order,
            tables,
            dimensions: converted_dims,
            dimensions_ctx: project.dimensions_ctx.clone(),
            module_refs,
        })
    }

    pub fn compile(&self) -> Result<CompiledModule<F>>
    where
        OrderedFloat<F>: Eq + std::hash::Hash,
    {
        Compiler::new(self).compile()
    }
}

#[cfg(test)]
impl<F: SimFloat> Module<F> {
    /// Get flow expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions that target offsets within this variable's range.
    pub fn get_flow_exprs(&self, var_name: &str) -> Vec<&Expr<F>> {
        let canonical_name = Ident::new(var_name);

        // Look up the variable's offset range
        let Some(model_offsets) = self.offsets.get(&self.ident) else {
            return vec![];
        };
        let Some(&(base_offset, size)) = model_offsets.get(&canonical_name) else {
            return vec![];
        };
        let offset_range = base_offset..base_offset + size;

        // Find all AssignCurr expressions that target offsets in this range
        self.runlist_flows
            .iter()
            .filter(|expr| {
                if let Expr::AssignCurr(off, _) = expr {
                    offset_range.contains(off)
                } else {
                    false
                }
            })
            .collect()
    }

    /// Get initial expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions in the initials runlist for this variable.
    pub fn get_initial_exprs(&self, var_name: &str) -> Vec<&Expr<F>> {
        let canonical_name = Ident::new(var_name);

        // Look up the variable's offset range
        let Some(model_offsets) = self.offsets.get(&self.ident) else {
            return vec![];
        };
        let Some(&(base_offset, size)) = model_offsets.get(&canonical_name) else {
            return vec![];
        };
        let offset_range = base_offset..base_offset + size;

        // Find all AssignCurr expressions that target offsets in this range
        self.runlist_initials
            .iter()
            .filter(|expr| {
                if let Expr::AssignCurr(off, _) = expr {
                    offset_range.contains(off)
                } else {
                    false
                }
            })
            .collect()
    }
}
