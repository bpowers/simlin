// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The compiler-local half of run-invariance classification (GH #712,
//! stage B1).
//!
//! A root-model flow-phase variable is *run-invariant* iff its dt-phase lowered
//! expressions (`compiler::Var.ast`) transitively reference only quantities
//! that do not change across timesteps: literals, `DT`/`INITIAL`/`FINAL` time
//! globals, `INIT(x)` of any variable (the initial-values buffer is frozen
//! after initials), static graphical-function tables with invariant indices,
//! pure builtins of invariant arguments, and other run-invariant variables.
//! See `docs/design-plans/2026-06-04-time-invariant-hoisting.md` for the full
//! definition and the soundness argument.
//!
//! This walk answers the LOCAL half of that: whether the expression holds a
//! time-dependent builtin, a lagged read, a module evaluation or a module
//! input, treating every variable reference as invariant. Which references
//! actually are invariant is a question about the model's dependency
//! relation, which `db::invariance::model_flows_invariant` answers over the
//! variable's `DepRef`s; the two halves together are the verdict.
//!
//! The walk is **exhaustive** over every `Expr` variant with explicit arms and
//! is **default-variant**: anything not positively recognized as invariant is
//! variant, and a future new `Expr` variant is a compile error here rather than
//! a silent misclassification. A builtin's class is its signature's
//! `Invariance`, so adding a builtin means stating its class in the table.

use crate::builtins::{BuiltinFn, Invariance};

use super::expr::{Expr, SubscriptIndex};

/// Returns true iff every expression in `exprs` (one variable's lowered
/// flow-phase statement list) is locally run-invariant.
///
/// `exprs` is the variable's own `Var.ast`: a list of statements that ends in
/// `AssignCurr`/`AssignNext`/`AssignTemp` writes plus any `AssignTemp`
/// scratch-array precomputations. A variable is invariant iff ALL of its
/// statements are invariant.
pub(crate) fn exprs_are_locally_invariant(exprs: &[Expr]) -> bool {
    exprs.iter().all(expr_is_invariant)
}

/// Returns true iff a single lowered expression is locally run-invariant.
///
/// Exhaustive over every `Expr` variant. Default-variant: a variant is
/// invariant only if positively matched here.
fn expr_is_invariant(expr: &Expr) -> bool {
    match expr {
        // Literals and DT are run-invariant by definition.
        Expr::Const(_, _) | Expr::Dt(_) => true,

        // A variable / array reference is invariant here: whether the
        // variable it names is invariant is the dependency relation's
        // question. Dynamic-subscript index exprs must be invariant -- a
        // variant index changes which element is read each step even when
        // the base array is invariant.
        Expr::Var(_, _) | Expr::StaticSubscript(_, _, _) => true,
        Expr::Subscript(_, indices, _, _) => indices.iter().all(subscript_index_is_invariant),

        // Temp arrays are intra-statement scratch: a `TempArray`/
        // `TempArrayElement` read is invariant iff the `AssignTemp` that
        // produced it was invariant. Because a variable's statement list is
        // classified as a whole (every statement must be invariant), and the
        // `AssignTemp` precedes its reads, the producing assignment's own
        // invariance is already checked by `exprs_are_locally_invariant`. So a
        // temp *read* contributes no new variant source -- it is invariant
        // here, and the producing `AssignTemp` carries the real verdict.
        Expr::TempArray(_, _, _) | Expr::TempArrayElement(_, _, _, _) => true,

        // Module evaluation and module inputs are conservatively variant: a
        // module instance's slots change per step, and a module input is a
        // parent-provided value.
        Expr::EvalModule(_, _, _, _) | Expr::ModuleInput(_, _) => false,

        // Builtins: see `builtin_is_invariant`.
        Expr::App(builtin, _) => builtin_is_invariant(builtin),

        // Compound exprs: invariant iff all operands are.
        Expr::Op2(_, l, r, _) => expr_is_invariant(l) && expr_is_invariant(r),
        Expr::Op1(_, operand, _) => expr_is_invariant(operand),
        Expr::If(cond, t, f, _) => {
            // The VM evaluates BOTH branches every step, so all three must be
            // invariant.
            expr_is_invariant(cond) && expr_is_invariant(t) && expr_is_invariant(f)
        }

        // Assignments: invariant iff the assigned expression is.
        Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) | Expr::AssignTemp(_, rhs, _) => {
            expr_is_invariant(rhs)
        }
    }
}

/// A dynamic-subscript index component is invariant iff its index expr(s) are.
fn subscript_index_is_invariant(idx: &SubscriptIndex) -> bool {
    match idx {
        SubscriptIndex::Single(e) => expr_is_invariant(e),
        SubscriptIndex::Range(start, end) => expr_is_invariant(start) && expr_is_invariant(end),
    }
}

/// Returns true iff a builtin application is locally run-invariant.
///
/// Decided by the signature's `Invariance`: a `Pure` builtin is invariant iff
/// every argument is (a graphical-function lookup's table holder and index
/// among them; the fixed time globals and nullary constants trivially);
/// `INIT(x)` is invariant for ANY `x` -- the init buffer is frozen after the
/// initials phase, so its argument is deliberately NOT walked; `TIME`,
/// `PULSE`/`RAMP`/`STEP` (time-dependent even with constant args) and
/// `PREVIOUS` (reads `prev_values`) are variant whatever their arguments.
fn builtin_is_invariant(builtin: &BuiltinFn<Expr>) -> bool {
    match builtin.signature().invariance {
        Invariance::Pure => builtin.args().into_iter().all(expr_is_invariant),
        Invariance::Snapshot => true,
        Invariance::TimeDependent | Invariance::Lagged => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Loc};
    use crate::builtins::BuiltinSig;
    use crate::compiler::VarRef;
    use crate::compiler::dimensions::UnaryOp;

    fn vref(n: usize) -> VarRef {
        VarRef::base(crate::common::Ident::new(&format!("v{n}")))
    }

    fn lit(n: f64) -> Expr {
        Expr::Const(n, Loc::default())
    }

    fn var(n: usize) -> Expr {
        Expr::Var(vref(n), Loc::default())
    }

    fn time() -> Expr {
        Expr::App(BuiltinFn::Time, Loc::default())
    }

    fn assign(rhs: Expr) -> Expr {
        Expr::AssignCurr(vref(0), Box::new(rhs))
    }

    /// Every `Expr` variant, as a row of the local verdict: the leaves, the
    /// compound forms with an invariant and a variant operand, and every
    /// write form. A variable reference is locally invariant by definition
    /// -- what it names is the dependency relation's question -- so the
    /// variant rows are the time-dependent builtin, the module forms and a
    /// variant subscript index.
    #[test]
    fn every_expr_variant_has_its_local_verdict() {
        let view = crate::ast::ArrayView::contiguous(vec![3]);
        let sum = |arg: Expr| Expr::App(BuiltinFn::Sum(Box::new(arg)), Loc::default());
        let rows: Vec<(&str, Expr, bool)> = vec![
            ("Const", assign(lit(3.0)), true),
            ("Dt", assign(Expr::Dt(Loc::default())), true),
            ("Var", assign(var(1)), true),
            (
                "StaticSubscript",
                assign(Expr::StaticSubscript(vref(2), view.clone(), Loc::default())),
                true,
            ),
            (
                "Subscript, invariant index",
                assign(Expr::Subscript(
                    vref(2),
                    vec![SubscriptIndex::Single(lit(1.0))],
                    vec![3],
                    Loc::default(),
                )),
                true,
            ),
            (
                "Subscript, variant index",
                assign(Expr::Subscript(
                    vref(2),
                    vec![SubscriptIndex::Single(time())],
                    vec![3],
                    Loc::default(),
                )),
                false,
            ),
            (
                "Subscript, variant range end",
                assign(Expr::Subscript(
                    vref(2),
                    vec![SubscriptIndex::Range(lit(1.0), time())],
                    vec![3],
                    Loc::default(),
                )),
                false,
            ),
            (
                "TempArray",
                assign(Expr::TempArray(0, view.clone(), Loc::default())),
                true,
            ),
            (
                "TempArrayElement",
                assign(Expr::TempArrayElement(0, view.clone(), 0, Loc::default())),
                true,
            ),
            (
                "EvalModule",
                Expr::EvalModule(
                    crate::common::Ident::new("m"),
                    crate::common::Ident::new("sub"),
                    std::collections::BTreeSet::new(),
                    vec![],
                ),
                false,
            ),
            (
                "ModuleInput",
                assign(Expr::ModuleInput(0, Loc::default())),
                false,
            ),
            ("App, pure of invariant", assign(sum(var(2))), true),
            ("App, pure of variant", assign(sum(time())), false),
            (
                "Op2, both invariant",
                assign(Expr::Op2(
                    BinaryOp::Add,
                    Box::new(var(1)),
                    Box::new(lit(2.0)),
                    Loc::default(),
                )),
                true,
            ),
            (
                "Op2, one variant",
                assign(Expr::Op2(
                    BinaryOp::Add,
                    Box::new(var(1)),
                    Box::new(time()),
                    Loc::default(),
                )),
                false,
            ),
            (
                "Op1, invariant",
                assign(Expr::Op1(UnaryOp::Not, Box::new(var(1)), Loc::default())),
                true,
            ),
            (
                "Op1, variant",
                assign(Expr::Op1(UnaryOp::Not, Box::new(time()), Loc::default())),
                false,
            ),
            (
                "If, all invariant",
                assign(Expr::If(
                    Box::new(lit(1.0)),
                    Box::new(var(1)),
                    Box::new(lit(0.0)),
                    Loc::default(),
                )),
                true,
            ),
            (
                // The VM evaluates both branches every step.
                "If, untaken branch variant",
                assign(Expr::If(
                    Box::new(lit(1.0)),
                    Box::new(var(1)),
                    Box::new(time()),
                    Loc::default(),
                )),
                false,
            ),
            (
                "AssignNext",
                Expr::AssignNext(vref(0), Box::new(var(1))),
                true,
            ),
            (
                "AssignTemp",
                Expr::AssignTemp(
                    0,
                    Box::new(Expr::StaticSubscript(vref(3), view.clone(), Loc::default())),
                    view.clone(),
                ),
                true,
            ),
            (
                "AssignTemp, variant producer",
                Expr::AssignTemp(0, Box::new(time()), view.clone()),
                false,
            ),
        ];
        for (label, expr, expected) in rows {
            assert_eq!(
                exprs_are_locally_invariant(std::slice::from_ref(&expr)),
                expected,
                "{label}"
            );
        }
        // A statement list is invariant iff every statement is.
        let chain = vec![
            Expr::AssignTemp(
                0,
                Box::new(Expr::StaticSubscript(vref(3), view.clone(), Loc::default())),
                view.clone(),
            ),
            assign(Expr::TempArrayElement(0, view.clone(), 0, Loc::default())),
        ];
        assert!(exprs_are_locally_invariant(&chain));
        let broken = vec![
            Expr::AssignTemp(0, Box::new(time()), view.clone()),
            assign(Expr::TempArrayElement(0, view, 0, Loc::default())),
        ];
        assert!(!exprs_are_locally_invariant(&broken));
    }

    /// Every builtin takes its local verdict from its signature's
    /// `Invariance` class, so the rows are the four classes, each pinned by
    /// one builtin the table files under it, with `INIT` of a variant
    /// argument as the `Snapshot` arm's whole content and `PREVIOUS` of an
    /// invariant argument as `Lagged`'s.
    #[test]
    fn every_invariance_class_has_its_local_verdict() {
        let rows: Vec<(BuiltinFn<Expr>, Invariance, bool)> = vec![
            (BuiltinFn::Time, Invariance::TimeDependent, false),
            (
                BuiltinFn::Pulse(Box::new(lit(1.0)), Box::new(lit(2.0)), None),
                Invariance::TimeDependent,
                false,
            ),
            (
                BuiltinFn::Ramp(Box::new(lit(1.0)), Box::new(lit(2.0)), None),
                Invariance::TimeDependent,
                false,
            ),
            (
                BuiltinFn::Step(Box::new(lit(1.0)), Box::new(lit(2.0))),
                Invariance::TimeDependent,
                false,
            ),
            (
                BuiltinFn::Previous(Box::new(var(1)), Box::new(lit(0.0))),
                Invariance::Lagged,
                false,
            ),
            (
                BuiltinFn::Init(Box::new(time())),
                Invariance::Snapshot,
                true,
            ),
            (BuiltinFn::TimeStep, Invariance::Pure, true),
            (BuiltinFn::StartTime, Invariance::Pure, true),
            (BuiltinFn::FinalTime, Invariance::Pure, true),
            (BuiltinFn::Pi, Invariance::Pure, true),
            (BuiltinFn::Inf, Invariance::Pure, true),
            (
                BuiltinFn::Lookup(Box::new(var(5)), Box::new(lit(3.0)), Loc::default()),
                Invariance::Pure,
                true,
            ),
            (
                BuiltinFn::Lookup(Box::new(var(5)), Box::new(time()), Loc::default()),
                Invariance::Pure,
                false,
            ),
        ];
        let mut classes_seen = std::collections::BTreeSet::new();
        for (builtin, class, expected) in rows {
            assert_eq!(builtin.signature().invariance, class, "{}", builtin.name());
            classes_seen.insert(format!("{class:?}"));
            let exprs = vec![assign(Expr::App(builtin, Loc::default()))];
            assert_eq!(exprs_are_locally_invariant(&exprs), expected);
        }
        // The rows span the enumeration.
        let all_classes: std::collections::BTreeSet<String> = BuiltinSig::ALL
            .iter()
            .map(|sig| format!("{:?}", sig.invariance))
            .collect();
        assert_eq!(classes_seen, all_classes);
    }
}
