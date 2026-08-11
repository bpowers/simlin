// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Deciding when a per-element link-score arm may be OMITTED rather than
//! materialized (GH #977), in its own file only to keep `ltm_augment.rs` under
//! the project line-count lint. Mounted into `ltm_augment`, so callers keep
//! naming these items `crate::ltm_augment::*`.

use crate::ast::{Expr0, IndexExpr0};
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
    /// positive predicate. Bit-exactness rests on a LAG-ALIGNMENT requirement
    /// that is easy to state and easy to miss: the partial equals
    /// `PREVIOUS(target)` only if every read in it is lagged by EXACTLY one
    /// step. Two shapes look entirely frozen and are not aligned -- an ORIGINAL
    /// `PREVIOUS(z)` from the target's own equation, which the wrap
    /// deliberately leaves untouched (so the partial reads `z(t-1)` where the
    /// anchor read `z(t-2)`), and a synthesized `PREVIOUS` nested inside
    /// another, which the subscript-index freeze produces. Both are rejected by
    /// [`partial_is_provably_previous_target`], and each is pinned by its own
    /// row in `db::ltm_value_gate_tests`; skipping either omits an arm worth
    /// close to the canonical +/-1 attribution.
    ///
    /// Bit-exactness has ONE disclosed exception, and it is a value change
    /// rather than a representation one: when the target slot is NON-FINITE.
    /// A materialized arm computes `NaN - NaN` (or `inf - inf`), the zero
    /// guards do not fire because `NaN = 0` is false, `SAFEDIV`'s fallback is
    /// for a zero denominator rather than a `NaN` one, and the arm evaluates to
    /// `NaN`; an omitted slot is `+0.0`. Measured, not argued:
    /// `db::ltm_value_gate_tests::a_nonfinite_target_arm_is_omitted_to_zero_not_nan`
    /// reproduces both sides.
    ///
    /// That trade is NOT adjudicated -- it is tracked as GH #1022 -- and the two
    /// relevant positions disagree.
    /// `crate::float`'s module docs hold that an engine-manufactured NaN is
    /// noise in a channel practitioners debug by hand, and this NaN is
    /// engine-made -- the guard form's own subtraction -- on an arm with no
    /// causal dependence on its source, so `0` is the structurally known answer.
    /// GH #542 points the other way: `ltm_post::denom_summand` excludes a `NaN`
    /// score from its partition denominator precisely so the bad entry's own
    /// numerator can stay `NaN` as "the honest per-loop 'undefined here'
    /// signal". Replacing some of those with `0` partially undoes that.
    /// Confined to models already producing non-finite values, and the signal
    /// survives on the target's own series and on every live arm.
    ///
    /// The tempting negative test -- "the link's source
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
    /// `PREVIOUS(..)`: its contents are read ONE step back. That is what the
    /// wrap's synthesized freezes do, and it is what makes the partial
    /// reproduce the target's previous value -- but only if the lag is exactly
    /// one. A `PREVIOUS` nested inside this one reads two steps back, so the
    /// walk MUST descend far enough to rule that out.
    LagsOneStep,
    /// `INIT(..)`: its value is the run's initial value, identical at every
    /// step, so it is genuinely step-invariant whatever it contains and the
    /// walk need not descend.
    StepInvariant,
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
        "previous" => BuiltinReach::LagsOneStep,
        "init" => BuiltinReach::StepInvariant,
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
/// "Frozen" is not enough on its own: the partial reproduces `target(t-1)` only
/// if every read is lagged by EXACTLY one step, so this takes the ORIGINAL
/// element expression as well as the emitted partial. An original `PREVIOUS(z)`
/// has to be found in the original, because in the emitted tree it is the same
/// node as a synthesized freeze and nothing distinguishes them; a NESTED
/// `PREVIOUS` is found in the partial, because the wrap is what introduces it.
/// Neither check subsumes the other -- reverting either one alone leaves the
/// other case wrongly omitted, measured row by row in
/// `db::ltm_value_gate_tests`.
///
/// A `Var` or `Subscript` reached outside a frozen subtree is a live read and
/// ends the walk, which is why subscript INDICES are never descended into: the
/// whole reference is already `NotEstablished`, so `IndexExpr0` needs no arm
/// here and a new index variant cannot change any verdict.
pub(super) fn partial_is_provably_previous_target(original: &Expr0, partial: &Expr0) -> bool {
    !contains_previous_call(original) && reach_of(partial) == Reach::Established
}

/// Does `expr` call `PREVIOUS` outside every `INIT(..)` subtree?
///
/// Asked of the ORIGINAL element equation, never of the partial, because in the
/// emitted tree an original `PREVIOUS(z)` and a synthesized freeze
/// `PREVIOUS(x)` are the same node and nothing distinguishes them. `INIT`
/// subtrees are skipped: `INIT(PREVIOUS(z))` is the run's initial value, a
/// constant, so it aligns at every step.
fn contains_previous_call(expr: &Expr0) -> bool {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::Subscript(_, indices, _) => indices.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => contains_previous_call(e),
            IndexExpr0::Range(l, r, _) => contains_previous_call(l) || contains_previous_call(r),
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::Op1(_, inner, _) => contains_previous_call(inner),
        Expr0::Op2(_, lhs, rhs, _) => contains_previous_call(lhs) || contains_previous_call(rhs),
        Expr0::If(c, t, f, _) => {
            contains_previous_call(c) || contains_previous_call(t) || contains_previous_call(f)
        }
        Expr0::App(UntypedBuiltinFn(name, args), _) => match classify_builtin_reach(name) {
            BuiltinReach::LagsOneStep => true,
            BuiltinReach::StepInvariant => false,
            BuiltinReach::PureInArgs | BuiltinReach::Varying => {
                args.iter().any(contains_previous_call)
            }
        },
    }
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
            // Read one step back -- the lag the anchor expects -- but ONLY if
            // nothing inside lags again. `PREVIOUS(q[PREVIOUS(ctr, ctr)])` reads
            // `q` at `t-1` indexed by `ctr` at `t-2`, where the anchor indexed
            // at `t-1`, so it is not `PREVIOUS(target)`.
            BuiltinReach::LagsOneStep => {
                if args.iter().any(contains_previous_call) {
                    Reach::NotEstablished
                } else {
                    Reach::Established
                }
            }
            BuiltinReach::StepInvariant => Reach::Established,
            BuiltinReach::PureInArgs => args
                .iter()
                .fold(Reach::Established, |acc, arg| acc.and(reach_of(arg))),
            BuiltinReach::Varying => Reach::NotEstablished,
        },
    }
}
