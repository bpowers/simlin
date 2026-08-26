// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod array_operand;
mod codegen;
pub mod context;
pub mod dimensions;
pub mod expr;
pub(crate) mod fold;
pub(crate) mod fragment;
pub(crate) mod invariance;
pub mod pretty;
pub mod subscript;
pub(crate) mod symbolic;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::ast::{ArrayView, Ast, BinaryOp, Loc, TempAllocator};
use crate::builtins::{ArgKind, ResultKind};
#[cfg(test)]
use crate::common::ErrorCode;
use crate::common::{Canonical, CanonicalElementName, Ident, Result};
#[cfg(test)]
use crate::dimensions::DimensionsContext;
use crate::dimensions::{Dimension, SubscriptIterator};
use crate::sim_err;
use crate::variable::{VarKind, Variable};

// Re-exports for crate-internal API
pub(crate) use self::codegen::ModuleCtx;
pub(crate) use self::context::Context;
pub(crate) use self::expr::{BuiltinFn, Expr, SubscriptIndex, Table, VarRef};

/// The total slot count of the variable a reference addresses **in whole**,
/// keyed by that reference.
///
/// This is all codegen needs from the symbol table now that references carry
/// names: the *extent* of a VECTOR ELM MAP's source variable
/// (`codegen::full_source_len`). Every other lookup that used to run backwards
/// through a `name -> (offset, size)` map -- the identity of a lookup table,
/// the base slot of a module instance -- reads the name off the reference
/// directly. Offsets are assigned once, at assembly, and never appear here.
///
/// Keyed by the whole `VarRef` rather than by name, because a reference does
/// not always name the variable whose extent it asks about. A CROSS-MODULE
/// reference `m·x` lowers to `VarRef { name: m, element_offset: x's slot
/// inside the instance }`, and `m`'s own slot count -- the whole sub-model
/// block -- is the extent of nothing a reference can name. Each instance
/// therefore contributes one entry per sub-model variable, at that variable's
/// slot; a reference sitting at a sub-model variable's base reports THAT
/// variable's extent, and one landing mid-array is simply absent. Absence is
/// the same answer an in-model mid-array reference gets, so the caller's
/// fallback ("all the view can honestly report") is unchanged.
///
/// Built once per fragment by [`fragment::reference_extents`], from the
/// fragment's dependency shapes. That is the only place the rule is DERIVED --
/// every production site reads the table `FragmentInput` carries, so lowering
/// and emission cannot disagree about what a reference addresses. (`codegen`'s
/// unit tests hand-build a table instead, in order to state the lookup's
/// contract against inputs of their own choosing; they exercise the reader,
/// not the rule.)
pub(crate) type VarSizes = HashMap<VarRef, usize>;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub struct Var {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) ast: Vec<Expr>,
}

/// A `Context` over hand-built dependency shapes for the unit tests of
/// `Var::new`'s mechanics below. Production contexts are built by
/// `fragment::lower_fragment` from a `FragmentInput`.
#[cfg(test)]
fn test_context<'a>(
    dimensions: &'a [Dimension],
    dimensions_ctx: &'a DimensionsContext,
    deps: &'a crate::common::IdentMap<Ident<Canonical>, fragment::DepShape>,
    var_sizes: &'a VarSizes,
    inputs: &'a BTreeSet<Ident<Canonical>>,
) -> Context<'a> {
    Context::new(
        context::ContextCore {
            dimensions,
            dimensions_ctx,
            deps,
            var_sizes,
            inputs,
        },
        false,
    )
}

/// Scalar dependency shapes for `names`.
#[cfg(test)]
fn scalar_deps(names: &[&str]) -> crate::common::IdentMap<Ident<Canonical>, fragment::DepShape> {
    names
        .iter()
        .map(|name| (Ident::new(name), fragment::DepShape::var(vec![])))
        .collect()
}

#[test]
fn test_fold_flows() {
    let inputs = BTreeSet::new();
    let deps = scalar_deps(&["a", "b", "c", "d"]);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    assert_eq!(Ok(None), ctx.fold_flows(&[]));
    assert_eq!(
        Ok(Some(Expr::Var(
            VarRef::base(Ident::new("a")),
            Loc::default()
        ))),
        ctx.fold_flows(&[Ident::new("a")])
    );
    assert_eq!(
        Ok(Some(Expr::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(Expr::Var(VarRef::base(Ident::new("a")), Loc::default())),
            Box::new(Expr::Var(VarRef::base(Ident::new("d")), Loc::default())),
            Loc::default(),
        ))),
        ctx.fold_flows(&[Ident::new("a"), Ident::new("d")])
    );

    // Test that fold_flows returns an error for non-existent flows
    let result = ctx.fold_flows(&[Ident::new("nonexistent")]);
    assert!(result.is_err(), "Expected error for non-existent flow");
}

/// Var::new for a module whose input source variable is missing from the
/// dependency shapes must return an error, not panic.  This guards against the
/// case where a module's input source is deleted but the module itself
/// still exists with its original references.
#[test]
fn test_module_var_new_missing_input_source_returns_error() {
    use crate::variable::ModuleInput;

    let inputs = BTreeSet::new();
    let module_ident = Ident::new("my_module");
    let model_name_ident: Ident<Canonical> = Ident::new("sub_model");

    // The module variable itself (an empty sub-model shape: the source is
    // what is missing, not the sub-model)
    let module_var = Variable::module_instance(
        module_ident.clone(),
        model_name_ident,
        vec![ModuleInput {
            src: Ident::new("missing_source"),
            dst: Ident::new("available"),
        }],
    );

    // deps only contain "my_module" -- NOT "missing_source"
    let mut deps: crate::common::IdentMap<Ident<Canonical>, fragment::DepShape> =
        Default::default();
    deps.insert(
        module_ident.clone(),
        fragment::DepShape::module(Arc::new(fragment::ModelShape::default())),
    );

    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    let result = Var::new(&ctx, &module_var);
    assert!(
        result.is_err(),
        "Var::new should return Err when a module input source is missing, not panic"
    );
}

#[test]
fn test_build_stock_update_expr_inflows_only() {
    let inputs = BTreeSet::new();
    let stock_var = Variable {
        ident: Ident::new("stock"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Stock {
            init_ast: None,
            inflows: vec![Ident::new("inflow")],
            outflows: vec![],
            non_negative: false,
        },
    };
    let deps = scalar_deps(&["stock", "inflow"]);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    let result = ctx
        .build_stock_update_expr(&VarRef::base(Ident::new("stock")), &stock_var)
        .unwrap();

    // stock + (inflow - 0.0) * dt
    // outflows should be Const(0.0) since there are none
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert_eq!(
            stock_box.as_ref(),
            &Expr::Var(VarRef::base(Ident::new("stock")), Loc::default())
        );
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, dt_box, _) = dt_update_box.as_ref() {
            assert!(matches!(dt_box.as_ref(), Expr::Dt(_)));
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_box, out_box, _) = sub_box.as_ref() {
                assert_eq!(
                    in_box.as_ref(),
                    &Expr::Var(VarRef::base(Ident::new("inflow")), Loc::default())
                );
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
    let inputs = BTreeSet::new();
    let stock_var = Variable {
        ident: Ident::new("stock"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Stock {
            init_ast: None,
            inflows: vec![],
            outflows: vec![Ident::new("outflow")],
            non_negative: false,
        },
    };
    let deps = scalar_deps(&["stock", "outflow"]);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    let result = ctx
        .build_stock_update_expr(&VarRef::base(Ident::new("stock")), &stock_var)
        .unwrap();

    // stock + (0.0 - outflow) * dt
    // inflows should be Const(0.0) since there are none
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert_eq!(
            stock_box.as_ref(),
            &Expr::Var(VarRef::base(Ident::new("stock")), Loc::default())
        );
        if let Expr::Op2(crate::ast::BinaryOp::Mul, sub_box, _, _) = dt_update_box.as_ref() {
            if let Expr::Op2(crate::ast::BinaryOp::Sub, in_box, out_box, _) = sub_box.as_ref() {
                assert!(
                    matches!(in_box.as_ref(), Expr::Const(v, _) if *v == 0.0),
                    "inflows should be Const(0.0) when empty"
                );
                assert_eq!(
                    out_box.as_ref(),
                    &Expr::Var(VarRef::base(Ident::new("outflow")), Loc::default())
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
fn test_build_stock_update_expr_no_flows() {
    let inputs = BTreeSet::new();
    let stock_var = Variable {
        ident: Ident::new("stock"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Stock {
            init_ast: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
        },
    };
    let deps = scalar_deps(&["stock"]);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    let result = ctx
        .build_stock_update_expr(&VarRef::base(Ident::new("stock")), &stock_var)
        .unwrap();

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
    let inputs = BTreeSet::new();
    let stock_var = Variable {
        ident: Ident::new("stock"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Stock {
            init_ast: None,
            inflows: vec![Ident::new("in1"), Ident::new("in2")],
            outflows: vec![Ident::new("out1"), Ident::new("out2")],
            non_negative: false,
        },
    };
    let deps = scalar_deps(&["stock", "in1", "in2", "out1", "out2"]);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    let result = ctx
        .build_stock_update_expr(&VarRef::base(Ident::new("stock")), &stock_var)
        .unwrap();

    // stock + ((in1 + in2) - (out1 + out2)) * dt
    if let Expr::Op2(crate::ast::BinaryOp::Add, stock_box, dt_update_box, _) = &result {
        assert_eq!(
            stock_box.as_ref(),
            &Expr::Var(VarRef::base(Ident::new("stock")), Loc::default())
        );
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
    let tp = TestProject::new("sparse_test")
        .named_dimension("dim", &["a", "b", "c"])
        .array_with_ranges(
            "x[dim]",
            vec![("a", "1"), ("b", "2")], // 'c' intentionally missing
        )
        .aux("y", "1", None);
    let _diagnostics = tp.error_diagnostics();
    let _compiled = tp.compile_incremental();
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
        crate::ast::Expr2::Const(
            "1".to_string(),
            crate::ast::Literal::new(1.0),
            Loc::default(),
        ),
    );
    elements.insert(
        CanonicalElementName::from_raw("b"),
        crate::ast::Expr2::Const(
            "2".to_string(),
            crate::ast::Literal::new(2.0),
            Loc::default(),
        ),
    );

    let var = Variable {
        ident: Ident::new("x"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Aux {
            ast: Some(Ast::Arrayed(
                dims.clone(),
                elements,
                Some(crate::ast::Expr2::Const(
                    "7".to_string(),
                    crate::ast::Literal::new(7.0),
                    Loc::default(),
                )),
                true,
            )),
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
        },
    };

    let mut deps: crate::common::IdentMap<Ident<Canonical>, fragment::DepShape> =
        Default::default();
    deps.insert(Ident::new("x"), fragment::DepShape::var(dims.clone()));

    let inputs = BTreeSet::new();
    let dims_ctx = DimensionsContext::from(std::slice::from_ref(&datamodel_dim));
    let var_sizes = fragment::reference_extents(&deps);
    let ctx = test_context(&dims, &dims_ctx, &deps, &var_sizes, &inputs);

    let lowered = Var::new(&ctx, &var).expect("arrayed lowering should succeed");

    let mut assigned: HashMap<usize, f64> = HashMap::new();
    for expr in lowered.ast {
        if let Expr::AssignCurr(dst, rhs) = expr {
            if let Expr::Const(value, _) = *rhs {
                assigned.insert(dst.element_offset, value);
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

/// The stock-update shape guard, in both directions.
///
/// Codegen emits every next-value assignment as the fused `BinOpAssignNext`,
/// so a stock update whose lowered form does not end in an `Op2` must be
/// REJECTED at lowering (a structured, per-variable error) rather than dropped
/// at emission (a stock that silently never integrates). This pins that
/// contract directly, since the shape it guards is unreachable from today's
/// `build_stock_update_expr` and so cannot be provoked through a model.
#[test]
fn stock_update_shape_guard_rejects_only_non_binary_updates() {
    let level = VarRef::base(Ident::new("level"));
    let var = Expr::Var(level.clone(), Loc::default());
    let c = || Expr::Const(1.0, Loc::default());
    let op2 = |op| Expr::Op2(op, Box::new(var.clone()), Box::new(c()), Loc::default());

    // The real shape: `curr + net * dt`.
    check_stock_updates_are_emittable(
        &[Expr::AssignNext(
            level.clone(),
            Box::new(op2(BinaryOp::Add)),
        )],
        "level",
    )
    .expect("the `Op2(Add, ..)` `build_stock_update_expr` produces must be accepted");

    // A non-`Op2` update -- the shape a `MAX(0, ..)` `non_negative` clamp
    // (GH #545) would produce.
    let err = check_stock_updates_are_emittable(
        &[Expr::AssignNext(
            level.clone(),
            Box::new(Expr::App(
                crate::builtins::BuiltinFn::Max(Box::new(c()), Some(Box::new(var.clone()))),
                Loc::default(),
            )),
        )],
        "level",
    )
    .expect_err("a builtin-wrapped stock update must be rejected, not silently dropped");
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    assert!(
        err.details.as_deref().unwrap_or("").contains("level"),
        "the error must name the stock so the diagnostic is attributable, got {err:?}"
    );

    // `Neq` is the one binary operator codegen does not leave as a trailing
    // `Op2` (it emits `Op2 Eq` then `Not`), so it is rejected too.
    assert!(
        check_stock_updates_are_emittable(
            &[Expr::AssignNext(
                level.clone(),
                Box::new(op2(BinaryOp::Neq))
            )],
            "level"
        )
        .is_err(),
        "a `Neq` update does not end in an Op2 opcode and must be rejected"
    );

    // Nothing else in an expression list is inspected.
    check_stock_updates_are_emittable(&[Expr::AssignCurr(level, Box::new(c()))], "aux")
        .expect("a non-stock assignment is not a stock update");
}

impl Var {
    pub(crate) fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        // if this variable is overriden by a module input, our expression is easy
        let ast: Vec<Expr> = if let Some((input_idx, _ident)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| n.as_str() == var.ident())
        {
            vec![Expr::AssignCurr(
                ctx.get_ref(&Ident::new(var.ident()))?,
                Box::new(Expr::ModuleInput(input_idx, Loc::default())),
            )]
        } else {
            match &var.kind {
                VarKind::Module { model_name, inputs } => {
                    let mut inputs = inputs.clone();
                    inputs.sort_unstable_by(|a, b| a.dst.partial_cmp(&b.dst).unwrap());
                    // Create input set for module lookup key
                    let input_set: BTreeSet<Ident<Canonical>> =
                        inputs.iter().map(|mi| mi.dst.clone()).collect();
                    let inputs: Vec<Expr> = inputs
                        .into_iter()
                        .map(|mi| Ok(Expr::Var(ctx.get_ref(&mi.src)?, Loc::default())))
                        .collect::<Result<Vec<_>>>()?;
                    vec![Expr::EvalModule(
                        var.ident.clone(),
                        model_name.clone(),
                        input_set,
                        inputs,
                    )]
                }
                VarKind::Stock { init_ast: ast, .. } => {
                    let base = ctx.get_base_ref(&Ident::new(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                let mut exprs = ctx.lower(ast)?;
                                let main_expr = exprs.pop().unwrap();
                                let main_expr = hoist_nested_array_builtins_in_scalar(
                                    main_expr, &mut exprs, &ctx.temps,
                                );
                                exprs.push(Expr::AssignCurr(base, Box::new(main_expr)));
                                exprs
                            }
                            Ast::ApplyToAll(dims, ast) => {
                                expand_a2a_with_hoisting(ctx, dims, ast, &base)?
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
                                &base,
                            )?,
                        }
                    } else {
                        let Some(ast) = ast.as_ref() else {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        };
                        match ast {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                base.clone(),
                                Box::new(ctx.build_stock_update_expr(&base, var)?),
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
                                            &ctx.get_ref(&Ident::new(var.ident()))?,
                                            var,
                                        )?;
                                        Ok(Expr::AssignNext(
                                            base.offset_by(i),
                                            Box::new(update_expr),
                                        ))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                VarKind::Aux {
                    tables,
                    is_table_only,
                    ..
                } => {
                    // A standalone lookup-only table is a static table consulted
                    // by callers via `LOOKUP(self, x)`, not a value-bearing
                    // variable: it is excluded from every runlist and produces no
                    // expression of its own. Return an empty fragment -- this
                    // keeps the diagnostic pass (which compiles every variable)
                    // from flagging its empty equation, and the fragment is never
                    // assembled into a runlist anyway (issue #606).
                    if *is_table_only {
                        return Ok(Var {
                            ident: Ident::new(var.ident()),
                            ast: vec![],
                        });
                    }
                    let base = ctx.get_base_ref(&Ident::new(var.ident()))?;
                    let ast = if ctx.is_initial {
                        var.init_ast()
                    } else {
                        var.ast()
                    };
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().to_string());
                    }
                    let exprs = match ast.as_ref().unwrap() {
                        Ast::Scalar(ast) => {
                            let mut exprs = ctx.lower(ast)?;
                            let main_expr = exprs.pop().unwrap();
                            let main_expr = hoist_nested_array_builtins_in_scalar(
                                main_expr, &mut exprs, &ctx.temps,
                            );
                            exprs.push(Expr::AssignCurr(base.clone(), Box::new(main_expr)));
                            exprs
                        }
                        Ast::ApplyToAll(dims, ast) => {
                            expand_a2a_with_hoisting(ctx, dims, ast, &base)?
                        }
                        Ast::Arrayed(dims, elements, default_ast, apply_default_for_missing) => {
                            expand_arrayed_with_hoisting(
                                ctx,
                                dims,
                                elements,
                                default_ast.as_ref(),
                                *apply_default_for_missing,
                                &base,
                            )?
                        }
                    };
                    // WITH LOOKUP (`var = WITH LOOKUP(input, table)`): a
                    // tables-bearing variable WITH a real input equation lowers
                    // to `LOOKUP(self, input)` -- per element for arrayed
                    // shapes (GH #909). (A standalone lookup-only table has no
                    // input and is handled above; ordinary auxes have no
                    // tables.)
                    apply_implicit_with_lookup(exprs, &base, tables)
                }
            }
        };
        // Fold constant subtrees once at compile time so the per-timestep
        // programs never re-evaluate `literal op literal` (including the
        // `0 - x` form every negative literal lowers to). Runs here -- the
        // single chokepoint every per-variable fragment lowering funnels
        // through -- so both backends (VM and wasmgen) see the folded form.
        let ast: Vec<Expr> = ast.into_iter().map(fold::fold_constants).collect();
        // Discharge codegen's "an array operand is a view over storage"
        // contract (GH #995). Runs at the same chokepoint and after folding,
        // so it sees the final tree both backends consume and never
        // materializes something folding would have collapsed.
        let ast = array_operand::materialize_computed_array_operands(ast, &ctx.temps);
        check_stock_updates_are_emittable(&ast, var.ident())?;
        // The allocator's count is the fragment's temp count: every id it
        // issued and kept is written by the lowered expressions, and nothing
        // writes an id it did not issue. A discarded classification lowering
        // or a dropped pre-expression would break this, which is the point.
        debug_assert_eq!(
            defined_temp_count(&ast),
            ctx.temps.count(),
            "temp ids defined by '{}' must be exactly the ids its allocator kept",
            var.ident()
        );
        Ok(Var {
            ident: Ident::new(var.ident()),
            ast,
        })
    }
}

/// The number of distinct temp ids `exprs` write.
fn defined_temp_count(exprs: &[Expr]) -> u32 {
    let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
    for expr in exprs {
        extract_temp_sizes(expr, &mut temp_sizes_map);
    }
    temp_sizes_map.len() as u32
}

/// Reject a stock update codegen could not emit, with a structured
/// per-variable error rather than an unattributed batch failure.
///
/// `Expr::AssignNext` is the only thing that ever writes `next[]`, and it is
/// synthesized here from `Context::build_stock_update_expr`, never by a user
/// equation. Codegen has no un-fused next-assign opcode: it emits
/// `BinOpAssignNext`, which requires the update expression's *last* emitted
/// opcode to be an `Op2`
/// (`symbolic::SymbolicByteCodeBuilder::fuse_trailing_op2_into_assign_next`).
/// Today that always holds -- `build_stock_update_expr` returns
/// `Op2(Add, curr, net * dt)` and constant folding cannot collapse it, since
/// its left operand is an `Expr::Var` -- so this check never fires.
///
/// It exists because the property is a *shape* invariant that a future change
/// could quietly break: implementing `non_negative` (GH #545) by wrapping the
/// update in `MAX(0, ..)` would end the walk in an `Apply`, which codegen
/// cannot emit as a next-value assignment.
///
/// What the guard buys is ATTRIBUTION, not the difference between failing and
/// not failing. Without it the build still fails, but late and vaguely:
/// codegen's error is swallowed by `compile_phase_to_per_var_bytecodes`'s
/// loud-safe `None`, the stock's phase goes missing, and `assemble_module`'s
/// `missing_vars` check reports the whole batch as
/// `failed to compile fragments for variables: <names>` with no reason and no
/// per-variable diagnostic. Checked here, at the chokepoint every lowering
/// path funnels through, the same failure instead surfaces as a typed
/// `NotSimulatable` `Diagnostic` attributed to the stock (through
/// `compile_var_fragment`'s `accumulate_var_compile_error`), which is what
/// invariant 7 asks for. Note that the `details` string below does not reach
/// the user today -- see the comment on `accumulate_var_compile_error` for why
/// and what the real fix is.
///
/// `Neq` is excluded along with the non-`Op2` shapes: it is the one binary
/// operator codegen does not emit as a trailing `Op2` (it emits `Op2 Eq` then
/// `Not`).
fn check_stock_updates_are_emittable(ast: &[Expr], ident: &str) -> Result<()> {
    for expr in ast {
        let Expr::AssignNext(_, rhs) = expr else {
            continue;
        };
        let emittable = matches!(rhs.as_ref(), Expr::Op2(op, _, _, _) if *op != BinaryOp::Neq);
        if !emittable {
            return sim_err!(
                NotSimulatable,
                format!(
                    "the dt update for stock '{ident}' is not a binary operation, \
                     so it cannot be emitted as a next-value assignment"
                )
            );
        }
    }
    Ok(())
}

/// Implicit WITH LOOKUP application (GH #909): rewrite each per-element
/// assignment of a tables-bearing, value-bearing variable so the element's
/// input equation is fed through the variable's graphical function, matching
/// Stella (`value = gf(input)`):
///
/// - scalar, or arrayed with a single VARIABLE-level gf: one shared table
///   (`tables.len() == 1`); every element looks up table index 0;
/// - non-A2A PER-ELEMENT gfs: `tables` holds one table per element at the
///   element's row-major declared-dimension index (`variable::build_tables` /
///   `reorder_arrayed_element_tables`), so element `i` looks up table `i`. An
///   element WITHOUT a gf carries an empty placeholder table and keeps its raw
///   input equation -- Stella has nothing to apply there (wrapping would turn
///   its value into NaN, the empty-table lookup result).
///
/// A table with NO points is deliberately treated as ABSENT for every shape,
/// scalars included: the variable evaluates its raw input equation rather
/// than NaN. (The pre-#909 scalar wrap keyed only on `tables` being
/// non-empty, so a degenerate zero-point gf produced NaN; the rule now
/// matches the arrayed empty-placeholder handling.)
///
/// A gf-BEARING element whose input equation is MISSING (an XMILE gf-only
/// `<element>`, GH #907, in a variable where other elements carry real
/// equations and there is no EXCEPT default) receives the arrayed
/// expansion's fabricated `Const(0.0)` input and therefore evaluates
/// `gf(0)`: the wrap applies uniformly to whatever input the expansion
/// produced. Both the old `0` and the new `gf(0)` are silent fabrications
/// (Stella rejects that element shape outright); the zero-fill itself, and
/// a diagnostic for this class, are the open remainder of GH #905.
///
/// Only the per-element `AssignCurr` nodes the expansion paths emit are
/// rewritten; hoisted pre-computations (`AssignTemp`) feed those assignments
/// and stay untouched. The table reference is
/// `Expr::Var(base.offset_by(table_idx))`: codegen's `extract_table_info` reads
/// the owning variable's name straight off the reference -- exactly the
/// resolution an explicit `LOOKUP(var[elem], x)` call site gets -- and emits the
/// same scalar `Lookup` opcode (`graphical_functions[base_gf + element_offset]`),
/// so both the VM and the wasm backend evaluate it with no new lowering.
fn apply_implicit_with_lookup(
    exprs: Vec<Expr>,
    base: &VarRef,
    tables: &[crate::variable::Table],
) -> Vec<Expr> {
    if tables.is_empty() {
        return exprs;
    }
    exprs
        .into_iter()
        .map(|expr| match expr {
            // The guard filters nothing in practice: every `AssignCurr` a
            // Var fragment emits targets this variable at or after its base
            // (pre-computations are `AssignTemp`). It exists purely as
            // defense so a hypothetical assignment to a *different* variable,
            // or one before the base, could never wrap against a nonsense
            // element index.
            Expr::AssignCurr(dst, value)
                if dst.name == base.name && dst.element_offset >= base.element_offset =>
            {
                let elem = dst.element_offset - base.element_offset;
                let table_idx = if tables.len() == 1 { 0 } else { elem };
                // `false` for an out-of-range index (defensive: `tables` is
                // either 1 or the element count) and for an empty placeholder.
                let has_table = tables
                    .get(table_idx)
                    .map(|t| !t.x.is_empty())
                    .unwrap_or(false);
                if has_table {
                    let loc = value.get_loc();
                    Expr::AssignCurr(
                        dst,
                        Box::new(Expr::App(
                            BuiltinFn::Lookup(
                                Box::new(Expr::Var(base.offset_by(table_idx), loc)),
                                value,
                                loc,
                            ),
                            loc,
                        )),
                    )
                } else {
                    Expr::AssignCurr(dst, value)
                }
            }
            other => other,
        })
        .collect()
}

/// For scalar equations, hoist nested array-producing builtins only where the
/// parent expects an array value (reducers/vector array args). Scalar-argument
/// positions are left unchanged so existing structured compile errors are
/// preserved instead of forcing scalar-element rewrites.
fn hoist_nested_array_builtins_in_scalar(
    main_expr: Expr,
    exprs: &mut Vec<Expr>,
    temps: &TempAllocator,
) -> Expr {
    if is_array_producing_builtin(&main_expr) || !contains_array_producing_builtin(&main_expr) {
        return main_expr;
    }

    let mut hoisted = Vec::new();
    let placeholder_view = ArrayView::contiguous(vec![1]);
    let rewritten = replace_nested_builtins_for_element(
        main_expr,
        0,
        &placeholder_view,
        temps,
        &mut hoisted,
        NestedBuiltinArgMode::ScalarContext,
    );
    exprs.extend(hoisted);
    rewritten
}

/// Check if an expression is an array-producing builtin that needs whole-array
/// evaluation rather than per-element scalar evaluation
/// (`ResultKind::Array`: a dedicated opcode writes the result into a temp).
fn is_array_producing_builtin(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::App(builtin, _) if matches!(builtin.result_kind(), ResultKind::Array { .. })
    )
}

/// Which snapshot buffer an array-valued `PREVIOUS`/`INIT` reads (GH #995).
///
/// Deliberately narrower than [`crate::bytecode::ViewStorage`]: a temp array has
/// no snapshot, so that pairing cannot be expressed here at all.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SnapshotRegion {
    Prev,
    Initial,
}

/// Classify a `PREVIOUS`/`INIT` call: `Some((argument, region))` when it is
/// ARRAY-valued, `None` when it is the ordinary scalar form (or not a
/// `PREVIOUS`/`INIT` at all).
///
/// This is the single definition of "this call takes the array route" for every
/// SCALAR position: `walk_expr` (inside a `BeginIter` body),
/// `collect_iter_source_views_impl` (which must pre-push exactly the views the
/// body reads), codegen's one-argument `Mean` arm, and [`array_operand`]'s
/// materializer, which must NOT move an array-valued `PREVIOUS` into a temp.
/// Those disagreeing is how a view gets pushed that nothing reads, or read that
/// nothing pushed. `walk_expr_as_view` deliberately does NOT ask this question
/// -- an explicit view operand takes ANY `PREVIOUS`/`INIT` that lowered to a
/// view, single-element included -- so it goes straight to
/// `Compiler::snapshot_static_view`'s `SnapshotPosition::ViewOperand` arm, whose
/// rustdoc carries the reason.
///
/// Array-valuedness is decided by the ARGUMENT's shape rather than by the call:
/// `PREVIOUS(vals[D])` inside a vector builtin is the whole array while
/// `PREVIOUS(matrix[E,1])` is one element, and only lowering knows which. An
/// argument that carries an array shape but did not lower to a view over storage
/// still classifies as array-valued -- `snapshot_static_view` then rejects it
/// loudly, which is the point: silently falling back to the scalar route would
/// read one element and broadcast it.
pub(super) fn snapshot_view_arg(builtin: &BuiltinFn) -> Option<(&Expr, SnapshotRegion)> {
    let (arg, region) = match builtin {
        BuiltinFn::Previous(arg, _) => (arg.as_ref(), SnapshotRegion::Prev),
        BuiltinFn::Init(arg) => (arg.as_ref(), SnapshotRegion::Initial),
        _ => return None,
    };
    let is_array = find_expr_array_view(arg).is_some_and(|view| !view.dims.is_empty());
    is_array.then_some((arg, region))
}

/// Extract the output ArrayView from an expression.  For array-producing builtins, the
/// output dimensions come from the builtin's "shaping" argument:
///   VectorElmMap(_, offset)    -> offset's view
///   VectorSortOrder(arr, _)    -> arr's view
///   AllocateAvailable(req,_,_) -> req's view
///
/// Everything else is elementwise, and an elementwise expression's shape is the
/// JOIN of its subexpressions' shapes -- the narrowest view they all broadcast
/// into ([`join_array_views`]), not whichever one is written first. Both consumers
/// need the join and for the same reason: an elementwise expression is evaluated
/// by codegen's `AssignTemp` -> `BeginIter` loop, which broadcasts each source
/// view onto the ITERATION by dimension id, so a source dimension the iteration
/// does not have reads NaN. Taking the first view instead made
/// `VECTOR SORT ORDER(small[d] + wide[e,d], 1)` iterate over `small`'s three
/// elements and return the sort order of three NaNs, while the commuted
/// `wide[e,d] + small[d]` -- the same array -- returned the right answer.
///
/// `None` means one of two things: no subexpression carries an array shape, or
/// two of them carry shapes neither of which contains the other. A view that
/// REPEATS a dimension name (`matrix[d,d]`) is NOT one of them -- as an
/// expression's sole shape it is returned like any other. That shape is refused
/// as a TEMP's shape, but the refusal lives at the one call site that can be
/// loud about it ([`array_operand::materialize_view_operand`]) rather than here;
/// see [`view_repeats_a_dimension`] for why the difference matters.
///
/// The four call sites do NOT all treat `None` the same way, and only one of
/// them is loud:
///
/// * [`array_operand::materialize_view_operand`] declines to materialize, which
///   leaves codegen to reject the operand with a diagnostic attributed to the
///   variable. This is the loud one.
/// * the three apply-to-all / arrayed hoisting sites SUBSTITUTE the variable's
///   own view (`unwrap_or_else(|| var_view.clone())`) with no diagnostic, and
///   the substituted view can be a DIFFERENT SIZE from the array the hoisted
///   builtin writes. That is a live hazard, not a latent one: while this
///   function refused a sole repeated-dimension view, `out[d] =
///   SUM(VECTOR SORT ORDER(matrix[d,d], 1))` sized the sort order's temp at
///   `out`'s three slots and the VM indexed past it -- a panic, which under
///   `panic = abort` takes the host process with it. Any future `None` this
///   function learns to return must be checked against these three sites, or
///   given to them as an `Err` instead.
/// * [`snapshot_view_arg`] reads only `is_empty()`, so a `None` classifies the
///   call as SCALAR and it compiles to `LoadPrev`/`LoadInitial`. Reaching it
///   needs a `PREVIOUS`/`INIT` whose argument survived `builtins_visitor`'s
///   helper rewriting as a multi-shape expression, which the array-shaped
///   passthrough predicate does not admit.
pub(super) fn find_expr_array_view(expr: &Expr) -> Option<ArrayView> {
    let mut views = Vec::new();
    collect_expr_array_views(expr, &mut views);
    join_array_views(views)
}

/// The narrowest of `views` that every one of them broadcasts into, or `None`
/// when there is no such view.
///
/// A single view is returned unchanged, dimensionless (a subscript collapsed to
/// one element) included, so this is a no-op wherever the shapes already agreed.
///
/// The widest view is CHOSEN rather than accumulated left to right: a fold would
/// call `[e], [d], [e,d]` incomparable on its second step even though the third
/// contains both. Two maximal views that disagree on AXIS ORDER (`[e,d]` and
/// `[d,e]` contain each other) are `None` rather than a coin flip -- the axis
/// order is the one `VECTOR SORT ORDER` sorts along and the layout every
/// consumer projects through, so guessing it is exactly the silently-wrong
/// answer this function exists to stop producing.
fn join_array_views(views: Vec<ArrayView>) -> Option<ArrayView> {
    if views.len() <= 1 {
        return views.into_iter().next();
    }
    let maximal: Vec<usize> = (0..views.len())
        .filter(|&i| views.iter().all(|other| view_contains(&views[i], other)))
        .collect();
    let &widest = maximal.first()?;
    if maximal
        .iter()
        .any(|&i| views[i].dim_names != views[widest].dim_names)
    {
        return None;
    }
    Some(views[widest].clone())
}

/// True when `view` names one dimension more than once (`matrix[d,d]`).
///
/// Such a view is a perfectly good ARRAY -- nine well-defined cells -- and this
/// says nothing about reading it directly. What it cannot be is the shape of a
/// temp that a computed operand is evaluated into, or of a snapshot region a
/// `PREVIOUS`/`INIT` view addresses, because every layer that projects between
/// an array and a temp does so BY DIMENSION NAME and takes the first match:
/// [`project_var_index_to_temp`] gives both `d` axes the same coordinate (so
/// `out[i,j]` reads `temp[i,i]`), and `codegen::array_view_to_static_temp` keys
/// `DimId`s by name, so the runtime broadcast has the same blind spot. Neither
/// can say WHICH `d` is meant.
///
/// It has exactly TWO callers, and both are positions that can refuse LOUDLY:
/// [`array_operand::materialize_view_operand`] (which leaves codegen to reject
/// the operand) and `codegen::snapshot_static_view` (which returns an `Err` of
/// its own). It deliberately does NOT live inside [`join_array_views`], because
/// [`find_expr_array_view`]'s other three consumers turn a `None` into a silent
/// substitution of the variable's own view -- and for `out[d] =
/// SUM(VECTOR SORT ORDER(matrix[d,d], 1))` that substituted a three-slot temp
/// for a nine-element sort order and the VM indexed past it. Refusing at the
/// loud sites refuses exactly the same equations and costs no others.
///
/// Scope, deliberately: this refuses only what GH #995 newly made compilable.
/// A repeated dimension read DIRECTLY (`out[d,d] = VECTOR SORT ORDER(matrix[d,d],
/// 1)`, or even `out[d,d] = matrix[d,d]`) compiles at the merge base and still
/// does, to the same first-axis-wins numbers -- measured, and pinned as a
/// disclosed residual by
/// `array_operand_materialization_tests::a_repeated_dimension_read_directly_is_a_pre_existing_residual`.
/// Widening the refusal to cover it would be a fix to a pre-existing defect
/// riding on an unrelated change, and the right fix is to make the projection
/// axis-identity-aware rather than to refuse the shape -- the more so because
/// the shape is legitimate: Vensim REJECTS the declaration -- run in Vensim DSS 2026-08-04, `vensim-probes/repeated_dimension.mdl` refuses to simulate with "DimA appears more than once on LHS" -- so no MDL-imported model can contain this shape and the residual is confined to hand-authored XMILE/JSON/protobuf. It is NOT illegitimate, though: the XMILE v1.0 spec exemplifies the declaration (`docs/reference/xmile-v1.0.html`, "A 2D non-apply-to-all array with dimensions X by X, where X is size 2", verified in-repo), so a conformant file may carry it and Simlin must keep reading it. The spec exemplifies only the DECLARATION, with per-element equations; it says nothing about what a REFERENCE such as `sq[X,X]` means, which is the part that is wrong here.
pub(super) fn view_repeats_a_dimension(view: &ArrayView) -> bool {
    (1..view.dim_names.len())
        .any(|i| !view.dim_names[i].is_empty() && view.dim_names[..i].contains(&view.dim_names[i]))
}

/// True when an iteration shaped like `outer` can read every element of `inner`.
///
/// The first branch is an IDENTICAL-shape test, and it carries both families
/// [`named_dims`] refuses. An UNNAMED view (a temp's `dim_names` are empty
/// strings) has no name to compare and needs none against a copy of itself. A
/// REPEATED name is the same: `square[d,d] + square[d,d]` is a well-defined
/// elementwise expression and joins to that shape, and the join is the right
/// answer to give -- it is the MATERIALIZER that then refuses to build a temp of
/// that shape ([`view_repeats_a_dimension`]), because the refusal is about
/// projecting into a temp rather than about comparing two views.
///
/// Beyond identity the relation is by dimension NAME and size, because that is
/// what the runtime broadcast matches on (`vm`'s `LoadIterViewAt` ->
/// [`crate::dimensions::match_dimensions_two_pass`]): a source dimension the
/// iteration cannot match by id reads NaN, so placing one positionally would be
/// a guess. A dimensionless view is contained by everything, which is how a
/// collapsed element such as `vals[1]` broadcasts without constraining the shape
/// around it.
fn view_contains(outer: &ArrayView, inner: &ArrayView) -> bool {
    if outer.dims == inner.dims && outer.dim_names == inner.dim_names {
        return true;
    }
    let (Some(outer), Some(inner)) = (named_dims(outer), named_dims(inner)) else {
        return false;
    };
    inner
        .iter()
        .all(|(name, size)| outer.iter().any(|(o, s)| o == name && s == size))
}

/// A view's `(dimension name, size)` pairs, or `None` when it does not name
/// every dimension or names one TWICE.
///
/// Both refusals are the same point: containment is decided by name, and
/// neither shape can answer it. An unnamed axis has nothing to match; a
/// `matrix[d,d]` view can say "contains `d` at size 3" but not WHICH `d`, so
/// `[d,d] contains [d]` is unanswerable rather than true. Both families still
/// reach [`view_contains`]'s identical-shape branch, which needs no name; what
/// refuses a repeated name as an expression's SOLE shape is
/// [`view_repeats_a_dimension`], at the materializer. See `array_operand`'s
/// "What still declines".
fn named_dims(view: &ArrayView) -> Option<Vec<(&str, usize)>> {
    if view.dim_names.len() != view.dims.len() || view.dim_names.iter().any(|n| n.is_empty()) {
        return None;
    }
    if (1..view.dim_names.len()).any(|i| view.dim_names[..i].contains(&view.dim_names[i])) {
        return None;
    }
    Some(
        view.dim_names
            .iter()
            .map(|n| n.as_str())
            .zip(view.dims.iter().copied())
            .collect(),
    )
}

/// Every array view the subexpressions of `expr` carry.
///
/// Split out from [`find_expr_array_view`] so the enumeration of which
/// positions carry a shape lives in exactly one place: which arguments an
/// array-producing builtin takes its shape from, which builtins are elementwise
/// (and so contribute every argument's shape), and which are scalar-valued and
/// contribute nothing.
///
/// The `If` CONDITION is visited. It contributes nothing to an `IF` whose arms
/// already agree, but the `BeginIter` body READS it
/// (`codegen::collect_iter_source_views_impl` pushes its view), so
/// `IF wide[e,d] > 0 THEN a[d] ELSE b[d]` does vary over `e` and the iteration
/// evaluating it has to as well.
///
/// A builtin's shape comes from its signature's `ResultKind` (an
/// array-producing builtin's `shape_from` argument, every argument of an
/// elementwise one, nothing for a scalar-valued one), so a new builtin is
/// classified in the table rather than silently unshaped here. The one
/// per-variant arm is `PREVIOUS`/`INIT`: a snapshot read has the shape of its
/// lagged argument only, never of the fallback.
fn collect_expr_array_views(expr: &Expr, out: &mut Vec<ArrayView>) {
    match expr {
        Expr::StaticSubscript(_, view, _) | Expr::TempArray(_, view, _) => out.push(view.clone()),
        Expr::App(builtin, _) => match builtin {
            // `PREVIOUS`/`INIT` DO carry a shape (GH #995): codegen reads an
            // array-valued one as its argument's view over a snapshot buffer,
            // so the result has exactly the lagged argument's shape -- and an
            // argument that collapsed to a single element yields none, which is
            // what keeps a scalar `PREVIOUS(s)` broadcasting instead of
            // reshaping the operand around it. The fallback is per-call scalar
            // state and never shapes the result.
            BuiltinFn::Previous(a, _) | BuiltinFn::Init(a) => collect_expr_array_views(a, out),
            other => match other.result_kind() {
                // An array-producing builtin takes its shape from one
                // argument: VECTOR ELM MAP's offsets, VECTOR SORT ORDER's and
                // RANK's array, the ALLOCATEs' requests.
                ResultKind::Array { shape_from } => {
                    collect_expr_array_views(other.args()[shape_from as usize], out)
                }
                // Elementwise scalar builtins: applied per iteration inside a
                // `BeginIter` body, so their result has the shape of whichever
                // argument carries one -- every one of them, for the join.
                ResultKind::Elementwise => {
                    for arg in other.args() {
                        collect_expr_array_views(arg, out);
                    }
                }
                // Scalar-valued: `Mean` (its single-argument form is a
                // REDUCTION to a scalar and its n-ary form a scalar mean), the
                // reducers, `VectorSelect`, the `Lookup` family (`LookupArray`'s
                // shape is the TABLE array's, which this would have to reach
                // through the gf registry), and the 0-arity builtins. Their
                // ARGUMENTS are not walked either: a reducer collapses whatever
                // it reads to one number, so `vals[d] + SUM(wide[*,*])` is
                // `[d]`-shaped and a walk that let `wide`'s view through would
                // widen the temp to a shape the operand does not have.
                ResultKind::Scalar => {}
            },
        },
        Expr::Op1(_, inner, _) => collect_expr_array_views(inner, out),
        Expr::Op2(_, lhs, rhs, _) => {
            collect_expr_array_views(lhs, out);
            collect_expr_array_views(rhs, out);
        }
        Expr::If(cond, t, f, _) => {
            collect_expr_array_views(t, out);
            collect_expr_array_views(f, out);
            collect_expr_array_views(cond, out);
        }
        _ => {}
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
        Expr::Op1(_, inner, _) | Expr::AssignTemp(_, inner, _) | Expr::AssignCurr(_, inner) => {
            contains_array_producing_builtin(inner)
        }
        Expr::If(cond, t, f, _) => {
            contains_array_producing_builtin(cond)
                || contains_array_producing_builtin(t)
                || contains_array_producing_builtin(f)
        }
        Expr::App(builtin, _) => builtin
            .args()
            .into_iter()
            .any(contains_array_producing_builtin),
        _ => false,
    }
}

/// Test-only wrapper exposing the production recursive
/// array-producing-builtin predicate (`contains_array_producing_builtin`,
/// which delegates to the private `is_array_producing_builtin`): true iff
/// any element of a variable's lowered per-element `Expr` list is, or
/// contains, an array-producing builtin (VectorElmMap/VectorSortOrder/
/// Rank/AllocateAvailable/AllocateByPriority), including nested as a
/// subexpression or hoisted into an `AssignTemp`.
/// `crate::db::dep_graph::array_producing_vars` reuses this exact
/// predicate rather than re-implementing the recursion.
#[cfg(test)]
pub(crate) fn exprs_contain_array_producing_builtin(exprs: &[Expr]) -> bool {
    exprs.iter().any(contains_array_producing_builtin)
}

#[cfg(test)]
mod exprs_contain_array_producing_builtin_tests {
    use super::*;

    fn vr(name: &str) -> VarRef {
        VarRef::base(Ident::new(name))
    }

    fn vem() -> Expr {
        // A minimal array-producing builtin call (args are irrelevant to
        // the predicate; only the `BuiltinFn` discriminant matters).
        Expr::App(
            BuiltinFn::VectorElmMap(
                Box::new(Expr::Const(0.0, Loc::default())),
                Box::new(Expr::Const(0.0, Loc::default())),
            ),
            Loc::default(),
        )
    }

    #[test]
    fn flags_top_level_array_producing_element() {
        // The scalar-lowering shape `AssignCurr(off, VECTOR ELM MAP(...))`
        // -- the top-level case the scalar path does NOT hoist:
        // `contains_ ⊇ is_` catches the top-level `App`.
        let exprs = vec![Expr::AssignCurr(vr("dst"), Box::new(vem()))];
        assert!(exprs_contain_array_producing_builtin(&exprs));
    }

    #[test]
    fn flags_array_producing_only_in_a_hoisted_assign_temp() {
        // The incomplete-sourcing guard: the `App` lives ONLY in a
        // hoisted `AssignTemp` (a non-first element); `AssignCurr` reads
        // the temp. `.iter().any` over the COMPLETE list + the
        // `AssignTemp` recursion must still flag it.
        let exprs = vec![
            Expr::AssignCurr(
                vr("dst"),
                Box::new(Expr::TempArray(
                    0,
                    ArrayView::contiguous(vec![1]),
                    Loc::default(),
                )),
            ),
            Expr::AssignTemp(0, Box::new(vem()), ArrayView::contiguous(vec![1])),
        ];
        assert!(exprs_contain_array_producing_builtin(&exprs));
    }

    #[test]
    fn does_not_flag_plain_exprs() {
        let exprs = vec![
            Expr::AssignCurr(vr("a"), Box::new(Expr::Const(1.0, Loc::default()))),
            Expr::AssignCurr(vr("b"), Box::new(Expr::Var(vr("a"), Loc::default()))),
        ];
        assert!(!exprs_contain_array_producing_builtin(&exprs));
    }

    #[test]
    fn does_not_flag_empty_list() {
        assert!(!exprs_contain_array_producing_builtin(&[]));
    }
}

/// How a subexpression is consumed while [`replace_nested_builtins_for_element`]
/// walks an expression.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NestedBuiltinArgMode {
    /// Expression is consumed as a scalar for the current A2A element.
    ScalarElement,
    /// Expression is consumed as a scalar in non-A2A context; nested
    /// array-producing builtins should remain untouched in this position.
    ScalarContext,
    /// Expression is consumed as an array value (e.g., SUM arg) and must keep
    /// full-array semantics.
    ArrayValue,
}

impl NestedBuiltinArgMode {
    fn scalar_child_mode(self) -> Self {
        match self {
            NestedBuiltinArgMode::ScalarElement | NestedBuiltinArgMode::ArrayValue => {
                NestedBuiltinArgMode::ScalarElement
            }
            NestedBuiltinArgMode::ScalarContext => NestedBuiltinArgMode::ScalarContext,
        }
    }
}

/// Replace array-producing builtins in an expression tree with reads of
/// hoisted temps. Each builtin is moved into an `AssignTemp` of its own,
/// pushed onto `hoisted` with an id from `temps`, and replaced by a read
/// projected from the variable's element index `var_idx` through the
/// builtin's own `ArrayView`, which handles nested builtins operating on
/// different dimensions. An element-invariant expression is rewritten once
/// and re-pointed at each other element with [`rebind_hoisted_reads`].
fn replace_nested_builtins_for_element(
    expr: Expr,
    var_idx: usize,
    var_view: &ArrayView,
    temps: &TempAllocator,
    hoisted: &mut Vec<Expr>,
    arg_mode: NestedBuiltinArgMode,
) -> Expr {
    if is_array_producing_builtin(&expr) {
        if matches!(arg_mode, NestedBuiltinArgMode::ScalarContext) {
            return expr;
        }
        let id = temps.alloc();
        let loc = expr.get_loc();
        let builtin_view = find_expr_array_view(&expr).unwrap_or_else(|| var_view.clone());
        hoisted.push(Expr::AssignTemp(id, Box::new(expr), builtin_view.clone()));
        return match arg_mode {
            NestedBuiltinArgMode::ScalarElement => {
                let element_idx = project_var_index_to_temp(var_idx, var_view, &builtin_view);
                Expr::TempArrayElement(id, builtin_view, element_idx, loc)
            }
            NestedBuiltinArgMode::ArrayValue => Expr::TempArray(id, builtin_view, loc),
            NestedBuiltinArgMode::ScalarContext => {
                unreachable!("ScalarContext array builtins should return without rewriting")
            }
        };
    }
    match expr {
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(replace_nested_builtins_for_element(
                *lhs, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *rhs, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            loc,
        ),
        Expr::Op1(op, inner, loc) => Expr::Op1(
            op,
            Box::new(replace_nested_builtins_for_element(
                *inner, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            loc,
        ),
        Expr::If(cond, t, f, loc) => Expr::If(
            Box::new(replace_nested_builtins_for_element(
                *cond, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *t, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *f, var_idx, var_view, temps, hoisted, arg_mode,
            )),
            loc,
        ),
        // Descend into builtin arguments while preserving whether each argument
        // expects a scalar element or a full array value: an array operand
        // (`ArgKind::Array`) is consumed whole and the scalar positions beside
        // it per element; a builtin with no array operand passes the enclosing
        // mode through to every argument.
        Expr::App(builtin, loc) => {
            let scalar_child_mode = arg_mode.scalar_child_mode();
            let rewritten = if builtin.has_array_operand() {
                builtin.map_with_kinds(|sub_expr, kind| {
                    let mode = match kind {
                        ArgKind::Array { .. } => NestedBuiltinArgMode::ArrayValue,
                        ArgKind::Scalar | ArgKind::Table => scalar_child_mode,
                        ArgKind::Ident => {
                            unreachable!("an identifier payload is not an expression argument")
                        }
                    };
                    replace_nested_builtins_for_element(
                        sub_expr, var_idx, var_view, temps, hoisted, mode,
                    )
                })
            } else {
                builtin.map(|sub_expr| {
                    replace_nested_builtins_for_element(
                        sub_expr, var_idx, var_view, temps, hoisted, arg_mode,
                    )
                })
            };
            Expr::App(rewritten, loc)
        }
        other => other,
    }
}

/// Re-point the hoisted-temp reads of an element-invariant expression at
/// element `var_idx`.
///
/// The shared hoisting branches rewrite an expression once with
/// [`replace_nested_builtins_for_element`]; every other element evaluates the
/// same expression (`expression_depends_on_active_dimension` has checked that
/// the lowered forms are identical) and reads the same temps at its own
/// projected index. Only `TempArrayElement` carries an element index -- a
/// whole-array read (`TempArray`, a reducer's operand) is the same for every
/// element -- and the walk covers exactly the positions the hoister rewrites:
/// the operands of `Op1`/`Op2`/`If` and builtin arguments.
fn rebind_hoisted_reads(expr: Expr, var_idx: usize, var_view: &ArrayView) -> Expr {
    match expr {
        Expr::TempArrayElement(id, view, _, loc) => {
            let element_idx = project_var_index_to_temp(var_idx, var_view, &view);
            Expr::TempArrayElement(id, view, element_idx, loc)
        }
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(rebind_hoisted_reads(*lhs, var_idx, var_view)),
            Box::new(rebind_hoisted_reads(*rhs, var_idx, var_view)),
            loc,
        ),
        Expr::Op1(op, inner, loc) => Expr::Op1(
            op,
            Box::new(rebind_hoisted_reads(*inner, var_idx, var_view)),
            loc,
        ),
        Expr::If(cond, t, f, loc) => Expr::If(
            Box::new(rebind_hoisted_reads(*cond, var_idx, var_view)),
            Box::new(rebind_hoisted_reads(*t, var_idx, var_view)),
            Box::new(rebind_hoisted_reads(*f, var_idx, var_view)),
            loc,
        ),
        Expr::App(builtin, loc) => Expr::App(
            builtin.map(|arg| rebind_hoisted_reads(arg, var_idx, var_view)),
            loc,
        ),
        other => other,
    }
}

/// Construct a contiguous ArrayView from A2A dimensions.
fn array_view_from_dims(dims: &[Dimension]) -> ArrayView {
    let dim_sizes: Vec<usize> = dims.iter().map(|d| d.len()).collect();
    let dim_names: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
    ArrayView::contiguous_with_names(dim_sizes, dim_names)
}

/// Which of an arrayed equation's arms an element evaluates: its own explicit
/// equation, or the EXCEPT default.
///
/// This is the identity the hoisting path keys on. An explicit arm belongs to
/// exactly one element; the default is the one arm several elements share, so
/// it is the arm whose hoist is emitted once and read per element.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ArrayedArm {
    Explicit(CanonicalElementName),
    Default,
}

/// The arm element `key` evaluates and its equation, or `None` when the
/// element has no explicit equation and no default applies to it.
fn arrayed_arm<'e>(
    elements: &'e HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&'e crate::ast::Expr2>,
    apply_default_for_missing: bool,
    key: &CanonicalElementName,
) -> Option<(ArrayedArm, &'e crate::ast::Expr2)> {
    if let Some(ast) = elements.get(key) {
        return Some((ArrayedArm::Explicit(key.clone()), ast));
    }
    if apply_default_for_missing {
        return default_ast.map(|ast| (ArrayedArm::Default, ast));
    }
    None
}

/// How an arm that several elements evaluate is hoisted: once, with each
/// element's reads re-pointed at its own index, or per element when the arm's
/// lowered form varies with the active element.
enum ArmHoist {
    Shared(Expr),
    PerElement,
}

/// Handle the Arrayed expansion, detecting array-producing builtins in
/// per-element expressions and hoisting them into AssignTemp pre-computations.
///
/// When a per-element expression is (or contains) an array-producing builtin
/// like VectorElmMap, VectorSortOrder, or AllocateAvailable, the builtin must
/// be evaluated once for the whole array and stored in temp. Each element then
/// reads its result via TempArrayElement.
///
/// The elements are lowered in order, each in its own temp scope. The first
/// one whose lowered form contains an array-producing builtin switches the
/// whole equation to the hoisting path: everything lowered so far is dropped
/// along with the temp ids it took, and that element's arm is the one hoisted
/// for every element that shares it. The first element alone cannot decide
/// this -- it may be a constant override while a later element uses a default
/// that contains the builtin.
fn expand_arrayed_with_hoisting(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    base: &VarRef,
) -> Result<Vec<Expr>> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());
    let mark = ctx.temps.mark();
    let scopes = ctx.temps.element_scopes();

    let mut exprs: Vec<Expr> = Vec::new();
    for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
        scopes.begin_element();
        let key = CanonicalElementName::from_raw(&subscripts.join(","));
        let Some((arm, ast)) = arrayed_arm(elements, default_ast, apply_default_for_missing, &key)
        else {
            exprs.push(Expr::AssignCurr(
                base.offset_by(i),
                Box::new(Expr::Const(0.0, Loc::default())),
            ));
            continue;
        };
        let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
        let mut elem_exprs = elem_ctx.lower(ast)?;
        let main_expr = elem_exprs.pop().unwrap();
        if contains_array_producing_builtin(&main_expr) {
            drop(scopes);
            ctx.temps.discard_since(mark);
            return expand_arrayed_hoisted(
                ctx,
                dims,
                elements,
                default_ast,
                apply_default_for_missing,
                base,
                &active_dims,
                arm,
                ast,
            );
        }
        elem_exprs.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
        exprs.extend(elem_exprs);
    }
    Ok(exprs)
}

/// Handle the A2A expansion for a single lowered expression, detecting
/// array-producing builtins and hoisting them into AssignTemp pre-computations.
///
/// Returns the complete list of expressions (pre-expressions + AssignTemp +
/// per-element AssignCurr nodes).
///
/// Element 0 is lowered first to detect the expression shape. Without an
/// array-producing builtin, the elements are lowered one after another, each
/// in its own temp scope; with one, element 0's lowering is dropped along with
/// its temp ids and the hoisting path lowers afresh.
fn expand_a2a_with_hoisting(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    base: &VarRef,
) -> Result<Vec<Expr>> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());
    let mark = ctx.temps.mark();
    let scopes = ctx.temps.element_scopes();

    let first_subscripts: Vec<String> = SubscriptIterator::new(dims).next().unwrap_or_default();
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let mut first_exprs = first_ctx.lower(ast)?;
    let main_expr = first_exprs.pop().unwrap();

    if contains_array_producing_builtin(&main_expr) {
        // Re-lower with lower_preserving_dimensions so that
        // IndexExpr3::Dimension references survive Pass 1 and reach
        // normalize_subscripts3 as ActiveDimRef.  Inside array-producing
        // builtins (lowered with preserve_wildcards_for_iteration)
        // ActiveDimRef is kept as Wildcard, preserving full array views.
        // Without this, Pass 1 resolves Dimension to a constant index
        // based on the first element's active subscripts, collapsing
        // array arguments to scalars.
        drop(scopes);
        ctx.temps.discard_since(mark);
        let mut first_exprs = first_ctx.lower_preserving_dimensions(ast)?;
        let main_expr = first_exprs.pop().unwrap();
        return expand_a2a_hoisted(ctx, dims, ast, base, &active_dims, first_exprs, main_expr);
    }

    // Not an array-producing builtin: the standard per-element loop, starting
    // from the already-lowered element 0.
    first_exprs.push(Expr::AssignCurr(base.clone(), Box::new(main_expr)));
    let mut all_exprs = first_exprs;
    for (i, subscripts) in SubscriptIterator::new(dims).enumerate().skip(1) {
        scopes.begin_element();
        let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
        let mut exprs = elem_ctx.lower(ast)?;
        let main_expr = exprs.pop().unwrap();
        exprs.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
        all_exprs.extend(exprs);
    }
    Ok(all_exprs)
}

/// Detect whether an array-producing builtin's lowered expression depends on
/// the active A2A dimension (e.g. `dir[D]` varies per element), by lowering
/// the equation for every element and comparing each against the first.
///
/// The lowerings are probes. Each element is lowered in its own temp scope, so
/// identical expressions number their temps identically and compare equal,
/// and the whole probe is discarded afterwards, so none of its ids reaches the
/// fragment. Early-exits on the first mismatch, so dimension-dependent
/// expressions are typically O(1). For dimension-independent expressions, the
/// O(N) cost is acceptable at compile time since SD model arrays are small.
fn expression_depends_on_active_dimension(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    active_dims: &Arc<[Dimension]>,
) -> Result<bool> {
    let mark = ctx.temps.mark();
    let mut reference: Option<Expr> = None;
    let mut depends = false;
    {
        let scopes = ctx.temps.element_scopes();
        for subscripts in SubscriptIterator::new(dims) {
            scopes.begin_element();
            let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
            let elem_main = elem_exprs.pop().unwrap();
            match &reference {
                None => reference = Some(elem_main),
                Some(first) => {
                    if *first != elem_main {
                        depends = true;
                        break;
                    }
                }
            }
        }
    }
    ctx.temps.discard_since(mark);
    Ok(depends)
}

/// Inner function for `expand_a2a_with_hoisting` when array-producing builtins
/// are detected. Handles both top-level and nested array-producing builtins.
fn expand_a2a_hoisted(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    base: &VarRef,
    active_dims: &Arc<[Dimension]>,
    first_exprs: Vec<Expr>,
    main_expr: Expr,
) -> Result<Vec<Expr>> {
    if is_array_producing_builtin(&main_expr) {
        let needs_per_element =
            expression_depends_on_active_dimension(ctx, dims, ast, active_dims)?;

        if needs_per_element {
            return expand_a2a_per_element_hoisted(
                ctx,
                dims,
                ast,
                base,
                active_dims,
                first_exprs,
                main_expr,
            );
        }

        let temp_id = ctx.temps.alloc();
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
                base.offset_by(i),
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
        // contains one nested inside (e.g. `10 + VECTOR ELM MAP(...)`).
        let needs_per_element =
            expression_depends_on_active_dimension(ctx, dims, ast, active_dims)?;

        let var_view = array_view_from_dims(dims);
        let mut result = first_exprs;

        if needs_per_element {
            // Scalar args vary by element: each element gets its own
            // AssignTemp blocks so the nested builtin is re-evaluated. The
            // element's Pass 1 pre-expressions are kept as well; their ids come
            // from the fragment allocator like every other temp's.
            for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                let elem_main = elem_exprs.pop().unwrap();
                result.extend(elem_exprs);

                // GH #578: fold any scalar-source / constant-offset ELM MAP
                // nested in this element's expression to a direct read before
                // the array-builtin hoister runs; whatever array-producing
                // builtins remain are hoisted normally.
                let elem_main = fold_scalar_source_elm_maps(ctx, elem_main);

                let mut hoisted = Vec::new();
                let elem_rewritten = replace_nested_builtins_for_element(
                    elem_main,
                    i,
                    &var_view,
                    &ctx.temps,
                    &mut hoisted,
                    NestedBuiltinArgMode::ScalarElement,
                );
                result.extend(hoisted);
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
                    Box::new(elem_rewritten),
                ));
            }
        } else {
            // Scalar args are constant: hoist once from element 0, then point
            // every element's reads at its own index in the shared temps.
            let mut hoisted = Vec::new();
            let rewritten = replace_nested_builtins_for_element(
                main_expr,
                0,
                &var_view,
                &ctx.temps,
                &mut hoisted,
                NestedBuiltinArgMode::ScalarElement,
            );
            result.extend(hoisted);
            let total_elements: usize = dims.iter().map(|d| d.len()).product();
            for i in 0..total_elements {
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
                    Box::new(rebind_hoisted_reads(rewritten.clone(), i, &var_view)),
                ));
            }
        }
        Ok(result)
    } else {
        unreachable!("expand_a2a_hoisted called without array-producing builtin")
    }
}

/// GH #578: fold a single element of `VECTOR ELM MAP(scalar_source, offset)`
/// into a direct read when the source is a fully-collapsed element reference
/// and the per-element offset is a compile-time constant.
///
/// Genuine Vensim maps the result over the source variable's FULL row-major
/// storage from the base the element reference establishes:
/// `result = source_full[base + round(offset)]`. When `source` is a scalar
/// `StaticSubscript` (its `view.offset` is the element's flat index and `off`
/// is the variable base) and `offset` folds to a constant, the whole read is
/// known at compile time: it is the variable slot `off + base + round(offset)`,
/// or `:NA:` (NaN) if that flat index is outside `[0, full_source_len)`.
///
/// This is what lets a scalar-source / expression-offset ELM MAP compile at
/// all: the array-producing ELM MAP opcode needs a *view* offset, but here the
/// per-element offset lowers to a `Const` (e.g. `(DimA - 1)` -> `0, 1, 2`),
/// which is not a view. Returns `None` for any shape this fold does not cover
/// (non-scalar source, non-constant offset), leaving the normal path to run.
fn try_fold_scalar_source_elm_map(ctx: &Context, main_expr: &Expr) -> Option<Expr> {
    let Expr::App(BuiltinFn::VectorElmMap(source, offset), loc) = main_expr else {
        return None;
    };
    // Source must be a fully-collapsed (scalar) element reference carrying its
    // base: a StaticSubscript with no remaining dimensions, whose `off` is the
    // variable base and `view.offset` the element's flat index within it.
    let (base, elem_flat) = match source.as_ref() {
        Expr::StaticSubscript(base, view, _) if view.dims.is_empty() => (base, view.offset),
        _ => return None,
    };
    // The per-element offset must be a compile-time constant (it is not a view,
    // so the ELM MAP opcode could not consume it anyway).
    let Expr::Const(offset_val, _) = fold::fold_constants((**offset).clone()) else {
        return None;
    };
    let full_len = ctx.full_var_len_for_base(base)?;
    // round() matches the VM's `vm_vector_elm_map` per-element offset rounding.
    let flat = elem_flat as i64 + offset_val.round() as i64;
    if flat < 0 || flat >= full_len as i64 {
        Some(Expr::Const(f64::NAN, *loc))
    } else {
        Some(Expr::Var(base.offset_by(flat as usize), *loc))
    }
}

/// Recursively apply [`try_fold_scalar_source_elm_map`] through the
/// scalar-value wrappers of a per-element expression, so a scalar-source ELM
/// MAP nested in arithmetic (`10 + VECTOR ELM MAP(x[three], (DimA-1))`) folds
/// too (GH #578).
///
/// Recursion is restricted to `Op2`/`If`, which propagate a scalar-value
/// context to their operands in this per-element lowering (unary minus is
/// already lowered to `Op2(Sub, 0, x)`). It deliberately does NOT descend into
/// `Expr::App` arguments: a reducer like `SUM(elm_map_array)` consumes the ELM
/// MAP as an *array*, and folding it to a single element there would be wrong.
fn fold_scalar_source_elm_maps(ctx: &Context, expr: Expr) -> Expr {
    if let Some(folded) = try_fold_scalar_source_elm_map(ctx, &expr) {
        return folded;
    }
    match expr {
        Expr::Op2(op, l, r, loc) => Expr::Op2(
            op,
            Box::new(fold_scalar_source_elm_maps(ctx, *l)),
            Box::new(fold_scalar_source_elm_maps(ctx, *r)),
            loc,
        ),
        Expr::If(c, t, f, loc) => Expr::If(
            Box::new(fold_scalar_source_elm_maps(ctx, *c)),
            Box::new(fold_scalar_source_elm_maps(ctx, *t)),
            Box::new(fold_scalar_source_elm_maps(ctx, *f)),
            loc,
        ),
        other => other,
    }
}

/// Per-element hoisting for array-producing builtins whose scalar arguments
/// depend on the active dimension (e.g. `vector_sort_order(vals[*], dir[D])`).
/// Each element gets its own AssignTemp so the builtin is re-evaluated with
/// the correct scalar argument value for that element.
#[allow(clippy::too_many_arguments)]
fn expand_a2a_per_element_hoisted(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    base: &VarRef,
    active_dims: &Arc<[Dimension]>,
    first_exprs: Vec<Expr>,
    first_main_expr: Expr,
) -> Result<Vec<Expr>> {
    let var_view = array_view_from_dims(dims);
    let mut result = first_exprs;
    let mut first_main_expr = Some(first_main_expr);

    for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
        let main_expr = match first_main_expr.take() {
            Some(first) => first,
            None => {
                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                let main = elem_exprs.pop().unwrap();
                // Keep the element's pre-expressions (intermediate temps from
                // complex subexpressions); their ids come from the fragment
                // allocator, so they are distinct from everything emitted so
                // far and the main expression's references stay aligned.
                result.extend(elem_exprs);
                main
            }
        };

        // GH #578: a scalar-source ELM MAP with a per-element constant offset
        // folds to a direct slot read (or :NA:), so the array-producing opcode
        // -- which requires a *view* offset the constant cannot supply -- is
        // skipped entirely. If the fold collapses the whole element expression
        // to a scalar (no array-producing builtin left), emit it directly with
        // no temp consumed.
        let main_expr = fold_scalar_source_elm_maps(ctx, main_expr);
        if !contains_array_producing_builtin(&main_expr) {
            result.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
            continue;
        }

        let temp_id = ctx.temps.alloc();
        let builtin_view = find_expr_array_view(&main_expr).unwrap_or_else(|| var_view.clone());
        let loc = main_expr.get_loc();
        let temp_idx = project_var_index_to_temp(i, &var_view, &builtin_view);

        result.push(Expr::AssignTemp(
            temp_id,
            Box::new(main_expr),
            builtin_view.clone(),
        ));
        result.push(Expr::AssignCurr(
            base.offset_by(i),
            Box::new(Expr::TempArrayElement(temp_id, builtin_view, temp_idx, loc)),
        ));
    }

    Ok(result)
}

/// Handle Arrayed equations where the hoisting arm coexists with other arms
/// (EXCEPT semantics). An element whose arm lowers to an expression containing
/// an array-producing builtin reads hoisted temps; every other element is
/// lowered normally.
///
/// The hoisting arm is classified up front and, when its lowered form does not
/// vary with the element, hoisted once at the first element's subscripts --
/// whichever arm that element itself evaluates -- for every element that
/// shares it. The EXCEPT default, when it is a different arm, is classified
/// the first time an element evaluates it. An explicit arm belongs to exactly
/// one element, so it is hoisted in place and shared with nothing.
#[allow(clippy::too_many_arguments)]
fn expand_arrayed_hoisted(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    base: &VarRef,
    active_dims: &Arc<[Dimension]>,
    hoisting_arm: ArrayedArm,
    hoisting_ast: &crate::ast::Expr2,
) -> Result<Vec<Expr>> {
    let first_subscripts: Vec<String> = SubscriptIterator::new(dims).next().unwrap_or_default();
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let mut first_exprs = first_ctx.lower_preserving_dimensions(hoisting_ast)?;
    let main_expr = first_exprs.pop().unwrap();
    let var_view = array_view_from_dims(dims);

    if !contains_array_producing_builtin(&main_expr) {
        unreachable!("expand_arrayed_hoisted called without array-producing builtin")
    }

    let hoisting_dim_dependent =
        expression_depends_on_active_dimension(ctx, dims, hoisting_ast, active_dims)?;
    let mut result = first_exprs;

    // The arms several elements can share, and how each is hoisted.
    let mut shared: HashMap<ArrayedArm, ArmHoist> = HashMap::new();
    let hoisting_hoist = if hoisting_dim_dependent {
        ArmHoist::PerElement
    } else {
        let mut hoisted = Vec::new();
        let rewritten = replace_nested_builtins_for_element(
            main_expr,
            0,
            &var_view,
            &ctx.temps,
            &mut hoisted,
            NestedBuiltinArgMode::ScalarElement,
        );
        result.extend(hoisted);
        ArmHoist::Shared(rewritten)
    };
    shared.insert(hoisting_arm, hoisting_hoist);

    for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
        let key = CanonicalElementName::from_raw(&subscripts.join(","));
        let Some((arm, ast)) = arrayed_arm(elements, default_ast, apply_default_for_missing, &key)
        else {
            result.push(Expr::AssignCurr(
                base.offset_by(i),
                Box::new(Expr::Const(0.0, Loc::default())),
            ));
            continue;
        };
        let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);

        // Classify the element's arm by its ordinary lowering. A plain arm
        // keeps that lowering; a hoisting arm drops it along with its temp
        // ids and is lowered again below, preserving dimension references.
        let mark = ctx.temps.mark();
        let mut elem_exprs = elem_ctx.lower(ast)?;
        let elem_main = elem_exprs.pop().unwrap();
        if !contains_array_producing_builtin(&elem_main) {
            result.extend(elem_exprs);
            result.push(Expr::AssignCurr(base.offset_by(i), Box::new(elem_main)));
            continue;
        }
        ctx.temps.discard_since(mark);

        if matches!(arm, ArrayedArm::Default) && !shared.contains_key(&arm) {
            // The first element evaluating the EXCEPT default when it is not
            // the hoisting arm: classify it, and hoist it here for every
            // later element that evaluates it.
            let hoist = if expression_depends_on_active_dimension(ctx, dims, ast, active_dims)? {
                ArmHoist::PerElement
            } else {
                let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                let elem_main = elem_exprs.pop().unwrap();
                result.extend(elem_exprs);
                let mut hoisted = Vec::new();
                let rewritten = replace_nested_builtins_for_element(
                    elem_main,
                    i,
                    &var_view,
                    &ctx.temps,
                    &mut hoisted,
                    NestedBuiltinArgMode::ScalarElement,
                );
                result.extend(hoisted);
                ArmHoist::Shared(rewritten)
            };
            shared.insert(arm.clone(), hoist);
        }

        if let Some(ArmHoist::Shared(rewritten)) = shared.get(&arm) {
            result.push(Expr::AssignCurr(
                base.offset_by(i),
                Box::new(rebind_hoisted_reads(rewritten.clone(), i, &var_view)),
            ));
            continue;
        }

        // The element hoists on its own: an arm whose lowered form varies
        // with the element, or an explicit arm of this element alone.
        let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
        let elem_main = elem_exprs.pop().unwrap();
        result.extend(elem_exprs);
        let mut hoisted = Vec::new();
        let elem_rewritten = replace_nested_builtins_for_element(
            elem_main,
            i,
            &var_view,
            &ctx.temps,
            &mut hoisted,
            NestedBuiltinArgMode::ScalarElement,
        );
        result.extend(hoisted);
        result.push(Expr::AssignCurr(
            base.offset_by(i),
            Box::new(elem_rewritten),
        ));
    }
    Ok(result)
}

/// Crate-visible wrapper for extract_temp_sizes.
pub(crate) fn extract_temp_sizes_pub(expr: &Expr, temp_sizes_map: &mut HashMap<u32, usize>) {
    extract_temp_sizes(expr, temp_sizes_map);
}

/// Recursively extract temporary array sizes from an expression.
/// Populates the temp_sizes_map with (temp_id, max_size) entries.
/// The elements of a plain apply-to-all or arrayed equation recycle one id
/// range (`TempAllocator::element_scopes`), so one id can be written with
/// views of different sizes by different elements; the slot is sized for the
/// largest.
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
            for arg in builtin.args() {
                extract_temp_sizes(arg, temp_sizes_map);
            }
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
