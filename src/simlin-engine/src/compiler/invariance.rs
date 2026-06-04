// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Run-invariance classification of lowered flow-phase expressions (GH #712,
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
//! This is the **functional core**: a pure walk over the lowered `Expr` tree
//! parameterized by an offset-classification callback. The callback resolves a
//! slot offset to the run-invariance verdict of its owning variable (invariant,
//! or variant because it is a dynamic var / a stock / a module-instance slot /
//! a time-global other than DT/INITIAL/FINAL). The two compile paths supply
//! different callbacks (the salsa per-fragment path resolves mini-offsets to
//! dependency names; the monolithic test path resolves model-global offsets via
//! the metadata map), but share this walk so they classify identically.
//!
//! The walk is **exhaustive** over every `Expr` and `BuiltinFn` variant with
//! explicit arms and is **default-variant**: anything not positively recognized
//! as invariant is variant, and a future new `Expr`/`BuiltinFn` variant is a
//! compile error here rather than a silent misclassification.

use crate::builtins::BuiltinFn;

use super::expr::{Expr, SubscriptIndex};

/// The run-invariance verdict for the variable owning a referenced slot offset.
///
/// The offset-classification callback returns this for an `Expr::Var(off)` /
/// `Expr::Subscript(off, ..)` / `Expr::StaticSubscript(off, ..)` base offset.
/// `Variant` covers every non-invariant source: a dynamic variable, a stock, a
/// slot inside a module instance's range, and a time-global other than
/// DT/INITIAL/FINAL (those three reach the lowered Expr as builtins, not as
/// `Var` offsets, but the callback rejects a stray `Var` reference to any
/// implicit-global slot defensively).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OffsetClass {
    /// The offset resolves to a run-invariant variable.
    Invariant,
    /// The offset resolves to a variant source (dynamic var, stock, module
    /// instance slot, or a time-global other than DT/INITIAL/FINAL).
    ///
    /// In production, `compute_flow_invariance_support` always returns
    /// `Invariant` (to check structural purity only), so `Variant` is only
    /// constructed in tests and by external callers building real callbacks.
    #[allow(dead_code)]
    Variant,
}

/// Returns true iff every expression in `exprs` (one variable's lowered
/// flow-phase statement list) is run-invariant, given the dependency verdicts
/// from `classify_offset`.
///
/// `exprs` is the variable's own `Var.ast`: a list of statements that ends in
/// `AssignCurr`/`AssignNext`/`AssignTemp` writes plus any `AssignTemp`
/// scratch-array precomputations. A variable is invariant iff ALL of its
/// statements are invariant.
pub(crate) fn exprs_are_invariant<F>(exprs: &[Expr], classify_offset: &F) -> bool
where
    F: Fn(usize) -> OffsetClass,
{
    exprs
        .iter()
        .all(|expr| expr_is_invariant(expr, classify_offset))
}

/// Returns true iff a single lowered expression is run-invariant.
///
/// Exhaustive over every `Expr` variant. Default-variant: a variant is
/// invariant only if positively matched here.
fn expr_is_invariant<F>(expr: &Expr, classify_offset: &F) -> bool
where
    F: Fn(usize) -> OffsetClass,
{
    match expr {
        // Literals and DT are run-invariant by definition.
        Expr::Const(_, _) | Expr::Dt(_) => true,

        // A variable / array reference is invariant iff its owning variable is
        // invariant (the callback rejects stocks, module-instance slots, and
        // time-globals other than DT/INITIAL/FINAL). Dynamic-subscript index
        // exprs must ALSO be invariant -- a variant index changes which element
        // is read each step even when the base array is invariant.
        Expr::Var(off, _) => classify_offset(*off) == OffsetClass::Invariant,
        Expr::StaticSubscript(off, _, _) => classify_offset(*off) == OffsetClass::Invariant,
        Expr::Subscript(off, indices, _, _) => {
            classify_offset(*off) == OffsetClass::Invariant
                && indices
                    .iter()
                    .all(|idx| subscript_index_is_invariant(idx, classify_offset))
        }

        // Temp arrays are intra-statement scratch: a `TempArray`/
        // `TempArrayElement` read is invariant iff the `AssignTemp` that
        // produced it was invariant. Because a variable's statement list is
        // classified as a whole (every statement must be invariant), and the
        // `AssignTemp` precedes its reads, the producing assignment's own
        // invariance is already checked by `exprs_are_invariant`. So a temp
        // *read* contributes no new variant source -- it is invariant here, and
        // the producing `AssignTemp` carries the real verdict.
        Expr::TempArray(_, _, _) | Expr::TempArrayElement(_, _, _, _) => true,

        // Module evaluation and module inputs are conservatively variant: a
        // module instance's slots change per step, and a module input is a
        // parent-provided value.
        Expr::EvalModule(_, _, _, _) | Expr::ModuleInput(_, _) => false,

        // Builtins: see `builtin_is_invariant`.
        Expr::App(builtin, _) => builtin_is_invariant(builtin, classify_offset),

        // Compound exprs: invariant iff all operands are.
        Expr::Op2(_, l, r, _) => {
            expr_is_invariant(l, classify_offset) && expr_is_invariant(r, classify_offset)
        }
        Expr::Op1(_, operand, _) => expr_is_invariant(operand, classify_offset),
        Expr::If(cond, t, f, _) => {
            // The VM evaluates BOTH branches every step, so all three must be
            // invariant.
            expr_is_invariant(cond, classify_offset)
                && expr_is_invariant(t, classify_offset)
                && expr_is_invariant(f, classify_offset)
        }

        // Assignments: invariant iff the assigned expression is.
        Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) | Expr::AssignTemp(_, rhs, _) => {
            expr_is_invariant(rhs, classify_offset)
        }
    }
}

/// A dynamic-subscript index component is invariant iff its index expr(s) are.
fn subscript_index_is_invariant<F>(idx: &SubscriptIndex, classify_offset: &F) -> bool
where
    F: Fn(usize) -> OffsetClass,
{
    match idx {
        SubscriptIndex::Single(e) => expr_is_invariant(e, classify_offset),
        SubscriptIndex::Range(start, end) => {
            expr_is_invariant(start, classify_offset) && expr_is_invariant(end, classify_offset)
        }
    }
}

/// Returns true iff a builtin application is run-invariant.
///
/// Exhaustive over every `BuiltinFn` variant. Default-variant: a builtin is
/// invariant only if it is a fixed time-global, `INIT(x)` (always invariant --
/// the init buffer is frozen after the initials phase), a graphical-function
/// lookup with an invariant index, or a pure function whose every argument is
/// invariant. `TIME`, `PULSE`/`RAMP`/`STEP` (time-dependent even with constant
/// args), and `PREVIOUS` (reads `prev_values`) are variant.
fn builtin_is_invariant<F>(builtin: &BuiltinFn<Expr>, classify_offset: &F) -> bool
where
    F: Fn(usize) -> OffsetClass,
{
    use BuiltinFn::*;
    // Walk a slice of subexpressions, all of which must be invariant.
    let all = |args: &[&Expr]| args.iter().all(|e| expr_is_invariant(e, classify_offset));
    match builtin {
        // Fixed time-globals and nullary constants.
        Inf | Pi | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => true,

        // TIME is the canonical variant builtin.
        Time => false,

        // INIT(x) of ANY x is invariant: the initial-values buffer is frozen
        // after the initials phase. The argument is NOT walked (deliberately).
        Init(_) => true,

        // PREVIOUS reads prev_values -- variant regardless of its argument.
        Previous(_, _) => false,

        // Time-dependent builtins: variant even with constant arguments.
        Pulse(_, _, _) | Ramp(_, _, _) | Step(_, _) => false,

        // Graphical-function lookups: the tables are static. Invariant iff the
        // table holder (arg 1) and the index (arg 2) are both invariant.
        Lookup(table, index, _)
        | LookupForward(table, index, _)
        | LookupBackward(table, index, _) => all(&[table, index]),

        // Pure scalar builtins of invariant arguments.
        Abs(a) | Arccos(a) | Arcsin(a) | Arctan(a) | Cos(a) | Exp(a) | Int(a) | Ln(a)
        | Log10(a) | Sign(a) | Sin(a) | Sqrt(a) | Tan(a) => all(&[a]),
        Max(a, b) | Min(a, b) => {
            expr_is_invariant(a, classify_offset)
                && b.as_ref()
                    .is_none_or(|b| expr_is_invariant(b, classify_offset))
        }
        Mean(args) => args.iter().all(|e| expr_is_invariant(e, classify_offset)),
        Quantum(a, b) => all(&[a, b]),
        SafeDiv(a, b, c) => {
            expr_is_invariant(a, classify_offset)
                && expr_is_invariant(b, classify_offset)
                && c.as_ref()
                    .is_none_or(|c| expr_is_invariant(c, classify_offset))
        }
        Sshape(a, b, c) => all(&[a, b, c]),

        // Array reducers of invariant arguments.
        Size(a) | Stddev(a) | Sum(a) => all(&[a]),

        // Array-producing builtins of invariant arguments. Pure (their result
        // is a deterministic function of their inputs), so an all-invariant
        // argument walk makes them invariant.
        Rank(a, b) => all(&[a, b]),
        VectorElmMap(a, b) | VectorSortOrder(a, b) => all(&[a, b]),
        AllocateAvailable(a, b, c) => all(&[a, b, c]),
        VectorSelect(a, b, c, d, e) | AllocateByPriority(a, b, c, d, e) => all(&[a, b, c, d, e]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Loc};
    use crate::compiler::dimensions::UnaryOp;

    /// An offset callback where a fixed set of offsets is invariant and all
    /// others are variant. Mirrors the per-model verdict map both production
    /// paths supply.
    fn classifier(invariant_offsets: &[usize]) -> impl Fn(usize) -> OffsetClass + '_ {
        move |off| {
            if invariant_offsets.contains(&off) {
                OffsetClass::Invariant
            } else {
                OffsetClass::Variant
            }
        }
    }

    fn lit(n: f64) -> Expr {
        Expr::Const(n, Loc::default())
    }

    fn var(off: usize) -> Expr {
        Expr::Var(off, Loc::default())
    }

    fn add(l: Expr, r: Expr) -> Expr {
        Expr::Op2(BinaryOp::Add, Box::new(l), Box::new(r), Loc::default())
    }

    /// A bare constant assignment: `dst = 3` lowered as `AssignCurr(0, Const)`.
    #[test]
    fn constant_is_invariant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(lit(3.0)))];
        assert!(exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = a + 2` where `a` (offset 1) is an invariant variable.
    #[test]
    fn const_derived_chain_is_invariant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(add(var(1), lit(2.0))))];
        assert!(exprs_are_invariant(&exprs, &classifier(&[1])));
    }

    /// `dst = a + 2` where `a` (offset 1) is a *variant* variable.
    #[test]
    fn variant_dependency_is_variant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(add(var(1), lit(2.0))))];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = TIME` is variant (the canonical time dependency).
    #[test]
    fn time_builtin_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = DT` is invariant (DT is fixed for the run).
    #[test]
    fn dt_is_invariant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(Expr::Dt(Loc::default())))];
        assert!(exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// TIME_STEP / INITIAL_TIME / FINAL_TIME / PI / INF are invariant builtins.
    #[test]
    fn fixed_time_globals_and_constants_are_invariant() {
        for bf in [
            BuiltinFn::TimeStep,
            BuiltinFn::StartTime,
            BuiltinFn::FinalTime,
            BuiltinFn::Pi,
            BuiltinFn::Inf,
        ] {
            let exprs = vec![Expr::AssignCurr(0, Box::new(Expr::App(bf, Loc::default())))];
            assert!(exprs_are_invariant(&exprs, &classifier(&[])));
        }
    }

    /// `dst = PULSE(1, 2)` is variant even with constant arguments (time-dependent).
    #[test]
    fn pulse_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Pulse(Box::new(lit(1.0)), Box::new(lit(2.0)), None),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = RAMP(1, 2)` is variant.
    #[test]
    fn ramp_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Ramp(Box::new(lit(1.0)), Box::new(lit(2.0)), None),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = STEP(1, 2)` is variant.
    #[test]
    fn step_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Step(Box::new(lit(1.0)), Box::new(lit(2.0))),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = PREVIOUS(a, 0)` is variant (reads prev_values), even of an
    /// invariant variable.
    #[test]
    fn previous_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Previous(Box::new(var(1)), Box::new(lit(0.0))),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[1])));
    }

    /// `dst = INIT(a)` is invariant for ANY `a` -- even a variant one -- because
    /// the initial-values buffer is frozen after the initials phase.
    #[test]
    fn init_of_any_variable_is_invariant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(BuiltinFn::Init(Box::new(var(1))), Loc::default())),
        )];
        // offset 1 is NOT invariant, yet INIT(a) is still invariant.
        assert!(exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `dst = LOOKUP(table, 3)` -- a lookup of a constant index -- is invariant.
    /// The table arg is a `Var(off)` to a static lookup-only holder, which the
    /// callback classifies invariant; the index is a constant.
    #[test]
    fn lookup_of_constant_is_invariant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Lookup(Box::new(var(5)), Box::new(lit(3.0)), Loc::default()),
                Loc::default(),
            )),
        )];
        // offset 5 (the static table holder) is invariant.
        assert!(exprs_are_invariant(&exprs, &classifier(&[5])));
    }

    /// `dst = LOOKUP(table, TIME)` -- a lookup whose index is TIME -- is variant.
    #[test]
    fn lookup_of_time_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Lookup(
                    Box::new(var(5)),
                    Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
                    Loc::default(),
                ),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[5])));
    }

    /// A stock dependency is variant (the callback classifies the stock's
    /// offset as Variant). `dst = stock + 1`.
    #[test]
    fn stock_dependency_is_variant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(add(var(1), lit(1.0))))];
        // offset 1 is a stock -> Variant.
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// False-positive class 1: a module-output read via a plain `Var` offset is
    /// variant (the callback classifies the module-instance slot as Variant).
    #[test]
    fn module_output_read_is_variant() {
        let exprs = vec![Expr::AssignCurr(0, Box::new(var(7)))];
        // offset 7 is a slot inside a module instance -> Variant.
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// False-positive class 2: a whole-array view read of a *variant* array via
    /// `StaticSubscript` is variant.
    #[test]
    fn static_subscript_of_variant_array_is_variant() {
        let view = crate::ast::ArrayView::contiguous(vec![3]);
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::StaticSubscript(2, view, Loc::default())),
        )];
        // offset 2 (the array base) is variant.
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// A `StaticSubscript` read of an *invariant* array is invariant.
    #[test]
    fn static_subscript_of_invariant_array_is_invariant() {
        let view = crate::ast::ArrayView::contiguous(vec![3]);
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::StaticSubscript(2, view, Loc::default())),
        )];
        assert!(exprs_are_invariant(&exprs, &classifier(&[2])));
    }

    /// `ModuleInput` is always variant (module instances are conservatively
    /// variant).
    #[test]
    fn module_input_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::ModuleInput(0, Loc::default())),
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `EvalModule` is variant.
    #[test]
    fn eval_module_is_variant() {
        use crate::common::Ident;
        use std::collections::BTreeSet;
        let exprs = vec![Expr::EvalModule(
            Ident::new("m"),
            Ident::new("sub"),
            BTreeSet::new(),
            vec![],
        )];
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// A dynamic `Subscript` whose index is invariant and whose base is
    /// invariant is invariant.
    #[test]
    fn dynamic_subscript_invariant_base_and_index_is_invariant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::Subscript(
                2,
                vec![SubscriptIndex::Single(lit(1.0))],
                vec![3],
                Loc::default(),
            )),
        )];
        assert!(exprs_are_invariant(&exprs, &classifier(&[2])));
    }

    /// A dynamic `Subscript` whose index expr is variant (e.g. references a
    /// variant variable) is variant even if the base array is invariant.
    #[test]
    fn dynamic_subscript_variant_index_is_variant() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::Subscript(
                2,
                vec![SubscriptIndex::Single(var(9))],
                vec![3],
                Loc::default(),
            )),
        )];
        // base offset 2 is invariant, but index var 9 is variant.
        assert!(!exprs_are_invariant(&exprs, &classifier(&[2])));
    }

    /// A `Sum` reducer of an invariant array is invariant; of a variant array,
    /// variant.
    #[test]
    fn sum_reducer_tracks_argument() {
        let view = crate::ast::ArrayView::contiguous(vec![3]);
        let inv = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Sum(Box::new(Expr::StaticSubscript(
                    2,
                    view.clone(),
                    Loc::default(),
                ))),
                Loc::default(),
            )),
        )];
        assert!(exprs_are_invariant(&inv, &classifier(&[2])));

        let variant = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::App(
                BuiltinFn::Sum(Box::new(Expr::StaticSubscript(2, view, Loc::default()))),
                Loc::default(),
            )),
        )];
        assert!(!exprs_are_invariant(&variant, &classifier(&[])));
    }

    /// AssignTemp / TempArray / TempArrayElement self-references within the
    /// statement list are classified by the expression assigned to the temp:
    /// an invariant temp computation makes the whole var invariant.
    #[test]
    fn arrayed_invariant_chain_via_temp_is_invariant() {
        let view = crate::ast::ArrayView::contiguous(vec![2]);
        let exprs = vec![
            // temp 0 = invariant array (base var 3, which is invariant)
            Expr::AssignTemp(
                0,
                Box::new(Expr::StaticSubscript(3, view.clone(), Loc::default())),
                view.clone(),
            ),
            // dst[0] = temp0[0]
            Expr::AssignCurr(
                0,
                Box::new(Expr::TempArrayElement(0, view.clone(), 0, Loc::default())),
            ),
            // dst[1] = temp0[1]
            Expr::AssignCurr(
                1,
                Box::new(Expr::TempArrayElement(0, view, 1, Loc::default())),
            ),
        ];
        assert!(exprs_are_invariant(&exprs, &classifier(&[3])));
    }

    /// A temp computed from a *variant* array makes the whole var variant.
    #[test]
    fn arrayed_variant_chain_via_temp_is_variant() {
        let view = crate::ast::ArrayView::contiguous(vec![2]);
        let exprs = vec![
            Expr::AssignTemp(
                0,
                Box::new(Expr::StaticSubscript(3, view.clone(), Loc::default())),
                view.clone(),
            ),
            Expr::AssignCurr(
                0,
                Box::new(Expr::TempArrayElement(0, view, 0, Loc::default())),
            ),
        ];
        // base var 3 is variant.
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }

    /// `If(cond, t, f)` is invariant iff cond, t, AND f are all invariant (the
    /// VM evaluates both branches).
    #[test]
    fn if_invariant_when_all_branches_invariant() {
        let inv = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::If(
                Box::new(lit(1.0)),
                Box::new(var(1)),
                Box::new(lit(0.0)),
                Loc::default(),
            )),
        )];
        assert!(exprs_are_invariant(&inv, &classifier(&[1])));

        let variant = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::If(
                Box::new(lit(1.0)),
                Box::new(var(1)),
                Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
                Loc::default(),
            )),
        )];
        // the false branch is TIME -> variant.
        assert!(!exprs_are_invariant(&variant, &classifier(&[1])));
    }

    /// Op1 (logical not) tracks its operand.
    #[test]
    fn op1_tracks_operand() {
        let exprs = vec![Expr::AssignCurr(
            0,
            Box::new(Expr::Op1(UnaryOp::Not, Box::new(var(1)), Loc::default())),
        )];
        assert!(exprs_are_invariant(&exprs, &classifier(&[1])));
        assert!(!exprs_are_invariant(&exprs, &classifier(&[])));
    }
}
