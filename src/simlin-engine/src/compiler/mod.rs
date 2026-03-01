// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod codegen;
pub mod context;
pub mod dimensions;
pub mod expr;
pub mod pretty;
pub mod subscript;
pub(crate) mod symbolic;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{ArrayView, Ast, Loc};
use crate::bytecode::CompiledModule;
use crate::common::{Canonical, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result};
use crate::dimensions::{Dimension, DimensionsContext, SubscriptIterator};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::Variable;
use crate::vm::{IMPLICIT_VAR_COUNT, ModuleKey};
use crate::{Error, sim_err};

// Re-exports for crate-internal API
pub(crate) use self::context::{Context, ContextCore, VariableMetadata};
pub(crate) use self::dimensions::UnaryOp;
pub(crate) use self::expr::{BuiltinFn, Expr, SubscriptIndex, Table};
pub(crate) use self::pretty::pretty;

use self::codegen::Compiler;

// Type alias to reduce complexity
type VariableOffsetMap = HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, (usize, usize)>>;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub struct Var {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) ast: Vec<Expr>,
}

#[test]
fn test_fold_flows() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let dummy_var = Variable::Var {
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("a"),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("b"),
        VariableMetadata {
            offset: 2,
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("c"),
        VariableMetadata {
            offset: 3,
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("d"),
        VariableMetadata {
            offset: 4,
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

    assert_eq!(Ok(None), ctx.fold_flows(&[]));
    assert_eq!(
        Ok(Some(Expr::Var(1, Loc::default()))),
        ctx.fold_flows(&[Ident::new("a")])
    );
    assert_eq!(
        Ok(Some(Expr::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(Expr::Var(1, Loc::default())),
            Box::new(Expr::Var(4, Loc::default())),
            Loc::default(),
        ))),
        ctx.fold_flows(&[Ident::new("a"), Ident::new("d")])
    );

    // Test that fold_flows returns an error for non-existent flows
    let result = ctx.fold_flows(&[Ident::new("nonexistent")]);
    assert!(result.is_err(), "Expected error for non-existent flow");
}

#[test]
fn test_build_stock_update_expr_inflows_only() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let stock_var = Variable::Stock {
        ident: Ident::new("stock"),
        init_ast: None,
        eqn: None,
        units: None,
        inflows: vec![Ident::new("inflow")],
        outflows: vec![],
        non_negative: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let dummy_var = Variable::Var {
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: 0,
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("inflow"),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

    let result = ctx.build_stock_update_expr(0, &stock_var).unwrap();

    // stock + (inflow - 0.0) * dt
    // outflows should be Const(0.0) since there are none
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert!(matches!(stock_box.as_ref(), Expr::Var(0, _)));
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, dt_box, _) = dt_update_box.as_ref() {
            assert!(matches!(dt_box.as_ref(), Expr::Dt(_)));
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_box, out_box, _) = sub_box.as_ref() {
                assert!(matches!(in_box.as_ref(), Expr::Var(1, _)));
                assert!(
                    matches!(out_box.as_ref(), Expr::Const(v, _) if *v == 0.0),
                    "outflows should be Const(0.0) when empty"
                );
            } else {
                panic!("Expected Sub expression in stock update");
            }
        } else {
            panic!("Expected Mul expression in stock update");
        }
    } else {
        panic!("Expected Add expression for stock update");
    }
}

#[test]
fn test_build_stock_update_expr_outflows_only() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let stock_var = Variable::Stock {
        ident: Ident::new("stock"),
        init_ast: None,
        eqn: None,
        units: None,
        inflows: vec![],
        outflows: vec![Ident::new("outflow")],
        non_negative: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let dummy_var = Variable::Var {
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: 0,
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("outflow"),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

    let result = ctx.build_stock_update_expr(0, &stock_var).unwrap();

    // stock + (0.0 - outflow) * dt
    // inflows should be Const(0.0) since there are none
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert!(matches!(stock_box.as_ref(), Expr::Var(0, _)));
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, _, _) = dt_update_box.as_ref() {
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_box, out_box, _) = sub_box.as_ref() {
                assert!(
                    matches!(in_box.as_ref(), Expr::Const(v, _) if *v == 0.0),
                    "inflows should be Const(0.0) when empty"
                );
                assert!(matches!(out_box.as_ref(), Expr::Var(1, _)));
            } else {
                panic!("Expected Sub expression in stock update");
            }
        } else {
            panic!("Expected Mul expression in stock update");
        }
    } else {
        panic!("Expected Add expression for stock update");
    }
}

#[test]
fn test_build_stock_update_expr_no_flows() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let stock_var = Variable::Stock {
        ident: Ident::new("stock"),
        init_ast: None,
        eqn: None,
        units: None,
        inflows: vec![],
        outflows: vec![],
        non_negative: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let dummy_var = Variable::Var {
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: 0,
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

    let result = ctx.build_stock_update_expr(0, &stock_var).unwrap();

    // stock + (0.0 - 0.0) * dt
    if let Expr::Op2(crate::ast::BinaryOp::Add, _, dt_update_box, _) = &result {
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, _, _) = dt_update_box.as_ref() {
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_box, out_box, _) = sub_box.as_ref() {
                assert!(
                    matches!(in_box.as_ref(), Expr::Const(v, _) if *v == 0.0),
                    "inflows should be Const(0.0)"
                );
                assert!(
                    matches!(out_box.as_ref(), Expr::Const(v, _) if *v == 0.0),
                    "outflows should be Const(0.0)"
                );
            } else {
                panic!("Expected Sub expression");
            }
        } else {
            panic!("Expected Mul expression");
        }
    } else {
        panic!("Expected Add expression");
    }
}

#[test]
fn test_build_stock_update_expr_multiple_flows() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let stock_var = Variable::Stock {
        ident: Ident::new("stock"),
        init_ast: None,
        eqn: None,
        units: None,
        inflows: vec![Ident::new("in1"), Ident::new("in2")],
        outflows: vec![Ident::new("out1"), Ident::new("out2")],
        non_negative: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let dummy_var = Variable::Var {
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    for (name, off) in [
        ("stock", 0),
        ("in1", 1),
        ("in2", 2),
        ("out1", 3),
        ("out2", 4),
    ] {
        metadata.insert(
            Ident::new(name),
            VariableMetadata {
                offset: off,
                size: 1,
                var: &dummy_var,
            },
        );
    }
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

    let result = ctx.build_stock_update_expr(0, &stock_var).unwrap();

    // stock + ((in1 + in2) - (out1 + out2)) * dt
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert!(matches!(stock_box.as_ref(), Expr::Var(0, _)));
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, dt_box, _) = dt_update_box.as_ref() {
            assert!(matches!(dt_box.as_ref(), Expr::Dt(_)));
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_sum, out_sum, _) = sub_box.as_ref() {
                // in1 + in2
                assert!(matches!(
                    in_sum.as_ref(),
                    Expr::Op2(crate::ast::BinaryOp::Add, _, _, _)
                ));
                // out1 + out2
                assert!(matches!(
                    out_sum.as_ref(),
                    Expr::Op2(crate::ast::BinaryOp::Add, _, _, _)
                ));
            } else {
                panic!("Expected Sub expression");
            }
        } else {
            panic!("Expected Mul expression");
        }
    } else {
        panic!("Expected Add expression");
    }
}

#[test]
fn test_sparse_array_element_returns_error_not_panic() {
    use crate::test_common::TestProject;

    // Build a project with a 3-element dimension but only 2 of the 3
    // element keys provided. The compiler must not panic on the missing
    // element key -- whether it reports an error or silently succeeds
    // depends on the pipeline stage, but no panic is the guarantee.
    let _result = TestProject::new("sparse_test")
        .named_dimension("dim", &["a", "b", "c"])
        .array_with_ranges(
            "x[dim]",
            vec![("a", "1"), ("b", "2")], // 'c' intentionally missing
        )
        .aux("y", "1", None)
        .compile();
    // Reaching this point without panicking is the success criterion.
    // Before the fix, elements[&canonical_key] would panic for the
    // missing "c" key.
}

#[test]
fn test_arrayed_default_equation_applies_to_missing_elements() {
    let datamodel_dim = crate::datamodel::Dimension::named(
        "dim".to_string(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    );
    let dim = Dimension::from(&datamodel_dim);
    let dims = vec![dim.clone()];

    let mut elements = HashMap::new();
    elements.insert(
        CanonicalElementName::from_raw("a"),
        crate::ast::Expr2::Const("1".to_string(), 1.0, Loc::default()),
    );
    elements.insert(
        CanonicalElementName::from_raw("b"),
        crate::ast::Expr2::Const("2".to_string(), 2.0, Loc::default()),
    );

    let var = Variable::Var {
        ident: Ident::new("x"),
        ast: Some(Ast::Arrayed(
            dims.clone(),
            elements,
            Some(crate::ast::Expr2::Const(
                "7".to_string(),
                7.0,
                Loc::default(),
            )),
            true,
        )),
        init_ast: None,
        eqn: None,
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };

    let mut model_metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    model_metadata.insert(
        Ident::new("x"),
        VariableMetadata {
            offset: 0,
            size: 3,
            var: &var,
        },
    );
    let mut metadata = HashMap::new();
    let model_name = Ident::new("main");
    metadata.insert(model_name.clone(), model_metadata);

    let inputs = BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let dims_ctx = DimensionsContext::from(std::slice::from_ref(&datamodel_dim));
    let ident = Ident::new("test");
    let ctx = Context::new(
        ContextCore {
            dimensions: &dims,
            dimensions_ctx: &dims_ctx,
            model_name: &model_name,
            metadata: &metadata,
            module_models: &module_models,
            inputs: &inputs,
        },
        &ident,
        false,
    );

    let lowered = Var::new(&ctx, &var).expect("arrayed lowering should succeed");

    let mut assigned: HashMap<usize, f64> = HashMap::new();
    for expr in lowered.ast {
        if let Expr::AssignCurr(off, rhs) = expr {
            if let Expr::Const(value, _) = *rhs {
                assigned.insert(off, value);
            } else {
                panic!("expected AssignCurr to use scalar constants in this test");
            }
        }
    }

    assert_eq!(assigned.get(&0), Some(&1.0));
    assert_eq!(assigned.get(&1), Some(&2.0));
    assert_eq!(
        assigned.get(&2),
        Some(&7.0),
        "missing element should use array default equation, not 0"
    );
}

impl Var {
    pub(crate) fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        // if this variable is overriden by a module input, our expression is easy
        let ast: Vec<Expr> = if let Some((off, _ident)) = ctx
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
                    let inputs: Vec<Expr> = inputs
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
                                expand_a2a_with_hoisting(ctx, dims, ast, off)?
                            }
                            Ast::Arrayed(
                                dims,
                                elements,
                                default_ast,
                                apply_default_for_missing,
                            ) => expand_arrayed_with_hoisting(
                                ctx,
                                dims,
                                elements,
                                default_ast.as_ref(),
                                *apply_default_for_missing,
                                off,
                            )?,
                        }
                    } else {
                        let Some(ast) = ast.as_ref() else {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        };
                        match ast {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)?),
                            )],
                            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => {
                                let active_dims = Arc::<[Dimension]>::from(dims.clone());
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let ctx = ctx.with_active_subscripts(
                                            active_dims.clone(),
                                            &subscripts,
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
                            expand_a2a_with_hoisting(ctx, dims, ast, off)?
                        }
                        Ast::Arrayed(dims, elements, default_ast, apply_default_for_missing) => {
                            expand_arrayed_with_hoisting(
                                ctx,
                                dims,
                                elements,
                                default_ast.as_ref(),
                                *apply_default_for_missing,
                                off,
                            )?
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

/// Check if an expression is an array-producing builtin that needs whole-array
/// evaluation rather than per-element scalar evaluation.
fn is_array_producing_builtin(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::App(
            BuiltinFn::VectorElmMap(_, _)
                | BuiltinFn::VectorSortOrder(_, _)
                | BuiltinFn::AllocateAvailable(_, _, _),
            _
        )
    )
}

/// Extract the output ArrayView from an expression, analogous to the
/// interpreter's `find_array_dims`.  For array-producing builtins, the
/// output dimensions come from the builtin's "shaping" argument:
///   VectorElmMap(_, offset)    -> offset's view
///   VectorSortOrder(arr, _)    -> arr's view
///   AllocateAvailable(req,_,_) -> req's view
fn find_expr_array_view(expr: &Expr) -> Option<ArrayView> {
    match expr {
        Expr::StaticSubscript(_, view, _) | Expr::TempArray(_, view, _) => Some(view.clone()),
        Expr::App(builtin, _) => match builtin {
            BuiltinFn::VectorElmMap(_, offset) => find_expr_array_view(offset),
            BuiltinFn::VectorSortOrder(arr, _) => find_expr_array_view(arr),
            BuiltinFn::AllocateAvailable(req, _, _) => find_expr_array_view(req),
            BuiltinFn::Abs(e)
            | BuiltinFn::Arccos(e)
            | BuiltinFn::Arcsin(e)
            | BuiltinFn::Arctan(e)
            | BuiltinFn::Cos(e)
            | BuiltinFn::Exp(e)
            | BuiltinFn::Int(e)
            | BuiltinFn::Ln(e)
            | BuiltinFn::Log10(e)
            | BuiltinFn::Sin(e)
            | BuiltinFn::Sqrt(e)
            | BuiltinFn::Tan(e) => find_expr_array_view(e),
            _ => None,
        },
        Expr::Op1(_, inner, _) => find_expr_array_view(inner),
        Expr::Op2(_, lhs, rhs, _) => {
            find_expr_array_view(lhs).or_else(|| find_expr_array_view(rhs))
        }
        Expr::If(_, t, f, _) => find_expr_array_view(t).or_else(|| find_expr_array_view(f)),
        _ => None,
    }
}

/// Given a variable's linear element index and its dimensions, compute the
/// corresponding index into a temp array whose dimensions are a subset.
///
/// For example, variable dims = [DimA(3), DimB(2)] and temp dims = [DimA(3)]:
///   var_idx 0 (A1,B1) -> temp_idx 0 (A1)
///   var_idx 1 (A1,B2) -> temp_idx 0 (A1)
///   var_idx 2 (A2,B1) -> temp_idx 1 (A2)
///   etc.
///
/// Matching is done by dimension name. Dimensions in the temp that are not
/// in the variable are iterated at position 0 (should not occur in practice).
fn project_var_index_to_temp(var_idx: usize, var_view: &ArrayView, temp_view: &ArrayView) -> usize {
    // Decompose var_idx into per-dimension coordinates (row-major)
    let mut remaining = var_idx;
    let var_ndims = var_view.dims.len();
    let mut var_coords: Vec<usize> = vec![0; var_ndims];
    for d in (0..var_ndims).rev() {
        var_coords[d] = remaining % var_view.dims[d];
        remaining /= var_view.dims[d];
    }

    // Build temp coordinates by matching dimension names
    let temp_ndims = temp_view.dims.len();
    let mut temp_coords: Vec<usize> = vec![0; temp_ndims];
    for (td, temp_name) in temp_view.dim_names.iter().enumerate() {
        if temp_name.is_empty() {
            continue;
        }
        for (vd, var_name) in var_view.dim_names.iter().enumerate() {
            if var_name == temp_name {
                temp_coords[td] = var_coords[vd];
                break;
            }
        }
    }

    // Recompose into linear index (row-major)
    let mut temp_idx = 0;
    let mut stride = 1;
    for d in (0..temp_ndims).rev() {
        temp_idx += temp_coords[d] * stride;
        stride *= temp_view.dims[d];
    }
    temp_idx
}

/// Recursively check whether any subexpression is an array-producing builtin.
fn contains_array_producing_builtin(expr: &Expr) -> bool {
    if is_array_producing_builtin(expr) {
        return true;
    }
    match expr {
        Expr::Op2(_, lhs, rhs, _) => {
            contains_array_producing_builtin(lhs) || contains_array_producing_builtin(rhs)
        }
        Expr::Op1(_, inner, _) => contains_array_producing_builtin(inner),
        Expr::If(cond, t, f, _) => {
            contains_array_producing_builtin(cond)
                || contains_array_producing_builtin(t)
                || contains_array_producing_builtin(f)
        }
        Expr::App(builtin, _) => builtin_contains_array_producing(builtin),
        _ => false,
    }
}

fn builtin_contains_array_producing(builtin: &BuiltinFn) -> bool {
    match builtin {
        BuiltinFn::Abs(e)
        | BuiltinFn::Arccos(e)
        | BuiltinFn::Arcsin(e)
        | BuiltinFn::Arctan(e)
        | BuiltinFn::Cos(e)
        | BuiltinFn::Exp(e)
        | BuiltinFn::Int(e)
        | BuiltinFn::Ln(e)
        | BuiltinFn::Log10(e)
        | BuiltinFn::Sign(e)
        | BuiltinFn::Sin(e)
        | BuiltinFn::Sqrt(e)
        | BuiltinFn::Tan(e) => contains_array_producing_builtin(e),
        BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
            contains_array_producing_builtin(a)
                || b.as_ref()
                    .is_some_and(|b| contains_array_producing_builtin(b))
        }
        BuiltinFn::SafeDiv(a, b, c) => {
            contains_array_producing_builtin(a)
                || contains_array_producing_builtin(b)
                || c.as_ref()
                    .is_some_and(|c| contains_array_producing_builtin(c))
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) => {
            contains_array_producing_builtin(a)
                || contains_array_producing_builtin(b)
                || c.as_ref()
                    .is_some_and(|c| contains_array_producing_builtin(c))
        }
        BuiltinFn::Sshape(a, b, c) => {
            contains_array_producing_builtin(a)
                || contains_array_producing_builtin(b)
                || contains_array_producing_builtin(c)
        }
        BuiltinFn::Quantum(a, b) | BuiltinFn::Step(a, b) => {
            contains_array_producing_builtin(a) || contains_array_producing_builtin(b)
        }
        _ => false,
    }
}

/// Replace array-producing builtins in an expression tree with
/// TempArrayElement references. Each nested builtin's index is projected
/// from the variable's element index using that builtin's own ArrayView,
/// handling the case where nested builtins operate on different dimensions.
/// On the first call (element 0), collects the hoisted AssignTemp expressions.
/// On subsequent calls, only performs the replacement using the same temp IDs.
fn replace_nested_builtins_for_element(
    expr: Expr,
    var_idx: usize,
    var_view: &ArrayView,
    temp_id: &mut u32,
    hoisted: &mut Vec<Expr>,
    collect_hoisted: bool,
) -> Expr {
    if is_array_producing_builtin(&expr) {
        let id = *temp_id;
        *temp_id += 1;
        let loc = expr.get_loc();
        let builtin_view = find_expr_array_view(&expr).unwrap_or_else(|| var_view.clone());
        let element_idx = project_var_index_to_temp(var_idx, var_view, &builtin_view);
        if collect_hoisted {
            hoisted.push(Expr::AssignTemp(id, Box::new(expr), builtin_view.clone()));
        }
        return Expr::TempArrayElement(id, builtin_view, element_idx, loc);
    }
    match expr {
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(replace_nested_builtins_for_element(
                *lhs,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            Box::new(replace_nested_builtins_for_element(
                *rhs,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            loc,
        ),
        Expr::Op1(op, inner, loc) => Expr::Op1(
            op,
            Box::new(replace_nested_builtins_for_element(
                *inner,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            loc,
        ),
        Expr::If(cond, t, f, loc) => Expr::If(
            Box::new(replace_nested_builtins_for_element(
                *cond,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            Box::new(replace_nested_builtins_for_element(
                *t,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            Box::new(replace_nested_builtins_for_element(
                *f,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
            )),
            loc,
        ),
        // Descend into builtin arguments so that patterns like ABS(VECTOR_ELM_MAP(...))
        // are correctly hoisted -- the array-producing builtin inside gets replaced with
        // a TempArrayElement reference, and the outer non-array-producing builtin is
        // preserved with its sub-expressions updated.
        Expr::App(builtin, loc) => Expr::App(
            builtin.map(|sub_expr| {
                replace_nested_builtins_for_element(
                    sub_expr,
                    var_idx,
                    var_view,
                    temp_id,
                    hoisted,
                    collect_hoisted,
                )
            }),
            loc,
        ),
        other => other,
    }
}

/// Find the next available temp ID by scanning existing expressions for
/// AssignTemp nodes. Uses the existing extract_temp_sizes infrastructure
/// which already walks the full expression tree.
fn next_available_temp_id(exprs: &[Expr]) -> u32 {
    let mut temp_sizes_map = HashMap::new();
    for expr in exprs {
        extract_temp_sizes(expr, &mut temp_sizes_map);
    }
    temp_sizes_map.keys().max().map(|m| m + 1).unwrap_or(0)
}

/// Construct a contiguous ArrayView from A2A dimensions.
fn array_view_from_dims(dims: &[Dimension]) -> ArrayView {
    let dim_sizes: Vec<usize> = dims.iter().map(|d| d.len()).collect();
    let dim_names: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
    ArrayView::contiguous_with_names(dim_sizes, dim_names)
}

/// Handle the Arrayed expansion, detecting array-producing builtins in
/// per-element expressions and hoisting them into AssignTemp pre-computations.
///
/// When a per-element expression is (or contains) an array-producing builtin
/// like VectorElmMap, VectorSortOrder, or AllocateAvailable, the builtin must
/// be evaluated once for the whole array and stored in temp. Each element then
/// reads its result via TempArrayElement.
fn expand_arrayed_with_hoisting(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    off: usize,
) -> Result<Vec<Expr>> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());

    // Scan ALL subscript combinations to find any equation that needs hoisting.
    // The first element alone may be a constant override while later elements
    // use a default (or explicit equation) containing array-producing builtins.
    let mut hoisting_ast: Option<&crate::ast::Expr2> = None;
    for subscripts in SubscriptIterator::new(dims) {
        let key = CanonicalElementName::from_raw(&subscripts.join(","));
        let ast = elements.get(&key).or(if apply_default_for_missing {
            default_ast
        } else {
            None
        });
        if let Some(ast) = ast {
            let probe_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            let mut probe_exprs = probe_ctx.lower(ast)?;
            let probe_main = probe_exprs.pop().unwrap();
            if is_array_producing_builtin(&probe_main)
                || contains_array_producing_builtin(&probe_main)
            {
                hoisting_ast = Some(ast);
                break;
            }
        }
    }

    if let Some(hoisting_ast) = hoisting_ast {
        expand_arrayed_hoisted(
            ctx,
            dims,
            elements,
            default_ast,
            apply_default_for_missing,
            off,
            &active_dims,
            hoisting_ast,
        )
    } else {
        // No array-producing builtins: standard per-element expansion
        let exprs: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
            .enumerate()
            .map(|(i, subscripts)| {
                let subscript_str = subscripts.join(",");
                let canonical_key = CanonicalElementName::from_raw(&subscript_str);
                let ast = match elements.get(&canonical_key) {
                    Some(ast) => ast,
                    None => {
                        if apply_default_for_missing && let Some(default_ast) = default_ast {
                            let ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                            return ctx.lower(default_ast).map(|mut exprs| {
                                let main_expr = exprs.pop().unwrap();
                                exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                                exprs
                            });
                        }
                        return Ok(vec![Expr::AssignCurr(
                            off + i,
                            Box::new(Expr::Const(0.0, Loc::default())),
                        )]);
                    }
                };
                let ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                ctx.lower(ast).map(|mut exprs| {
                    let main_expr = exprs.pop().unwrap();
                    exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                    exprs
                })
            })
            .collect();
        Ok(exprs?.into_iter().flatten().collect())
    }
}

/// Handle the A2A expansion for a single lowered expression, detecting
/// array-producing builtins and hoisting them into AssignTemp pre-computations.
///
/// Returns the complete list of expressions (pre-expressions + AssignTemp +
/// per-element AssignCurr nodes).
///
fn expand_a2a_with_hoisting(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    off: usize,
) -> Result<Vec<Expr>> {
    // Lower once using element 0's subscripts to detect the expression shape.
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());
    let first_subscripts: Vec<String> = SubscriptIterator::new(dims).next().unwrap_or_default();
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let mut first_exprs = first_ctx.lower(ast)?;
    let main_expr = first_exprs.pop().unwrap();

    if is_array_producing_builtin(&main_expr) || contains_array_producing_builtin(&main_expr) {
        // Re-lower with lower_preserving_dimensions so that
        // IndexExpr3::Dimension references survive Pass 1 and reach
        // normalize_subscripts3 as ActiveDimRef.  Inside array-producing
        // builtins (lowered with preserve_wildcards_for_iteration)
        // ActiveDimRef is kept as Wildcard, preserving full array views.
        // Without this, Pass 1 resolves Dimension to a constant index
        // based on the first element's active subscripts, collapsing
        // array arguments to scalars.
        let mut first_exprs = first_ctx.lower_preserving_dimensions(ast)?;
        let main_expr = first_exprs.pop().unwrap();
        return expand_a2a_hoisted(ctx, dims, ast, off, &active_dims, first_exprs, main_expr);
    }

    // Not an array-producing builtin: fall back to the standard per-element loop.
    // We already lowered element 0, so start from there.
    first_exprs.push(Expr::AssignCurr(off, Box::new(main_expr)));
    let rest: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
        .enumerate()
        .skip(1)
        .map(|(i, subscripts)| {
            let ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            ctx.lower(ast).map(|mut exprs| {
                let main_expr = exprs.pop().unwrap();
                exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                exprs
            })
        })
        .collect();
    let mut all_exprs = first_exprs;
    all_exprs.extend(rest?.into_iter().flatten());
    Ok(all_exprs)
}

/// Inner function for `expand_a2a_with_hoisting` when array-producing builtins
/// are detected. Handles both top-level and nested array-producing builtins.
fn expand_a2a_hoisted(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    off: usize,
    active_dims: &Arc<[Dimension]>,
    first_exprs: Vec<Expr>,
    main_expr: Expr,
) -> Result<Vec<Expr>> {
    if is_array_producing_builtin(&main_expr) {
        let temp_id = next_available_temp_id(&first_exprs);
        let var_view = array_view_from_dims(dims);
        let builtin_view = find_expr_array_view(&main_expr).unwrap_or_else(|| var_view.clone());
        let total_elements: usize = dims.iter().map(|d| d.len()).product();
        let loc = main_expr.get_loc();

        let mut result = first_exprs;
        result.push(Expr::AssignTemp(
            temp_id,
            Box::new(main_expr),
            builtin_view.clone(),
        ));
        for i in 0..total_elements {
            let temp_idx = project_var_index_to_temp(i, &var_view, &builtin_view);
            result.push(Expr::AssignCurr(
                off + i,
                Box::new(Expr::TempArrayElement(
                    temp_id,
                    builtin_view.clone(),
                    temp_idx,
                    loc,
                )),
            ));
        }
        Ok(result)
    } else if contains_array_producing_builtin(&main_expr) {
        // The top-level expression is not an array-producing builtin, but it
        // contains one nested inside (e.g. `10 + VECTOR ELM MAP(...)`). Hoist
        // the nested builtin(s) into AssignTemp pre-computations using element
        // 0's lowering (which has correct array views from
        // `with_preserved_wildcards`). Then for each element, re-lower and
        // replace the nested builtins with TempArrayElement reads from the
        // already-computed temps.
        let base_temp_id = next_available_temp_id(&first_exprs);
        let var_view = array_view_from_dims(dims);

        let mut hoisted = Vec::new();
        let mut temp_id = base_temp_id;
        let rewritten = replace_nested_builtins_for_element(
            main_expr,
            0,
            &var_view,
            &mut temp_id,
            &mut hoisted,
            true,
        );

        let mut result = first_exprs;
        result.extend(hoisted);
        result.push(Expr::AssignCurr(off, Box::new(rewritten)));

        // Remaining elements: re-lower, then replace nested builtins with
        // TempArrayElement reads at the correct element index.
        for (i, subscripts) in SubscriptIterator::new(dims).enumerate().skip(1) {
            let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
            let elem_main = elem_exprs.pop().unwrap();

            let mut tid = base_temp_id;
            let mut unused = Vec::new();
            let elem_rewritten = replace_nested_builtins_for_element(
                elem_main,
                i,
                &var_view,
                &mut tid,
                &mut unused,
                false,
            );
            result.push(Expr::AssignCurr(off + i, Box::new(elem_rewritten)));
        }
        Ok(result)
    } else {
        unreachable!("expand_a2a_hoisted called without array-producing builtin")
    }
}

/// Handle Arrayed equations where the hoisting equation coexists with
/// per-element overrides (EXCEPT semantics). Elements whose resolved equation
/// contains array-producing builtins get TempArrayElement reads; all others
/// are lowered normally. Each element is individually probed to classify it.
#[allow(clippy::too_many_arguments)]
fn expand_arrayed_hoisted(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    off: usize,
    active_dims: &Arc<[Dimension]>,
    hoisting_ast: &crate::ast::Expr2,
) -> Result<Vec<Expr>> {
    let first_subscripts: Vec<String> = SubscriptIterator::new(dims).next().unwrap_or_default();
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let mut first_exprs = first_ctx.lower_preserving_dimensions(hoisting_ast)?;
    let main_expr = first_exprs.pop().unwrap();
    let var_view = array_view_from_dims(dims);

    if contains_array_producing_builtin(&main_expr) {
        let base_temp_id = next_available_temp_id(&first_exprs);

        // Build AssignTemp blocks from the hoisting expression.
        // replace_nested_builtins_for_element handles both top-level builtins
        // (the entire expression IS a builtin) and nested builtins (a builtin
        // wrapped in arithmetic), so we don't need separate branches.
        let mut hoisted = Vec::new();
        let mut temp_id = base_temp_id;
        let _ = replace_nested_builtins_for_element(
            main_expr,
            0,
            &var_view,
            &mut temp_id,
            &mut hoisted,
            true,
        );

        let mut result = first_exprs;
        result.extend(hoisted);

        // Track which ASTs have had their temps emitted, keyed by pointer.
        // All elements using the default equation share one set of temps;
        // each distinct override gets its own AssignTemp blocks.
        let mut ast_temp_bases: HashMap<*const crate::ast::Expr2, u32> = HashMap::new();
        ast_temp_bases.insert(hoisting_ast as *const _, base_temp_id);

        for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
            let key = CanonicalElementName::from_raw(&subscripts.join(","));
            let elem_ast = elements.get(&key).or(if apply_default_for_missing {
                default_ast
            } else {
                None
            });

            let uses_hoisted = if let Some(ast) = elem_ast {
                let probe_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut probe_exprs = probe_ctx.lower(ast)?;
                let probe_main = probe_exprs.pop().unwrap();
                contains_array_producing_builtin(&probe_main)
            } else {
                false
            };

            if uses_hoisted {
                let ast = elem_ast.unwrap();
                let ast_ptr = ast as *const crate::ast::Expr2;

                // If this AST hasn't been seen before (different override),
                // emit its own AssignTemp blocks with fresh temp IDs.
                let elem_base_tid = if let Some(&tid) = ast_temp_bases.get(&ast_ptr) {
                    tid
                } else {
                    let disc_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                    let mut disc_exprs = disc_ctx.lower_preserving_dimensions(ast)?;
                    let disc_main = disc_exprs.pop().unwrap();
                    let new_base = temp_id;
                    result.extend(disc_exprs);
                    let mut new_hoisted = Vec::new();
                    let _ = replace_nested_builtins_for_element(
                        disc_main,
                        i,
                        &var_view,
                        &mut temp_id,
                        &mut new_hoisted,
                        true,
                    );
                    result.extend(new_hoisted);
                    ast_temp_bases.insert(ast_ptr, new_base);
                    new_base
                };

                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                let elem_main = elem_exprs.pop().unwrap();
                let mut tid = elem_base_tid;
                let mut unused = Vec::new();
                let elem_rewritten = replace_nested_builtins_for_element(
                    elem_main,
                    i,
                    &var_view,
                    &mut tid,
                    &mut unused,
                    false,
                );
                result.push(Expr::AssignCurr(off + i, Box::new(elem_rewritten)));
            } else if let Some(ast) = elem_ast {
                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower(ast)?;
                let elem_main = elem_exprs.pop().unwrap();
                result.extend(elem_exprs);
                result.push(Expr::AssignCurr(off + i, Box::new(elem_main)));
            } else {
                result.push(Expr::AssignCurr(
                    off + i,
                    Box::new(Expr::Const(0.0, Loc::default())),
                ));
            }
        }
        Ok(result)
    } else {
        unreachable!("expand_arrayed_hoisted called without array-producing builtin")
    }
}

/// Crate-visible wrapper for extract_temp_sizes.
pub(crate) fn extract_temp_sizes_pub(expr: &Expr, temp_sizes_map: &mut HashMap<u32, usize>) {
    extract_temp_sizes(expr, temp_sizes_map);
}

/// Recursively extract temporary array sizes from an expression.
/// Populates the temp_sizes_map with (temp_id, max_size) entries.
/// Since temp IDs restart at 0 for each lower() call, the same ID may be
/// reused across different expressions with different sizes. We track the
/// maximum size per ID to ensure the temp buffer is large enough for all uses.
fn extract_temp_sizes(expr: &Expr, temp_sizes_map: &mut HashMap<u32, usize>) {
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
fn extract_temp_sizes_from_builtin(builtin: &BuiltinFn, temp_sizes_map: &mut HashMap<u32, usize>) {
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
        BuiltinFn::Quantum(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            if let Some(c) = c {
                extract_temp_sizes(c, temp_sizes_map);
            }
        }
        BuiltinFn::Sshape(a, b, c) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            extract_temp_sizes(c, temp_sizes_map);
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
        BuiltinFn::VectorSelect(a, b, c, d, e) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            extract_temp_sizes(c, temp_sizes_map);
            extract_temp_sizes(d, temp_sizes_map);
            extract_temp_sizes(e, temp_sizes_map);
        }
        BuiltinFn::VectorElmMap(a, b) | BuiltinFn::VectorSortOrder(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
        }
        BuiltinFn::AllocateAvailable(a, b, c) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            extract_temp_sizes(c, temp_sizes_map);
        }
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime
        | BuiltinFn::IsModuleInput(_, _) => {}
        // Single expression builtins replacing stdlib modules
        BuiltinFn::Previous(expr) | BuiltinFn::Init(expr) => {
            extract_temp_sizes(expr, temp_sizes_map);
        }
    }
}

/// Per-variable initial expressions, kept alongside the flat runlist for
/// interpreter compatibility.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct VarInitial {
    pub(crate) ident: Ident<Canonical>,
    /// Sorted, deduplicated offsets extracted from AssignCurr nodes.
    pub(crate) offsets: Vec<usize>,
    pub(crate) ast: Vec<Expr>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct Module {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) inputs: HashSet<Ident<Canonical>>,
    pub(crate) n_slots: usize,         // number of f64s we need storage for
    pub(crate) n_temps: usize,         // number of temporary arrays
    pub(crate) temp_sizes: Vec<usize>, // size of each temporary array
    pub(crate) runlist_initials: Vec<Expr>,
    pub(crate) runlist_initials_by_var: Vec<VarInitial>,
    pub(crate) runlist_flows: Vec<Expr>,
    pub(crate) runlist_stocks: Vec<Expr>,
    pub(crate) offsets: VariableOffsetMap,
    pub(crate) runlist_order: Vec<Ident<Canonical>>,
    pub(crate) tables: HashMap<Ident<Canonical>, Vec<Table>>,
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

pub(crate) fn build_metadata<'p>(
    project: &'p Project,
    model_name: &Ident<Canonical>,
    is_root: bool,
    all_offsets: &mut HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'p>>>,
) {
    use std::sync::LazyLock;

    static IMPLICIT_TIME: LazyLock<Variable> = LazyLock::new(|| Variable::Var {
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
    });
    static IMPLICIT_DT: LazyLock<Variable> = LazyLock::new(|| Variable::Var {
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
    });
    static IMPLICIT_INITIAL_TIME: LazyLock<Variable> = LazyLock::new(|| Variable::Var {
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
    });
    static IMPLICIT_FINAL_TIME: LazyLock<Variable> = LazyLock::new(|| Variable::Var {
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
    });

    let model = &project.models[model_name];
    let var_names: Vec<&Ident<Canonical>> = {
        let mut var_names: Vec<_> = model.variables.keys().collect();
        var_names.sort_unstable();
        var_names
    };
    let var_count = var_names.len() + if is_root { IMPLICIT_VAR_COUNT } else { 0 };
    let mut offsets: HashMap<Ident<Canonical>, VariableMetadata<'p>> =
        HashMap::with_capacity(var_count);

    let mut i = 0;
    if is_root {
        offsets.insert(
            Ident::new("time"),
            VariableMetadata {
                offset: 0,
                size: 1,
                var: &IMPLICIT_TIME,
            },
        );
        offsets.insert(
            Ident::new("dt"),
            VariableMetadata {
                offset: 1,
                size: 1,
                var: &IMPLICIT_DT,
            },
        );
        offsets.insert(
            Ident::new("initial_time"),
            VariableMetadata {
                offset: 2,
                size: 1,
                var: &IMPLICIT_INITIAL_TIME,
            },
        );
        offsets.insert(
            Ident::new("final_time"),
            VariableMetadata {
                offset: 3,
                size: 1,
                var: &IMPLICIT_FINAL_TIME,
            },
        );
        i += IMPLICIT_VAR_COUNT;
    }

    for canonical_ident in var_names {
        let size = if let Variable::Module { model_name, .. } = &model.variables[canonical_ident] {
            if !all_offsets.contains_key(model_name) {
                build_metadata(project, model_name, false, all_offsets);
            }
            let sub_offsets = &all_offsets[model_name];
            sub_offsets.values().map(|metadata| metadata.size).sum()
        } else if let Some(Ast::ApplyToAll(dims, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _, _, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else {
            1
        };
        offsets.insert(
            canonical_ident.clone(),
            VariableMetadata {
                offset: i,
                size,
                var: &model.variables[canonical_ident],
            },
        );
        i += size;
    }

    all_offsets.insert(model_name.clone(), offsets);
}

fn calc_n_slots(
    all_metadata: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'_>>>,
    model_name: &Ident<Canonical>,
) -> usize {
    let metadata = &all_metadata[model_name];

    metadata.values().map(|v| v.size).sum()
}

impl Module {
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
        let mut metadata = HashMap::with_capacity(project.models.len());
        build_metadata(project, model_name, is_root, &mut metadata);

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
                &Context::new(
                    ContextCore {
                        dimensions: &converted_dims,
                        dimensions_ctx: &project.dimensions_ctx,
                        model_name,
                        metadata: &metadata,
                        module_models: &module_models,
                        inputs,
                    },
                    ident,
                    is_initial,
                ),
                &model.variables[ident],
            )
        };

        let initial_vars = instantiation
            .runlist_initials
            .iter()
            .map(|ident| build_var(ident, true))
            .collect::<Result<Vec<Var>>>()?;
        let flow_vars = instantiation
            .runlist_flows
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;
        let stock_vars = instantiation
            .runlist_stocks
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;

        let mut runlist_order = Vec::with_capacity(flow_vars.len() + stock_vars.len());
        runlist_order.extend(flow_vars.iter().map(|v| v.ident.clone()));
        runlist_order.extend(stock_vars.iter().map(|v| v.ident.clone()));

        // Build per-variable initials before flattening
        let runlist_initials_by_var: Vec<VarInitial> = initial_vars
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
        let runlist_initials: Vec<Expr> = initial_vars.into_iter().flat_map(|v| v.ast).collect();
        let runlist_flows: Vec<Expr> = flow_vars.into_iter().flat_map(|v| v.ast).collect();
        let runlist_stocks: Vec<Expr> = stock_vars.into_iter().flat_map(|v| v.ast).collect();

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

        let tables: Result<HashMap<Ident<Canonical>, Vec<Table>>> = var_names
            .iter()
            .map(|id| {
                let canonical_id = Ident::new(id);
                (id, &model.variables[&canonical_id])
            })
            .filter(|(_, v)| !v.tables().is_empty())
            .map(|(id, v)| {
                let tables_result: Result<Vec<Table>> =
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

    pub fn compile(&self) -> Result<CompiledModule> {
        Compiler::new(self).compile()
    }
}

#[cfg(test)]
impl Module {
    /// Get flow expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions that target offsets within this variable's range.
    pub fn get_flow_exprs(&self, var_name: &str) -> Vec<&Expr> {
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
    pub fn get_initial_exprs(&self, var_name: &str) -> Vec<&Expr> {
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
