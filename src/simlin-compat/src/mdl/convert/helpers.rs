// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helper functions for MDL to datamodel conversion.

use crate::mdl::ast::{CallKind, Equation as MdlEquation, Expr, FullEquation, Lhs};
use crate::mdl::builtins::to_lower_space;
use crate::mdl::xmile_compat::{format_unit_expr, space_to_underbar};

use super::types::ConvertError;

/// Convert a name to canonical form (lowercase with spaces).
pub(super) fn canonical_name(name: &str) -> String {
    to_lower_space(name)
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
        Expr::App(name, _, _, CallKind::Builtin, _) => to_lower_space(name) == "integ",
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
}
