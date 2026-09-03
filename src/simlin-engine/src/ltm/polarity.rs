// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static polarity analysis for causal links.
//!
//! Determines whether an increase in `from` produces an increase or
//! decrease in `to` by recursively walking `Expr2` ASTs. Loop polarity
//! (Reinforcing / Balancing / Undetermined) is then derived by counting
//! negative links per cycle in `graph.rs`.

use crate::ast::{Ast, BinaryOp, Expr2, IndexExpr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident};
use crate::variable::{VarKind, Variable};

use super::types::{LinkPolarity, normalize_module_ref};

/// Analyze the polarity of how a variable appears in an equation
pub(super) fn analyze_link_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &crate::variable::LoweredVariableMap,
) -> LinkPolarity {
    analyze_ast_polarity(ast, from_var, variables)
}

/// Polarity of a hoisted reducer's body with respect to ONE source variable
/// referenced inside it -- the discriminating polarity of a
/// `source -> $⁚ltm⁚agg⁚{n}` hop (GH #737 review follow-ups I1/I1b). The
/// source may be a scalar feeder (`scale` in `SUM(pop[*] * scale)`) or an
/// arrayed row / co-source (`pop` or `weight` in
/// `SUM(pop[*] * (1 - weight[*]))`): either way the hop's sign is the sign
/// of d(body)/d(source), which only the body's composition determines --
/// "the reducer is monotone in its summands" says nothing about a variable
/// the summand body negates.
///
/// So `SUM(pop[*])` and `SUM(pop[*] * scale)` w.r.t. `pop` are Positive,
/// `SUM(pop[*] * scale)` w.r.t. `scale` is Positive,
/// `SUM(pop[*] * (1 - weight[*]))` w.r.t. `weight` is Negative, and an
/// indeterminate body (a compound co-factor like `(k - pop[*])`, or a
/// non-monotone reducer like STDDEV) stays Unknown -- never a confident
/// wrong label.
///
/// This used to be `analyze_link_polarity` with a feeder-hop-only
/// `mul_convention` flag enabling the positive-by-convention Mul one-side
/// rule; that rule is now part of the general analyzer (see the Mul arm of
/// [`analyze_expr_polarity_with_context`]), so the only thing this entry
/// point adds is accepting a bare `BuiltinFn` instead of a whole equation.
///
/// `reducer` is the reducer call itself (`ltm_agg::AggNode::reducer`), not a
/// whole equation: an aggregate node's equation IS one reducer application,
/// so the `Ast` wrapper the caller used to reconstruct carried no
/// information -- [`analyze_ast_polarity`] treats `Ast::Scalar` and
/// `Ast::ApplyToAll` through the same arm, and an agg is never
/// `Ast::Arrayed`.
pub(super) fn analyze_source_to_agg_polarity(
    reducer: &BuiltinFn<Expr2>,
    source: &Ident<Canonical>,
    variables: &crate::variable::LoweredVariableMap,
) -> LinkPolarity {
    analyze_builtin_polarity(reducer, source, LinkPolarity::Positive, Some(variables))
}

/// AST-level dispatch for [`analyze_link_polarity`]: per-element equations
/// fold like the `Ast::Arrayed` rule (first concrete polarity wins; a
/// direction disagreement collapses to Unknown).
fn analyze_ast_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &crate::variable::LoweredVariableMap,
) -> LinkPolarity {
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::Arrayed(_, elements, default_expr, _) => {
            // For arrayed equations, check all elements
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let elem_polarity = analyze_expr_polarity_with_context(
                    expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                );
                if polarity == LinkPolarity::Unknown {
                    polarity = elem_polarity;
                } else if polarity != elem_polarity && elem_polarity != LinkPolarity::Unknown {
                    // Mixed polarities
                    return LinkPolarity::Unknown;
                }
            }
            if let Some(default_expr) = default_expr {
                let default_polarity = analyze_expr_polarity_with_context(
                    default_expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                );
                if polarity == LinkPolarity::Unknown {
                    polarity = default_polarity;
                } else if polarity != default_polarity && default_polarity != LinkPolarity::Unknown
                {
                    return LinkPolarity::Unknown;
                }
            }
            polarity
        }
    }
}

/// Polarity of `consumer_ast` with respect to a reducer subexpression --
/// the polarity of a synthetic aggregate-node hop `$⁚ltm⁚agg → consumer`.
///
/// The aggregate node stands in for an inlined reducer (`SUM(pop[*])`,
/// `MEAN(...)`) that appears in `consumer`'s equation as a `SUM(...)`
/// subexpression rather than as a variable reference, so ordinary
/// `analyze_link_polarity` (which matches `Var(agg)` occurrences) returns
/// `Unknown`. This substitutes the subexpression -- matched by its
/// canonical printed form `reducer_subexpr_text` (exactly the
/// `AggNode::reducer_key` `enumerate_agg_nodes` stores) -- with a
/// bare `Var(agg_name)` and runs the ordinary analysis on the result.
///
/// Returns `Unknown` if the subexpression isn't found (graceful: the hop
/// then stays Unknown-polarity, as it was before GH #516).
pub(super) fn analyze_agg_consumer_polarity(
    consumer_ast: &Ast<Expr2>,
    reducer_subexpr_text: &str,
    agg_name: &Ident<Canonical>,
    variables: &crate::variable::LoweredVariableMap,
) -> LinkPolarity {
    let analyze = |expr: &Expr2| -> LinkPolarity {
        let substituted = substitute_subexpr_in_expr2(expr, reducer_subexpr_text, agg_name);
        analyze_expr_polarity_with_context(
            &substituted,
            agg_name,
            LinkPolarity::Positive,
            Some(variables),
        )
    };
    match consumer_ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => analyze(expr),
        Ast::Arrayed(_, elements, default_expr, _) => {
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let p = analyze(expr);
                if polarity == LinkPolarity::Unknown {
                    polarity = p;
                } else if polarity != p && p != LinkPolarity::Unknown {
                    return LinkPolarity::Unknown;
                }
            }
            if let Some(default_expr) = default_expr {
                let p = analyze(default_expr);
                if polarity == LinkPolarity::Unknown {
                    polarity = p;
                } else if polarity != p && p != LinkPolarity::Unknown {
                    return LinkPolarity::Unknown;
                }
            }
            polarity
        }
    }
}

/// Rebuild `expr`, replacing every subtree whose canonical printed form
/// equals `target_text` with a bare `Var(replacement)`. Used only by
/// [`analyze_agg_consumer_polarity`]; the printed-form comparison mirrors
/// how `enumerate_agg_nodes` keys aggregate nodes (`Expr2` is `Eq` but not
/// `Hash`, so the dedup map is keyed by that printed form).
fn substitute_subexpr_in_expr2(
    expr: &Expr2,
    target_text: &str,
    replacement: &Ident<Canonical>,
) -> Expr2 {
    if crate::patch::expr2_to_string(expr) == target_text {
        return Expr2::Var(replacement.clone(), None, crate::ast::Loc::default());
    }
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => expr.clone(),
        Expr2::App(builtin, bounds, loc) => Expr2::App(
            builtin
                .clone()
                .map(|e| substitute_subexpr_in_expr2(&e, target_text, replacement)),
            bounds.clone(),
            *loc,
        ),
        Expr2::Subscript(ident, indices, bounds, loc) => Expr2::Subscript(
            ident.clone(),
            indices
                .iter()
                .map(|idx| substitute_subexpr_in_index(idx, target_text, replacement))
                .collect(),
            bounds.clone(),
            *loc,
        ),
        Expr2::Op1(op, rhs, bounds, loc) => Expr2::Op1(
            *op,
            Box::new(substitute_subexpr_in_expr2(rhs, target_text, replacement)),
            bounds.clone(),
            *loc,
        ),
        Expr2::Op2(op, lhs, rhs, bounds, loc) => Expr2::Op2(
            *op,
            Box::new(substitute_subexpr_in_expr2(lhs, target_text, replacement)),
            Box::new(substitute_subexpr_in_expr2(rhs, target_text, replacement)),
            bounds.clone(),
            *loc,
        ),
        Expr2::If(cond, then_e, else_e, bounds, loc) => Expr2::If(
            Box::new(substitute_subexpr_in_expr2(cond, target_text, replacement)),
            Box::new(substitute_subexpr_in_expr2(
                then_e,
                target_text,
                replacement,
            )),
            Box::new(substitute_subexpr_in_expr2(
                else_e,
                target_text,
                replacement,
            )),
            bounds.clone(),
            *loc,
        ),
    }
}

fn substitute_subexpr_in_index(
    idx: &IndexExpr2,
    target_text: &str,
    replacement: &Ident<Canonical>,
) -> IndexExpr2 {
    match idx {
        IndexExpr2::Expr(e) => {
            IndexExpr2::Expr(substitute_subexpr_in_expr2(e, target_text, replacement))
        }
        IndexExpr2::Range(l, r, loc) => IndexExpr2::Range(
            substitute_subexpr_in_expr2(l, target_text, replacement),
            substitute_subexpr_in_expr2(r, target_text, replacement),
            *loc,
        ),
        other => other.clone(),
    }
}

/// The `Expr2::App` half of [`analyze_expr_polarity_with_context`],
/// reachable from a `BuiltinFn` on its own so that a hoisted reducer's
/// polarity ([`analyze_source_to_agg_polarity`]) can be analysed without
/// wrapping the builtin back up in an `Expr2::App` it was taken out of.
///
/// The `App` node's own `ArrayBounds` and `Loc` are not read by any arm, so
/// nothing is lost by dropping the wrapper.
fn analyze_builtin_polarity(
    builtin: &BuiltinFn<Expr2>,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> LinkPolarity {
    match builtin {
        // All three lookup variants share the `(table_expr, index_expr, loc)`
        // shape and the same polarity story: the result is non-decreasing in
        // the index when the table is, so the link polarity is the argument's
        // monotonicity composed with the table's.
        BuiltinFn::Lookup(table_expr, index_expr, _)
        | BuiltinFn::LookupForward(table_expr, index_expr, _)
        | BuiltinFn::LookupBackward(table_expr, index_expr, _) => {
            let arg_polarity = analyze_expr_polarity_with_context(
                index_expr,
                from_var,
                LinkPolarity::Positive,
                variables,
            );

            if arg_polarity == LinkPolarity::Unknown {
                return LinkPolarity::Unknown;
            }

            // Composing argument monotonicity with table monotonicity is plain
            // sign multiplication; an Unknown on either side absorbs.
            arg_polarity.compose(lookup_table_polarity(table_expr, variables))
        }
        // Non-decreasing single-arg builtins: propagate inner polarity.
        // Int (floor) and Round (nearest, ties to even) are step functions
        // with discontinuities, but are still non-decreasing, which is
        // sufficient for polarity propagation.
        BuiltinFn::Exp(inner)
        | BuiltinFn::Ln(inner)
        | BuiltinFn::Log10(inner)
        | BuiltinFn::Sqrt(inner)
        | BuiltinFn::Arctan(inner)
        | BuiltinFn::Int(inner)
        | BuiltinFn::Round(inner) => {
            analyze_expr_polarity_with_context(inner, from_var, current_polarity, variables)
        }
        // Max/Min (scalar two-arg form): non-decreasing in each argument
        BuiltinFn::Max(a, Some(b)) | BuiltinFn::Min(a, Some(b)) => {
            let pol_a =
                analyze_expr_polarity_with_context(a, from_var, current_polarity, variables);
            let pol_b =
                analyze_expr_polarity_with_context(b, from_var, current_polarity, variables);
            match (pol_a, pol_b) {
                // When one side returns Unknown, we must check whether it actually
                // references from_var. Unknown from an independent expression (e.g.
                // a constant or unrelated variable) means we can use the other side's
                // polarity. Unknown from a dependent expression (e.g. ABS(x)) means
                // the result is truly non-monotonic.
                (LinkPolarity::Unknown, known) => {
                    if expr_references_var(a, from_var) {
                        LinkPolarity::Unknown
                    } else {
                        known
                    }
                }
                (known, LinkPolarity::Unknown) => {
                    if expr_references_var(b, from_var) {
                        LinkPolarity::Unknown
                    } else {
                        known
                    }
                }
                // Both agree: propagate
                (a_pol, b_pol) if a_pol == b_pol => a_pol,
                // Disagree: unknown
                _ => LinkPolarity::Unknown,
            }
        }
        // Array reducers SUM and MEAN: monotone in every input element, so
        // polarity is the polarity of the (single) array argument.
        // MEAN's variant carries Vec<Expr> to also represent the variadic scalar
        // form MEAN(a, b, c); for polarity that form is still monotone in each
        // argument, so we combine arg polarities the same way Add does (any
        // disagreement collapses to Unknown).
        BuiltinFn::Sum(arg) => {
            analyze_expr_polarity_with_context(arg, from_var, current_polarity, variables)
        }
        BuiltinFn::Mean(args) => {
            let mut combined = LinkPolarity::Unknown;
            for arg in args {
                let arg_pol =
                    analyze_expr_polarity_with_context(arg, from_var, current_polarity, variables);
                // Hoist the self-reference + Unknown short circuit ahead of the
                // per-arg combiner so that any non-monotone dependence on
                // from_var (e.g. ABS(x)) collapses the whole mean to Unknown,
                // regardless of arg order. This mirrors the Add path: an
                // Unknown that references from_var poisons the result; an
                // Unknown that's independent of from_var (e.g. an unrelated
                // variable or constant) is just skipped. Without this hoist a
                // first-iteration ABS(x) would seed `combined` with Unknown and
                // then be silently overwritten by a later known-polarity arg.
                if arg_pol == LinkPolarity::Unknown && expr_references_var(arg, from_var) {
                    return LinkPolarity::Unknown;
                }
                match (combined, arg_pol) {
                    // Independent Unknown (constant, unrelated var): skip.
                    (_, LinkPolarity::Unknown) => {}
                    // First known polarity wins.
                    (LinkPolarity::Unknown, pol) => combined = pol,
                    // Same polarity across args: stable.
                    (a_pol, b_pol) if a_pol == b_pol => {}
                    // Disagreement among known polarities collapses to Unknown.
                    _ => return LinkPolarity::Unknown,
                }
            }
            combined
        }
        // Array reducers MAX/MIN (single-arg form): max/min of a monotone family
        // is monotone, so propagate the inner polarity.
        BuiltinFn::Max(a, None) | BuiltinFn::Min(a, None) => {
            analyze_expr_polarity_with_context(a, from_var, current_polarity, variables)
        }
        // STDDEV is non-monotone (variance has no fixed sign w.r.t. inputs).
        // RANK depends on the rest of the array, so its sign w.r.t. one element
        // is not determined. Both must explicitly return Unknown.
        BuiltinFn::Stddev(_) | BuiltinFn::Rank(_, _) => LinkPolarity::Unknown,
        _ => LinkPolarity::Unknown,
    }
}

/// The recursive polarity walk: how does `expr` move when `from_var` moves,
/// with optional variable context for constant-sign and lookup-table
/// resolution.
pub(super) fn analyze_expr_polarity_with_context(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> LinkPolarity {
    match expr {
        Expr2::Const(_, _, _) => LinkPolarity::Unknown,
        Expr2::Var(ident, _, _) => {
            let normalized = normalize_module_ref(ident);
            if &normalized == from_var || ident == from_var {
                current_polarity
            } else {
                LinkPolarity::Unknown
            }
        }
        // Whole-array reductions wrap a Subscript around the same identifier
        // that a scalar reference would carry as Expr2::Var. The reducer arms
        // below (Sum/Mean/single-arg Max/Min) recurse into their argument; for
        // the production case `SUM(x[*])` that argument lowers to
        // `Subscript(x, [Wildcard], _, _)`, not `Var(x, ...)`. Mirror the Var
        // handler so the identifier comparison succeeds and the reducer's
        // monotonicity guarantee carries through.
        //
        // When the array name matches `from_var`, the indices still need
        // inspection: if any index expression also references `from_var`
        // (e.g. `arr[INT(arr[i])]` or `arr[arr]`), the relationship is
        // non-monotone -- shifting `from_var` moves both the lookup target
        // and the index in lockstep -- and we must return Unknown. The
        // dominant cases (literal, wildcard, range, expressions over OTHER
        // variables) leave indices independent of `from_var`, and the
        // reducer's monotonicity guarantee carries through unchanged.
        //
        // When the array name does NOT match `from_var`, contribute Unknown:
        // we can't classify references that thread through another array
        // here. Combining operators above (Add/Sub/Mul/Div, Mean variadic)
        // detect any `from_var` reference inside indices via their own
        // `expr_references_var` checks.
        Expr2::Subscript(ident, indices, _, _) => {
            let normalized = normalize_module_ref(ident);
            if &normalized == from_var || ident == from_var {
                if indices.iter().any(|idx| match idx {
                    IndexExpr2::Expr(e) => expr_references_var(e, from_var),
                    IndexExpr2::Range(lo, hi, _) => {
                        expr_references_var(lo, from_var) || expr_references_var(hi, from_var)
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => false,
                }) {
                    LinkPolarity::Unknown
                } else {
                    current_polarity
                }
            } else {
                LinkPolarity::Unknown
            }
        }
        Expr2::App(builtin, _, _) => {
            analyze_builtin_polarity(builtin, from_var, current_polarity, variables)
        }
        Expr2::Op2(op, left, right, _, _) => {
            let left_pol =
                analyze_expr_polarity_with_context(left, from_var, current_polarity, variables);
            let right_pol =
                analyze_expr_polarity_with_context(right, from_var, current_polarity, variables);

            match op {
                BinaryOp::Add => match (left_pol, right_pol) {
                    (LinkPolarity::Unknown, pol) => {
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (pol, LinkPolarity::Unknown) => {
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (a, b) if a == b => a,
                    _ => LinkPolarity::Unknown,
                },
                BinaryOp::Sub => match (left_pol, right_pol) {
                    (LinkPolarity::Unknown, pol) => {
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            flip_polarity(pol)
                        }
                    }
                    (pol, LinkPolarity::Unknown) => {
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (a, b) if a == flip_polarity(b) => a,
                    _ => LinkPolarity::Unknown,
                },
                BinaryOp::Mul => {
                    // Multiplication needs the SIGN of the other operand to
                    // determine polarity, not just whether it's independent of
                    // from_var. This is why Mul consults `cofactor_value_sign`
                    // rather than the bare expr_references_var pattern Add/Sub
                    // use.
                    if left_pol != LinkPolarity::Unknown && right_pol != LinkPolarity::Unknown {
                        // BOTH factors depend on from_var (a non-Unknown
                        // polarity only arises from a from_var reference --
                        // constants and unrelated variables analyze to
                        // Unknown). The product rule
                        // d(f*g)/dx = f'g + fg' mixes the operands' VALUES
                        // into the partial's sign, so the derivative signs
                        // alone do not determine it. Under the positive-value
                        // labeling convention (see the Div arm) the sum IS
                        // sign-definite when both derivative signs AGREE and
                        // both operand values are positive-by-convention:
                        // f,g > 0 and sign(f') == sign(g') gives f'g + fg'
                        // that shared sign. That covers `pop * pop / capacity`
                        // (quadratic crowding: P). Everything else is
                        // genuinely indeterminate: the pre-fix sign
                        // COMPOSITION labeled logistic growth
                        // `r*pop*(1 - pop/K)` a definite Negative while its
                        // true partial `r*(1 - 2*pop/K)` flips sign at K/2 --
                        // and no value convention rescues it, because the
                        // factor `(1 - pop/K)` is a compound expression whose
                        // value sign itself flips.
                        if left_pol == right_pol
                            && operand_positive_by_convention(left, variables)
                            && operand_positive_by_convention(right, variables)
                        {
                            left_pol
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else if left_pol != LinkPolarity::Unknown {
                        // Only left carries from_var's derivative sign; the
                        // co-factor scales it by its VALUE sign. A provable
                        // sign (a numeric literal through any unary
                        // negations, or a variable whose whole equation is
                        // one) is used exactly; a bare named quantity is
                        // positive by the SD labeling convention -- the same
                        // convention the Div arm and the both-sides rules
                        // apply, and the reading every CLD gives
                        // `net_growth = population * fractional_growth`
                        // (`population -> net_growth` is `+`). A compound
                        // co-factor (`k - x`, `1 - pop/K`) has a DERIVED
                        // value sign, not a conventional one -- that is the
                        // logistic-growth class whose sign genuinely flips
                        // -- so it stays Unknown. The `expr_references_var`
                        // guard keeps a co-factor that depends on `from_var`
                        // non-monotonically (`ABS(x)`, or `pop[from_var]`
                        // threading it through an index) on the Unknown
                        // path: such a factor is not a pure scale.
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            match cofactor_value_sign(right, variables) {
                                Some(true) => left_pol,
                                Some(false) => flip_polarity(left_pol),
                                None => LinkPolarity::Unknown,
                            }
                        }
                    } else if right_pol != LinkPolarity::Unknown {
                        // Mirror of the left-carries-polarity arm above.
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            match cofactor_value_sign(left, variables) {
                                Some(true) => right_pol,
                                Some(false) => flip_polarity(right_pol),
                                None => LinkPolarity::Unknown,
                            }
                        }
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Div => {
                    // Division's partial depends on the VALUE sign of the
                    // from_var-independent operand, not just independence:
                    //   d(n/y)/dy = -n/y^2      -- sign is -sign(n)
                    //   d(f/y)/dx = f'(x)/y     -- sign is sign(f')*sign(y)
                    // When the independent operand's sign is PROVABLE (a
                    // numeric literal through any unary negations, or a
                    // variable whose whole equation is one), use it -- the
                    // pre-fix rules ignored it entirely, mislabeling `-5/y`
                    // (truly Positive) as Negative and `x/-5` (truly
                    // Negative) as Positive.
                    //
                    // For a NON-constant independent operand we keep the
                    // conventional SD assumption that quantities are
                    // positive-valued (numerator passes polarity through,
                    // denominator flips it). This is a documented labeling
                    // CONVENTION, not a proof: `share = pop / total` reads as
                    // "total -> share is Negative" on every SD diagram even
                    // though no analysis proves `pop > 0`. A sign-flipping
                    // operand value would flip the label -- but a divisor
                    // that crosses zero is already numerically catastrophic,
                    // so real models keep divisor/numerator signs fixed, and
                    // the runtime loop-score SIGN factors (which this label
                    // never feeds) remain exact regardless. Note the Div
                    // convention is deliberately BROADER than Mul's: it
                    // applies to compound independent operands too
                    // (`(a-b)/y` flips), because the
                    // divisor-cannot-cross-zero argument covers the whole
                    // quotient, whereas a compound Mul co-factor crossing
                    // zero is routine (the logistic-growth class) and stays
                    // Unknown.
                    let value_sign =
                        |e: &Expr2| -> Option<bool> { provable_value_sign(e, variables) };
                    match (left_pol, right_pol) {
                        (LinkPolarity::Unknown, pol) => {
                            if expr_references_var(left, from_var) {
                                LinkPolarity::Unknown
                            } else {
                                match value_sign(left) {
                                    // Provably negative numerator inverts the
                                    // conventional denominator flip; unknown
                                    // sign falls back to the positive-value
                                    // convention (flip).
                                    Some(false) => pol,
                                    _ => flip_polarity(pol),
                                }
                            }
                        }
                        (pol, LinkPolarity::Unknown) => {
                            if expr_references_var(right, from_var) {
                                LinkPolarity::Unknown
                            } else {
                                match value_sign(right) {
                                    // Provably negative denominator inverts
                                    // the conventional pass-through; unknown
                                    // sign falls back to the positive-value
                                    // convention (pass through).
                                    Some(false) => flip_polarity(pol),
                                    _ => pol,
                                }
                            }
                        }
                        // Both sides depend on from_var: the quotient rule
                        // d(f/g)/dx = (f'g - fg')/g^2. Mirroring the Mul
                        // both-sides case, the sign is determinate under the
                        // positive-value convention exactly when the
                        // derivative signs OPPOSE (f' > 0, g' < 0 with
                        // f,g > 0 gives both quotient-rule terms positive,
                        // and vice versa) and both operand values are
                        // positive-by-convention. A compound operand like
                        // `(1 - x)` in `exp(x)/(1 - x)` defeats the value
                        // convention (its own sign flips, here at x = 1, and
                        // the partial's at x = 2), so it stays Unknown.
                        (a, b)
                            if a == flip_polarity(b)
                                && operand_positive_by_convention(left, variables)
                                && operand_positive_by_convention(right, variables) =>
                        {
                            a
                        }
                        _ => LinkPolarity::Unknown,
                    }
                }
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::Op1(op, operand, _, _) => {
            let operand_pol =
                analyze_expr_polarity_with_context(operand, from_var, current_polarity, variables);
            match op {
                crate::ast::UnaryOp::Not => flip_polarity(operand_pol),
                crate::ast::UnaryOp::Negative => flip_polarity(operand_pol),
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::If(_, true_branch, false_branch, _, _) => {
            // For IF-THEN-ELSE, check both branches
            let true_pol = analyze_expr_polarity_with_context(
                true_branch,
                from_var,
                current_polarity,
                variables,
            );
            let false_pol = analyze_expr_polarity_with_context(
                false_branch,
                from_var,
                current_polarity,
                variables,
            );

            if true_pol == false_pol {
                true_pol
            } else {
                LinkPolarity::Unknown
            }
        }
    }
}

/// Flip the polarity
pub(super) fn flip_polarity(pol: LinkPolarity) -> LinkPolarity {
    match pol {
        LinkPolarity::Positive => LinkPolarity::Negative,
        LinkPolarity::Negative => LinkPolarity::Positive,
        LinkPolarity::Unknown => LinkPolarity::Unknown,
    }
}

/// Check whether an expression tree contains any reference to a specific variable.
/// Used to distinguish "independent of from_var" (returns Unknown because expression
/// doesn't reference from_var at all) from "non-monotonically dependent" (returns
/// Unknown but DOES reference from_var, e.g. ABS(x)).
pub(super) fn expr_references_var(expr: &Expr2, var: &Ident<Canonical>) -> bool {
    match expr {
        Expr2::Const(_, _, _) => false,
        Expr2::Var(ident, _, _) => ident == var || &normalize_module_ref(ident) == var,
        Expr2::Subscript(ident, indices, _, _) => {
            ident == var
                || indices.iter().any(|idx| match idx {
                    IndexExpr2::Expr(e) => expr_references_var(e, var),
                    IndexExpr2::Range(lo, hi, _) => {
                        expr_references_var(lo, var) || expr_references_var(hi, var)
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => false,
                })
        }
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            builtin.for_each_expr_ref(|child| {
                if !found {
                    found = expr_references_var(child, var);
                }
            });
            found
        }
        Expr2::Op2(_, left, right, _, _) => {
            expr_references_var(left, var) || expr_references_var(right, var)
        }
        Expr2::Op1(_, operand, _, _) => expr_references_var(operand, var),
        Expr2::If(cond, t, f, _, _) => {
            expr_references_var(cond, var)
                || expr_references_var(t, var)
                || expr_references_var(f, var)
        }
    }
}

/// Sign of a numeric-literal expression, seen through any chain of unary
/// negations: `Some(true)` for `5` / `--5`, `Some(false)` for `-5`, `None`
/// for `0` or anything that is not a literal.
///
/// Seeing through `Op1(Negative, ..)` is load-bearing, not a nicety: the
/// lexer takes no leading sign (`lexer::scan_number`), so a model equation
/// `-5` reaches this analysis as a NEGATION of the literal `5` and a
/// Const-only predicate is blind to every negative constant a user can
/// actually write.
pub(super) fn literal_sign(expr: &Expr2) -> Option<bool> {
    match expr {
        Expr2::Const(_, n, _) => {
            let v = n.value();
            if v > 0.0 {
                Some(true)
            } else if v < 0.0 {
                Some(false)
            } else {
                None
            }
        }
        Expr2::Op1(crate::ast::UnaryOp::Negative, inner, _, _) => {
            literal_sign(inner).map(|sign| !sign)
        }
        _ => None,
    }
}

/// PROVABLE value sign of an expression: a numeric literal (through unary
/// negations), or a bare reference to a variable whose whole scalar
/// equation is one. `None` = not provable. Deliberately does not chase
/// variable-to-variable chains (`a = b; b = 5`): one level matches the
/// historical `is_positive_variable` depth and cannot recurse on a cycle.
pub(super) fn provable_value_sign(
    expr: &Expr2,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> Option<bool> {
    if let Some(sign) = literal_sign(expr) {
        return Some(sign);
    }
    match expr {
        Expr2::Op1(crate::ast::UnaryOp::Negative, inner, _, _) => {
            provable_value_sign(inner, variables).map(|sign| !sign)
        }
        Expr2::Var(ident, _, _) => {
            let var = variables?.get(ident)?;
            if let Some(Ast::Scalar(var_expr)) = var.ast() {
                literal_sign(var_expr)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// VALUE sign of a Mul co-factor already known to be independent of
/// `from_var`: a provable sign when one exists, else positive by the SD
/// labeling convention for a bare named quantity (`Var` / `Subscript` --
/// stocks, flows, rates, capacities are conventionally positive-valued,
/// which is the sign every CLD assigns such a link), propagated through
/// unary negation (`-z` is negative by the same convention). A COMPOUND
/// co-factor (`k - x`, `1 - pop/K`) gets `None`: its value sign is derived,
/// not conventional, and the canonical mid-run polarity flips (logistic
/// growth) come exactly from such factors.
fn cofactor_value_sign(
    expr: &Expr2,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> Option<bool> {
    if let Some(sign) = provable_value_sign(expr, variables) {
        return Some(sign);
    }
    match expr {
        Expr2::Op1(crate::ast::UnaryOp::Negative, inner, _, _) => {
            cofactor_value_sign(inner, variables).map(|sign| !sign)
        }
        Expr2::Var(..) | Expr2::Subscript(..) => Some(true),
        _ => None,
    }
}

/// Whether an operand's runtime VALUE may be assumed positive under the SD
/// labeling convention used by the Mul/Div both-sides-dependent polarity
/// rules: a bare variable/subscript reference (named SD quantities --
/// stocks, flows, rates, capacities -- are conventionally positive-valued),
/// a positive numeric constant, or a variable whose whole equation is one.
///
/// A COMPOUND expression (`1 - pop/K`, `K - pop`, ...) is never
/// positive-by-convention: its value sign is derived, not a modeling
/// convention, and the canonical mid-run polarity flips (logistic growth)
/// come exactly from such factors. A provably-negative constant or
/// constant-valued variable is excluded defensively even though the
/// both-sides arms can only see operands that reference `from_var`.
fn operand_positive_by_convention(
    expr: &Expr2,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> bool {
    match provable_value_sign(expr, variables) {
        Some(sign) => sign,
        None => matches!(expr, Expr2::Var(..) | Expr2::Subscript(..)),
    }
}

/// Analyze the polarity of a graphical function/lookup table
/// Returns Positive if monotonically increasing, Negative if monotonically decreasing, Unknown otherwise
pub(super) fn analyze_graphical_function_polarity(table: &crate::variable::Table) -> LinkPolarity {
    // Need at least 2 points to determine monotonicity
    if table.x.len() < 2 || table.y.len() < 2 {
        return LinkPolarity::Unknown;
    }

    let mut all_increasing = true;
    let mut all_decreasing = true;
    let mut all_constant = true;

    // Classify each segment by its SLOPE `dy/dx`, not the raw y-delta `dy`
    // (#536). Comparing `dy` against a y-range-relative epsilon (#492) is wrong
    // for non-uniform x-spacing: a small `dy` over a small `dx` is a large
    // slope (a real, fast change) yet reads as a plateau, while a small `dy`
    // over a wide `dx` is a negligible slope yet reads as a real change. Either
    // way the monotonicity verdict can be wrong.
    //
    // The slope tolerance is set as `1e-6 * (y_max - y_min) / avg_dx` where
    // `avg_dx = x_span / (n - 1)` is the average x-spacing. The per-segment
    // noise threshold then becomes `slope_epsilon * dx = 1e-6 * (y_max - y_min)
    // * (dx / avg_dx)`. On uniformly-spaced tables every segment has `dx ==
    // avg_dx`, so the threshold reduces EXACTLY to `1e-6 * (y_max - y_min)` --
    // the same y-range-relative dy epsilon #492 used, preserving import-noise
    // tolerance for finely-sampled tables. For non-uniform tables the threshold
    // scales proportionally with segment width, so a narrow steep segment (small
    // dx, large slope) is still caught by the slope comparison while a wide
    // gentle segment (large dx, small slope) keeps the same proportional
    // tolerance -- the original #536 motivation.
    //
    // Ascending x is the VM binary-search lookup precondition (vm.rs `Lookup`),
    // so dx > 0 on any runtime-valid table and the slope sign equals the dy sign.
    let y_min = table.y.iter().copied().fold(f64::INFINITY, f64::min);
    let y_max = table.y.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let x_min = table.x.iter().copied().fold(f64::INFINITY, f64::min);
    let x_max = table.x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let x_span = x_max - x_min;

    // Check consecutive pairs of points. `x.len() == y.len()` after
    // `parse_table` (an absent `x_points` is filled with a uniform ramp), so
    // index `i` is in bounds for both; iterate the common length defensively
    // anyway in case a `Table` is ever built with mismatched columns.
    let n = table.x.len().min(table.y.len());
    // avg_dx: average x-spacing over the n-1 segments in the iterated range.
    // When x_span == 0 or n < 2 the table is degenerate; slope_epsilon falls
    // back to the absolute floor.
    let avg_dx = if n >= 2 && x_span > 0.0 {
        x_span / (n - 1) as f64
    } else {
        0.0
    };
    let slope_epsilon = if avg_dx > 0.0 {
        (1e-6 * (y_max - y_min) / avg_dx).max(1e-12)
    } else {
        1e-12
    };
    for i in 1..n {
        let dx = table.x[i] - table.x[i - 1];
        let dy = table.y[i] - table.y[i - 1];

        if dx == 0.0 {
            // Degenerate vertical segment. A duplicate point (dy == 0 too) is
            // redundant -- skip it as non-determining. A genuine vertical step
            // (dy != 0) is an ambiguous lookup (two outputs for one input) with
            // an undefined slope, so bail to Unknown rather than guess a
            // polarity.
            if dy == 0.0 {
                continue;
            }
            return LinkPolarity::Unknown;
        }

        // Ascending x is the VM binary-search lookup precondition (vm.rs
        // `Lookup`), so on any runtime-valid table dx > 0 and slope sign == dy
        // sign.
        let slope = dy / dx;

        if slope > slope_epsilon {
            all_decreasing = false;
            all_constant = false;
        } else if slope < -slope_epsilon {
            all_increasing = false;
            all_constant = false;
        } else {
            // slope is approximately 0 (within tolerance): an effectively-flat
            // segment. It doesn't break monotonicity but isn't strictly
            // increasing/decreasing either.
        }
    }

    // If all changes are zero (constant function), return Unknown
    if all_constant {
        return LinkPolarity::Unknown;
    }

    // Return polarity based on monotonicity
    if all_increasing {
        LinkPolarity::Positive
    } else if all_decreasing {
        LinkPolarity::Negative
    } else {
        LinkPolarity::Unknown
    }
}

/// Compose the polarity of a link INTO an implicit WITH-LOOKUP target with
/// the target's graphical-function monotonicity (GH #910).
///
/// A value-bearing, tables-carrying variable (`var = WITH LOOKUP(input,
/// table)`: tables present AND a real input equation) lowers at compile
/// time to `LOOKUP(self_gf, input)` -- per element for arrayed shapes
/// (`compiler::apply_implicit_with_lookup`, GH #909). The equation text
/// contains no `LOOKUP` call, so the AST walk alone sees only the raw
/// input polarity; the gf's monotonicity must be composed on top, exactly
/// as the explicit-`LOOKUP` arm composes `lookup_table_polarity`.
///
/// `input_polarity` is the AST-derived polarity of the raw input equation
/// with respect to the link source. Rules, mirroring the compiler's wrap:
///
/// - not a `VarKind::Aux`, table-only (a static table, no implicit wrap),
///   or no tables: `input_polarity` unchanged;
/// - no non-empty table at all: a zero-point gf is treated as ABSENT by
///   the compiler (the raw input evaluates unwrapped), so `input_polarity`
///   stands;
/// - per-element tables with at least one empty placeholder (a gf-less
///   element keeps its raw input equation): gf-bearing elements compose
///   while placeholder elements stay raw -- the two agree only when the
///   folded table polarity is `Positive` (composition is then the
///   identity); otherwise `Unknown`;
/// - otherwise: plain sign composition with the folded table polarity
///   (one shared table, or direction-agreeing per-element tables; a
///   non-monotone or disagreeing fold absorbs to `Unknown`).
pub(super) fn compose_with_lookup_polarity(
    input_polarity: LinkPolarity,
    to_var: &Variable,
) -> LinkPolarity {
    let VarKind::Aux {
        tables,
        is_table_only: false,
        ..
    } = &to_var.kind
    else {
        return input_polarity;
    };
    if tables.is_empty() || !tables.iter().any(|t| !t.x.is_empty()) {
        return input_polarity;
    }
    let gf_polarity = fold_per_element_table_polarity(tables);
    let has_unwrapped_element = tables.len() > 1 && tables.iter().any(|t| t.x.is_empty());
    if has_unwrapped_element {
        if gf_polarity == LinkPolarity::Positive {
            input_polarity
        } else {
            LinkPolarity::Unknown
        }
    } else {
        input_polarity.compose(gf_polarity)
    }
}

/// Aggregate the per-element graphical-function tables of an arrayed GF into a
/// single link polarity, mirroring the `Ast::Arrayed` per-element fold in
/// [`analyze_link_polarity`]: adopt the first concrete polarity, and if two
/// elements disagree on *direction* (`Positive` vs `Negative`) the result is
/// `Unknown`; an `Unknown` from a non-monotone (or empty placeholder) element
/// among direction-agreeing ones is ignored, so the link stays concrete.
fn fold_per_element_table_polarity(tables: &[crate::variable::Table]) -> LinkPolarity {
    let mut polarity = LinkPolarity::Unknown;
    for table in tables {
        let table_polarity = analyze_graphical_function_polarity(table);
        if polarity == LinkPolarity::Unknown {
            polarity = table_polarity;
        } else if polarity != table_polarity && table_polarity != LinkPolarity::Unknown {
            return LinkPolarity::Unknown;
        }
    }
    polarity
}

/// Polarity contributed by the graphical-function table named by a `LOOKUP`
/// builtin's first argument: a bare `Var(gf)` reference (a scalar GF, or a
/// whole-array reference to a per-element GF inside an apply-to-all body), or a
/// subscripted `gf[idx]` reference (a `FixedIndex` element selecting one
/// element's table, or a dimension-iterator over a per-element GF). The caller
/// composes this with the index argument's monotonicity.
///
/// `ltm/polarity.rs` is *upstream* of `db/analysis.rs` (the module dependency
/// runs `db::analysis -> crate::ltm`, not the reverse), so this can't reuse
/// `classify_subscript_shape` / `RefShape` -- it classifies directly on
/// `&[IndexExpr2]` using `Dimension::get_offset`. The classifier is *total*:
/// every `IndexExpr2` variant and every `Expr2` table-expression form is
/// handled, falling to `Unknown` for anything not statically resolvable (a user
/// can write an arbitrary subscript), so there is deliberately no
/// `unreachable!()` here.
fn lookup_table_polarity(
    table_expr: &Expr2,
    variables: Option<&crate::variable::LoweredVariableMap>,
) -> LinkPolarity {
    let Some(variables) = variables else {
        return LinkPolarity::Unknown;
    };
    match table_expr {
        Expr2::Var(name, _, _) => {
            let Some(var) = variables.get(&*crate::common::canonicalize(name.as_str())) else {
                return LinkPolarity::Unknown;
            };
            let VarKind::Aux { tables, .. } = &var.kind else {
                return LinkPolarity::Unknown;
            };
            // A bare reference to a per-element GF variable inside an
            // apply-to-all body (`effect[D] = LOOKUP(curve, dose)` where
            // `curve` is a per-element GF over `D`) reads every element's
            // table, so aggregate their polarities the same way the
            // `Ast::Arrayed` per-element fold does. A scalar GF -- or an
            // arrayed variable carrying a single variable-level GF shared by
            // all elements -- has one table; use it directly.
            if var.get_dimensions().is_some() && tables.len() > 1 {
                fold_per_element_table_polarity(tables)
            } else {
                tables
                    .first()
                    .map(analyze_graphical_function_polarity)
                    .unwrap_or(LinkPolarity::Unknown)
            }
        }
        Expr2::Subscript(name, indices, _, _) => {
            let Some(var) = variables.get(&*crate::common::canonicalize(name.as_str())) else {
                return LinkPolarity::Unknown;
            };
            let VarKind::Aux { tables, .. } = &var.kind else {
                return LinkPolarity::Unknown;
            };
            let Some(dims) = var.get_dimensions() else {
                return LinkPolarity::Unknown;
            };
            // Conservative for multi-dimensional GFs: resolving a joint table
            // offset would need row-major flattening of the per-element table
            // list, which the current LTM polarity cases don't require.
            let [dim] = dims else {
                return LinkPolarity::Unknown;
            };
            let [index] = indices.as_slice() else {
                return LinkPolarity::Unknown;
            };
            match index {
                // A whole-extent / positional / range subscript can't pick a
                // single element's table statically.
                IndexExpr2::Wildcard(_)
                | IndexExpr2::StarRange(_, _)
                | IndexExpr2::DimPosition(_, _)
                | IndexExpr2::Range(_, _, _) => LinkPolarity::Unknown,
                IndexExpr2::Expr(Expr2::Var(elem, _, _)) => {
                    if let Some(offset) = dim.get_offset(
                        &crate::common::CanonicalElementName::from_raw(elem.as_str()),
                    ) {
                        // `LOOKUP(curve[NYC], x)`: the polarity of NYC's
                        // specific table.
                        tables
                            .get(offset)
                            .map(analyze_graphical_function_polarity)
                            .unwrap_or(LinkPolarity::Unknown)
                    } else if elem.as_str() == dim.name() {
                        // `curve[D]` inside `effect[D] = LOOKUP(curve[D], ..)`:
                        // a dimension-iterator over the per-element GF. The
                        // link is determinate only if every element's table
                        // agrees on direction. A mapped iterator over a
                        // *different* dimension would need a `DimensionsContext`
                        // to resolve (not available here), so it stays Unknown.
                        fold_per_element_table_polarity(tables)
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                IndexExpr2::Expr(Expr2::Const(text, _, _)) => {
                    // A 1-based integer index into the GF source's dimension.
                    text.trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|&n| n >= 1 && n <= dim.len())
                        .and_then(|n| tables.get(n - 1))
                        .map(analyze_graphical_function_polarity)
                        .unwrap_or(LinkPolarity::Unknown)
                }
                // Any other index expression (a computed index, etc.) isn't
                // statically resolvable to one element's table.
                IndexExpr2::Expr(_) => LinkPolarity::Unknown,
            }
        }
        // A computed table expression can't occur for a real GF, but be total.
        _ => LinkPolarity::Unknown,
    }
}
