// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compile-time constant folding over the lowered [`Expr`] IR.
//!
//! Folds constant-only subtrees into a single `Expr::Const` so the VM (and the
//! wasm backend, which lowers the same compiled bytecode) never re-evaluates
//! `literal op literal` at runtime. On C-LEARN the pre-fold bytecode carried
//! ~800 such sites in the per-timestep flow program, including one for every
//! negative literal (unary minus lowers to `0 - x` in
//! `Context::lower_from_expr3`).
//!
//! Two hard rules keep this pass bit-exact with the unfolded program:
//!
//! 1. **Fold with the VM's own semantics.** Arithmetic goes through
//!    [`crate::vm::eval_op2`] and truthiness through [`crate::vm::is_truthy`],
//!    so a folded result is the same f64 the interpreter would have produced
//!    (including `Eq`'s ULP-based `approx_eq` and `Mod`'s `rem_euclid`).
//! 2. **Only IEEE-exact operations fold.** `^` (`Op2::Exp` -> `powf`) and the
//!    transcendental builtins call platform libm, whose results may differ
//!    across platforms; folding them would bake a platform-dependent literal
//!    into the (otherwise platform-deterministic) compiled artifact and the
//!    wasm blob. They are left for runtime, where each backend computes them
//!    with its own (tested) implementation.
//!
//! No algebraic rewrites are performed: `x * (2*3)` folds the `2*3` leaf, but
//! `(x*2)*3` is left alone -- f64 multiplication is non-associative, so
//! reassociation would change bits.

use crate::ast::BinaryOp;
use crate::bytecode::Op2;
use crate::vm::{eval_op2, is_truthy};

use super::dimensions::UnaryOp;
use super::expr::{Expr, SubscriptIndex};

/// Evaluate a binary op over two constants with the VM's exact runtime
/// semantics, or `None` if the operator is excluded from folding (`^`, whose
/// `powf` is platform libm and must stay a runtime computation).
fn eval_const_binary_op(op: BinaryOp, l: f64, r: f64) -> Option<f64> {
    let result = match op {
        BinaryOp::Add => eval_op2(Op2::Add, l, r),
        BinaryOp::Sub => eval_op2(Op2::Sub, l, r),
        BinaryOp::Mul => eval_op2(Op2::Mul, l, r),
        BinaryOp::Div => eval_op2(Op2::Div, l, r),
        BinaryOp::Mod => eval_op2(Op2::Mod, l, r),
        BinaryOp::Gt => eval_op2(Op2::Gt, l, r),
        BinaryOp::Gte => eval_op2(Op2::Gte, l, r),
        BinaryOp::Lt => eval_op2(Op2::Lt, l, r),
        BinaryOp::Lte => eval_op2(Op2::Lte, l, r),
        BinaryOp::Eq => eval_op2(Op2::Eq, l, r),
        // Neq has no Op2 form: codegen emits `Eq` then `Not`, so fold the
        // same composition.
        BinaryOp::Neq => {
            if is_truthy(eval_op2(Op2::Eq, l, r)) {
                0.0
            } else {
                1.0
            }
        }
        BinaryOp::And => eval_op2(Op2::And, l, r),
        BinaryOp::Or => eval_op2(Op2::Or, l, r),
        // powf is platform libm: folding would make compiled output (and the
        // emitted wasm blob) platform-dependent. Keep it a runtime op.
        BinaryOp::Exp => return None,
    };
    Some(result)
}

/// Recursively fold constant subtrees in a lowered expression.
///
/// Children are always folded first; a node folds itself only when every
/// operand it needs is a constant and its operation is IEEE-exact (see module
/// docs). Locations are preserved on folded nodes (the folded constant takes
/// the whole expression's loc).
pub(crate) fn fold_constants(expr: Expr) -> Expr {
    match expr {
        Expr::Const(..)
        | Expr::Var(..)
        | Expr::StaticSubscript(..)
        | Expr::TempArray(..)
        | Expr::TempArrayElement(..)
        | Expr::Dt(..)
        | Expr::ModuleInput(..) => expr,
        Expr::Subscript(off, indices, bounds, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    SubscriptIndex::Single(e) => SubscriptIndex::Single(fold_constants(e)),
                    SubscriptIndex::Range(s, e) => {
                        SubscriptIndex::Range(fold_constants(s), fold_constants(e))
                    }
                })
                .collect();
            Expr::Subscript(off, indices, bounds, loc)
        }
        Expr::App(builtin, loc) => {
            // Fold within arguments; the builtin application itself is left
            // for runtime (transcendentals are platform libm, and the cheap
            // exact ones -- ABS/INT/MIN/MAX -- are not worth a special case
            // until measurement says otherwise).
            Expr::App(builtin.map(fold_constants), loc)
        }
        Expr::EvalModule(ident, model_name, input_set, args) => {
            let args = args.into_iter().map(fold_constants).collect();
            Expr::EvalModule(ident, model_name, input_set, args)
        }
        Expr::Op2(op, l, r, loc) => {
            let l = fold_constants(*l);
            let r = fold_constants(*r);
            if let (Expr::Const(lv, _), Expr::Const(rv, _)) = (&l, &r)
                && let Some(folded) = eval_const_binary_op(op, *lv, *rv)
            {
                return Expr::Const(folded, loc);
            }
            Expr::Op2(op, Box::new(l), Box::new(r), loc)
        }
        Expr::Op1(op, r, loc) => {
            let r = fold_constants(*r);
            if let (UnaryOp::Not, Expr::Const(rv, _)) = (op, &r) {
                // Mirrors the VM's Opcode::Not: `(!is_truthy(r)) as i8 as f64`.
                return Expr::Const((!is_truthy(*rv)) as i8 as f64, loc);
            }
            Expr::Op1(op, Box::new(r), loc)
        }
        Expr::If(cond, t, f, loc) => {
            let cond = fold_constants(*cond);
            let t = fold_constants(*t);
            let f = fold_constants(*f);
            if let Expr::Const(cv, _) = cond {
                // Mirrors the VM's SetCond/If pair: select on truthiness. Both
                // branches were just folded, and the discarded branch cannot
                // affect the selected value (the VM evaluates both eagerly and
                // discards one), so dropping it is value-preserving.
                return if is_truthy(cv) { t } else { f };
            }
            Expr::If(Box::new(cond), Box::new(t), Box::new(f), loc)
        }
        Expr::AssignCurr(off, rhs) => Expr::AssignCurr(off, Box::new(fold_constants(*rhs))),
        Expr::AssignNext(off, rhs) => Expr::AssignNext(off, Box::new(fold_constants(*rhs))),
        Expr::AssignTemp(id, rhs, view) => {
            Expr::AssignTemp(id, Box::new(fold_constants(*rhs)), view)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Loc;
    use crate::builtins::BuiltinFn;

    fn c(v: f64) -> Expr {
        Expr::Const(v, Loc::default())
    }

    fn op2(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::Op2(op, Box::new(l), Box::new(r), Loc::default())
    }

    fn var(off: usize) -> Expr {
        Expr::Var(off, Loc::default())
    }

    fn assert_folds_to(expr: Expr, expected: f64) {
        match fold_constants(expr) {
            Expr::Const(v, _) => {
                assert_eq!(
                    v.to_bits(),
                    expected.to_bits(),
                    "folded to {v}, expected {expected}"
                );
            }
            other => panic!("expected Const, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn folds_arithmetic() {
        assert_folds_to(op2(BinaryOp::Add, c(2.0), c(3.0)), 5.0);
        assert_folds_to(op2(BinaryOp::Sub, c(2.0), c(3.0)), -1.0);
        assert_folds_to(op2(BinaryOp::Mul, c(2.0), c(3.0)), 6.0);
        assert_folds_to(op2(BinaryOp::Div, c(1.0), c(8.0)), 0.125);
        // Mod matches the VM's rem_euclid semantics.
        assert_folds_to(op2(BinaryOp::Mod, c(7.0), c(3.0)), 1.0);
        assert_folds_to(op2(BinaryOp::Mod, c(-7.0), c(3.0)), 2.0);
    }

    #[test]
    fn folds_unary_negative_lowering() {
        // Unary minus lowers to `0 - x` (Context::lower_from_expr3), so a
        // negative literal arrives here as Sub(Const(0), Const(x)).
        assert_folds_to(op2(BinaryOp::Sub, c(0.0), c(2.5)), -2.5);
    }

    #[test]
    fn folds_nested_cascade() {
        // (1 + 2) * 2 -> 6
        let inner = op2(BinaryOp::Add, c(1.0), c(2.0));
        assert_folds_to(op2(BinaryOp::Mul, inner, c(2.0)), 6.0);
    }

    #[test]
    fn folds_div_by_zero_like_runtime() {
        assert_folds_to(op2(BinaryOp::Div, c(1.0), c(0.0)), f64::INFINITY);
        match fold_constants(op2(BinaryOp::Div, c(0.0), c(0.0))) {
            Expr::Const(v, _) => assert!(v.is_nan()),
            _ => panic!("expected Const NaN"),
        }
    }

    #[test]
    fn folds_comparisons_and_logic() {
        assert_folds_to(op2(BinaryOp::Gt, c(2.0), c(1.0)), 1.0);
        assert_folds_to(op2(BinaryOp::Lt, c(2.0), c(1.0)), 0.0);
        assert_folds_to(op2(BinaryOp::Gte, c(2.0), c(2.0)), 1.0);
        assert_folds_to(op2(BinaryOp::Lte, c(3.0), c(2.0)), 0.0);
        assert_folds_to(op2(BinaryOp::Eq, c(1.0), c(1.0)), 1.0);
        assert_folds_to(op2(BinaryOp::Neq, c(1.0), c(2.0)), 1.0);
        assert_folds_to(op2(BinaryOp::Neq, c(1.0), c(1.0)), 0.0);
        assert_folds_to(op2(BinaryOp::And, c(1.0), c(0.0)), 0.0);
        assert_folds_to(op2(BinaryOp::Or, c(1.0), c(0.0)), 1.0);
    }

    #[test]
    fn exp_is_not_folded() {
        // powf is platform libm; the operator must survive to runtime.
        let expr = op2(BinaryOp::Exp, c(2.0), c(3.0));
        match fold_constants(expr) {
            Expr::Op2(BinaryOp::Exp, l, r, _) => {
                assert!(matches!(*l, Expr::Const(v, _) if v == 2.0));
                assert!(matches!(*r, Expr::Const(v, _) if v == 3.0));
            }
            _ => panic!("Exp must not fold"),
        }
    }

    #[test]
    fn no_reassociation_through_variables() {
        // (x * 2) * 3: the inner node has a Var operand, so nothing folds --
        // and we must NOT reassociate to x * (2*3).
        let expr = op2(BinaryOp::Mul, op2(BinaryOp::Mul, var(0), c(2.0)), c(3.0));
        match fold_constants(expr) {
            Expr::Op2(BinaryOp::Mul, l, r, _) => {
                assert!(matches!(*r, Expr::Const(v, _) if v == 3.0));
                match *l {
                    Expr::Op2(BinaryOp::Mul, ll, lr, _) => {
                        assert!(matches!(*ll, Expr::Var(0, _)));
                        assert!(matches!(*lr, Expr::Const(v, _) if v == 2.0));
                    }
                    _ => panic!("inner Mul must be preserved"),
                }
            }
            _ => panic!("outer Mul must be preserved"),
        }
    }

    #[test]
    fn folds_const_subtree_under_variable_op() {
        // (2 * 3) * x -> 6 * x
        let expr = op2(BinaryOp::Mul, op2(BinaryOp::Mul, c(2.0), c(3.0)), var(0));
        match fold_constants(expr) {
            Expr::Op2(BinaryOp::Mul, l, r, _) => {
                assert!(matches!(*l, Expr::Const(v, _) if v == 6.0));
                assert!(matches!(*r, Expr::Var(0, _)));
            }
            _ => panic!("expected Mul(Const(6), Var)"),
        }
    }

    #[test]
    fn folds_not() {
        let expr = Expr::Op1(UnaryOp::Not, Box::new(c(0.0)), Loc::default());
        assert_folds_to(expr, 1.0);
        let expr = Expr::Op1(UnaryOp::Not, Box::new(c(2.0)), Loc::default());
        assert_folds_to(expr, 0.0);
    }

    #[test]
    fn folds_if_with_const_condition() {
        let expr = Expr::If(
            Box::new(op2(BinaryOp::Gt, c(2.0), c(1.0))),
            Box::new(var(1)),
            Box::new(var(2)),
            Loc::default(),
        );
        assert!(matches!(fold_constants(expr), Expr::Var(1, _)));

        let expr = Expr::If(
            Box::new(c(0.0)),
            Box::new(var(1)),
            Box::new(op2(BinaryOp::Add, c(1.0), c(1.0))),
            Loc::default(),
        );
        assert!(matches!(fold_constants(expr), Expr::Const(v, _) if v == 2.0));
    }

    #[test]
    fn if_with_dynamic_condition_folds_branches_only() {
        let expr = Expr::If(
            Box::new(var(0)),
            Box::new(op2(BinaryOp::Add, c(1.0), c(1.0))),
            Box::new(var(2)),
            Loc::default(),
        );
        match fold_constants(expr) {
            Expr::If(cond, t, f, _) => {
                assert!(matches!(*cond, Expr::Var(0, _)));
                assert!(matches!(*t, Expr::Const(v, _) if v == 2.0));
                assert!(matches!(*f, Expr::Var(2, _)));
            }
            _ => panic!("If with dynamic condition must be preserved"),
        }
    }

    #[test]
    fn folds_inside_builtin_args() {
        // STEP(1 + 2, t) -- STEP itself is time-dependent and must survive,
        // but its constant argument subtree folds.
        let expr = Expr::App(
            BuiltinFn::Step(
                Box::new(op2(BinaryOp::Add, c(1.0), c(2.0))),
                Box::new(var(0)),
            ),
            Loc::default(),
        );
        match fold_constants(expr) {
            Expr::App(BuiltinFn::Step(a, b), _) => {
                assert!(matches!(*a, Expr::Const(v, _) if v == 3.0));
                assert!(matches!(*b, Expr::Var(0, _)));
            }
            _ => panic!("Step must be preserved with folded args"),
        }
    }

    #[test]
    fn folds_inside_assignments_and_module_args() {
        let expr = Expr::AssignCurr(7, Box::new(op2(BinaryOp::Mul, c(2.0), c(3.0))));
        match fold_constants(expr) {
            Expr::AssignCurr(7, rhs) => {
                assert!(matches!(*rhs, Expr::Const(v, _) if v == 6.0));
            }
            _ => panic!("AssignCurr must be preserved"),
        }

        let expr = Expr::AssignNext(3, Box::new(op2(BinaryOp::Add, c(1.0), c(1.0))));
        match fold_constants(expr) {
            Expr::AssignNext(3, rhs) => {
                assert!(matches!(*rhs, Expr::Const(v, _) if v == 2.0));
            }
            _ => panic!("AssignNext must be preserved"),
        }

        let expr = Expr::EvalModule(
            crate::common::Ident::new("m"),
            crate::common::Ident::new("model"),
            Default::default(),
            vec![op2(BinaryOp::Add, c(1.0), c(1.0)), var(0)],
        );
        match fold_constants(expr) {
            Expr::EvalModule(_, _, _, args) => {
                assert!(matches!(args[0], Expr::Const(v, _) if v == 2.0));
                assert!(matches!(args[1], Expr::Var(0, _)));
            }
            _ => panic!("EvalModule must be preserved"),
        }
    }

    #[test]
    fn folds_subscript_indices() {
        let expr = Expr::Subscript(
            0,
            vec![
                SubscriptIndex::Single(op2(BinaryOp::Add, c(1.0), c(1.0))),
                SubscriptIndex::Range(c(1.0), op2(BinaryOp::Add, c(1.0), c(2.0))),
            ],
            vec![5, 5],
            Loc::default(),
        );
        match fold_constants(expr) {
            Expr::Subscript(0, indices, _, _) => {
                assert!(
                    matches!(&indices[0], SubscriptIndex::Single(Expr::Const(v, _)) if *v == 2.0)
                );
                assert!(
                    matches!(&indices[1], SubscriptIndex::Range(_, Expr::Const(v, _)) if *v == 3.0)
                );
            }
            _ => panic!("Subscript must be preserved"),
        }
    }

    /// End-to-end: the fold must actually run during compilation, so a
    /// constant subtree in a real equation reaches the bytecode as one folded
    /// literal instead of a per-timestep `literal op literal` computation.
    #[test]
    fn compiled_bytecode_carries_folded_literal() {
        let compiled = crate::test_common::TestProject::new("fold_integration")
            .aux("x", "5", None)
            .aux("y", "x * (2.5 * 4)", None)
            .compile_incremental()
            .unwrap();

        let root = &compiled.modules[&compiled.root];
        let literals = &root.compiled_flows.literals;
        assert!(
            literals.contains(&10.0),
            "folded literal 10.0 missing from flows literals: {literals:?}"
        );
        assert!(
            !literals.contains(&2.5) && !literals.contains(&4.0),
            "unfolded operand literals still present: {literals:?}"
        );
    }

    #[test]
    fn eq_uses_vm_approx_eq_semantics() {
        // 0.1 + 0.2 != 0.3 bit-exactly, but the VM's Eq uses ULP-based
        // approx_eq, which treats them as equal. The fold must agree.
        let sum = 0.1_f64 + 0.2_f64;
        assert_ne!(sum.to_bits(), 0.3_f64.to_bits());
        let runtime = eval_op2(Op2::Eq, sum, 0.3);
        assert_folds_to(op2(BinaryOp::Eq, c(sum), c(0.3)), runtime);
    }
}
