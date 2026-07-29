// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod codegen;
pub mod context;
pub mod dimensions;
pub mod expr;
pub(crate) mod fold;
pub(crate) mod invariance;
pub mod pretty;
pub mod subscript;
pub(crate) mod symbolic;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::ast::{ArrayView, Ast, BinaryOp, Loc};
#[cfg(test)]
use crate::bytecode::CompiledModule;
use crate::common::{Canonical, CanonicalElementName, Ident, Result};
#[cfg(test)]
use crate::common::{Error, ErrorCode, ErrorKind};
#[cfg(test)]
use crate::dimensions::DimensionsContext;
use crate::dimensions::{Dimension, SubscriptIterator};
#[cfg(test)]
use crate::model::ModelStage1;
#[cfg(test)]
use crate::project::Project;
use crate::sim_err;
use crate::variable::Variable;
#[cfg(test)]
use crate::vm::IMPLICIT_VAR_COUNT;

// Re-exports for crate-internal API
pub(crate) use self::codegen::ModuleCtx;
pub(crate) use self::context::{Context, ContextCore, VariableMetadata, whole_variable_extents};
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
/// Built once per emission unit by [`context::whole_variable_extents`]. That is
/// the only place the rule is DERIVED -- every production site calls it, so
/// lowering and emission cannot disagree about what a reference addresses.
/// (`codegen`'s unit tests hand-build a table instead, in order to state the
/// lookup's contract against inputs of their own choosing; they exercise the
/// reader, not the rule.)
pub(crate) type VarSizes = HashMap<VarRef, usize>;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub struct Var {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) ast: Vec<Expr>,
}

#[test]
fn test_fold_flows() {
    let inputs = &BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
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
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    metadata.insert(
        Ident::new("a"),
        VariableMetadata {
            offset: Some(1),
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("b"),
        VariableMetadata {
            offset: Some(2),
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("c"),
        VariableMetadata {
            offset: Some(3),
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("d"),
        VariableMetadata {
            offset: Some(4),
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = crate::common::IdentMap::default();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

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
/// metadata must return an error, not panic.  This guards against the
/// case where a module's input source is deleted but the module itself
/// still exists with its original references.
#[test]
fn test_module_var_new_missing_input_source_returns_error() {
    use crate::variable::ModuleInput;

    let inputs = &BTreeSet::new();
    let module_ident = Ident::new("my_module");
    let model_name_ident: Ident<Canonical> = Ident::new("sub_model");

    // module_models maps "main" -> { "my_module" -> "sub_model" }
    let mut module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
    let mut main_modules = crate::common::IdentMap::default();
    main_modules.insert(module_ident.clone(), model_name_ident.clone());
    let main_ident = Ident::new("main");
    module_models.insert(main_ident.clone(), main_modules);

    // The module variable itself (1 slot in the parent model)
    let module_var = Variable::Module {
        ident: module_ident.clone(),
        model_name: model_name_ident,
        units: None,
        inputs: vec![ModuleInput {
            src: Ident::new("missing_source"),
            dst: Ident::new("available"),
        }],
        errors: vec![],
        unit_errors: vec![],
    };

    // metadata only contains "my_module" -- NOT "missing_source"
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    metadata.insert(
        module_ident.clone(),
        VariableMetadata {
            offset: Some(IMPLICIT_VAR_COUNT),
            size: 1,
            var: &module_var,
        },
    );
    let mut metadata2 = crate::common::IdentMap::default();
    metadata2.insert(main_ident.clone(), metadata);

    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &module_ident,
        false,
    );

    let result = Var::new(&ctx, &module_var);
    assert!(
        result.is_err(),
        "Var::new should return Err when a module input source is missing, not panic"
    );
}

#[test]
fn test_build_stock_update_expr_inflows_only() {
    let inputs = &BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
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
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: Some(0),
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("inflow"),
        VariableMetadata {
            offset: Some(1),
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = crate::common::IdentMap::default();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

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
    let inputs = &BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
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
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: Some(0),
            size: 1,
            var: &dummy_var,
        },
    );
    metadata.insert(
        Ident::new("outflow"),
        VariableMetadata {
            offset: Some(1),
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = crate::common::IdentMap::default();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

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
    let inputs = &BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
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
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    metadata.insert(
        Ident::new("stock"),
        VariableMetadata {
            offset: Some(0),
            size: 1,
            var: &dummy_var,
        },
    );
    let mut metadata2 = crate::common::IdentMap::default();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

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
    let inputs = &BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
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
    let mut metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
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
                offset: Some(off),
                size: 1,
                var: &dummy_var,
            },
        );
    }
    let mut metadata2 = crate::common::IdentMap::default();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let var_sizes = whole_variable_extents(&metadata2, &main_ident);
    let ctx = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );

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

    let var = Variable::Var {
        ident: Ident::new("x"),
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
        eqn: None,
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };

    let mut model_metadata: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>> =
        Default::default();
    model_metadata.insert(
        Ident::new("x"),
        VariableMetadata {
            offset: Some(0),
            size: 3,
            var: &var,
        },
    );
    let mut metadata = crate::common::IdentMap::default();
    let model_name = Ident::new("main");
    metadata.insert(model_name.clone(), model_metadata);

    let inputs = BTreeSet::new();
    let module_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();
    let dims_ctx = DimensionsContext::from(std::slice::from_ref(&datamodel_dim));
    let ident = Ident::new("test");
    let var_sizes = whole_variable_extents(&metadata, &model_name);
    let ctx = Context::new(
        ContextCore {
            dimensions: &dims,
            dimensions_ctx: &dims_ctx,
            model_name: &model_name,
            metadata: &metadata,
            var_sizes: &var_sizes,
            module_models: &module_models,
            inputs: &inputs,
        },
        &ident,
        false,
    );

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
                        .map(|mi| Ok(Expr::Var(ctx.get_ref(&mi.src)?, Loc::default())))
                        .collect::<Result<Vec<_>>>()?;
                    vec![Expr::EvalModule(
                        ident.clone(),
                        model_name.clone(),
                        input_set,
                        inputs,
                    )]
                }
                Variable::Stock { init_ast: ast, .. } => {
                    let base = ctx.get_base_ref(&Ident::new(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                let mut exprs = ctx.lower(ast)?;
                                let main_expr = exprs.pop().unwrap();
                                let main_expr =
                                    hoist_nested_array_builtins_in_scalar(main_expr, &mut exprs);
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
                Variable::Var {
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
                            let main_expr =
                                hoist_nested_array_builtins_in_scalar(main_expr, &mut exprs);
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
        // single chokepoint every lowering path (monolithic Module::compile
        // and the salsa per-variable fragment path) funnels through -- so both
        // backends (VM and wasmgen) see the folded form.
        let ast: Vec<Expr> = ast.into_iter().map(fold::fold_constants).collect();
        check_stock_updates_are_emittable(&ast, var.ident())?;
        Ok(Var {
            ident: Ident::new(var.ident()),
            ast,
        })
    }
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
fn hoist_nested_array_builtins_in_scalar(main_expr: Expr, exprs: &mut Vec<Expr>) -> Expr {
    if is_array_producing_builtin(&main_expr) || !contains_array_producing_builtin(&main_expr) {
        return main_expr;
    }

    let mut temp_id = next_available_temp_id(exprs);
    let mut hoisted = Vec::new();
    let placeholder_view = ArrayView::contiguous(vec![1]);
    let rewritten = replace_nested_builtins_for_element(
        main_expr,
        0,
        &placeholder_view,
        &mut temp_id,
        &mut hoisted,
        true,
        NestedBuiltinArgMode::ScalarContext,
    );
    exprs.extend(hoisted);
    rewritten
}

/// Check if an expression is an array-producing builtin that needs whole-array
/// evaluation rather than per-element scalar evaluation.
fn is_array_producing_builtin(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::App(
            BuiltinFn::VectorElmMap(_, _)
                | BuiltinFn::VectorSortOrder(_, _)
                | BuiltinFn::Rank(_, _)
                | BuiltinFn::AllocateAvailable(_, _, _)
                | BuiltinFn::AllocateByPriority(_, _, _, _, _),
            _
        )
    )
}

/// Extract the output ArrayView from an expression.  For array-producing builtins, the
/// output dimensions come from the builtin's "shaping" argument:
///   VectorElmMap(_, offset)    -> offset's view
///   VectorSortOrder(arr, _)    -> arr's view
///   AllocateAvailable(req,_,_) -> req's view
fn find_expr_array_view(expr: &Expr) -> Option<ArrayView> {
    match expr {
        Expr::StaticSubscript(_, view, _) | Expr::TempArray(_, view, _) => Some(view.clone()),
        Expr::App(builtin, _) => match builtin {
            BuiltinFn::VectorElmMap(_, offset) => find_expr_array_view(offset),
            BuiltinFn::VectorSortOrder(arr, _) | BuiltinFn::Rank(arr, _) => {
                find_expr_array_view(arr)
            }
            BuiltinFn::AllocateAvailable(req, _, _)
            | BuiltinFn::AllocateByPriority(req, _, _, _, _) => find_expr_array_view(req),
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

/// Offset all temp-related IDs (AssignTemp, TempArray, TempArrayElement) in an
/// expression by `offset` so they don't collide with previously emitted temps.
fn remap_temp_ids(expr: Expr, offset: u32) -> Expr {
    if offset == 0 {
        return expr;
    }
    match expr {
        Expr::AssignTemp(id, inner, view) => {
            Expr::AssignTemp(id + offset, Box::new(remap_temp_ids(*inner, offset)), view)
        }
        Expr::TempArray(id, view, loc) => Expr::TempArray(id + offset, view, loc),
        Expr::TempArrayElement(id, view, idx, loc) => {
            Expr::TempArrayElement(id + offset, view, idx, loc)
        }
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(remap_temp_ids(*lhs, offset)),
            Box::new(remap_temp_ids(*rhs, offset)),
            loc,
        ),
        Expr::Op1(op, inner, loc) => Expr::Op1(op, Box::new(remap_temp_ids(*inner, offset)), loc),
        Expr::If(cond, t, f, loc) => Expr::If(
            Box::new(remap_temp_ids(*cond, offset)),
            Box::new(remap_temp_ids(*t, offset)),
            Box::new(remap_temp_ids(*f, offset)),
            loc,
        ),
        Expr::App(builtin, loc) => Expr::App(builtin.map(|e| remap_temp_ids(e, offset)), loc),
        Expr::AssignCurr(off, inner) => {
            Expr::AssignCurr(off, Box::new(remap_temp_ids(*inner, offset)))
        }
        Expr::AssignNext(off, inner) => {
            Expr::AssignNext(off, Box::new(remap_temp_ids(*inner, offset)))
        }
        Expr::Subscript(off, indices, dim_sizes, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    SubscriptIndex::Single(e) => SubscriptIndex::Single(remap_temp_ids(e, offset)),
                    SubscriptIndex::Range(lo, hi) => SubscriptIndex::Range(
                        remap_temp_ids(lo, offset),
                        remap_temp_ids(hi, offset),
                    ),
                })
                .collect();
            Expr::Subscript(off, indices, dim_sizes, loc)
        }
        Expr::EvalModule(ident, model, inputs, args) => {
            let args = args
                .into_iter()
                .map(|e| remap_temp_ids(e, offset))
                .collect();
            Expr::EvalModule(ident, model, inputs, args)
        }
        other => other,
    }
}

/// Find the highest temp ID referenced in an expression, if any.
fn find_max_temp_id(expr: &Expr) -> Option<u32> {
    match expr {
        Expr::AssignTemp(id, inner, _) => Some((*id).max(find_max_temp_id(inner).unwrap_or(*id))),
        Expr::TempArray(id, _, _) | Expr::TempArrayElement(id, _, _, _) => Some(*id),
        Expr::Op2(_, lhs, rhs, _) => match (find_max_temp_id(lhs), find_max_temp_id(rhs)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, None) | (None, a) => a,
        },
        Expr::Op1(_, inner, _) | Expr::AssignCurr(_, inner) | Expr::AssignNext(_, inner) => {
            find_max_temp_id(inner)
        }
        Expr::If(cond, t, f, _) => [
            find_max_temp_id(cond),
            find_max_temp_id(t),
            find_max_temp_id(f),
        ]
        .into_iter()
        .flatten()
        .max(),
        Expr::App(builtin, _) => {
            let mut max_id = None;
            builtin.for_each_expr_ref(|e| {
                if let Some(id) = find_max_temp_id(e) {
                    max_id = Some(max_id.map_or(id, |m: u32| m.max(id)));
                }
            });
            max_id
        }
        Expr::Subscript(_, indices, _, _) => {
            let mut max_id = None;
            for idx in indices {
                let ids = match idx {
                    SubscriptIndex::Single(e) => [find_max_temp_id(e), None],
                    SubscriptIndex::Range(lo, hi) => [find_max_temp_id(lo), find_max_temp_id(hi)],
                };
                for id in ids.into_iter().flatten() {
                    max_id = Some(max_id.map_or(id, |m: u32| m.max(id)));
                }
            }
            max_id
        }
        Expr::EvalModule(_, _, _, args) => {
            let mut max_id = None;
            for arg in args {
                if let Some(id) = find_max_temp_id(arg) {
                    max_id = Some(max_id.map_or(id, |m: u32| m.max(id)));
                }
            }
            max_id
        }
        _ => None,
    }
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
        Expr::App(builtin, _) => builtin_contains_array_producing(builtin),
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

fn builtin_contains_array_producing(builtin: &BuiltinFn) -> bool {
    let mut found = false;
    builtin.for_each_expr_ref(|e| {
        if !found && contains_array_producing_builtin(e) {
            found = true;
        }
    });
    found
}

/// Replace array-producing builtins in an expression tree with
/// TempArrayElement references. Each nested builtin's index is projected
/// from the variable's element index using that builtin's own ArrayView,
/// handling the case where nested builtins operate on different dimensions.
/// On the first call (element 0), collects the hoisted AssignTemp expressions.
/// On subsequent calls, only performs the replacement using the same temp IDs.
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

fn replace_nested_builtins_for_element(
    expr: Expr,
    var_idx: usize,
    var_view: &ArrayView,
    temp_id: &mut u32,
    hoisted: &mut Vec<Expr>,
    collect_hoisted: bool,
    arg_mode: NestedBuiltinArgMode,
) -> Expr {
    if is_array_producing_builtin(&expr) {
        if matches!(arg_mode, NestedBuiltinArgMode::ScalarContext) {
            return expr;
        }
        let id = *temp_id;
        *temp_id += 1;
        let loc = expr.get_loc();
        let builtin_view = find_expr_array_view(&expr).unwrap_or_else(|| var_view.clone());
        if collect_hoisted {
            hoisted.push(Expr::AssignTemp(id, Box::new(expr), builtin_view.clone()));
        }
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
                *lhs,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
                arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *rhs,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
                arg_mode,
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
                arg_mode,
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
                arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *t,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
                arg_mode,
            )),
            Box::new(replace_nested_builtins_for_element(
                *f,
                var_idx,
                var_view,
                temp_id,
                hoisted,
                collect_hoisted,
                arg_mode,
            )),
            loc,
        ),
        // Descend into builtin arguments while preserving whether each argument
        // expects a scalar element or a full array value.
        Expr::App(builtin, loc) => {
            let scalar_child_mode = arg_mode.scalar_child_mode();
            let rewritten = match builtin {
                BuiltinFn::Sum(arg) => {
                    BuiltinFn::Sum(Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )))
                }
                BuiltinFn::Stddev(arg) => {
                    BuiltinFn::Stddev(Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )))
                }
                BuiltinFn::Size(arg) => {
                    BuiltinFn::Size(Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )))
                }
                BuiltinFn::Max(arg, None) => BuiltinFn::Max(
                    Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )),
                    None,
                ),
                BuiltinFn::Min(arg, None) => BuiltinFn::Min(
                    Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )),
                    None,
                ),
                BuiltinFn::Mean(args) if args.len() == 1 => {
                    let mut it = args.into_iter();
                    let arg = it.next().expect("Mean(args) len checked");
                    BuiltinFn::Mean(vec![replace_nested_builtins_for_element(
                        arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )])
                }
                BuiltinFn::Rank(arg, direction) => BuiltinFn::Rank(
                    Box::new(replace_nested_builtins_for_element(
                        *arg,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )),
                    Box::new(replace_nested_builtins_for_element(
                        *direction,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        scalar_child_mode,
                    )),
                ),
                BuiltinFn::VectorSelect(selection, expr, max_value, action, error_handling) => {
                    BuiltinFn::VectorSelect(
                        Box::new(replace_nested_builtins_for_element(
                            *selection,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *expr,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *max_value,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *action,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *error_handling,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                    )
                }
                BuiltinFn::VectorElmMap(source, offsets) => BuiltinFn::VectorElmMap(
                    Box::new(replace_nested_builtins_for_element(
                        *source,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )),
                    Box::new(replace_nested_builtins_for_element(
                        *offsets,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        NestedBuiltinArgMode::ArrayValue,
                    )),
                ),
                BuiltinFn::VectorSortOrder(array_expr, direction_expr) => {
                    BuiltinFn::VectorSortOrder(
                        Box::new(replace_nested_builtins_for_element(
                            *array_expr,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *direction_expr,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                    )
                }
                BuiltinFn::AllocateAvailable(requests, profile, avail) => {
                    BuiltinFn::AllocateAvailable(
                        Box::new(replace_nested_builtins_for_element(
                            *requests,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *profile,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *avail,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                    )
                }
                BuiltinFn::AllocateByPriority(requests, priority, size, width, supply) => {
                    BuiltinFn::AllocateByPriority(
                        Box::new(replace_nested_builtins_for_element(
                            *requests,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *priority,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            NestedBuiltinArgMode::ArrayValue,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *size,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *width,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                        Box::new(replace_nested_builtins_for_element(
                            *supply,
                            var_idx,
                            var_view,
                            temp_id,
                            hoisted,
                            collect_hoisted,
                            scalar_child_mode,
                        )),
                    )
                }
                other => other.map(|sub_expr| {
                    replace_nested_builtins_for_element(
                        sub_expr,
                        var_idx,
                        var_view,
                        temp_id,
                        hoisted,
                        collect_hoisted,
                        arg_mode,
                    )
                }),
            };
            Expr::App(rewritten, loc)
        }
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
///
fn expand_arrayed_with_hoisting(
    ctx: &Context,
    dims: &[Dimension],
    elements: &HashMap<CanonicalElementName, crate::ast::Expr2>,
    default_ast: Option<&crate::ast::Expr2>,
    apply_default_for_missing: bool,
    base: &VarRef,
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
            base,
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
                                exprs
                                    .push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
                                exprs
                            });
                        }
                        return Ok(vec![Expr::AssignCurr(
                            base.offset_by(i),
                            Box::new(Expr::Const(0.0, Loc::default())),
                        )]);
                    }
                };
                let ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                ctx.lower(ast).map(|mut exprs| {
                    let main_expr = exprs.pop().unwrap();
                    exprs.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
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
    base: &VarRef,
) -> Result<Vec<Expr>> {
    let active_dims = Arc::<[Dimension]>::from(dims.to_vec());

    // Lower once using element 0's subscripts to detect the expression shape.
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
        return expand_a2a_hoisted(ctx, dims, ast, base, &active_dims, first_exprs, main_expr);
    }

    // Not an array-producing builtin: fall back to the standard per-element loop.
    // We already lowered element 0, so start from there.
    first_exprs.push(Expr::AssignCurr(base.clone(), Box::new(main_expr)));
    let rest: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
        .enumerate()
        .skip(1)
        .map(|(i, subscripts)| {
            let ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            ctx.lower(ast).map(|mut exprs| {
                let main_expr = exprs.pop().unwrap();
                exprs.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
                exprs
            })
        })
        .collect();
    let mut all_exprs = first_exprs;
    all_exprs.extend(rest?.into_iter().flatten());
    Ok(all_exprs)
}

/// Detect whether an array-producing builtin's lowered expression depends on
/// the active A2A dimension (e.g. `dir[D]` varies per element). Compares the
/// lowered expression for element 0 against element 1 and the last element to
/// handle both single- and multi-dimensional cases.
fn expression_depends_on_active_dimension(
    ctx: &Context,
    dims: &[Dimension],
    ast: &crate::ast::Expr2,
    active_dims: &Arc<[Dimension]>,
    reference_expr: &Expr,
) -> Result<bool> {
    // Compare element 0's lowered expression against every other element.
    // Early-exits on the first mismatch, so dimension-dependent expressions
    // are typically O(1). For dimension-independent expressions, the O(N)
    // cost is acceptable at compile time since SD model arrays are small.
    for subscripts in SubscriptIterator::new(dims).skip(1) {
        let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
        let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
        let elem_main = elem_exprs.pop().unwrap();
        if *reference_expr != elem_main {
            return Ok(true);
        }
    }
    Ok(false)
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
            expression_depends_on_active_dimension(ctx, dims, ast, active_dims, &main_expr)?;

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
            expression_depends_on_active_dimension(ctx, dims, ast, active_dims, &main_expr)?;

        let base_temp_id = next_available_temp_id(&first_exprs);
        let var_view = array_view_from_dims(dims);
        let mut result = first_exprs;

        if needs_per_element {
            // Scalar args vary by element: each element gets its own
            // AssignTemp blocks so the nested builtin is re-evaluated.
            let mut temp_id = base_temp_id;
            for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                let elem_main = elem_exprs.pop().unwrap();

                // Preserve pre-expressions; remap with a consistent base.
                let remap_base = temp_id;
                if !elem_exprs.is_empty() {
                    let remapped: Vec<_> = elem_exprs
                        .into_iter()
                        .map(|e| remap_temp_ids(e, remap_base))
                        .collect();
                    for e in &remapped {
                        if let Some(max) = find_max_temp_id(e) {
                            temp_id = temp_id.max(max + 1);
                        }
                    }
                    result.extend(remapped);
                }
                let elem_main = remap_temp_ids(elem_main, remap_base);
                if let Some(max) = find_max_temp_id(&elem_main) {
                    temp_id = temp_id.max(max + 1);
                }

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
                    &mut temp_id,
                    &mut hoisted,
                    true,
                    NestedBuiltinArgMode::ScalarElement,
                );
                result.extend(hoisted);
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
                    Box::new(elem_rewritten),
                ));
            }
        } else {
            // Scalar args are constant: hoist once from element 0, then
            // rewrite subsequent elements to read from the shared temps.
            let mut hoisted = Vec::new();
            let mut temp_id = base_temp_id;
            let rewritten = replace_nested_builtins_for_element(
                main_expr,
                0,
                &var_view,
                &mut temp_id,
                &mut hoisted,
                true,
                NestedBuiltinArgMode::ScalarElement,
            );
            result.extend(hoisted);
            result.push(Expr::AssignCurr(base.clone(), Box::new(rewritten)));

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
                    NestedBuiltinArgMode::ScalarElement,
                );
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
                    Box::new(elem_rewritten),
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
    let base_temp_id = next_available_temp_id(&first_exprs);

    let mut result = first_exprs;

    let mut next_tid = base_temp_id;

    for (i, subscripts) in SubscriptIterator::new(dims).enumerate() {
        let main_expr = if i == 0 {
            first_main_expr.clone()
        } else {
            let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
            let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
            let main = elem_exprs.pop().unwrap();
            // Preserve pre-expressions (e.g. intermediate temps from complex
            // subexpressions). Remap with a consistent base so temp refs in
            // the main expression stay aligned with pre-expression definitions.
            let remap_base = next_tid;
            if !elem_exprs.is_empty() {
                let remapped: Vec<_> = elem_exprs
                    .into_iter()
                    .map(|e| remap_temp_ids(e, remap_base))
                    .collect();
                for e in &remapped {
                    if let Some(max) = find_max_temp_id(e) {
                        next_tid = next_tid.max(max + 1);
                    }
                }
                result.extend(remapped);
            }
            let main = remap_temp_ids(main, remap_base);
            if let Some(max) = find_max_temp_id(&main) {
                next_tid = next_tid.max(max + 1);
            }
            main
        };

        // GH #578: a scalar-source ELM MAP with a per-element constant offset
        // folds to a direct slot read (or :NA:), so the array-producing opcode
        // -- which requires a *view* offset the constant cannot supply -- is
        // skipped entirely. If the fold collapses the whole element expression
        // to a scalar (no array-producing builtin left), emit it directly with
        // no temp consumed.
        let main_expr = fold_scalar_source_elm_maps(ctx, main_expr);
        if !is_array_producing_builtin(&main_expr) && !contains_array_producing_builtin(&main_expr)
        {
            result.push(Expr::AssignCurr(base.offset_by(i), Box::new(main_expr)));
            continue;
        }

        let temp_id = next_tid;
        next_tid = temp_id + 1;
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
    base: &VarRef,
    active_dims: &Arc<[Dimension]>,
    hoisting_ast: &crate::ast::Expr2,
) -> Result<Vec<Expr>> {
    let first_subscripts: Vec<String> = SubscriptIterator::new(dims).next().unwrap_or_default();
    let first_ctx = ctx.with_active_subscripts(active_dims.clone(), &first_subscripts);
    let mut first_exprs = first_ctx.lower_preserving_dimensions(hoisting_ast)?;
    let main_expr = first_exprs.pop().unwrap();
    let var_view = array_view_from_dims(dims);

    if contains_array_producing_builtin(&main_expr) {
        let hoisting_dim_dependent = expression_depends_on_active_dimension(
            ctx,
            dims,
            hoisting_ast,
            active_dims,
            &main_expr,
        )?;
        let base_temp_id = next_available_temp_id(&first_exprs);

        let mut result = first_exprs;
        let mut temp_id = base_temp_id;

        // When the hoisting expression's scalar args are constant, hoist once
        // and share the temps. When they depend on the active dimension, each
        // element gets its own AssignTemp blocks.
        let mut ast_temp_bases: HashMap<*const crate::ast::Expr2, u32> = HashMap::new();
        if !hoisting_dim_dependent {
            let mut hoisted = Vec::new();
            let _ = replace_nested_builtins_for_element(
                main_expr,
                0,
                &var_view,
                &mut temp_id,
                &mut hoisted,
                true,
                NestedBuiltinArgMode::ScalarElement,
            );
            result.extend(hoisted);
            ast_temp_bases.insert(hoisting_ast as *const _, base_temp_id);
        }

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

                // Check if this AST's expression depends on the active
                // dimension. For the hoisting_ast, this was pre-computed.
                // For overrides, probe on first encounter.
                let is_dim_dependent = if std::ptr::eq(ast, hoisting_ast) {
                    hoisting_dim_dependent
                } else if ast_temp_bases.contains_key(&ast_ptr) {
                    false
                } else {
                    let probe_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                    let mut probe_exprs = probe_ctx.lower_preserving_dimensions(ast)?;
                    let probe_main = probe_exprs.pop().unwrap();
                    expression_depends_on_active_dimension(
                        ctx,
                        dims,
                        ast,
                        active_dims,
                        &probe_main,
                    )?
                };

                if is_dim_dependent {
                    // Per-element hoisting: each element creates its own
                    // AssignTemp blocks.
                    let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                    let mut elem_exprs = elem_ctx.lower_preserving_dimensions(ast)?;
                    let elem_main = elem_exprs.pop().unwrap();

                    // Preserve pre-expressions; remap with a consistent base.
                    let remap_base = temp_id;
                    if !elem_exprs.is_empty() {
                        let remapped: Vec<_> = elem_exprs
                            .into_iter()
                            .map(|e| remap_temp_ids(e, remap_base))
                            .collect();
                        for e in &remapped {
                            if let Some(max) = find_max_temp_id(e) {
                                temp_id = temp_id.max(max + 1);
                            }
                        }
                        result.extend(remapped);
                    }
                    let elem_main = remap_temp_ids(elem_main, remap_base);
                    if let Some(max) = find_max_temp_id(&elem_main) {
                        temp_id = temp_id.max(max + 1);
                    }

                    let mut hoisted = Vec::new();
                    let elem_rewritten = replace_nested_builtins_for_element(
                        elem_main,
                        i,
                        &var_view,
                        &mut temp_id,
                        &mut hoisted,
                        true,
                        NestedBuiltinArgMode::ScalarElement,
                    );
                    result.extend(hoisted);
                    result.push(Expr::AssignCurr(
                        base.offset_by(i),
                        Box::new(elem_rewritten),
                    ));
                    continue;
                }

                // If this AST hasn't been seen before (different override),
                // emit its own AssignTemp blocks with fresh temp IDs.
                let elem_base_tid = if let Some(&tid) = ast_temp_bases.get(&ast_ptr) {
                    tid
                } else {
                    let disc_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                    let mut disc_exprs = disc_ctx.lower_preserving_dimensions(ast)?;
                    let disc_main = disc_exprs.pop().unwrap();
                    // lower_preserving_dimensions restarts temp IDs at 0;
                    // remap pre-expressions AND the main expression so they
                    // don't collide with previously emitted temps in [0, temp_id).
                    let disc_exprs: Vec<_> = disc_exprs
                        .into_iter()
                        .map(|e| remap_temp_ids(e, temp_id))
                        .collect();
                    let disc_main = remap_temp_ids(disc_main, temp_id);
                    for e in &disc_exprs {
                        if let Some(max) = find_max_temp_id(e) {
                            temp_id = temp_id.max(max + 1);
                        }
                    }
                    if let Some(max) = find_max_temp_id(&disc_main) {
                        temp_id = temp_id.max(max + 1);
                    }
                    result.extend(disc_exprs);
                    // new_base must be set AFTER advancing past remapped
                    // pre-expressions so subsequent elements' TempArrayElement
                    // reads align with the hoisted AssignTemp IDs.
                    let new_base = temp_id;
                    let mut new_hoisted = Vec::new();
                    let _ = replace_nested_builtins_for_element(
                        disc_main,
                        i,
                        &var_view,
                        &mut temp_id,
                        &mut new_hoisted,
                        true,
                        NestedBuiltinArgMode::ScalarElement,
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
                    NestedBuiltinArgMode::ScalarElement,
                );
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
                    Box::new(elem_rewritten),
                ));
            } else if let Some(ast) = elem_ast {
                let elem_ctx = ctx.with_active_subscripts(active_dims.clone(), &subscripts);
                let mut elem_exprs = elem_ctx.lower(ast)?;
                let elem_main = elem_exprs.pop().unwrap();
                // lower() restarts temp IDs at 0 for each call. Remap any
                // temp IDs produced here so they don't collide with the
                // hoisted temps that occupy IDs [0, temp_id).
                let elem_exprs: Vec<_> = elem_exprs
                    .into_iter()
                    .map(|e| remap_temp_ids(e, temp_id))
                    .collect();
                let elem_main = remap_temp_ids(elem_main, temp_id);
                // Advance temp_id past any remapped IDs
                for e in &elem_exprs {
                    if let Some(max) = find_max_temp_id(e) {
                        temp_id = temp_id.max(max + 1);
                    }
                }
                if let Some(max) = find_max_temp_id(&elem_main) {
                    temp_id = temp_id.max(max + 1);
                }
                result.extend(elem_exprs);
                result.push(Expr::AssignCurr(base.offset_by(i), Box::new(elem_main)));
            } else {
                result.push(Expr::AssignCurr(
                    base.offset_by(i),
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
        BuiltinFn::Rank(a, direction) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(direction, temp_sizes_map);
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
        BuiltinFn::AllocateByPriority(a, b, c, d, e) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            extract_temp_sizes(c, temp_sizes_map);
            extract_temp_sizes(d, temp_sizes_map);
            extract_temp_sizes(e, temp_sizes_map);
        }
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime
        | BuiltinFn::IsModuleInput(_, _) => {}
        // Scalar lag/initial builtins
        BuiltinFn::Previous(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
        }
        BuiltinFn::Init(expr) => {
            extract_temp_sizes(expr, temp_sizes_map);
        }
    }
}

/// Per-variable initial expressions, kept alongside the flat runlist.
///
/// A whole model, compiled as one unit.
///
/// **`#[cfg(test)]`, and that gate is a load-bearing assertion, not tidiness.**
/// Production compilation is per-variable and incremental: every fragment
/// emitter builds a [`ModuleCtx`] and calls codegen directly (GH #964). Until
/// this change three production sites built a stand-in one-variable `Module`
/// by struct literal and deep-cloned five fields into it per phase. Gating the
/// type makes "no production `compiler::Module` literal remains" a compile
/// error rather than a claim someone has to re-audit.
///
/// Every field here is one codegen reads; see [`ModuleCtx`], which is exactly
/// the set of things `Compiler` looks at and which [`Module::compile`] hands
/// it by reference. `runlist_initials` is the exception -- codegen compiles
/// initials per variable out of `runlist_initials_by_var`, and the flat list
/// exists only for the `get_initial_exprs` accessor.
#[cfg(test)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct Module {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) inputs: BTreeSet<Ident<Canonical>>,
    pub(crate) temp_sizes: Vec<usize>,
    #[allow(dead_code)]
    pub(crate) runlist_initials: Vec<Expr>,
    pub(crate) runlist_initials_by_var: Vec<Var>,
    pub(crate) runlist_flows: Vec<Expr>,
    pub(crate) runlist_stocks: Vec<Expr>,
    /// The whole-model layout the emitted symbolic module is resolved against.
    /// This is the monolithic path's analogue of the salsa `compute_layout`
    /// query, and it is consulted exactly once, at the end of `compile()`.
    pub(crate) layout: crate::compiler::symbolic::VariableLayout,
    pub(crate) var_sizes: VarSizes,
    pub(crate) tables: HashMap<Ident<Canonical>, Vec<Table>>,
    pub(crate) dimensions: Vec<Dimension>,
    pub(crate) dimensions_ctx: DimensionsContext,
}

#[cfg(test)]
pub(crate) fn calc_module_model_map(
    project: &Project,
    model_name: &Ident<Canonical>,
) -> crate::common::IdentMap<
    Ident<Canonical>,
    crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
> {
    let mut all_models: crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>>,
    > = Default::default();

    let model = Arc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    let mut current_mapping: crate::common::IdentMap<Ident<Canonical>, Ident<Canonical>> =
        Default::default();

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

#[cfg(test)]
pub(crate) fn build_metadata<'p>(
    project: &'p Project,
    model_name: &Ident<Canonical>,
    is_root: bool,
    all_offsets: &mut crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'p>>,
    >,
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
    let mut offsets: crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'p>> =
        crate::common::IdentMap::with_capacity_and_hasher(var_count, Default::default());

    let mut i = 0;
    if is_root {
        offsets.insert(
            Ident::new("time"),
            VariableMetadata {
                offset: Some(0),
                size: 1,
                var: &IMPLICIT_TIME,
            },
        );
        offsets.insert(
            Ident::new("dt"),
            VariableMetadata {
                offset: Some(1),
                size: 1,
                var: &IMPLICIT_DT,
            },
        );
        offsets.insert(
            Ident::new("initial_time"),
            VariableMetadata {
                offset: Some(2),
                size: 1,
                var: &IMPLICIT_INITIAL_TIME,
            },
        );
        offsets.insert(
            Ident::new("final_time"),
            VariableMetadata {
                offset: Some(3),
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
                offset: Some(i),
                size,
                var: &model.variables[canonical_ident],
            },
        );
        i += size;
    }

    all_offsets.insert(model_name.clone(), offsets);
}

#[cfg(test)]
fn calc_n_slots(
    all_metadata: &crate::common::IdentMap<
        Ident<Canonical>,
        crate::common::IdentMap<Ident<Canonical>, VariableMetadata<'_>>,
    >,
    model_name: &Ident<Canonical>,
) -> usize {
    let metadata = &all_metadata[model_name];

    metadata.values().map(|v| v.size).sum()
}

#[cfg(test)]
impl Module {
    /// Borrow this whole-model unit as the emission context codegen consumes.
    /// Every field is a reference into `self`; nothing is copied.
    pub(crate) fn as_ctx(&self) -> ModuleCtx<'_> {
        ModuleCtx {
            ident: &self.ident,
            inputs: &self.inputs,
            temp_sizes: &self.temp_sizes,
            runlist_initials_by_var: &self.runlist_initials_by_var,
            runlist_flows: &self.runlist_flows,
            runlist_stocks: &self.runlist_stocks,
            var_sizes: &self.var_sizes,
            tables: &self.tables,
            dimensions: &self.dimensions,
            dimensions_ctx: &self.dimensions_ctx,
        }
    }

    /// Emit this whole model and resolve it against its own layout.
    ///
    /// Codegen is address-neutral, so the monolithic path reaches concrete
    /// bytecode the same way production assembly does: through
    /// `resolve_module`. That is the whole reason one emitter can serve both --
    /// there is no second, offset-emitting codegen to keep in step.
    pub fn compile(&self) -> Result<CompiledModule> {
        let sym = self.as_ctx().compile()?;
        crate::compiler::symbolic::resolve_module(&sym, &self.layout)
            .map_err(|msg| Error::new(ErrorKind::Simulation, ErrorCode::NotSimulatable, Some(msg)))
    }
}

#[cfg(test)]
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
        let mut metadata: crate::common::IdentMap<_, _> =
            crate::common::IdentMap::with_capacity_and_hasher(
                project.models.len(),
                Default::default(),
            );
        build_metadata(project, model_name, is_root, &mut metadata);

        let n_slots = calc_n_slots(&metadata, model_name);
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            var_names.sort_unstable();
            var_names
        };
        let module_models = calc_module_model_map(project, model_name);

        let converted_dims: Vec<Dimension> = project
            .datamodel
            .dimensions
            .iter()
            .map(Dimension::from)
            .collect();

        // Built once and shared by lowering (below) and emission (`compile`,
        // through `as_ctx`), so the two agree about where a VECTOR ELM MAP
        // source's storage ends.
        let var_sizes: VarSizes = whole_variable_extents(&metadata, model_name);

        let build_var = |ident: &Ident<Canonical>, is_initial| {
            Var::new(
                &Context::new(
                    ContextCore {
                        dimensions: &converted_dims,
                        dimensions_ctx: &project.dimensions_ctx,
                        model_name,
                        metadata: &metadata,
                        var_sizes: &var_sizes,
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

        // Per-variable initials, kept alongside the flattened runlist. The
        // `CompiledInitial::offsets` these used to carry are re-derived from
        // the resolved bytecode's `AssignCurr` operands by `resolve_module`,
        // which is the same set (every `Expr::AssignCurr` emits exactly one
        // current-value write opcode for its target).
        let runlist_initials_by_var: Vec<Var> = initial_vars.to_vec();

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
        let mut temp_sizes: Vec<usize> = vec![0; temp_sizes_map.len()];
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

        let model_metadata = &metadata[model_name];
        let layout = crate::compiler::symbolic::VariableLayout::from_offset_map(
            &model_metadata
                .iter()
                .filter_map(|(k, v)| v.offset.map(|off| (k.clone(), (off, v.size))))
                .collect(),
            n_slots,
        );

        Ok(Module {
            ident: model_name.clone(),
            inputs: inputs.clone(),
            temp_sizes,
            runlist_initials,
            runlist_initials_by_var,
            runlist_flows,
            runlist_stocks,
            layout,
            var_sizes,
            tables,
            dimensions: converted_dims,
            dimensions_ctx: project.dimensions_ctx.clone(),
        })
    }
}

#[cfg(test)]
impl Module {
    /// Get flow expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions that target offsets within this variable's range.
    pub fn get_flow_exprs(&self, var_name: &str) -> Vec<&Expr> {
        let canonical_name = Ident::new(var_name);
        self.runlist_flows
            .iter()
            .filter(|expr| matches!(expr, Expr::AssignCurr(dst, _) if dst.name == canonical_name))
            .collect()
    }

    /// Get initial expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions in the initials runlist for this variable.
    pub fn get_initial_exprs(&self, var_name: &str) -> Vec<&Expr> {
        let canonical_name = Ident::new(var_name);
        self.runlist_initials
            .iter()
            .filter(|expr| matches!(expr, Expr::AssignCurr(dst, _) if dst.name == canonical_name))
            .collect()
    }
}
