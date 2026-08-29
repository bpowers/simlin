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

use crate::ast::{ArrayView, Ast, BinaryOp, Loc, SparseInfo};
use crate::builtins::ResultKind;
#[cfg(test)]
use crate::common::ErrorCode;
use crate::common::{Canonical, CanonicalDimensionName, CanonicalElementName, Ident, Result};
use crate::dimensions::DimensionsContext;
use crate::dimensions::{Axis, Dimension, NoAxisRelations, SubscriptIterator, match_axes_partial};
use crate::sim_err;
use crate::variable::{VarKind, Variable};

// Re-exports for crate-internal API
pub(crate) use self::codegen::ModuleCtx;
/// Codegen's classification of a lowered `PREVIOUS`/`INIT` argument, for the
/// agreement test that checks it against the parse's classification of the
/// same call (`db::prev_init_tests`). Exported alone rather than by opening
/// `codegen`, which has no other crate-wide consumer.
#[cfg(test)]
pub(crate) use self::codegen::lowered_snapshot_arg;
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

struct LoweredAssignments {
    exprs: Vec<Expr>,
}

impl LoweredAssignments {
    fn new(exprs: Vec<Expr>) -> Self {
        Self { exprs }
    }
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
        diagnostics: vec![],
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
        diagnostics: vec![],
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
        diagnostics: vec![],
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
        diagnostics: vec![],
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
        diagnostics: vec![],
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
        let lowered = if let Some((input_idx, _ident)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| n.as_str() == var.ident())
        {
            LoweredAssignments::new(vec![Expr::AssignCurr(
                ctx.get_ref(&Ident::new(var.ident()))?,
                Box::new(Expr::ModuleInput(input_idx, Loc::default())),
            )])
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
                    LoweredAssignments::new(vec![Expr::EvalModule(
                        var.ident.clone(),
                        model_name.clone(),
                        input_set,
                        inputs,
                    )])
                }
                VarKind::Stock { init_ast: ast, .. } => {
                    let base = ctx.get_base_ref(&Ident::new(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => LoweredAssignments::new(vec![Expr::AssignCurr(
                                base,
                                Box::new(ctx.lower(ast)?),
                            )]),
                            Ast::ApplyToAll(dims, ast) => expand_a2a(ctx, dims, ast, &base)?,
                            Ast::Arrayed(
                                dims,
                                elements,
                                default_ast,
                                apply_default_for_missing,
                            ) => expand_arrayed(
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
                            Ast::Scalar(_) => LoweredAssignments::new(vec![Expr::AssignNext(
                                base.clone(),
                                Box::new(ctx.build_stock_update_expr(&base, var)?),
                            )]),
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
                                LoweredAssignments::new(exprs?)
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
                    let mut lowered = match ast.as_ref().unwrap() {
                        Ast::Scalar(ast) => LoweredAssignments::new(vec![Expr::AssignCurr(
                            base.clone(),
                            Box::new(ctx.lower(ast)?),
                        )]),
                        Ast::ApplyToAll(dims, ast) => expand_a2a(ctx, dims, ast, &base)?,
                        Ast::Arrayed(dims, elements, default_ast, apply_default_for_missing) => {
                            expand_arrayed(
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
                    lowered.exprs = apply_implicit_with_lookup(lowered.exprs, &base, tables);
                    lowered
                }
            }
        };
        // Fold constant subtrees once at compile time so the per-timestep
        // programs never re-evaluate `literal op literal` (including the
        // `0 - x` form every negative literal lowers to). Runs here -- the
        // single chokepoint every per-variable fragment lowering funnels
        // through -- so both backends (VM and wasmgen) see the folded form.
        let LoweredAssignments { exprs } = lowered;
        let ast: Vec<Expr> = exprs
            .into_iter()
            .map(fold::fold_constants)
            .map(|expr| fold_scalar_source_elm_maps(ctx, expr))
            .collect();
        // Discharge codegen's "an array operand is a view over storage"
        // contract (GH #995). Runs at the same chokepoint and after folding,
        // so it sees the final tree both backends consume and never
        // materializes something folding would have collapsed.
        let target_view = var.get_dimensions().map(array_view_from_dims);
        let ast = array_operand::materialize_arrays(
            ast,
            target_view.as_ref(),
            ctx.dimensions_ctx,
            &ctx.temps,
        );
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
/// rewritten. Final array materialization runs afterward, so any `AssignTemp`
/// it introduces sees the already-wrapped lookup expression. The table reference is
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
            // Var fragment emits targets this variable at or after its base;
            // temp materialization runs after this rewrite. The guard exists
            // purely as defense so a hypothetical assignment to a different
            // variable, or one before the base, could never wrap against a
            // nonsense element index.
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

/// Extract the output ArrayView from an expression. For array-producing builtins, the
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
/// `None` means either no subexpression carries an array shape or the shapes
/// have no unambiguous containing view. The sole materializer treats that as a
/// loud refusal by leaving the expression for codegen's attributed diagnostic;
/// it never substitutes the enclosing variable's shape. A view that repeats a
/// dimension name (`matrix[d,d]`) is still returned when it is the expression's
/// sole shape, then refused only if projecting it through temp storage would
/// lose occurrence identity. See [`view_repeats_a_dimension`].
pub(super) fn find_expr_array_view(expr: &Expr) -> Option<ArrayView> {
    let mut views = Vec::new();
    collect_expr_array_views(expr, &mut views);
    join_array_views(views)
}

/// The shape of a computed array operand, including the cross-product union
/// that Expr2 permits inside array arguments.
///
/// Containment remains the first rule. For disjoint named axes, an enclosing
/// target supplies the semantic order; a scalar reducer has no observable
/// result-axis order, so source traversal order is sufficient. Unnamed or
/// repeated axes cannot be unioned without guessing identity.
pub(super) fn find_array_operand_view(
    expr: &mut Expr,
    target: Option<&ArrayView>,
    dimensions: &DimensionsContext,
) -> Option<ArrayView> {
    let mut views = Vec::new();
    collect_expr_array_views(expr, &mut views);
    if let Some(view) = join_array_views(views.clone()) {
        return Some(view);
    }

    for view in &views {
        if view.dim_names.len() != view.dims.len()
            || view.dim_names.iter().any(|name| name.is_empty())
            || view_repeats_a_dimension(view)
        {
            return None;
        }
    }
    if views.is_empty() {
        return None;
    }

    if let Some(target) = target {
        let target_axes = declared_view_axes(target, dimensions)?;
        let mut covered = vec![false; target_axes.len()];
        for view in &views {
            let source_axes = declared_view_axes(view, dimensions)?;
            for (source_axis, matched) in match_axes_partial(&source_axes, &target_axes, dimensions)
                .into_iter()
                .enumerate()
            {
                let (target_axis, relation) = matched?;
                if matches!(relation, crate::dimensions::AxisMatch::Exact)
                    && view.dims[source_axis] != target.dims[target_axis]
                {
                    // A range keeps its parent's name. Equal names with
                    // different extents are slices, not the same axis.
                    return None;
                }
                covered[target_axis] = true;
            }
        }
        let names: Vec<_> = target
            .dim_names
            .iter()
            .enumerate()
            .filter(|(axis, _)| covered[*axis])
            .map(|(_, name)| name.clone())
            .collect();
        let dims: Vec<_> = target
            .dims
            .iter()
            .enumerate()
            .filter(|(axis, _)| covered[*axis])
            .map(|(_, len)| *len)
            .collect();
        if dims.is_empty() {
            return None;
        }
        let output = ArrayView::contiguous_with_names(dims, names);
        let aligned = align_array_operand_views(expr.clone(), &output, dimensions)?;
        *expr = aligned;
        return Some(output);
    }

    // A scalar consumer supplies no semantic target order. Preserve the
    // established cross-product rule for unrelated named axes; a mapped join
    // without a target is refused because choosing an axis would choose which
    // element correspondence the expression follows.
    let mut axes: Vec<(String, usize)> = Vec::new();
    for view in views {
        for (name, len) in view.dim_names.into_iter().zip(view.dims) {
            if let Some((_, existing_len)) = axes.iter().find(|(existing, _)| *existing == name) {
                if *existing_len != len {
                    return None;
                }
            } else {
                axes.push((name, len));
            }
        }
    }
    let (names, dims): (Vec<_>, Vec<_>) = axes.into_iter().unzip();
    Some(ArrayView::contiguous_with_names(dims, names))
}

fn declared_view_axes<'a>(
    view: &'a ArrayView,
    dimensions: &'a DimensionsContext,
) -> Option<Vec<Axis<'a>>> {
    if view.dim_names.len() != view.dims.len() {
        return None;
    }
    view.dim_names
        .iter()
        .zip(&view.dims)
        .map(|(name, &len)| {
            if name.is_empty() {
                return None;
            }
            let canonical = CanonicalDimensionName::from_raw(name);
            match dimensions.get(&canonical) {
                Some(dim) if dim.len() == len => Some(Axis::of(dim)),
                Some(_) => None,
                None => Some(Axis::named(name, len)),
            }
        })
        .collect()
}

/// Retarget the view-bearing positions of one computed operand onto `output`.
/// The recursion deliberately mirrors `collect_expr_array_views`: a scalar
/// builtin is a nested reduction and keeps its own independent view geometry.
fn align_array_operand_views(
    expr: Expr,
    output: &ArrayView,
    dimensions: &DimensionsContext,
) -> Option<Expr> {
    Some(match expr {
        Expr::StaticSubscript(base, view, loc) => {
            Expr::StaticSubscript(base, align_array_view(view, output, dimensions)?, loc)
        }
        Expr::TempArray(id, view, loc) => {
            Expr::TempArray(id, align_array_view(view, output, dimensions)?, loc)
        }
        Expr::App(builtin, loc) => {
            let builtin = match builtin {
                BuiltinFn::Previous(arg, fallback) => BuiltinFn::Previous(
                    Box::new(align_array_operand_views(*arg, output, dimensions)?),
                    fallback,
                ),
                BuiltinFn::Init(arg) => BuiltinFn::Init(Box::new(align_array_operand_views(
                    *arg, output, dimensions,
                )?)),
                other => match other.result_kind() {
                    ResultKind::Array { .. } | ResultKind::Elementwise => {
                        let mut failed = false;
                        let mapped = other.map(|arg| {
                            align_array_operand_views(arg.clone(), output, dimensions)
                                .unwrap_or_else(|| {
                                    failed = true;
                                    arg
                                })
                        });
                        if failed {
                            return None;
                        }
                        mapped
                    }
                    ResultKind::Scalar => other,
                },
            };
            Expr::App(builtin, loc)
        }
        Expr::Op1(op, inner, loc) => Expr::Op1(
            op,
            Box::new(align_array_operand_views(*inner, output, dimensions)?),
            loc,
        ),
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(align_array_operand_views(*lhs, output, dimensions)?),
            Box::new(align_array_operand_views(*rhs, output, dimensions)?),
            loc,
        ),
        Expr::If(cond, then_expr, else_expr, loc) => Expr::If(
            Box::new(align_array_operand_views(*cond, output, dimensions)?),
            Box::new(align_array_operand_views(*then_expr, output, dimensions)?),
            Box::new(align_array_operand_views(*else_expr, output, dimensions)?),
            loc,
        ),
        other => other,
    })
}

/// Express `view` in `output`'s axis identities. A mapped axis is represented
/// as a sparse view whose offsets are the canonical executed-read
/// correspondence, so VM and wasm share the element translation.
fn align_array_view(
    mut view: ArrayView,
    output: &ArrayView,
    dimensions: &DimensionsContext,
) -> Option<ArrayView> {
    let source_axes = declared_view_axes(&view, dimensions)?;
    let target_axes = declared_view_axes(output, dimensions)?;
    for (source_axis, matched) in match_axes_partial(&source_axes, &target_axes, dimensions)
        .into_iter()
        .enumerate()
    {
        let (target_axis, relation) = matched?;
        let target_name = &output.dim_names[target_axis];
        let target_len = output.dims[target_axis];
        match relation {
            crate::dimensions::AxisMatch::Exact => {
                if view.dims[source_axis] != target_len {
                    return None;
                }
            }
            crate::dimensions::AxisMatch::Mapped { .. } => {
                let source_name = &view.dim_names[source_axis];
                let correspondence = dimensions.executed_read_correspondence(
                    &CanonicalDimensionName::from_raw(target_name),
                    &CanonicalDimensionName::from_raw(source_name),
                )?;
                if correspondence.len() != target_len {
                    return None;
                }
                let source_dim = dimensions.get(&CanonicalDimensionName::from_raw(source_name))?;
                let mut offsets: Vec<usize> = correspondence
                    .iter()
                    .map(|element| source_dim.get_offset(element))
                    .collect::<Option<_>>()?;
                if let Some(existing) = view.sparse.iter().find(|s| s.dim_index == source_axis) {
                    offsets = offsets
                        .into_iter()
                        .map(|offset| existing.parent_offsets.get(offset).copied())
                        .collect::<Option<_>>()?;
                }
                view.sparse.retain(|s| s.dim_index != source_axis);
                view.sparse.push(SparseInfo {
                    dim_index: source_axis,
                    parent_offsets: offsets,
                });
                view.dims[source_axis] = target_len;
            }
            crate::dimensions::AxisMatch::BySize => {
                debug_assert_eq!(view.dims[source_axis], target_len);
            }
            // The production DimensionsContext deliberately withholds the
            // partial subdimension rung for element-resolving callers.
            crate::dimensions::AxisMatch::Subdimension => return None,
        }
        view.dim_names[source_axis] = target_name.clone();
    }
    Some(view)
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
/// Such a view is a valid array with distinct positional axes. Direct
/// apply-to-all reads retain those occurrences and pair them positionally.
/// Computed operands and snapshot views still cannot use it as storage because
/// VM temp broadcasts identify axes by `DimId`; two equal names cannot say
/// which occurrence a coordinate belongs to. Both callers refuse loudly rather
/// than collapsing the axes. XMILE 1.0 section 4.1 explicitly demonstrates a
/// two-dimensional `X by X` array, so repeated declarations cannot be rejected
/// globally merely to simplify temp projection.
pub(super) fn view_repeats_a_dimension(view: &ArrayView) -> bool {
    (1..view.dim_names.len())
        .any(|i| !view.dim_names[i].is_empty() && view.dim_names[..i].contains(&view.dim_names[i]))
}

/// True when an iteration shaped like `outer` can read every element of `inner`.
///
/// The first branch is an IDENTICAL-shape test, and it carries both families
/// [`named_axes`] refuses. An UNNAMED view (a temp's `dim_names` are empty
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
    let (Some(outer), Some(inner)) = (named_axes(outer), named_axes(inner)) else {
        return false;
    };
    // `dimensions::match_axes_partial` is the one axis-matching precedence.
    // A view carries each axis's NAME and LENGTH but not which kind of
    // dimension produced it and no declared relations, so `Axis::named` and
    // `NoAxisRelations` leave exactly the exact-name rule -- which is what
    // containment can be decided on. The length is checked here rather than by
    // the matcher because a RANGE-derived axis keeps its parent's name at a
    // smaller size, and reading `arr[2:4]` where `arr` is expected would be
    // the silently-wrong answer.
    match_axes_partial(&inner, &outer, &NoAxisRelations)
        .iter()
        .zip(inner.iter())
        .all(|(matched, axis)| {
            matched
                .as_ref()
                .is_some_and(|(outer_idx, _)| outer[*outer_idx].len == axis.len)
        })
}

/// A view's axes, or `None` when it does not name every axis or names one
/// TWICE.
///
/// Both refusals are the same point: containment is decided by name, and
/// neither shape can answer it. An unnamed axis has nothing to match; a
/// `matrix[d,d]` view can say "contains `d` at size 3" but not WHICH `d`, so
/// `[d,d] contains [d]` is unanswerable rather than true. Both families still
/// reach [`view_contains`]'s identical-shape branch, which needs no name; what
/// refuses a repeated name as an expression's SOLE shape is
/// [`view_repeats_a_dimension`], at the materializer. See `array_operand`'s
/// "What still declines".
fn named_axes(view: &ArrayView) -> Option<Vec<Axis<'_>>> {
    if view.dim_names.len() != view.dims.len() || view.dim_names.iter().any(|n| n.is_empty()) {
        return None;
    }
    if (1..view.dim_names.len()).any(|i| view.dim_names[..i].contains(&view.dim_names[i])) {
        return None;
    }
    Some(
        view.dim_names
            .iter()
            .zip(view.dims.iter())
            .map(|(name, &len)| Axis::named(name.as_str(), len))
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

/// Construct a contiguous ArrayView from declared dimensions.
fn array_view_from_dims(dims: &[Dimension]) -> ArrayView {
    let dim_sizes: Vec<usize> = dims.iter().map(|d| d.len()).collect();
    let dim_names: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
    ArrayView::contiguous_with_names(dim_sizes, dim_names)
}

/// Lower one expression for every apply-to-all element. Array operands are
/// intentionally left intact; [`array_operand::materialize_arrays`] owns the
/// only post-resolution materialization pass.
fn expand_a2a(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    base: &VarRef,
) -> Result<LoweredAssignments> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());
    let mut elements = SubscriptIterator::new(dims);
    let Some(first_subscripts) = elements.next() else {
        return Ok(LoweredAssignments::new(Vec::new()));
    };
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let prepared = first_ctx.prepare(ast)?;
    let first = Expr::AssignCurr(base.clone(), Box::new(first_ctx.lower_prepared(&prepared)?));
    let rest = elements.enumerate().map(|(i, subscripts)| {
        let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
        let value = elem_ctx.lower_prepared(&prepared)?;
        Ok(Expr::AssignCurr(base.offset_by(i + 1), Box::new(value)))
    });
    let exprs: Vec<Expr> = std::iter::once(Ok(first))
        .chain(rest)
        .collect::<Result<_>>()?;
    Ok(LoweredAssignments::new(exprs))
}

/// Lower the explicit or default equation selected by each arrayed element.
/// Missing elements without an applicable default retain the established zero
/// fill; diagnostics for that source shape are produced before this stage.
fn expand_arrayed(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    base: &VarRef,
) -> Result<LoweredAssignments> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());
    let mut subscripts = SubscriptIterator::new(dims);
    let Some(first_subscripts) = subscripts.next() else {
        return Ok(LoweredAssignments::new(Vec::new()));
    };
    let preparation_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let default_prepared = if apply_default_for_missing {
        default_ast
            .map(|ast| preparation_ctx.prepare(ast))
            .transpose()?
    } else {
        None
    };
    let all_subscripts = std::iter::once(first_subscripts).chain(subscripts);
    let exprs: Vec<Expr> = all_subscripts
        .enumerate()
        .map(|(i, subscripts)| {
            let key = CanonicalElementName::from_raw(&subscripts.join(","));
            let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            let value = if let Some(ast) = elements.get(&key) {
                let prepared = elem_ctx.prepare(ast)?;
                elem_ctx.lower_prepared(&prepared)?
            } else if let Some(prepared) = &default_prepared {
                elem_ctx.lower_prepared(prepared)?
            } else {
                Expr::Const(0.0, Loc::default())
            };
            Ok(Expr::AssignCurr(base.offset_by(i), Box::new(value)))
        })
        .collect::<Result<_>>()?;
    Ok(LoweredAssignments::new(exprs))
}

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
/// Recursion is restricted to scalar-value wrappers (`Assign*`, `Op2`, and
/// `If`; unary minus is
/// already lowered to `Op2(Sub, 0, x)`). It deliberately does NOT descend into
/// `Expr::App` arguments: a reducer like `SUM(elm_map_array)` consumes the ELM
/// MAP as an *array*, and folding it to a single element there would be wrong.
fn fold_scalar_source_elm_maps(ctx: &Context, expr: Expr) -> Expr {
    if let Some(folded) = try_fold_scalar_source_elm_map(ctx, &expr) {
        return folded;
    }
    match expr {
        Expr::AssignCurr(dst, value) => {
            Expr::AssignCurr(dst, Box::new(fold_scalar_source_elm_maps(ctx, *value)))
        }
        Expr::AssignNext(dst, value) => {
            Expr::AssignNext(dst, Box::new(fold_scalar_source_elm_maps(ctx, *value)))
        }
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
