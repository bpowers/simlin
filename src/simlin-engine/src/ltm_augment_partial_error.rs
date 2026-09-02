// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The ceteris-paribus partial-equation FAILURE vocabulary: the typed error a
//! partial-equation builder returns instead of emitting a wrong-but-compiling
//! score, plus the array-producing-builtin walk the `RankLikePartial` class is
//! decided by. A child of `ltm_augment` (mounted via `#[path]`, so `super::*`
//! resolves the parent's items and callers keep their
//! `crate::ltm_augment::*` paths); split out only for the per-file line cap.

use crate::ast::{Expr0, IndexExpr0};
use crate::builtins::UntypedBuiltinFn;

/// A parse failure in a ceteris-paribus partial-equation builder.
///
/// The ceteris-paribus PREVIOUS-wrapping transform ([`wrap_non_matching_in_previous`])
/// can only run on a successfully-parsed `Expr0`. If `Expr0::new` returns
/// `Err` (genuinely unparseable text) or `Ok(None)` (an empty/whitespace
/// equation), there is *no* AST to wrap, so the transform cannot be applied.
///
/// Why this is an error rather than a silent fallback (GH #311): the prior
/// code returned the lowercased input text unchanged on parse failure. With
/// no PREVIOUS() wrapping, that "partial" is identical to the target's full
/// equation, so the link-score numerator `(partial - PREVIOUS(target))`
/// equals the denominator `(target - PREVIOUS(target))` and the score
/// magnitude collapses to a constant `|Δz/Δz| = 1` -- a hidden attribution
/// error that is *worse* than no score at all, and one that compiles cleanly
/// so no downstream diagnostic catches it. Returning a structured error lets
/// the (db-bearing) caller skip emitting the link-score variable and surface
/// a `Warning` naming the variable and the offending equation text, the
/// established "loud failure" pattern in this codebase
/// (cf. `emit_unscoreable_disjoint_edge_warning`).
///
/// The text being parsed is itself produced by the engine (`print_eqn` /
/// `expr2_to_string` over a compiled AST), so `Err` is effectively
/// unreachable in production; `Ok(None)` is reachable for a target with an
/// empty equation. Either way the failure is rare and unexpected -- exactly
/// the case where a silent semantics-changing fallback is most dangerous.
///
/// `UnfreezablePartial` (GH #743) is the second loud-failure class: the
/// equation parsed fine, but neither ceteris-paribus convention can be
/// rendered as a compilable equation -- the changed-first partial would
/// freeze an array slice (`PREVIOUS(matrix[d1,*])`) and the changed-last
/// fallback is unfreezable too (or has no live occurrence to freeze). The
/// caller skips the score and warns.
///
/// The compilability premise behind the array-slice half has MOVED TWICE and
/// the class is retained on neither of its original grounds. `PREVIOUS` of an
/// array slice was a hard compile error in a user equation and a
/// silently-stubbed-to-0 helper in an LTM fragment (poisoning a score into
/// plausible garbage like the constant `-1/growth-rate`); GH #1003 then
/// materialized the freeze as its own synthetic variable
/// ([`crate::ltm_augment_array_freeze`]), so most of these score instead of
/// declining; and GH #995 phase C3 gave the inline form a codegen path of its
/// own (a view over `prev_values`). What is left is the residue no helper can
/// materialize -- a dynamically pinned slice -- which is why the class stays.
/// Whether the freeze helper itself is still needed now that the inline form
/// compiles is an open simplification, deliberately not taken with C3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartialEquationErrorKind {
    /// The equation text failed to parse (or was empty); there is no AST
    /// to transform.
    Parse,
    /// Neither the changed-first nor the changed-last ceteris-paribus
    /// convention can be rendered as a compilable equation (GH #743).
    UnfreezablePartial,
    /// The live source is a BARE reference to an arrayed variable inside an
    /// array-reducer argument (GH #779): the changed-last partial cannot be
    /// rendered faithfully for it, and the spelling's own execution
    /// semantics carry a spurious factor (GH #789). Selects a diagnostic
    /// that names the shape and the subscripted-spelling workaround.
    BareReducerFeeder,
    /// An arrayed dep of the target's equation cannot be projected onto the
    /// target element this partial is for, so no correct element subscript
    /// exists for it. `equation_text` carries `dep@element`. Emitting anyway
    /// leaves the dep's dimension-name subscript in a scalar fragment, which
    /// becomes a `PREVIOUS`-capture helper that cannot lower WHILE THE PARENT
    /// STILL COMPILES -- a score that silently reads part of its own equation
    /// as 0. The reachable cause is a pair with no DECLARED correspondence at
    /// all -- two dimensions sharing element names, which the simulation
    /// resolves by name while `allocate_implicit_axes_partial` pairs axes only
    /// by name or by a declared mapping. (An explicit element map was the
    /// reachable cause until GH #997 made that spelling projectable.)
    UnprojectableDep,
    /// The target's equation applies an ORDER-STATISTIC, array-producing
    /// builtin (`VECTOR SORT ORDER`, `RANK`, `ALLOCATE AVAILABLE`,
    /// `ALLOCATE BY PRIORITY`) and this partial is a per-element SCALAR one
    /// (GH #995 option C): the scalarization pins the builtin's argument down
    /// to a single element, and an order statistic of one element is
    /// meaningless (`vm_vector_sort_order` on a 1-element view is rank 0
    /// always). Today such a fragment also fails codegen loudly
    /// (an array in a position that consumes one value); declining at
    /// generation keeps the drop loud even if a future widening of the
    /// materializer (option A) makes the fragment compile -- which would
    /// otherwise convert it into a silent constant-0 partial. The element pin belongs on the
    /// RESULT (the A2A-shaped whole-array score, which stays emitted), never
    /// on a rank-like builtin's argument.
    RankLikePartial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartialEquationError {
    /// The original (pre-transform) equation text the failure is about. The
    /// db-bearing caller embeds this in the diagnostic message so the failure
    /// names the concrete offending equation.
    pub equation_text: String,
    /// Which loud-failure class this is; selects the diagnostic wording.
    pub kind: PartialEquationErrorKind,
}

impl PartialEquationError {
    pub(crate) fn new(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::Parse,
        }
    }

    pub(super) fn unfreezable(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::UnfreezablePartial,
        }
    }

    pub(super) fn bare_reducer_feeder(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::BareReducerFeeder,
        }
    }

    /// `dep` cannot be projected onto target element `element`.
    pub(crate) fn unprojectable_dep(dep: &str, element: &str) -> Self {
        PartialEquationError {
            equation_text: format!("{dep}@{element}"),
            kind: PartialEquationErrorKind::UnprojectableDep,
        }
    }

    pub(super) fn rank_like_partial(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::RankLikePartial,
        }
    }
}

/// Does `expr` apply an ARRAY-PRODUCING builtin -- the set a per-element
/// SCALAR partial must decline over (GH #995 option C)?
///
/// The set is exactly codegen's AssignTemp-required family
/// (`compiler::codegen`'s `TodoArrayBuiltin` arms): `VECTOR SORT ORDER`,
/// `RANK`, `ALLOCATE AVAILABLE`, `ALLOCATE BY PRIORITY`, and
/// `VECTOR ELM MAP` -- every builtin whose RESULT is an array, which a
/// scalar fragment cannot hold. The order-statistic subset (everything but
/// ELM MAP) is additionally a semantic trap: pinning its argument to one
/// element changes the ranking rather than selecting a slot, so those must
/// stay declined even if a future widening of the materializer makes the
/// fragment compile. Deliberately NOT in the set: `VECTOR SELECT`, whose selection
/// reduces to a scalar (per-element pinning of the non-reduced axes is
/// exactly right). This is the same result-type distinction
/// `ltm_agg::reducer_collapses_to_scalar` draws for `RANK` (GH #771/#742),
/// applied at the partial-generation boundary.
pub(super) fn contains_rank_like_builtin(expr: &Expr0) -> bool {
    let is_rank_like = |name: &str| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "vector_sort_order"
                | "rank"
                | "allocate_available"
                | "allocate_by_priority"
                | "vector_elm_map"
        )
    };
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::Subscript(_, indices, _) => indices.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => contains_rank_like_builtin(e),
            IndexExpr0::Range(l, r, _) => {
                contains_rank_like_builtin(l) || contains_rank_like_builtin(r)
            }
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            is_rank_like(name) || args.iter().any(contains_rank_like_builtin)
        }
        Expr0::Op1(_, inner, _) => contains_rank_like_builtin(inner),
        Expr0::Op2(_, l, r, _) => contains_rank_like_builtin(l) || contains_rank_like_builtin(r),
        Expr0::If(c, t, e, _) => {
            contains_rank_like_builtin(c)
                || contains_rank_like_builtin(t)
                || contains_rank_like_builtin(e)
        }
    }
}
