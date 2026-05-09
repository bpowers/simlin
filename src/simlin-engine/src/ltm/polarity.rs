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
    match ast {
        Ast::Scalar(expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::ApplyToAll(_, expr) => analyze_expr_polarity_with_context(
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

/// Recursively analyze expression polarity with optional context for looking up tables
pub(super) fn analyze_expr_polarity_with_context(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
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
        Expr2::App(crate::builtins::BuiltinFn::Lookup(table_expr, index_expr, _), _, _) => {
            // Check if the argument contains our from_var
            let arg_polarity = analyze_expr_polarity_with_context(
                index_expr,
                from_var,
                LinkPolarity::Positive,
                variables,
            );

            if arg_polarity == LinkPolarity::Unknown {
                return LinkPolarity::Unknown;
            }

            // Try to find the table and analyze its monotonicity
            // TODO: support Expr2::Subscript for subscripted lookup tables (per-element gf)
            let table_name = match table_expr.as_ref() {
                Expr2::Var(name, _, _) => Some(name.as_str()),
                _ => None,
            };

            if let (Some(vars), Some(table_name)) = (variables, table_name)
                && let Some(Variable::Var { tables, .. }) =
                    vars.get(&*crate::common::canonicalize(table_name))
                && let Some(t) = tables.first()
            {
                let table_polarity = analyze_graphical_function_polarity(t);
                // Combine the polarities: composing argument monotonicity
                // with table monotonicity is plain sign multiplication.
                return arg_polarity.compose(table_polarity);
            }
            LinkPolarity::Unknown
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
            analyze_expr_polarity_with_context(inner, from_var, current_polarity, variables)
        }
        // Max/Min (scalar two-arg form): non-decreasing in each argument
        Expr2::App(crate::builtins::BuiltinFn::Max(a, Some(b)), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, Some(b)), _, _) => {
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
        Expr2::App(crate::builtins::BuiltinFn::Sum(arg), _, _) => {
            analyze_expr_polarity_with_context(arg, from_var, current_polarity, variables)
        }
        Expr2::App(crate::builtins::BuiltinFn::Mean(args), _, _) => {
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
        Expr2::App(crate::builtins::BuiltinFn::Max(a, None), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, None), _, _) => {
            analyze_expr_polarity_with_context(a, from_var, current_polarity, variables)
        }
        // STDDEV is non-monotone (variance has no fixed sign w.r.t. inputs).
        // RANK depends on the rest of the array, so its sign w.r.t. one element
        // is not determined. Both must explicitly return Unknown.
        Expr2::App(crate::builtins::BuiltinFn::Stddev(_), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Rank(_, _), _, _) => LinkPolarity::Unknown,
        Expr2::App(_, _, _) => LinkPolarity::Unknown,
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
                    // Multiplication needs the SIGN of the other operand to determine
                    // polarity, not just whether it's independent of from_var.
                    // This is why Mul uses is_positive_constant/is_negative_constant
                    // rather than the expr_references_var pattern used by Add/Sub/Div.
                    // If both have known polarity, combine them
                    if left_pol != LinkPolarity::Unknown && right_pol != LinkPolarity::Unknown {
                        // Sign multiplication: ++ -> +, +- -> -, -- -> +.
                        left_pol.compose(right_pol)
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
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Div => match (left_pol, right_pol) {
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

    // Check consecutive pairs of points
    for i in 1..table.y.len() {
        let dy = table.y[i] - table.y[i - 1];

        // Use a small epsilon for floating point comparison
        const EPSILON: f64 = 1e-10;

        if dy > EPSILON {
            all_decreasing = false;
            all_constant = false;
        } else if dy < -EPSILON {
            all_increasing = false;
            all_constant = false;
        } else {
            // dy is approximately 0 (within epsilon)
            // This doesn't break monotonicity but isn't strictly increasing/decreasing
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
