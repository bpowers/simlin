// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static polarity analysis for causal links.
//!
//! Determines whether an increase in `from` produces an increase or
//! decrease in `to` by recursively walking `Expr2` ASTs. Loop polarity
//! (Reinforcing / Balancing / Undetermined) is then derived by counting
//! negative links per cycle in `graph.rs`.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr2, IndexExpr2};
use crate::common::{Canonical, Ident};
use crate::variable::Variable;

use super::types::{LinkPolarity, normalize_module_ref};

/// Analyze the polarity of how a variable appears in an equation
pub(super) fn analyze_link_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> LinkPolarity {
    analyze_ast_polarity(ast, from_var, variables, /* mul_convention = */ false)
}

/// Polarity of a hoisted reducer's body with respect to a *scalar feeder*
/// referenced inside it -- the discriminating polarity of a
/// `feeder -> $⁚ltm⁚agg⁚{n}` hop (GH #737 review follow-up).
///
/// This is `analyze_link_polarity` with the positive-by-convention Mul rule
/// enabled (`mul_convention`): a Mul whose feeder-independent co-factor is a
/// bare named quantity (`pop[*]` in `SUM(pop[*] * scale)`) passes the
/// feeder's derivative sign through, the same SD labeling convention the Div
/// arm and the both-sides-dependent Mul/Div rules already apply. So
/// `SUM(pop[*] * scale)` is Positive, `SUM(pop[*] * (1 - scale))` is
/// Negative, and an indeterminate body (a compound co-factor like
/// `(k - pop[*])`, or a non-monotone reducer) stays Unknown -- never a
/// confident wrong label.
///
/// The convention rule is deliberately NOT part of the general analyzer:
/// applied to arbitrary model equations it would relabel sign-indefinite
/// links (the logistic-growth class the Mul both-sides comment documents).
/// An agg's body is the one context where the blanket monotone-Positive
/// label was previously applied unconditionally, so a convention-scoped
/// analysis is strictly more accurate there.
pub(super) fn analyze_feeder_to_agg_polarity(
    agg_ast: &Ast<Expr2>,
    feeder: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> LinkPolarity {
    analyze_ast_polarity(agg_ast, feeder, variables, /* mul_convention = */ true)
}

/// Shared AST-level dispatch for [`analyze_link_polarity`] /
/// [`analyze_feeder_to_agg_polarity`]: per-element equations fold like the
/// `Ast::Arrayed` rule (first concrete polarity wins; a direction
/// disagreement collapses to Unknown).
fn analyze_ast_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
    mul_convention: bool,
) -> LinkPolarity {
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => analyze_expr_polarity_impl(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
            mul_convention,
        ),
        Ast::Arrayed(_, elements, default_expr, _) => {
            // For arrayed equations, check all elements
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let elem_polarity = analyze_expr_polarity_impl(
                    expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                    mul_convention,
                );
                if polarity == LinkPolarity::Unknown {
                    polarity = elem_polarity;
                } else if polarity != elem_polarity && elem_polarity != LinkPolarity::Unknown {
                    // Mixed polarities
                    return LinkPolarity::Unknown;
                }
            }
            if let Some(default_expr) = default_expr {
                let default_polarity = analyze_expr_polarity_impl(
                    default_expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                    mul_convention,
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
/// `AggNode::equation_text` key `enumerate_agg_nodes` stores) -- with a
/// bare `Var(agg_name)` and runs the ordinary analysis on the result.
///
/// Returns `Unknown` if the subexpression isn't found (graceful: the hop
/// then stays Unknown-polarity, as it was before GH #516).
pub(super) fn analyze_agg_consumer_polarity(
    consumer_ast: &Ast<Expr2>,
    reducer_subexpr_text: &str,
    agg_name: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
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
/// how `enumerate_agg_nodes` keys aggregate nodes (`Expr2` is not `Eq`).
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

/// Recursively analyze expression polarity with optional context for looking up tables
pub(super) fn analyze_expr_polarity_with_context(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
) -> LinkPolarity {
    analyze_expr_polarity_impl(
        expr,
        from_var,
        current_polarity,
        variables,
        /* mul_convention = */ false,
    )
}

/// The recursive polarity walk. `mul_convention` enables the
/// positive-by-convention Mul one-side rule (feeder-hop analysis ONLY; see
/// [`analyze_feeder_to_agg_polarity`]); `false` reproduces the general
/// analyzer exactly.
fn analyze_expr_polarity_impl(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
    mul_convention: bool,
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
        // All three lookup variants share the `(table_expr, index_expr, loc)`
        // shape and the same polarity story: the result is non-decreasing in
        // the index when the table is, so the link polarity is the argument's
        // monotonicity composed with the table's.
        Expr2::App(
            crate::builtins::BuiltinFn::Lookup(table_expr, index_expr, _)
            | crate::builtins::BuiltinFn::LookupForward(table_expr, index_expr, _)
            | crate::builtins::BuiltinFn::LookupBackward(table_expr, index_expr, _),
            _,
            _,
        ) => {
            let arg_polarity = analyze_expr_polarity_impl(
                index_expr,
                from_var,
                LinkPolarity::Positive,
                variables,
                mul_convention,
            );

            if arg_polarity == LinkPolarity::Unknown {
                return LinkPolarity::Unknown;
            }

            // Composing argument monotonicity with table monotonicity is plain
            // sign multiplication; an Unknown on either side absorbs.
            arg_polarity.compose(lookup_table_polarity(table_expr, variables))
        }
        // Non-decreasing single-arg builtins: propagate inner polarity.
        // Int (floor) is a step function with discontinuities, but is still
        // non-decreasing, which is sufficient for polarity propagation.
        Expr2::App(crate::builtins::BuiltinFn::Exp(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Ln(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Log10(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Sqrt(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Arctan(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Int(inner), _, _) => {
            analyze_expr_polarity_impl(inner, from_var, current_polarity, variables, mul_convention)
        }
        // Max/Min (scalar two-arg form): non-decreasing in each argument
        Expr2::App(crate::builtins::BuiltinFn::Max(a, Some(b)), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, Some(b)), _, _) => {
            let pol_a = analyze_expr_polarity_impl(
                a,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );
            let pol_b = analyze_expr_polarity_impl(
                b,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );
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
        Expr2::App(crate::builtins::BuiltinFn::Sum(arg), _, _) => {
            analyze_expr_polarity_impl(arg, from_var, current_polarity, variables, mul_convention)
        }
        Expr2::App(crate::builtins::BuiltinFn::Mean(args), _, _) => {
            let mut combined = LinkPolarity::Unknown;
            for arg in args {
                let arg_pol = analyze_expr_polarity_impl(
                    arg,
                    from_var,
                    current_polarity,
                    variables,
                    mul_convention,
                );
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
        Expr2::App(crate::builtins::BuiltinFn::Max(a, None), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, None), _, _) => {
            analyze_expr_polarity_impl(a, from_var, current_polarity, variables, mul_convention)
        }
        // STDDEV is non-monotone (variance has no fixed sign w.r.t. inputs).
        // RANK depends on the rest of the array, so its sign w.r.t. one element
        // is not determined. Both must explicitly return Unknown.
        Expr2::App(crate::builtins::BuiltinFn::Stddev(_), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Rank(_, _), _, _) => LinkPolarity::Unknown,
        Expr2::App(_, _, _) => LinkPolarity::Unknown,
        Expr2::Op2(op, left, right, _, _) => {
            let left_pol = analyze_expr_polarity_impl(
                left,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );
            let right_pol = analyze_expr_polarity_impl(
                right,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );

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
                    // Multiplication needs the SIGN of the other operand to determine
                    // polarity, not just whether it's independent of from_var.
                    // This is why Mul uses is_positive_constant/is_negative_constant
                    // rather than the expr_references_var pattern used by Add/Sub/Div.
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
                        // Only left has polarity, check if right is a constant or constant-valued variable
                        if is_positive_constant(right)
                            || (variables.is_some()
                                && is_positive_variable(right, variables.unwrap()))
                        {
                            left_pol
                        } else if is_negative_constant(right)
                            || (variables.is_some()
                                && is_negative_variable(right, variables.unwrap()))
                        {
                            flip_polarity(left_pol)
                        } else if mul_convention
                            && !expr_references_var(right, from_var)
                            && operand_positive_by_convention(right, variables)
                        {
                            // Feeder-hop analysis only (see
                            // `analyze_feeder_to_agg_polarity`): a bare named
                            // co-factor independent of `from_var` is positive
                            // by the SD labeling convention -- the same
                            // convention the Div arm and the both-sides rules
                            // already apply -- so the dependent side's
                            // derivative sign passes through. The
                            // `expr_references_var` guard keeps a co-factor
                            // that threads `from_var` through an index
                            // (`pop[from_var]`) on the Unknown path.
                            left_pol
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else if right_pol != LinkPolarity::Unknown {
                        // Only right has polarity, check if left is a constant or constant-valued variable
                        if is_positive_constant(left)
                            || (variables.is_some()
                                && is_positive_variable(left, variables.unwrap()))
                        {
                            right_pol
                        } else if is_negative_constant(left)
                            || (variables.is_some()
                                && is_negative_variable(left, variables.unwrap()))
                        {
                            flip_polarity(right_pol)
                        } else if mul_convention
                            && !expr_references_var(left, from_var)
                            && operand_positive_by_convention(left, variables)
                        {
                            // Mirror of the left-arm convention rule above.
                            right_pol
                        } else {
                            LinkPolarity::Unknown
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
                    // numeric constant, or a variable whose whole equation is
                    // one), use it -- the pre-fix rules ignored it entirely,
                    // mislabeling `-5/y` (truly Positive) as Negative and
                    // `x/-5` (truly Negative) as Positive.
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
                    // never feeds) remain exact regardless.
                    let value_sign = |e: &Expr2| -> Option<bool> {
                        if is_positive_constant(e)
                            || variables.is_some_and(|v| is_positive_variable(e, v))
                        {
                            Some(true)
                        } else if is_negative_constant(e)
                            || variables.is_some_and(|v| is_negative_variable(e, v))
                        {
                            Some(false)
                        } else {
                            None
                        }
                    };
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
            let operand_pol = analyze_expr_polarity_impl(
                operand,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );
            match op {
                crate::ast::UnaryOp::Not => flip_polarity(operand_pol),
                crate::ast::UnaryOp::Negative => flip_polarity(operand_pol),
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::If(_, true_branch, false_branch, _, _) => {
            // For IF-THEN-ELSE, check both branches
            let true_pol = analyze_expr_polarity_impl(
                true_branch,
                from_var,
                current_polarity,
                variables,
                mul_convention,
            );
            let false_pol = analyze_expr_polarity_impl(
                false_branch,
                from_var,
                current_polarity,
                variables,
                mul_convention,
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

/// Check if expression is a positive constant
pub(super) fn is_positive_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n > 0.0,
        _ => false,
    }
}

/// Check if expression is a negative constant
pub(super) fn is_negative_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n < 0.0,
        _ => false,
    }
}

/// Check if a variable has a positive constant value
pub(super) fn is_positive_variable(
    expr: &Expr2,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a positive constant
        return is_positive_constant(var_expr);
    }
    false
}

/// Check if a variable has a negative constant value
pub(super) fn is_negative_variable(
    expr: &Expr2,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a negative constant
        return is_negative_constant(var_expr);
    }
    false
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
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
) -> bool {
    if is_negative_constant(expr) || variables.is_some_and(|v| is_negative_variable(expr, v)) {
        return false;
    }
    matches!(expr, Expr2::Var(..) | Expr2::Subscript(..))
        || is_positive_constant(expr)
        || variables.is_some_and(|v| is_positive_variable(expr, v))
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
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
) -> LinkPolarity {
    let Some(variables) = variables else {
        return LinkPolarity::Unknown;
    };
    match table_expr {
        Expr2::Var(name, _, _) => {
            let Some(var) = variables.get(&*crate::common::canonicalize(name.as_str())) else {
                return LinkPolarity::Unknown;
            };
            let Variable::Var { tables, .. } = var else {
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
            let Variable::Var { tables, .. } = var else {
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
