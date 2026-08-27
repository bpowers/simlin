// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The first-DT initial value of every `PREVIOUS` the LTM ceteris-paribus
//! walkers SYNTHESIZE (GH #975).
//!
//! One rule, consumed by the two walkers that descend into subscript indices
//! (`wrap_non_matching_in_previous` and `wrap_matching_in_previous`); the other
//! two document that they never do. In its own file only to keep
//! `ltm_augment.rs` under the project line-count lint; it is `#[path]`-mounted
//! as a child module, so callers still name it `crate::ltm_augment::*`.

use crate::ast::Expr0;
use crate::builtins::UntypedBuiltinFn;

/// Build the `PREVIOUS` call an LTM ceteris-paribus walker freezes `expr` with,
/// choosing the FIRST-DT initial value from the position `expr` occupies.
///
/// The XMILE spec (`docs/reference/xmile-v1.0.html` §3.5.6) defines `PREVIOUS`
/// as taking a *variable and initial value expression*, returning "the value of
/// price in the last DT, **or zero in the first DT**" for `PREVIOUS(price, 0)`.
/// The unary spelling these walkers used to emit is desugared to `PREVIOUS(x, 0)`
/// by `builtins_visitor`, so every frozen read used `0` as its first-DT value.
///
/// That is a sound default for a *value* position and a broken one for a
/// subscript INDEX: subscripts are 1-based, so `0` is out of range for every
/// dimension, and the read yields NaN. The NaN is not confined to the first DT.
/// A frozen dynamic index sits inside an outer freeze (`PREVIOUS(pop[r,
/// PREVIOUS(idx)])`), whose argument `builtins_visitor::hoist_capture` hoists
/// into a capture-helper aux -- so the helper evaluates to NaN at t=0 and the
/// outer `PREVIOUS` serves that NaN as the score's FIRST LIVE step (GH #975,
/// observed as `0, NaN, 0.4974...`). A NaN the engine manufactures is
/// indistinguishable on a graph from the modeller's own division by zero, which
/// is what makes this a defect rather than a cosmetic wart (see
/// [`crate::float`]'s module docs).
///
/// In index position the un-lagged index is the only well-defined answer -- at
/// the first DT "the index one step ago" IS the current index -- and it is in
/// range by construction, since it is the index the target's own equation reads
/// at that step. So the operand doubles as its own initial-value expression.
/// Passing it explicitly also *strengthens* the runlist: `classify_dependencies`
/// walks a `PREVIOUS` fallback with `in_init` (not `in_previous`) set, so the
/// index's identifiers leave `previous_only` and the same-step ordering edge the
/// fallback needs is created rather than filtered out of `dt_deps`.
///
/// Value positions keep the bare unary spelling, and deliberately -- but the
/// reason is that `0` is a VALID value, not that it is unobservable. The guard
/// form's `if (TIME = INITIAL_TIME) then 0` arm does own the score's own first
/// step, but a value-position freeze that lands inside a `hoist_capture` helper
/// is read at step 1 by exactly the route this issue is about. What makes it
/// benign there is that `0` is in range for a value where it is out of range for
/// a 1-based subscript, and that it is the spec's own answer for the
/// doubly-lagged read such a helper performs.
pub(super) fn freeze_at_previous(
    expr: Expr0,
    loc: crate::builtins::Loc,
    in_subscript_index: bool,
) -> Expr0 {
    let args = if in_subscript_index {
        vec![expr.clone(), expr]
    } else {
        vec![expr]
    };
    Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), args), loc)
}
