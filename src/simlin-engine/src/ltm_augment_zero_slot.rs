// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Deciding when a per-element link-score arm may be OMITTED rather than
//! materialized (GH #977), in its own file only to keep `ltm_augment.rs` under
//! the project line-count lint. Mounted into `ltm_augment`, so callers keep
//! naming these items `crate::ltm_augment::*`.

use crate::ast::Expr0;
use crate::builtins::UntypedBuiltinFn;

/// Whether the caller's result is a whole VARIABLE's equation or one slot of an
/// `Ast::Arrayed` one -- which is the only thing that decides whether a
/// structurally-zero partial may be dropped instead of built.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZeroSlotPolicy {
    /// Always build the guard form, even when the partial is the fully-frozen
    /// target. Required wherever the result is a whole VARIABLE's equation
    /// rather than one slot of an `Ast::Arrayed` one: dropping it there would
    /// delete the variable, changing the emitted score set and the layout.
    Materialize,
    /// Drop the arm when the transformed partial is PROVABLY `PREVIOUS(target)`
    /// ([`partial_is_provably_previous_target`], GH #977). The omitted slot is
    /// then absent from the `Arrayed` element map, and
    /// `compiler::expand_arrayed_with_hoisting` lowers an absent slot to a
    /// single `AssignCurr(off, Const(0.0))` -- one opcode in place of a full
    /// guard form that recomputes the same zero the long way.
    ///
    /// Sound ONLY when the target's `apply_default_to_missing` is FALSE. Under
    /// EXCEPT semantics an absent slot picks up the DEFAULT equation instead of
    /// zero, so an omitted arm would silently take the default's value.
    /// [`super::build_arrayed_link_score_equation`] enforces that, being the only
    /// caller that knows the target's flag.
    ///
    /// This is a BIT-EXACT transformation, and that is the whole point of the
    /// positive predicate. The tempting negative test -- "the link's source
    /// stayed frozen" -- says nothing about what else the arm reads, and
    /// collapsing on it changes 187 C-LEARN result slots across 35 link-score
    /// variables (151 by >= 1.0, worst 8,086.97 -> 0), because the wrap does not
    /// freeze everything that varies: a live `time()` remains, and a
    /// raw-vs-canonical element-spelling mismatch can leave the source itself
    /// unwrapped. Those are tracked as #1016 and the wrap defects in #977; this
    /// predicate is correct whether or not they are fixed, because it asks about
    /// the emitted tree rather than about the wrap's bookkeeping.
    OmitStructuralZero,
}

/// Whether a walk established that a subtree cannot vary between the previous
/// and current step.
///
/// A named verdict rather than a `bool` because the failure mode this predicate
/// exists to prevent is a match arm that inspects a node and then neglects to
/// decide (GH #977: a prior negative-criterion collapse was withdrawn after
/// seven adversarial review rounds, each finding a different node whose
/// "trivial" arm was not actually zero). Every arm below returns one of these,
/// the argument walk is a verdict-returning fold rather than a unit-returning
/// callback, and the matches carry no catch-all -- so a new `Expr0` variant is a
/// compile error rather than a silent `Established`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Every leaf reachable here is a literal or sits inside a frozen subtree.
    Established,
    /// Something reachable here can differ between steps -- or the walk could
    /// not prove otherwise, which is the same answer.
    NotEstablished,
}

impl Reach {
    /// `Established` iff both halves are. The fold's combining step, named so
    /// the walk never open-codes the conjunction.
    fn and(self, other: Reach) -> Reach {
        match (self, other) {
            (Reach::Established, Reach::Established) => Reach::Established,
            (Reach::NotEstablished, _) | (_, Reach::NotEstablished) => Reach::NotEstablished,
        }
    }
}

/// How a builtin call bears on the walk. The classification is by NAME, so it
/// cannot be exhaustive over a type -- which is exactly why the unrecognized
/// case is a named variant decided at the match below rather than a fall-through.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BuiltinReach {
    /// The call's contents are read at the PREVIOUS step, so the subtree is
    /// frozen whatever it contains and the walk must NOT descend into it.
    FrozenSubtree,
    /// Deterministic in its arguments and independent of the step, so the
    /// verdict is the fold over its arguments.
    PureInArgs,
    /// Reads the clock, a table, or something otherwise unrecognized. Either
    /// way the walk cannot establish invariance.
    Varying,
}

/// Classify a builtin by name for [`partial_is_provably_previous_target`].
///
/// `lookup` is deliberately `Varying` even though a graphical function is a
/// compile-time constant: it would only matter for an arm whose lookup index is
/// itself invariant, and GH #977 measured that relaxation as buying **exactly
/// zero** additional arms on C-LEARN (those arms hit a live `time()` inside the
/// lookup's own index immediately afterwards). Conservative and free.
fn classify_builtin_reach(name: &str) -> BuiltinReach {
    // Lowercased at parse time, but classify case-insensitively so a future
    // caller with raw source spelling cannot silently fall into `Varying`.
    let lowered = name.to_ascii_lowercase();
    match lowered.as_str() {
        "previous" | "init" => BuiltinReach::FrozenSubtree,
        "abs" | "arccos" | "arcsin" | "arctan" | "cos" | "exp" | "inf" | "int" | "ln" | "log10"
        | "max" | "min" | "pi" | "safediv" | "sign" | "sin" | "sqrt" | "tan" => {
            BuiltinReach::PureInArgs
        }
        // Everything else -- `time`, `dt`, `initial_time`, `final_time`, `step`,
        // `ramp`, `pulse`, `lookup`, the stateful macros, and any builtin added
        // after this was written -- cannot be established as invariant here.
        _ => BuiltinReach::Varying,
    }
}

/// Is `partial` provably equal to `PREVIOUS(target)` -- i.e. does it recompute
/// the target from inputs that cannot have changed since the previous step?
///
/// This is the soundness condition for dropping an `Ast::Arrayed` link-score arm
/// (GH #977). The arm's numerator is `partial - PREVIOUS(target)`; when the
/// partial reads nothing that varies, it reproduces the value that PRODUCED
/// `PREVIOUS(target)`, the numerator is identically zero, and an absent slot's
/// `AssignCurr(off, Const(0.0))` computes the same thing for one opcode.
///
/// The test is POSITIVE -- "everything reachable outside a frozen subtree is a
/// literal" -- rather than the negative "the link's source stayed frozen". The
/// negative form asks a different question, one that says nothing about the rest
/// of the arm; see [`ZeroSlotPolicy::OmitStructuralZero`] for what that costs.
///
/// A `Var` or `Subscript` reached outside a frozen subtree is a live read and
/// ends the walk, which is why subscript INDICES are never descended into: the
/// whole reference is already `NotEstablished`, so `IndexExpr0` needs no arm
/// here and a new index variant cannot change any verdict.
pub(super) fn partial_is_provably_previous_target(partial: &Expr0) -> bool {
    reach_of(partial) == Reach::Established
}

fn reach_of(expr: &Expr0) -> Reach {
    match expr {
        Expr0::Const(..) => Reach::Established,
        // A live read of model state: the value it yields this step is exactly
        // what the wrap was supposed to freeze and did not.
        Expr0::Var(..) => Reach::NotEstablished,
        Expr0::Subscript(..) => Reach::NotEstablished,
        Expr0::Op1(_, inner, _) => reach_of(inner),
        Expr0::Op2(_, lhs, rhs, _) => reach_of(lhs).and(reach_of(rhs)),
        Expr0::If(cond, then, other, _) => reach_of(cond).and(reach_of(then)).and(reach_of(other)),
        Expr0::App(UntypedBuiltinFn(name, args), _) => match classify_builtin_reach(name) {
            // Do NOT descend: the contents are read at the previous step, so
            // whatever they reference is frozen by construction.
            BuiltinReach::FrozenSubtree => Reach::Established,
            BuiltinReach::PureInArgs => args
                .iter()
                .fold(Reach::Established, |acc, arg| acc.and(reach_of(arg))),
            BuiltinReach::Varying => Reach::NotEstablished,
        },
    }
}
