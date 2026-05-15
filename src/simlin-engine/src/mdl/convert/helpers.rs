// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helper functions for MDL to datamodel conversion.

use crate::mdl::ast::{CallKind, Equation as MdlEquation, Expr, FullEquation, Lhs};
use crate::mdl::builtins::{eq_lower_space, to_lower_space};
use crate::mdl::xmile_compat::{format_unit_expr, quoted_space_to_underbar, space_to_underbar};

use super::types::ConvertError;

/// Convert a name to canonical form (lowercase with spaces).
pub(super) fn canonical_name(name: &str) -> String {
    to_lower_space(name)
}

/// Canonicalize a raw name to the engine variable-ident form.
///
/// This is exactly how a body variable's ident is produced: the symbol table
/// keys on `to_lower_space` (so the macro body's primary-output equation
/// `EXPRESSION MACRO = ...` becomes the body variable `expression_macro`) and
/// `build_variable` then runs `quoted_space_to_underbar` on that canonical
/// key. Composing the two here keeps `MacroSpec.parameters` /
/// `primary_output` byte-identical to the body variables they name and to the
/// synthesized port-variable idents.
pub(super) fn variable_ident(name: &str) -> String {
    quoted_space_to_underbar(&to_lower_space(name))
}

/// Extract the engine variable ident of a macro formal-parameter / output
/// `Expr`.
///
/// A macro header parses its argument and `:`-output lists as expressions;
/// in a valid macro each is a bare `Expr::Var`. The ident is canonicalized
/// via [`variable_ident`] so it is byte-identical to how the body equations
/// reference the parameter and to a synthesized port variable's ident.
/// Returns `None` for a non-`Var` expression, which signals a malformed
/// macro header.
pub(super) fn macro_param_ident(expr: &Expr<'_>) -> Option<String> {
    match expr {
        Expr::Var(name, _subscripts, _) => Some(variable_ident(name)),
        _ => None,
    }
}

/// Recursively rewrite Vensim `$`-suffixed *time* references in a macro body
/// expression to canonical engine time idents.
///
/// `Time$`, `TIME STEP$`, `INITIAL TIME$`, `FINAL TIME$` (and `DT$`) are
/// Vensim's escape, valid only inside a macro body, for reaching the caller's
/// global time variables. After lexing such a reference is an `Expr::Var`
/// whose name *includes* the trailing `$`. The engine already resolves the
/// bare canonical time idents inside a module body at any nesting depth (they
/// are zero-arity builtins), so the only work is this front-end name
/// translation; it runs *before* the body equation is formatted.
///
/// Scope: only `$`-*time* is translated. A `$`-suffixed reference to a
/// non-time variable is left untouched (out of Phase 2 scope), as is a bare
/// `time`/`time step` without the `$` (an ordinary name the engine resolves
/// itself). The match is on the name lowercased and space-normalized with the
/// trailing `$` stripped. Recurses through `Op1`/`Op2`/`Paren` and the args
/// *and* `output_bindings` of `App` so a nested `$`-time reference is caught.
pub(super) fn rewrite_dollar_time(expr: &mut Expr<'_>) {
    match expr {
        Expr::Var(name, _subscripts, _) => {
            if let Some(canonical) = canonical_dollar_time(name) {
                *name = std::borrow::Cow::Borrowed(canonical);
            }
        }
        Expr::Op1(_, inner, _) | Expr::Paren(inner, _) => rewrite_dollar_time(inner),
        Expr::Op2(_, left, right, _) => {
            rewrite_dollar_time(left);
            rewrite_dollar_time(right);
        }
        Expr::App(_, _, args, _, output_bindings, _) => {
            for arg in args.iter_mut() {
                rewrite_dollar_time(arg);
            }
            for binding in output_bindings.iter_mut() {
                rewrite_dollar_time(binding);
            }
        }
        Expr::Const(_, _) | Expr::Literal(_, _) | Expr::Na(_) => {}
    }
}

/// If `name` is a Vensim `$`-suffixed time reference, return the canonical
/// engine ident it maps to; otherwise `None`.
fn canonical_dollar_time(name: &str) -> Option<&'static str> {
    let stripped = name.strip_suffix('$')?;
    if eq_lower_space(stripped, "time") {
        Some("time")
    } else if eq_lower_space(stripped, "time step") {
        Some("time_step")
    } else if eq_lower_space(stripped, "initial time") {
        Some("initial_time")
    } else if eq_lower_space(stripped, "final time") {
        Some("final_time")
    } else if eq_lower_space(stripped, "dt") {
        Some("dt")
    } else {
        None
    }
}

/// Get the name from an equation's LHS.
pub(super) fn get_equation_name(eq: &MdlEquation<'_>) -> Option<String> {
    match eq {
        MdlEquation::Regular(lhs, _)
        | MdlEquation::EmptyRhs(lhs, _)
        | MdlEquation::Implicit(lhs)
        | MdlEquation::Lookup(lhs, _)
        | MdlEquation::WithLookup(lhs, _, _)
        | MdlEquation::Data(lhs, _)
        | MdlEquation::TabbedArray(lhs, _)
        | MdlEquation::NumberList(lhs, _) => Some(lhs.name.to_string()),
        MdlEquation::SubscriptDef(name, _) => Some(name.to_string()),
        MdlEquation::Equivalence(name, _, _) => Some(name.to_string()),
    }
}

/// Get the LHS from an equation if it has one.
pub(super) fn get_lhs<'a, 'input>(eq: &'a MdlEquation<'input>) -> Option<&'a Lhs<'input>> {
    match eq {
        MdlEquation::Regular(lhs, _)
        | MdlEquation::EmptyRhs(lhs, _)
        | MdlEquation::Implicit(lhs)
        | MdlEquation::Lookup(lhs, _)
        | MdlEquation::WithLookup(lhs, _, _)
        | MdlEquation::Data(lhs, _)
        | MdlEquation::TabbedArray(lhs, _)
        | MdlEquation::NumberList(lhs, _) => Some(lhs),
        MdlEquation::SubscriptDef(_, _) | MdlEquation::Equivalence(_, _, _) => None,
    }
}

/// Check if an equation has a top-level INTEG call (making it a stock).
///
/// Only the root expression determines stock type. An auxiliary like
/// `x = MAX(INTEG(a, 0), INTEG(b, 0))` or `x = a + INTEG(b, 0)` should NOT
/// be marked as a stock - only `x = INTEG(rate, init)` should.
/// Parens are allowed: `x = (INTEG(rate, init))` is also a stock.
pub(super) fn equation_is_stock(eq: &MdlEquation<'_>) -> bool {
    match eq {
        MdlEquation::Regular(_, expr) => is_top_level_integ(expr),
        _ => false,
    }
}

/// Check if an expression is a top-level INTEG call.
/// Only checks the root expression, allowing parens but not nested in other constructs.
pub(super) fn is_top_level_integ(expr: &Expr<'_>) -> bool {
    match expr {
        Expr::App(name, _, _, CallKind::Builtin, _, _) => eq_lower_space(name, "integ"),
        Expr::Paren(inner, _) => is_top_level_integ(inner),
        _ => false,
    }
}

/// Extract a constant value from an equation if it's a simple constant.
pub(super) fn extract_constant_value(eq: &MdlEquation<'_>) -> Option<f64> {
    match eq {
        MdlEquation::Regular(_, expr) => extract_expr_constant(expr),
        _ => None,
    }
}

/// Compute the Cartesian product of multiple vectors in row-major order.
///
/// For example: `[[a, b], [1, 2]]` produces `["a,1", "a,2", "b,1", "b,2"]`
/// (first dimension varies slowest).
pub(super) fn cartesian_product(dim_elements: &[Vec<String>]) -> Vec<String> {
    if dim_elements.is_empty() {
        return vec![];
    }
    if dim_elements.len() == 1 {
        return dim_elements[0].clone();
    }

    // Start with the first dimension
    let mut result: Vec<Vec<&str>> = dim_elements[0].iter().map(|e| vec![e.as_str()]).collect();

    // Multiply in each subsequent dimension
    for dim in &dim_elements[1..] {
        let mut new_result = Vec::with_capacity(result.len() * dim.len());
        for prefix in &result {
            for elem in dim {
                let mut new_combo = prefix.clone();
                new_combo.push(elem.as_str());
                new_result.push(new_combo);
            }
        }
        result = new_result;
    }

    // Join each combination into a comma-separated string (no spaces - compiler expects "a,b" not "a, b")
    result.into_iter().map(|combo| combo.join(",")).collect()
}

/// Extract a constant from an expression if it's a simple constant.
pub(super) fn extract_expr_constant(expr: &Expr<'_>) -> Option<f64> {
    match expr {
        Expr::Const(v, _) => Some(*v),
        Expr::Op1(crate::mdl::ast::UnaryOp::Negative, inner, _) => {
            extract_expr_constant(inner).map(|v| -v)
        }
        Expr::Paren(inner, _) => extract_expr_constant(inner),
        _ => None,
    }
}

/// Expand a numeric range like (A1-A10) to individual elements.
pub(super) fn expand_range(start: &str, end: &str) -> Result<Vec<String>, ConvertError> {
    // Find where numeric suffix starts
    let start_num_pos = start
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    let end_num_pos = end
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);

    let start_prefix = &start[..start_num_pos];
    let end_prefix = &end[..end_num_pos];

    if start_prefix != end_prefix || start_num_pos != end_num_pos {
        return Err(ConvertError::InvalidRange(format!(
            "Bad subscript range specification: {} - {}",
            start, end
        )));
    }

    let low: u32 = start[start_num_pos..]
        .parse()
        .map_err(|_| ConvertError::InvalidRange(format!("Invalid range start: {}", start)))?;
    let high: u32 = end[end_num_pos..]
        .parse()
        .map_err(|_| ConvertError::InvalidRange(format!("Invalid range end: {}", end)))?;

    if low >= high {
        return Err(ConvertError::InvalidRange(format!(
            "Bad subscript range specification: {} >= {}",
            low, high
        )));
    }

    Ok((low..=high)
        .map(|n| format!("{}{}", space_to_underbar(start_prefix), n))
        .collect())
}

/// Extract units string from a FullEquation.
/// When units have only a range (no expr), returns "1" (dimensionless).
/// This matches xmutil's UnitsRange() behavior.
pub(super) fn extract_units(eq: &FullEquation<'_>) -> Option<String> {
    let units = eq.units.as_ref()?;
    match &units.expr {
        Some(expr) => Some(format_unit_expr(expr)),
        None if units.range.is_some() => Some("1".to_string()),
        None => None,
    }
}

/// Extract units from the first equation in a list that has units.
/// This iterates ALL equations (not just the first) to handle cases where
/// the first equation is AFO or otherwise lacks units.
pub(super) fn extract_first_units(equations: &[FullEquation<'_>]) -> Option<String> {
    equations.iter().find_map(|eq| extract_units(eq))
}

/// Extract documentation and units from the first equation that has either.
/// In Vensim, element-specific equations use `~~|` for all but the last,
/// which carries `~ units ~ docs |`. We search all equations (not just
/// "valid" ones) because the metadata may be on an AFO or empty-RHS equation.
pub(super) fn extract_metadata(equations: &[FullEquation<'_>]) -> (String, Option<String>) {
    // Search from the end since Vensim convention puts metadata on the last equation
    for eq in equations.iter().rev() {
        let has_comment = eq.comment.is_some();
        let has_units = extract_units(eq).is_some();
        if has_comment || has_units {
            let doc = eq
                .comment
                .as_ref()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let units = extract_units(eq);
            return (doc, units);
        }
    }
    (String::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::ast::Loc;

    #[test]
    fn test_expand_range() {
        let result = expand_range("A1", "A5").unwrap();
        assert_eq!(result, vec!["A1", "A2", "A3", "A4", "A5"]);
    }

    #[test]
    fn test_expand_range_two_digit() {
        let result = expand_range("Item10", "Item15").unwrap();
        assert_eq!(
            result,
            vec!["Item10", "Item11", "Item12", "Item13", "Item14", "Item15"]
        );
    }

    #[test]
    fn test_expand_range_mismatch() {
        let result = expand_range("A1", "B5");
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_range_invalid_order() {
        let result = expand_range("A5", "A1");
        assert!(result.is_err());
    }

    #[test]
    fn test_canonical_name() {
        assert_eq!(canonical_name("My Variable"), "my variable");
        assert_eq!(canonical_name("INITIAL TIME"), "initial time");
    }

    #[test]
    fn test_format_number() {
        use crate::mdl::xmile_compat::format_number;
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(3.125), "3.125");
        assert_eq!(format_number(1.5), "1.5");
        // Scientific notation for very large/small
        assert!(format_number(1e10).contains('e'));
        assert!(format_number(1e-10).contains('e'));
    }

    fn var(name: &str) -> Expr<'static> {
        Expr::Var(
            std::borrow::Cow::Owned(name.to_string()),
            vec![],
            Loc::new(0, 0),
        )
    }

    fn var_name(expr: &Expr<'_>) -> String {
        match expr {
            Expr::Var(n, _, _) => n.to_string(),
            other => panic!("expected Var, got {:?}", other),
        }
    }

    #[test]
    fn test_rewrite_dollar_time_scalar_refs() {
        let mut e = var("Time$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "time");

        let mut e = var("TIME STEP$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "time_step");

        let mut e = var("Initial Time$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "initial_time");

        let mut e = var("FINAL TIME$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "final_time");

        let mut e = var("dt$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "dt");
    }

    #[test]
    fn test_rewrite_dollar_time_leaves_non_time_untouched() {
        // A non-time `$`-suffixed reference is out of Phase 2 scope.
        let mut e = var("foo$");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "foo$");

        // An ordinary (no `$`) reference is untouched.
        let mut e = var("input");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "input");

        // `time` without the `$` escape is an ordinary variable name here and
        // must not be rewritten (the engine resolves the bare builtin itself).
        let mut e = var("time");
        rewrite_dollar_time(&mut e);
        assert_eq!(var_name(&e), "time");
    }

    #[test]
    fn test_rewrite_dollar_time_recurses_nested() {
        // Shape: c + MYMACRO(Time$, x)
        let app = Expr::App(
            std::borrow::Cow::Borrowed("MYMACRO"),
            vec![],
            vec![var("Time$"), var("x")],
            CallKind::Symbol,
            vec![],
            Loc::new(0, 0),
        );
        let mut e = Expr::Op2(
            crate::mdl::ast::BinaryOp::Add,
            Box::new(var("c")),
            Box::new(app),
            Loc::new(0, 0),
        );

        rewrite_dollar_time(&mut e);

        let (left, right) = match &e {
            Expr::Op2(_, l, r, _) => (l.as_ref(), r.as_ref()),
            other => panic!("expected Op2, got {:?}", other),
        };
        assert_eq!(var_name(left), "c");
        match right {
            Expr::App(_, _, args, _, _, _) => {
                assert_eq!(var_name(&args[0]), "time");
                assert_eq!(var_name(&args[1]), "x");
            }
            other => panic!("expected App, got {:?}", other),
        }
    }

    #[test]
    fn test_rewrite_dollar_time_recurses_op1_paren_and_output_bindings() {
        // -(Time$) inside a Paren, plus an App output binding.
        let mut e = Expr::Op1(
            crate::mdl::ast::UnaryOp::Negative,
            Box::new(Expr::Paren(Box::new(var("TIME STEP$")), Loc::new(0, 0))),
            Loc::new(0, 0),
        );
        rewrite_dollar_time(&mut e);
        let inner = match &e {
            Expr::Op1(_, b, _) => match b.as_ref() {
                Expr::Paren(p, _) => p.as_ref(),
                other => panic!("expected Paren, got {:?}", other),
            },
            other => panic!("expected Op1, got {:?}", other),
        };
        assert_eq!(var_name(inner), "time_step");

        // Output bindings of an App must also be rewritten.
        let mut app = Expr::App(
            std::borrow::Cow::Borrowed("add3"),
            vec![],
            vec![var("a")],
            CallKind::Symbol,
            vec![var("Final Time$")],
            Loc::new(0, 0),
        );
        rewrite_dollar_time(&mut app);
        match &app {
            Expr::App(_, _, _, _, bindings, _) => {
                assert_eq!(var_name(&bindings[0]), "final_time");
            }
            other => panic!("expected App, got {:?}", other),
        }
    }
}
