// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the LTM synthetic-variable equation generators ([`super`]).
//! Split out of `ltm_augment.rs` to keep that file under the project
//! line-count lint; this is the `#[cfg(test)] mod tests` body, included via
//! `#[path]` so `use super::*` still resolves the module's private items.

use super::*;
use crate::common::{CanonicalDimensionName, CanonicalElementName};
use crate::dimensions::{Dimension, NamedDimension};

fn make_named_dimension(name: &str, elements: &[&str]) -> Dimension {
    use std::collections::HashMap;
    let canonical_elements: Vec<CanonicalElementName> = elements
        .iter()
        .map(|e| CanonicalElementName::from_raw(e))
        .collect();
    let indexed: HashMap<CanonicalElementName, usize> = canonical_elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.clone(), i))
        .collect();
    Dimension::Named(
        CanonicalDimensionName::from_raw(name),
        NamedDimension {
            elements: canonical_elements,
            indexed_elements: indexed,
            maps_to: None,
            mappings: vec![],
        },
    )
}

fn make_indexed_dimension(name: &str, size: u32) -> Dimension {
    Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
}

/// Build a `HashSet<Ident<Canonical>>` from string slices for use in
/// per-shape partial-equation tests. Each input string is canonicalized
/// via `Ident::new`, matching the wrapping path that
/// `build_partial_equation_shaped` exercises.
fn deps_set(idents: &[&str]) -> HashSet<Ident<Canonical>> {
    idents.iter().map(|s| Ident::new(s)).collect()
}

/// Source-dimension element names for the per-shape partial-equation
/// tests using a single `Region` dimension with elements `nyc` and
/// `boston` (canonical lowercase form, in source-declared order).
/// Used by `classify_expr0_subscript_shape` to validate that a literal
/// subscript like `[NYC]` resolves to a known element.
fn region_dim_elements() -> Vec<Vec<String>> {
    vec![vec!["nyc".to_string(), "boston".to_string()]]
}

/// Regression test for the integer-literal bounds asymmetry between
/// the Expr0 and Expr2 classifiers. The Expr2 classifier
/// (`db::ltm_ir::resolve_literal_index`) validates integer
/// literals against the indexed dimension's size and returns None
/// (so the shape becomes `DynamicIndex`) for out-of-range values.
/// The Expr0 classifier here previously accepted any `u32`-parseable
/// `Const`, so `pop[999]` over an indexed dim of size 2 would
/// classify as `FixedIndex(["999"])` here while the edge emitter
/// classifies it as `DynamicIndex`. The shapes wouldn't match,
/// the live reference would be wrapped in `PREVIOUS()`, and the
/// link score would silently zero out.
///
/// Both classifiers must agree -- so out-of-range integer literals
/// classify as `DynamicIndex`.
#[test]
fn classify_expr0_rejects_out_of_range_integer_literal() {
    use crate::ast::{Expr0, IndexExpr0, Loc};

    // Indexed-style source_dim_elements: position 0 is an indexed
    // dim of size 2 (elements "1", "2"). "999" is out of range.
    let dims = vec![vec!["1".to_string(), "2".to_string()]];
    let indices = vec![IndexExpr0::Expr(Expr0::Const(
        "999".to_string(),
        999.0,
        Loc::default(),
    ))];

    let shape = classify_expr0_subscript_shape(&indices, &dims, None);
    assert_eq!(
        shape,
        RefShape::DynamicIndex,
        "out-of-range integer literal must classify as DynamicIndex \
         to agree with Expr2's resolve_literal_index; got {shape:?}",
    );

    // Same fixture: is_literal_element_index must also reject.
    assert!(
        !is_literal_element_index(&indices[0], 0, &dims),
        "is_literal_element_index must reject out-of-range integer literal",
    );

    // Sanity: the same classifier still accepts an in-range integer.
    let in_range = vec![IndexExpr0::Expr(Expr0::Const(
        "1".to_string(),
        1.0,
        Loc::default(),
    ))];
    let in_range_shape = classify_expr0_subscript_shape(&in_range, &dims, None);
    assert_eq!(
        in_range_shape,
        RefShape::FixedIndex(vec!["1".to_string()]),
        "in-range integer literal must classify as FixedIndex; got {in_range_shape:?}",
    );
}

/// Regression test: integer-literal subscripts must canonicalize to
/// the engine's "1"-based string form before lookup, so `pop[01]`
/// (zero-padded) classifies as `FixedIndex(["1"])` -- the same form
/// `dimension_element_names` produces and the same form the Expr2
/// edge emitter (`db::ltm_ir::resolve_literal_index`) returns
/// after this fix. Without canonicalization, `pop[01]` would be
/// rejected as non-literal here (string "01" doesn't match "1" in
/// `source_dim_elements`) while the Expr2 classifier accepted it
/// at the original "01" text -- shapes disagree, the live ref gets
/// wrapped in `PREVIOUS()`, and the link score silently zeros.
#[test]
fn classify_expr0_canonicalizes_integer_literal_subscript() {
    use crate::ast::{Expr0, IndexExpr0, Loc};

    // Indexed-style source_dim_elements: position 0 is an indexed
    // dim of size 5 (elements "1".."5").
    let dims = vec![
        vec!["1", "2", "3", "4", "5"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<String>>(),
    ];
    let indices = vec![IndexExpr0::Expr(Expr0::Const(
        "01".to_string(),
        1.0,
        Loc::default(),
    ))];

    let shape = classify_expr0_subscript_shape(&indices, &dims, None);
    assert_eq!(
        shape,
        RefShape::FixedIndex(vec!["1".to_string()]),
        "zero-padded integer literal must canonicalize to '1' so the \
         Expr0 and Expr2 classifiers agree; got {shape:?}",
    );

    assert!(
        is_literal_element_index(&indices[0], 0, &dims),
        "is_literal_element_index must accept canonicalized integer literal",
    );
}

// -- substitute_reducers_in_equation tests --

/// Baseline: a reducer that is the whole equation is substituted by its
/// agg name.
#[test]
fn substitute_reducers_whole_equation() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("SUM(pop[*])", &reducers).unwrap();
    assert_eq!(out, "\"$⁚ltm⁚agg⁚0\"");
}

/// Baseline: a reducer nested in an arithmetic subexpression is
/// substituted; the surrounding structure is preserved.
#[test]
fn substitute_reducers_nested_in_arithmetic() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("base / SUM(pop[*])", &reducers).unwrap();
    assert_eq!(out, "base / \"$⁚ltm⁚agg⁚0\"");
}

/// Regression: a reducer used as a *subscript index expression*
/// (`stock[SUM(idx[*])]`) is hoisted into a synthetic agg by
/// `walk_subexpr_for_aggs` (which descends into `IndexExpr2::Expr`), so
/// `substitute_reducers_in_expr0` must likewise descend into the
/// `IndexExpr0::Expr` index of a `Subscript` and replace it -- otherwise
/// the agg→target link-score equation for such a target would keep the
/// reducer text live (no live `Var(agg)`), and the partial-equation
/// builder would never PREVIOUS-wrap or hold-live the agg correctly.
#[test]
fn substitute_reducers_inside_subscript_index() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("stock[SUM(idx[*])]", &reducers).unwrap();
    assert_eq!(out, "stock[\"$⁚ltm⁚agg⁚0\"]");
}

/// Regression: a reducer used as one bound of a *range* subscript
/// (`stock[1:SUM(idx[*])]`) is also reachable by the agg walker
/// (`IndexExpr2::Range`), so the substituter must descend into both
/// `IndexExpr0::Range` bounds.
#[test]
fn substitute_reducers_inside_subscript_range_bound() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("stock[1:SUM(idx[*])]", &reducers).unwrap();
    assert_eq!(out, "stock[1:\"$⁚ltm⁚agg⁚0\"]");
}

/// A reducer nested deep inside a subscript index expression (inside an
/// arithmetic op that is itself the index) is still substituted.
#[test]
fn substitute_reducers_deep_inside_subscript_index() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("stock[SUM(idx[*]) + 1]", &reducers).unwrap();
    assert_eq!(out, "stock[\"$⁚ltm⁚agg⁚0\" + 1]");
}

/// Wildcard / star-range / dim-position subscript indices have no
/// sub-expression to recurse into and must pass through untouched.
#[test]
fn substitute_reducers_leaves_wildcard_subscript_alone() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let out = substitute_reducers_in_equation("pop[*]", &reducers).unwrap();
    assert_eq!(out, "pop[*]");
}

// -- GH #661: a parse failure in substitute_reducers_in_equation must be a
//    loud Err, never the input returned unchanged --
//
// `substitute_reducers_in_equation` replaces a recognized inline reducer
// subexpression with its `$⁚ltm⁚agg⁚{n}` aggregate-node name to build the
// `agg → target` partial. If the parse of its input fails *before*
// substitution, the historical code returned the input unchanged, so the
// inline reducer silently survived into the agg→target partial -- the
// agg-substitution-omission sibling of the GH #311 PREVIOUS-omission hazard.
// That partial then compiles cleanly while referencing the live reducer
// instead of the hoisted aggregate node, a wrong-but-clean-compiling link
// score. The fix returns a structured `Err` so the db-bearing caller skips
// the variable and surfaces a `Warning`, mirroring the GH #311 treatment.

/// A genuinely unparseable input with reducers to substitute must return
/// `Err` carrying the offending text, NOT the input unchanged. The dangling
/// binary operator below is rejected by the parser.
#[test]
fn substitute_reducers_parse_error_is_err() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    let bad = "SUM(pop[*]) * * base";
    match substitute_reducers_in_equation(bad, &reducers) {
        Err(err) => assert_eq!(
            err.equation_text, bad,
            "the error must carry the original equation text for the diagnostic"
        ),
        Ok(out) => panic!(
            "a parse failure must be a loud Err so the inline reducer never \
             silently survives into the agg→target partial; got Ok({out:?})"
        ),
    }
}

/// An empty / whitespace input parses as `Ok(None)` (no AST), which is also
/// a failure for substitution purposes -- there is nothing to substitute and
/// returning the empty text would feed an unsubstituted (reducer-free, here
/// empty) body into the agg→target partial. It must be a loud `Err` once
/// there are reducers to substitute.
#[test]
fn substitute_reducers_empty_equation_is_err() {
    let mut reducers = HashMap::new();
    reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
    for empty in ["", "   ", "\t\n"] {
        let result = substitute_reducers_in_equation(empty, &reducers);
        assert!(
            result.is_err(),
            "an empty/whitespace input must be a loud Err; got {result:?} for {empty:?}"
        );
    }
}

/// The empty-`reducers` early return is unaffected: with no reducers to
/// substitute the function is a pure pass-through and never parses, so even
/// an unparseable input returns `Ok` with the text unchanged (the caller has
/// nothing to skip). This guards the early-return invariant the production
/// `slot_text` closure relies on.
#[test]
fn substitute_reducers_empty_reducers_passes_through_unparseable() {
    let reducers = HashMap::new();
    let bad = "SUM(pop[*]) * * base";
    let out = substitute_reducers_in_equation(bad, &reducers)
        .expect("an empty reducer map must pass through without parsing");
    assert_eq!(out, bad);
}

// -- subscript_idents_at_element tests --

/// A dep referenced through the *dimension-name* subscript form
/// (`reference_emissions[cop]`, the A2A iterated reference) must be
/// pinned to the element exactly like a bare reference: in the
/// per-target-element scalar link score, `[cop]` is meaningless (there is
/// no active A2A dimension) and forces a synthesized helper aux per
/// occurrence -- the dominant residual helper source on C-LEARN
/// (~27k call sites, GH #654).
#[test]
fn test_subscript_idents_at_element_pins_dimension_name_indices() {
    let idents = deps_set(&["reference_emissions", "pct_change"]);
    // Parsed function names round-trip lowercased through print_eqn, so
    // the expected text spells `previous(...)`.
    let result = subscript_idents_at_element(
        "PREVIOUS(reference_emissions[cop]) * (PREVIOUS(pct_change[cop]) / c + 1)",
        &idents,
        "cop·oecd_us",
    )
    .unwrap();
    assert_eq!(
        result,
        "previous(reference_emissions[cop·oecd_us]) * (previous(pct_change[cop·oecd_us]) / c + 1)"
    );
}

/// Multi-dimensional pinning matches indices to dimensions by NAME, so a
/// dep declared over a subset of the target's dimensions (or with them in
/// a different order) pins each index to the right element.
#[test]
fn test_subscript_idents_at_element_pins_by_dimension_name() {
    let idents = deps_set(&["row_input", "matrix"]);
    let result = subscript_idents_at_element(
        "row_input[age] + matrix[age,region]",
        &idents,
        "region·nyc,age·adult",
    )
    .unwrap();
    assert_eq!(
        result,
        "row_input[age·adult] + matrix[age·adult, region·nyc]"
    );
}

/// Indices that are already element literals (not dimension names) are
/// left untouched, and unqualified pinned elements (no `dim·` part)
/// cannot pin dimension-name indices -- those keep the conservative form.
#[test]
fn test_subscript_idents_at_element_leaves_literal_indices() {
    let idents = deps_set(&["dep"]);
    // `nyc` is an element literal, not the dimension name `region`.
    let result =
        subscript_idents_at_element("dep[nyc] + dep[region]", &idents, "region·la").unwrap();
    assert_eq!(result, "dep[nyc] + dep[region·la]");
}

// -- dimension_element_names tests --

#[test]
fn test_dimension_element_names_named() {
    let dim = make_named_dimension("Region", &["NYC", "Boston", "LA"]);
    let names = dimension_element_names(&dim);
    assert_eq!(names, vec!["nyc", "boston", "la"]);
}

#[test]
fn test_dimension_element_names_indexed() {
    // Indexed dimensions use 1-based indexing to match the engine's
    // subscript formatting (see dimensions.rs SubscriptIterator).
    let dim = make_indexed_dimension("Index", 4);
    let names = dimension_element_names(&dim);
    assert_eq!(names, vec!["1", "2", "3", "4"]);
}

#[test]
fn test_dimension_element_names_empty() {
    let dim = make_named_dimension("Empty", &[]);
    let names = dimension_element_names(&dim);
    assert!(names.is_empty());
}

#[test]
fn test_dimension_element_names_indexed_zero() {
    let dim = make_indexed_dimension("Zero", 0);
    let names = dimension_element_names(&dim);
    assert!(names.is_empty());
}

// -- ReducerKind tests --

#[test]
fn test_reducer_kind_equality() {
    assert_eq!(ReducerKind::Linear, ReducerKind::Linear);
    assert_eq!(ReducerKind::Nonlinear, ReducerKind::Nonlinear);
    assert_eq!(ReducerKind::Constant, ReducerKind::Constant);
    assert_ne!(ReducerKind::Linear, ReducerKind::Nonlinear);
    assert_ne!(ReducerKind::Linear, ReducerKind::Constant);
    assert_ne!(ReducerKind::Nonlinear, ReducerKind::Constant);
}

#[test]
fn test_reducer_kind_clone() {
    let kind = ReducerKind::Linear;
    let cloned = kind.clone();
    assert_eq!(kind, cloned);
}

// -- classify_reducer tests --

use crate::ast::{Ast, Expr2, IndexExpr2};
use crate::builtins::{BuiltinFn, Loc};

/// Build a Variable::Var with a hand-built Expr2 AST.
fn var_with_expr(expr: Expr2) -> Variable {
    Variable::Var {
        ident: Ident::new("target"),
        ast: Some(Ast::Scalar(expr)),
        init_ast: None,
        eqn: None,
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

/// Build an Expr2 representing `var_name[*]` (subscript with wildcard).
fn subscript_wildcard(var_name: &str) -> Expr2 {
    Expr2::Subscript(
        Ident::new(var_name),
        vec![IndexExpr2::Wildcard(Loc::default())],
        None,
        Loc::default(),
    )
}

/// Build an Expr2 representing a plain variable reference.
fn var_ref(name: &str) -> Expr2 {
    Expr2::Var(Ident::new(name), None, Loc::default())
}

#[test]
fn test_classify_reducer_sum() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Linear);
    assert_eq!(result.name, "SUM");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_mean() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Mean(vec![inner]), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Linear);
    assert_eq!(result.name, "MEAN");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_min() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Min(Box::new(inner), None), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Nonlinear);
    assert_eq!(result.name, "MIN");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_max() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Max(Box::new(inner), None), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Nonlinear);
    assert_eq!(result.name, "MAX");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_stddev() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Stddev(Box::new(inner)), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Nonlinear);
    assert_eq!(result.name, "STDDEV");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_rank() {
    let inner = subscript_wildcard("population");
    let direction = Expr2::Const("1".to_string(), 1.0, Loc::default());
    let expr = Expr2::App(
        BuiltinFn::Rank(Box::new(inner), Box::new(direction)),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Nonlinear);
    assert_eq!(result.name, "RANK");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_size() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Size(Box::new(inner)), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Constant);
    assert_eq!(result.name, "SIZE");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_no_reducer() {
    // A plain addition: x + y
    let expr = Expr2::Op2(
        crate::ast::BinaryOp::Add,
        Box::new(var_ref("x")),
        Box::new(var_ref("y")),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "x");
    assert_eq!(result, None);
}

#[test]
fn test_classify_reducer_wrong_source() {
    let inner = subscript_wildcard("population");
    let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
    let var = var_with_expr(expr);
    // Looking for a different source variable
    let result = classify_reducer(&var, "other_var");
    assert_eq!(result, None);
}

#[test]
fn test_classify_reducer_nested_in_expression() {
    // 2 * SUM(population[*]) + 1
    // Reducer is NOT at the top level, so is_bare should be false.
    let inner = subscript_wildcard("population");
    let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
    let two = Expr2::Const("2".to_string(), 2.0, Loc::default());
    let one = Expr2::Const("1".to_string(), 1.0, Loc::default());
    let mul = Expr2::Op2(
        crate::ast::BinaryOp::Mul,
        Box::new(two),
        Box::new(sum_expr),
        None,
        Loc::default(),
    );
    let expr = Expr2::Op2(
        crate::ast::BinaryOp::Add,
        Box::new(mul),
        Box::new(one),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Linear);
    assert_eq!(result.name, "SUM");
    assert!(!result.is_bare);
}

#[test]
fn test_classify_reducer_nested_in_scalar_max() {
    // MAX(SUM(population[*]), 0) -- scalar MAX wrapping array SUM
    // The SUM is nested inside a non-reducer App, so is_bare should be false.
    let inner = subscript_wildcard("population");
    let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
    let zero = Expr2::Const("0".to_string(), 0.0, Loc::default());
    let expr = Expr2::App(
        BuiltinFn::Max(Box::new(sum_expr), Some(Box::new(zero))),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Linear);
    assert_eq!(result.name, "SUM");
    assert!(!result.is_bare);
}

#[test]
fn test_classify_reducer_var_ref_no_subscript() {
    // SUM with a plain var reference (no subscript) should still match
    let inner = var_ref("population");
    let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    let result = result.expect("expected a classified reducer");
    assert_eq!(result.kind, ReducerKind::Linear);
    assert_eq!(result.name, "SUM");
    assert!(result.is_bare);
}

#[test]
fn test_classify_reducer_no_ast() {
    // Variable without an AST
    let var: Variable = Variable::Var {
        ident: Ident::new("target"),
        ast: None,
        init_ast: None,
        eqn: None,
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let result = classify_reducer(&var, "population");
    assert_eq!(result, None);
}

#[test]
fn test_classify_reducer_two_arg_min_not_reducer() {
    // MIN(x, y) with two args is NOT an array reducer
    let inner1 = var_ref("population");
    let inner2 = var_ref("threshold");
    let expr = Expr2::App(
        BuiltinFn::Min(Box::new(inner1), Some(Box::new(inner2))),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    assert_eq!(result, None);
}

#[test]
fn test_classify_reducer_two_arg_max_not_reducer() {
    // MAX(x, y) with two args is NOT an array reducer
    let inner1 = var_ref("population");
    let inner2 = var_ref("threshold");
    let expr = Expr2::App(
        BuiltinFn::Max(Box::new(inner1), Some(Box::new(inner2))),
        None,
        Loc::default(),
    );
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "population");
    assert_eq!(result, None);
}

// -- generate_element_to_scalar_equation tests --

#[test]
fn test_generate_sum_equation() {
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "total_pop",
        "nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    // Should contain the algebraic shortcut
    assert!(eq.contains("PREVIOUS(total_pop)"), "equation: {eq}");
    assert!(eq.contains("population[nyc]"), "equation: {eq}");
    assert!(eq.contains("PREVIOUS(population[nyc])"), "equation: {eq}");
    // Should not enumerate other elements (algebraic shortcut avoids them)
    assert!(
        !eq.contains("[boston]"),
        "equation should not enumerate boston: {eq}"
    );
    assert!(
        !eq.contains("[la]"),
        "equation should not enumerate la: {eq}"
    );
}

#[test]
fn test_generate_mean_equation() {
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "avg_pop",
        "nyc",
        &elements,
        &ReducerKind::Linear,
        "MEAN",
        true,
        None,
    );
    // MEAN divides by N
    assert!(eq.contains("/ 3"), "equation: {eq}");
    assert!(eq.contains("PREVIOUS(avg_pop)"), "equation: {eq}");
}

#[test]
fn test_generate_min_equation() {
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "min_pop",
        "nyc",
        &elements,
        &ReducerKind::Nonlinear,
        "MIN",
        true,
        None,
    );
    // Should enumerate all elements with nested binary MIN calls
    assert!(eq.contains("population[nyc]"), "equation: {eq}");
    assert!(
        eq.contains("PREVIOUS(population[boston])"),
        "equation: {eq}"
    );
    assert!(eq.contains("PREVIOUS(population[la])"), "equation: {eq}");
    // Nested binary calls: MIN(a, MIN(b, c))
    assert!(
        eq.contains(
            "MIN(population[nyc], MIN(PREVIOUS(population[boston]), PREVIOUS(population[la])))"
        ),
        "equation: {eq}"
    );
}

#[test]
fn test_generate_max_equation() {
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "max_pop",
        "boston",
        &elements,
        &ReducerKind::Nonlinear,
        "MAX",
        true,
        None,
    );
    // boston is the current element, so nyc and la are wrapped
    // Nested binary calls: MAX(a, MAX(b, c))
    assert!(
        eq.contains(
            "MAX(PREVIOUS(population[nyc]), MAX(population[boston], PREVIOUS(population[la])))"
        ),
        "equation: {eq}"
    );
}

#[test]
fn test_generate_stddev_equation() {
    // STDDEV's per-element ceteris-paribus partial: the unrolled
    // population-variance `sqrt` formula holding `s[d1]` live and the
    // other elements frozen at PREVIOUS, matching the engine's
    // population-variance (divisor N) STDDEV. The exact-string
    // assertion pins precedence and spacing so regressions are caught
    // (mirrors `test_generate_full_reduce_unchanged_after_refactor`).
    let elements = vec!["d1".to_string(), "d2".to_string(), "d3".to_string()];
    let eq = generate_element_to_scalar_equation(
        "s",
        "total",
        "d1",
        &elements,
        &ReducerKind::Nonlinear,
        "STDDEV",
        true,
        None,
    );
    assert_eq!(
        eq,
        "if (TIME = INITIAL_TIME) then 0 else if ((total - PREVIOUS(total)) = 0) OR ((s[d1] - PREVIOUS(s[d1])) = 0) then 0 else SAFEDIV((sqrt((((s[d1] - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d2]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d3]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2)) / 3) - PREVIOUS(total)), ABS((total - PREVIOUS(total))), 0) * SIGN((s[d1] - PREVIOUS(s[d1])))"
    );
    // The live source element drives the partial; the other elements
    // are frozen at PREVIOUS.
    assert!(eq.contains("sqrt("), "equation: {eq}");
    assert!(eq.contains("s[d1]"), "equation: {eq}");
    assert!(eq.contains("PREVIOUS(s[d2])"), "equation: {eq}");
    assert!(eq.contains("PREVIOUS(s[d3])"), "equation: {eq}");
    // Population variance squares deviations, never cubes.
    assert!(!eq.contains("^3"), "equation: {eq}");
}

#[test]
fn test_generate_stddev_single_element_is_zero() {
    // The variance of a single element is identically 0, so the
    // partial is the literal `"0"` (mirrors MIN/MAX's `args.len() == 1`
    // special case -- avoids emitting `sqrt(((... - ...)^2) / 1)`).
    let elements = vec!["d1".to_string()];
    let partial = generate_nonlinear_partial("s", "total", "d1", &elements, "STDDEV");
    assert_eq!(partial, "0");
}

#[test]
fn test_generate_rank_keeps_delta_ratio() {
    // RANK is an order statistic: non-differentiable, array-argument-only,
    // and unreachable via real models (RANK returns an array, so it
    // cannot be a scalar/A2A reducer RHS). The documented conservative
    // stand-in is the delta-ratio against the target -- i.e.
    // `generate_nonlinear_partial` returns just the target reference, so
    // the surrounding link-score formula degenerates to |Δtarget/Δtarget|.
    // Pinning this here makes RANK's treatment an explicit choice, not a
    // silent fallback.
    let elements = vec!["d1".to_string(), "d2".to_string(), "d3".to_string()];
    let partial = generate_nonlinear_partial("s", "total", "d1", &elements, "RANK");
    assert_eq!(partial, quote_ident("total"));
    assert!(!partial.contains("sqrt"), "partial: {partial}");
    assert!(!partial.contains("PREVIOUS("), "partial: {partial}");
}

#[test]
fn test_generate_constant_returns_zero() {
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "size_pop",
        "nyc",
        &elements,
        &ReducerKind::Constant,
        "SIZE",
        true,
        None,
    );
    assert_eq!(eq, "0");
}

#[test]
fn test_generate_nested_reducer_uses_delta_ratio() {
    // When the reducer is nested (is_bare=false), the equation should
    // fall back to the delta-ratio approach (using target directly)
    // instead of the algebraic shortcut.
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let eq = generate_element_to_scalar_equation(
        "population",
        "total_pop",
        "nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        false, // nested reducer
        None,
    );
    // Should NOT use the algebraic shortcut (PREVIOUS(target) + delta)
    assert!(
        !eq.contains("PREVIOUS(total_pop) +"),
        "should not use algebraic shortcut for nested reducer: {eq}"
    );
    // Should still have the standard link score wrapping
    assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
    assert!(eq.contains("SAFEDIV("), "equation: {eq}");
    // The partial equation uses target directly (delta-ratio approach)
    assert!(
        eq.contains("(total_pop - PREVIOUS(total_pop))"),
        "should use target variable in delta-ratio: {eq}"
    );
}

#[test]
fn test_generate_link_score_wrapping() {
    let elements = vec!["a".to_string(), "b".to_string()];
    let eq = generate_element_to_scalar_equation(
        "src",
        "tgt",
        "a",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    // Should have initial time guard
    assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
    // Should have zero-change guards
    assert!(eq.contains("(tgt - PREVIOUS(tgt)) = 0"), "equation: {eq}");
    assert!(
        eq.contains("(src[a] - PREVIOUS(src[a])) = 0"),
        "equation: {eq}"
    );
    // Should have the single-numerator magnitude and sign parts
    // (SAFEDIV(N, ABS(dT), 0) * SIGN(dS); see link_score_guard_form).
    assert!(eq.contains("SAFEDIV("), "equation: {eq}");
    assert!(eq.contains("ABS("), "equation: {eq}");
    assert!(eq.contains("* SIGN("), "equation: {eq}");
}

#[test]
fn test_generate_special_chars_quoted() {
    let elements = vec!["nyc".to_string()];
    let eq = generate_element_to_scalar_equation(
        "$\u{205A}ltm\u{205A}var",
        "total",
        "nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    // Source name with special chars should be quoted
    assert!(eq.contains("\"$\u{205A}ltm\u{205A}var\""), "equation: {eq}");
}

// -- GH #744: body-aware linear partial (ReducerBodyCtx) tests --

/// Owned backing storage for a [`ReducerBodyCtx`] in tests.
struct BodyCtxFixture {
    body_text: String,
    live_source: String,
    arrayed_dep_dims: std::collections::HashMap<String, usize>,
    model_deps: HashSet<String>,
    row_dim_names: Vec<String>,
    live_read_slice: Option<Vec<crate::ltm_agg::AxisRead>>,
}

impl BodyCtxFixture {
    fn new(
        body_text: &str,
        live_source: &str,
        arrayed: &[(&str, usize)],
        scalars: &[&str],
        row_dims: &[&str],
    ) -> Self {
        let arrayed_dep_dims: std::collections::HashMap<String, usize> =
            arrayed.iter().map(|(n, d)| (n.to_string(), *d)).collect();
        let model_deps: HashSet<String> = arrayed
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(scalars.iter().map(|s| s.to_string()))
            .collect();
        BodyCtxFixture {
            body_text: body_text.to_string(),
            live_source: live_source.to_string(),
            arrayed_dep_dims,
            model_deps,
            row_dim_names: row_dims.iter().map(|s| s.to_string()).collect(),
            live_read_slice: None,
        }
    }

    /// Attach the live source's accepted read slice (the hoisted-agg
    /// callers' configuration), enabling the Iterated-axis-position
    /// resolution of mismatched-arity dep indices.
    fn with_live_slice(mut self, slice: Vec<crate::ltm_agg::AxisRead>) -> Self {
        self.live_read_slice = Some(slice);
        self
    }

    fn ctx(&self) -> ReducerBodyCtx<'_> {
        ReducerBodyCtx {
            body_text: &self.body_text,
            live_source: &self.live_source,
            arrayed_dep_dims: &self.arrayed_dep_dims,
            model_deps: &self.model_deps,
            row_dim_names: &self.row_dim_names,
            dims_ctx: None,
            live_read_slice: self.live_read_slice.as_deref(),
        }
    }
}

/// A bare-source body must emit the legacy linear shortcut byte-identically
/// (the same string the `None`-context path produces).
#[test]
fn test_body_aware_bare_source_matches_legacy() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new("pop[*]", "pop", &[("pop", 1)], &[], &["region"]);
    let with_body = generate_element_to_scalar_equation(
        "pop",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    let legacy = generate_element_to_scalar_equation(
        "pop",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    assert_eq!(with_body, legacy, "bare body must keep the legacy shortcut");
}

/// A co-source coefficient body (`pop[*] * (1 - weight[*])` w.r.t. `weight`)
/// must evaluate the body at the row: the live evaluation freezes `pop` and
/// keeps `weight[row]` live; the frozen evaluation freezes both.
#[test]
fn test_body_aware_co_source_partial() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * (1 - weight[*])",
        "weight",
        &[("pop", 1), ("weight", 1)],
        &[],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "weight",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    // Live evaluation: pop frozen, weight[row] live.
    assert!(
        eq.contains("PREVIOUS(pop[region·nyc]) * (1 - weight[region·nyc])"),
        "equation: {eq}"
    );
    // Frozen evaluation: both frozen.
    assert!(
        eq.contains("PREVIOUS(pop[region·nyc]) * (1 - PREVIOUS(weight[region·nyc]))"),
        "equation: {eq}"
    );
    // The partial is anchored at PREVIOUS(target).
    assert!(eq.contains("PREVIOUS(total) + "), "equation: {eq}");
    // The other row never appears (it cancels against PREVIOUS(target)).
    assert!(!eq.contains("region·boston]"), "equation: {eq}");
}

/// A scalar feeder coefficient (`pop[*] * scale` w.r.t. `pop`) freezes the
/// feeder at PREVIOUS in both evaluations, so the numerator carries
/// `Δpop[row] * PREVIOUS(scale)`.
#[test]
fn test_body_aware_scalar_feeder_coefficient() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("pop[region·nyc] * PREVIOUS(scale)"),
        "equation: {eq}"
    );
    assert!(
        eq.contains("PREVIOUS(pop[region·nyc]) * PREVIOUS(scale)"),
        "equation: {eq}"
    );
}

/// MEAN divides the body delta by the co-reduced element count.
#[test]
fn test_body_aware_mean_divides_by_n() {
    let elements = vec![
        "region·nyc".to_string(),
        "region·boston".to_string(),
        "region·la".to_string(),
    ];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "avg",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "MEAN",
        true,
        Some(&fixture.ctx()),
    );
    assert!(eq.contains(" / 3"), "equation: {eq}");
}

/// GH #767 (T5 flip of the old un-pinnable bail): a mismatched-axis-count
/// dep indexed SOLELY by the row's dimension names (the iterated-dim
/// projection feeder `frac[d1]` inside the 2-D co-source row partial) is
/// pinned BY NAME to the row's element and FROZEN -- the changed-first
/// partial holds `PREVIOUS(frac[d1·a])` at the scored row instead of
/// bailing to the delta-ratio form (which scored a wrong-magnitude ±1).
#[test]
fn test_body_aware_projection_feeder_dep_pins_by_dim_name() {
    let elements = vec!["d1·a,d2·x".to_string(), "d1·a,d2·y".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[d1, *] * frac[d1]",
        "matrix",
        &[("matrix", 2), ("frac", 1)],
        &[],
        &["d1", "d2"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "growth",
        "d1·a,d2·x",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    let live = "matrix[d1·a, d2·x] * PREVIOUS(frac[d1·a])";
    let frozen = "PREVIOUS(matrix[d1·a, d2·x]) * PREVIOUS(frac[d1·a])";
    assert!(
        eq.contains(&format!("PREVIOUS(growth) + (({live}) - ({frozen}))")),
        "the changed-first partial must pin the feeder by dim name and freeze it: {eq}"
    );
}

/// GH #767 review (the repeated-dim hazard): with the live source's slice
/// available, a mismatched-arity feeder dep's index resolves to the
/// slice's ITERATED axis position -- for `matrix[D1,D1]` read as
/// `SUM(matrix[*, D1] * frac[D1])` (slice `[Reduced, Iterated]`, row
/// `(r1, r2)` feeding slot `r2`) the feeder pins to `frac[d1·r2]`, never
/// the same-named Reduced axis's `r1` element a first-match name lookup
/// would pick (a silently wrong frozen co-factor).
#[test]
fn test_body_aware_repeated_dim_feeder_pins_at_iterated_axis() {
    use crate::ltm_agg::AxisRead;
    let elements = vec!["d1·r1,d1·r2".to_string(), "d1·r2,d1·r2".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[*, d1] * frac[d1]",
        "matrix",
        &[("matrix", 2), ("frac", 1)],
        &[],
        &["d1", "d1"],
    )
    .with_live_slice(vec![
        AxisRead::Reduced { subset: None },
        AxisRead::Iterated {
            dim: "d1".to_string(),
            source_dim: "d1".to_string(),
        },
    ]);
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "growth",
        "d1·r1,d1·r2",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("PREVIOUS(frac[d1·r2])"),
        "the feeder must pin at the Iterated axis's row element: {eq}"
    );
    assert!(
        !eq.contains("frac[d1·r1]"),
        "the feeder must not pin at the same-named Reduced axis's element: {eq}"
    );
}

/// GH #767 review: WITHOUT a live slice, an AMBIGUOUS dim name (repeated
/// among the row's axes) bails to the delta-ratio fallback rather than
/// first-matching -- the pre-GH #767 behavior for every mismatched dep.
#[test]
fn test_body_aware_ambiguous_dim_name_without_slice_falls_back() {
    let elements = vec!["d1·r1,d1·r2".to_string(), "d1·r2,d1·r2".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[*, d1] * frac[d1]",
        "matrix",
        &[("matrix", 2), ("frac", 1)],
        &[],
        &["d1", "d1"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "growth",
        "d1·r1,d1·r2",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("SAFEDIV((growth - PREVIOUS(growth))"),
        "an ambiguous dim name must bail to the delta-ratio form: {eq}"
    );
    assert!(!eq.contains("frac["), "equation: {eq}");
}

/// A genuinely un-pinnable mismatched-axis-count dep -- one whose index is
/// NOT a row dimension name (`q[d9]`, `d9` outside the row's axes) -- still
/// degrades to the delta-ratio fallback, not a mis-pinned equation. (The
/// GH #767 by-name pin applies only when every index resolves to a row
/// axis.)
#[test]
fn test_body_aware_unpinnable_falls_back_to_delta_ratio() {
    let elements = vec!["d1·a,d2·x".to_string(), "d1·a,d2·y".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[d1, *] * q[d9]",
        "matrix",
        &[("matrix", 2), ("q", 1)],
        &[],
        &["d1", "d2"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "growth",
        "d1·a,d2·x",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    // Delta-ratio form: the partial IS the target, so the numerator is
    // (growth - PREVIOUS(growth)) and q/matrix bodies never appear.
    assert!(
        eq.contains("SAFEDIV((growth - PREVIOUS(growth))"),
        "equation: {eq}"
    );
    assert!(!eq.contains("q["), "equation: {eq}");
}

/// A nested array reducer inside the body (`pop[*] * MIN(q[*])`) cannot be
/// row-pinned (the inner reduce spans the whole slice); it must also fall
/// back to the delta-ratio form.
#[test]
fn test_body_aware_nested_reducer_falls_back_to_delta_ratio() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * MIN(q[*])",
        "pop",
        &[("pop", 1), ("q", 1)],
        &[],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("SAFEDIV((total - PREVIOUS(total))"),
        "equation: {eq}"
    );
}

/// `classify_reducer` must surface the reducer argument's canonical text.
#[test]
fn test_classify_reducer_returns_body_text() {
    let pop = subscript_wildcard("pop");
    let weight = subscript_wildcard("weight");
    let one = Expr2::Const("1".to_string(), 1.0, Loc::default());
    let coeff = Expr2::Op2(
        crate::ast::BinaryOp::Sub,
        Box::new(one),
        Box::new(weight),
        None,
        Loc::default(),
    );
    let body = Expr2::Op2(
        crate::ast::BinaryOp::Mul,
        Box::new(pop),
        Box::new(coeff),
        None,
        Loc::default(),
    );
    let expr = Expr2::App(BuiltinFn::Sum(Box::new(body)), None, Loc::default());
    let var = var_with_expr(expr);
    let result = classify_reducer(&var, "weight").expect("expected a classified reducer");
    assert_eq!(result.body_text, "pop[*] * (1 - weight[*])");
}

/// `expr_reference_idents` collects canonical heads (including inside
/// subscript index expressions) but not function names.
#[test]
fn test_expr_reference_idents() {
    let idents = expr_reference_idents("pop[*] * SAFEDIV(scale, other[idx + 1], 0)");
    assert!(idents.contains("pop"));
    assert!(idents.contains("scale"));
    assert!(idents.contains("other"));
    assert!(idents.contains("idx"));
    assert!(!idents.contains("safediv"));
}

// -- generate_element_to_reduced_equation tests (partial reduce) --
//
// A partial reduce `agg[D1] = SUM(matrix[D1,*])` collapses only the
// D2 axis: for source element `matrix[d1,d2]` the relevant target is
// `agg[d1]`, and the ceteris-paribus partial holds the other
// `matrix[d1,*]` elements (over the reduced axis D2) at PREVIOUS. The
// target reference (`to_q`) and the source reference (`source_elem`)
// must both be subscripted -- by the result-axis element on the
// target side and by the full source tuple on the source side.

#[test]
fn test_generate_reduced_sum_equation() {
    // agg[D1] = SUM(matrix[D1,*]), D1 = {a, b}, D2 = {x, y}.
    // For matrix[a,x] -> agg[a], the partial is the SUM algebraic
    // shortcut with the target pinned to agg[a] and the source pinned
    // to matrix[a,x]; the other reduced-axis element (matrix[a,y])
    // must NOT appear (the shortcut avoids enumerating it).
    let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "agg",
        "a,x",
        "a",
        &coreduced,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    assert!(
        eq.contains("PREVIOUS(agg[a]) + (matrix[a,x] - PREVIOUS(matrix[a,x]))"),
        "equation: {eq}"
    );
    // Target reference is subscripted by the result element.
    assert!(
        eq.contains("(agg[a] - PREVIOUS(agg[a])) = 0"),
        "equation: {eq}"
    );
    // Source reference is the full source tuple.
    assert!(
        eq.contains("(matrix[a,x] - PREVIOUS(matrix[a,x])) = 0"),
        "equation: {eq}"
    );
    // The other reduced-axis element must not be enumerated.
    assert!(
        !eq.contains("matrix[a,y]"),
        "SUM shortcut should not enumerate matrix[a,y]: {eq}"
    );
    // No literal "(0)" partial -- a real partial expression is emitted,
    // in the single-numerator guard form.
    assert!(eq.contains("SAFEDIV("), "equation: {eq}");
    assert!(eq.contains("* SIGN("), "equation: {eq}");
}

#[test]
fn test_generate_reduced_mean_equation() {
    // MEAN divides by the *reduced-axis* cardinality (|D2| = 2),
    // not by the total number of matrix elements.
    let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "row_mean",
        "a,x",
        "a",
        &coreduced,
        &ReducerKind::Linear,
        "MEAN",
        true,
        None,
    );
    assert!(
        eq.contains("PREVIOUS(row_mean[a]) + (matrix[a,x] - PREVIOUS(matrix[a,x])) / 2"),
        "equation: {eq}"
    );
}

#[test]
fn test_generate_reduced_min_equation() {
    // MIN over the reduced axis: nested binary MIN calls over the
    // matrix[a,*] elements (D2 = {x, y}), with matrix[a,x] live and
    // matrix[a,y] wrapped in PREVIOUS. Elements from other rows
    // (matrix[b,*]) must NOT appear.
    let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "row_min",
        "a,x",
        "a",
        &coreduced,
        &ReducerKind::Nonlinear,
        "MIN",
        true,
        None,
    );
    assert!(
        eq.contains("MIN(matrix[a,x], PREVIOUS(matrix[a,y]))"),
        "equation: {eq}"
    );
    // The partial's target reference is the row element.
    assert!(eq.contains("PREVIOUS(row_min[a])"), "equation: {eq}");
    // Elements from other rows must not appear.
    assert!(!eq.contains("matrix[b"), "equation: {eq}");
}

#[test]
fn test_generate_reduced_max_equation() {
    // The current element rides anywhere in the nesting; here it's
    // the first of the reduced-axis elements.
    let coreduced = vec!["b,x".to_string(), "b,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "row_max",
        "b,y",
        "b",
        &coreduced,
        &ReducerKind::Nonlinear,
        "MAX",
        true,
        None,
    );
    assert!(
        eq.contains("MAX(PREVIOUS(matrix[b,x]), matrix[b,y])"),
        "equation: {eq}"
    );
}

#[test]
fn test_generate_reduced_constant_returns_zero() {
    let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "row_size",
        "a,x",
        "a",
        &coreduced,
        &ReducerKind::Constant,
        "SIZE",
        true,
        None,
    );
    assert_eq!(eq, "0");
}

#[test]
fn test_generate_reduced_nested_uses_delta_ratio() {
    // A nested reducer (is_bare = false) falls back to the delta-ratio
    // form referencing the row element directly -- same as the scalar
    // case, just with the target subscripted.
    let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
    let eq = generate_element_to_reduced_equation(
        "matrix",
        "row_agg",
        "a,x",
        "a",
        &coreduced,
        &ReducerKind::Linear,
        "SUM",
        false,
        None,
    );
    assert!(
        !eq.contains("PREVIOUS(row_agg[a]) +"),
        "should not use the algebraic shortcut for a nested reducer: {eq}"
    );
    assert!(
        eq.contains("(row_agg[a] - PREVIOUS(row_agg[a]))"),
        "should use the row element in the delta-ratio: {eq}"
    );
    assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
}

#[test]
fn test_generate_full_reduce_unchanged_after_refactor() {
    // The full-reduce path must stay byte-identical after extracting
    // the shared body for the partial-reduce case.
    let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
    let scalar_eq = generate_element_to_scalar_equation(
        "population",
        "total_pop",
        "nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        None,
    );
    // A full reduce is the degenerate partial reduce where the result
    // axis is empty: passing an empty result element and the full
    // element list as the "coreduced" set must reproduce the scalar
    // equation, except the target reference picks up `[]` -- so we
    // don't claim equality here, only that the scalar path's text is
    // stable (the explicit-string assertion below catches regressions).
    assert_eq!(
        scalar_eq,
        "if (TIME = INITIAL_TIME) then 0 else if ((total_pop - PREVIOUS(total_pop)) = 0) OR ((population[nyc] - PREVIOUS(population[nyc])) = 0) then 0 else SAFEDIV((PREVIOUS(total_pop) + (population[nyc] - PREVIOUS(population[nyc])) - PREVIOUS(total_pop)), ABS((total_pop - PREVIOUS(total_pop))), 0) * SIGN((population[nyc] - PREVIOUS(population[nyc])))"
    );
}

// -- build_partial_equation_shaped: per-shape partial equation tests --
//
// Each test below pins the exact text that
// `build_partial_equation_shaped` must return when handed a specific
// `RefShape`. The expected strings were captured from `print_eqn` during
// Task 0.5 reconnaissance and are already canonicalized: identifiers and
// element names are lowercase (`print_ident` routes through
// `canonicalize`), parsed function names are lowercase (the parser
// lowercases function tokens at parse time, so `SUM` round-trips as
// `sum`), synthesized `PREVIOUS` keeps uppercase (it's constructed as a
// literal `"PREVIOUS"` `UntypedBuiltinFn`), binary operators get a
// single space on each side, and parens are reintroduced for precedence.
// Whitespace canonicalization happens entirely inside `print_eqn`, so the
// assertions can use the literal expected string without any pre-trim.
//
// The Bare and Wildcard tests don't need `source_dim_elements` because
// their classification doesn't depend on element-name lookups (Bare is a
// top-level Var; Wildcard is detected from the `[*]` index alone). The
// FixedIndex tests pass `region_dim_elements()` so
// `classify_expr0_subscript_shape` can validate `[NYC]` and `[Boston]`
// against the source's declared elements; otherwise both literal indices
// would fall back to `DynamicIndex` and both subscripts would be wrapped.

#[test]
fn test_partial_equation_share_bare_shape() {
    // share[R] = population / SUM(population[*])
    // For the bare-Var reference (`population`), the bare ref stays live
    // and the wildcard reducer -- "other content" for this Bare link --
    // is wrapped in PREVIOUS() *as a whole*: `PREVIOUS(sum(population[*]))`,
    // which is PREVIOUS of the scalar total and evaluates fine. The
    // earlier form `sum(PREVIOUS(population[*]))` was the GH #517 bug --
    // identically `0.0` at every step under an active A2A dimension
    // because codegen has no LoadPrev-of-array-view path.
    let equation = "population / SUM(population[*])";
    let deps = deps_set(&["population"]);
    let source = Ident::<Canonical>::new("population");
    let partial =
        build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None, None)
            .unwrap();
    assert_eq!(partial, "population / PREVIOUS(sum(population[*]))");
}

/// A LOOKUP call's first argument names a graphical-function table, not a
/// causal value reference: it must never be wrapped in PREVIOUS. Wrapping
/// it produces `lookup(PREVIOUS(table), ...)`, which cannot compile (a
/// lookup-only table variable has no value slot), so the link-score
/// fragment silently zeroes -- the WRLD3 failure mode where every
/// table-mediated link (`food_per_capita -> lifetime_multiplier_from_food`,
/// etc.) scored identically 0.
#[test]
fn test_partial_equation_lookup_table_arg_not_wrapped() {
    // lifetime_multiplier_from_food =
    //   lookup(lifetime_multiplier_from_food_table, food_per_capita / subsistence)
    let equation = "lookup(food_table, food_per_capita / subsistence)";
    let deps = deps_set(&["food_table", "food_per_capita", "subsistence"]);
    let source = Ident::<Canonical>::new("food_per_capita");
    let partial =
        build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None, None)
            .unwrap();
    assert_eq!(
        partial, "lookup(food_table, food_per_capita / PREVIOUS(subsistence))",
        "the table argument must stay a bare identifier; only value deps are wrapped"
    );

    // Same invariant for the extrapolating variants.
    for func in ["lookup_forward", "lookup_backward"] {
        let equation = format!("{func}(food_table, food_per_capita / subsistence)");
        let partial = build_partial_equation_shaped(
            &equation,
            &deps,
            &source,
            &RefShape::Bare,
            &[],
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            partial,
            format!("{func}(food_table, food_per_capita / PREVIOUS(subsistence))")
        );
    }
}

/// The WITH LOOKUP lowering references the variable's own table as
/// `lookup(self_var, input)`. The self-reference is the table holder, so
/// it stays bare; the (live) input is held live and other deps wrapped.
#[test]
fn test_partial_equation_with_lookup_self_table_not_wrapped() {
    let equation = "lookup(target_var, input / scale)";
    let deps = deps_set(&["target_var", "input", "scale"]);
    let source = Ident::<Canonical>::new("input");
    let partial =
        build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None, None)
            .unwrap();
    assert_eq!(partial, "lookup(target_var, input / PREVIOUS(scale))");
}

/// GH #511: an iterated-dimension source subscript (`row_sum[D1]` inside
/// an apply-to-all-over-`D1 x D2` equation) is normalized to bare
/// `row_sum` in the partial -- either held live (`live_shape == Bare`)
/// or `PREVIOUS(row_sum)` (a `Var` arg, which codegen accepts), never
/// `PREVIOUS(row_sum[d1])` (a `PREVIOUS(Subscript(...))`, which trips
/// the codegen assertion). The model equation `row_sum[D1] * c` is
/// untouched -- only the LTM partial's `Expr0` is normalized.
#[test]
fn test_partial_equation_iterated_dim_source_normalized_to_bare() {
    let equation = "row_sum[D1] * c";
    let target_iterated_dims = vec!["d1".to_string(), "d2".to_string()];
    // `row_sum` is over `D1`; `c` is scalar.
    let source_dim_names = vec!["d1".to_string()];
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx: None,
        dep_dims: None,
    };

    // `row_sum` is the live source (Bare): `row_sum[D1]` -> bare
    // `row_sum`, held live; `c` -> `PREVIOUS(c)`.
    let deps = deps_set(&["row_sum", "c"]);
    let live = Ident::<Canonical>::new("row_sum");
    let partial = build_partial_equation_shaped(
        equation,
        &deps,
        &live,
        &RefShape::Bare,
        // `source_dim_elements` is empty: `row_sum`'s single dimension is
        // identified by name via `iter_ctx`, not by element membership.
        &[],
        Some(&iter_ctx),
        None,
    )
    .unwrap();
    assert_eq!(
        partial, "row_sum * PREVIOUS(c)",
        "the iterated-dim source `row_sum[D1]` must be held live as bare `row_sum`"
    );

    // Now `c` is the live source: `row_sum[D1]` is a non-live dep ->
    // bare `row_sum` wrapped as `PREVIOUS(row_sum)`, NOT
    // `PREVIOUS(row_sum[d1])`.
    let live_c = Ident::<Canonical>::new("c");
    let partial_c = build_partial_equation_shaped(
        equation,
        &deps,
        &live_c,
        &RefShape::Bare,
        &[],
        Some(&iter_ctx),
        None,
    )
    .unwrap();
    assert!(
        partial_c.contains("PREVIOUS(row_sum)"),
        "the iterated-dim dep `row_sum[D1]` must be frozen as PREVIOUS(row_sum); got: {partial_c}"
    );
    assert!(
        !partial_c.contains("PREVIOUS(row_sum["),
        "must NOT produce PREVIOUS(row_sum[d1]) (a PREVIOUS-of-Subscript); got: {partial_c}"
    );
}

#[test]
fn test_partial_equation_reducer_wrapped_whole_with_fixed_index_live() {
    // x[R] = pop[NYC] + SUM(pop[*]) -- the FixedIndex(nyc) link keeps
    // `pop[nyc]` live; the coexisting `SUM(pop[*])` is "other content"
    // and must be PREVIOUS-wrapped as a whole, not recursed into (GH
    // #517). `dims` lets `classify_expr0_subscript_shape` recognize
    // `[NYC]` as a literal element.
    let equation = "pop[NYC] + SUM(pop[*])";
    let deps = deps_set(&["pop"]);
    let source = Ident::<Canonical>::new("pop");
    let dims = vec![vec!["nyc".to_string(), "boston".to_string()]];
    let partial = build_partial_equation_shaped(
        equation,
        &deps,
        &source,
        &RefShape::FixedIndex(vec!["nyc".to_string()]),
        &dims,
        None,
        None,
    )
    .unwrap();
    assert_eq!(partial, "pop[nyc] + PREVIOUS(sum(pop[*]))");
}

#[test]
fn test_partial_equation_two_reducers_both_wrapped_whole() {
    // y = SUM(a[*]) / SUM(b[*]) with `c` as the live source: neither
    // reducer carries the live ref, so both are PREVIOUS-wrapped whole
    // (GH #517). `c` does not appear, so nothing stays live -- the point
    // here is purely that the reducers don't get `sum(PREVIOUS(...))`.
    let equation = "(c + SUM(a[*])) / SUM(b[*])";
    let deps = deps_set(&["a", "b", "c"]);
    let source = Ident::<Canonical>::new("c");
    let partial =
        build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None, None)
            .unwrap();
    assert_eq!(partial, "(c + PREVIOUS(sum(a[*]))) / PREVIOUS(sum(b[*]))");
}

#[test]
fn test_partial_equation_wildcard_live_shape_holds_reducer_arg() {
    // A `RefShape::Wildcard` `live_shape` keeps the `population[*]`
    // reducer argument live and wraps every other reference in
    // PREVIOUS(). Full inlined reducers are hoisted into `$⁚ltm⁚agg⁚{n}`
    // nodes, so `build_partial_equation_shaped` only sees a Wildcard
    // `live_shape` for the conservative-slice case `SUM(pop[NYC, *])`
    // that `enumerate_agg_nodes` deliberately does not hoist; the
    // textbook full-reduce shape below pins the same wrapping rule that
    // case exercises.
    let equation = "population / SUM(population[*])";
    let deps = deps_set(&["population"]);
    let source = Ident::<Canonical>::new("population");
    let partial = build_partial_equation_shaped(
        equation,
        &deps,
        &source,
        &RefShape::Wildcard,
        &[],
        None,
        None,
    )
    .unwrap();
    assert_eq!(partial, "PREVIOUS(population) / sum(population[*])");
}

#[test]
fn test_partial_equation_migration_pressure_fixed_nyc() {
    // migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01
    // For the FixedIndex(nyc) shape, the `population[nyc]` reference stays
    // live and `population[boston]` is wrapped in PREVIOUS(). Element names
    // in the FixedIndex variant are lowercase canonical form -- they must
    // match the AST subscript text, which `print_ident` lowercases via
    // `canonicalize`.
    let equation = "(population[NYC] - population[Boston]) * 0.01";
    let deps = deps_set(&["population"]);
    let source = Ident::<Canonical>::new("population");
    let dims = region_dim_elements();
    let partial = build_partial_equation_shaped(
        equation,
        &deps,
        &source,
        &RefShape::FixedIndex(vec!["nyc".to_string()]),
        &dims,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        partial,
        "(population[nyc] - PREVIOUS(population[boston])) * 0.01"
    );
}

#[test]
fn test_partial_equation_migration_pressure_fixed_boston() {
    // Same equation text as the NYC case -- the per-shape builder works
    // per (reference-site, shape) pair, so the input equation is the
    // host expression and the `live_shape` selects which subscripted
    // population ref survives. Here `FixedIndex(boston)` keeps
    // `population[boston]` live and wraps `population[nyc]`.
    let equation = "(population[NYC] - population[Boston]) * 0.01";
    let deps = deps_set(&["population"]);
    let source = Ident::<Canonical>::new("population");
    let dims = region_dim_elements();
    let partial = build_partial_equation_shaped(
        equation,
        &deps,
        &source,
        &RefShape::FixedIndex(vec!["boston".to_string()]),
        &dims,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        partial,
        "(PREVIOUS(population[nyc]) - population[boston]) * 0.01"
    );
}

// -- AC2.4: other-source refs always wrapped, unknown idents passthrough --
//
// The two tests below pin behavior for references that aren't the live
// source. The first verifies that another known dep is wrapped regardless
// of which shape is live. The second verifies that an identifier that
// doesn't appear in `deps` (e.g., a typo or unresolved external) passes
// through unchanged -- the per-shape builder doesn't treat unknown idents
// as wrap candidates because they could be function names or noise that
// downstream parsing will diagnose separately.

#[test]
fn partial_equation_other_source_always_wrapped() {
    // Equation has a reference to `helper` (other dep) plus the live
    // source `pop`. The `helper` reference must be wrapped regardless
    // of `live_shape`; `pop` stays live because the shape is `Bare`.
    let deps = deps_set(&["pop", "helper"]);
    let live = Ident::<Canonical>::new("pop");
    let shape = RefShape::Bare;
    let dims = region_dim_elements();

    let partial =
        build_partial_equation_shaped("pop * helper", &deps, &live, &shape, &dims, None, None)
            .unwrap();
    assert!(partial.contains("PREVIOUS(helper)"), "partial: {partial}");
    assert!(!partial.contains("PREVIOUS(pop)"), "partial: {partial}");
}

#[test]
fn partial_equation_unknown_ident_unchanged() {
    // A reference to a variable not in `deps` (e.g., a typo or external)
    // is left alone -- it's not a known dep and shouldn't be wrapped.
    let deps = deps_set(&["pop"]);
    let live = Ident::<Canonical>::new("pop");
    let shape = RefShape::Bare;
    let dims = region_dim_elements();

    let partial =
        build_partial_equation_shaped("pop + unknown", &deps, &live, &shape, &dims, None, None)
            .unwrap();
    assert!(partial.contains("unknown"), "partial: {partial}");
    assert!(!partial.contains("PREVIOUS(unknown)"), "partial: {partial}");
}

// -- GH #311: parse failure must be a loud error, never a silent
//    semantics-changing fallback --
//
// The ceteris-paribus partial of an equation that cannot be parsed is
// undefined: there is no AST to PREVIOUS-wrap. The historical code
// returned the lowercased input text unchanged, so the "partial" was
// identical to the target's full equation -- and the link-score
// numerator `(partial - PREVIOUS(target))` then equals the denominator
// `(target - PREVIOUS(target))`, collapsing the score magnitude to a
// constant `|Δz/Δz| = 1`. That is a hidden attribution error that
// *compiles cleanly*, so no downstream diagnostic catches it. The fix
// returns a structured `Err` so the db-bearing caller skips the variable
// and surfaces a `Warning`.

/// A genuinely unparseable equation must return `Err`, NOT the lowercased
/// input. The text below has a dangling binary operator that the parser
/// rejects.
#[test]
fn build_partial_equation_shaped_parse_error_is_err() {
    let deps = deps_set(&["pop", "helper"]);
    let live = Ident::<Canonical>::new("pop");
    let shape = RefShape::Bare;
    let dims = region_dim_elements();

    let bad = "pop * * helper";
    let result = build_partial_equation_shaped(bad, &deps, &live, &shape, &dims, None, None);
    match result {
        Err(err) => assert_eq!(
            err.equation_text, bad,
            "the error must carry the original equation text for the diagnostic"
        ),
        Ok(partial) => {
            panic!("a parse failure must be a loud Err, not a silent fallback; got Ok({partial:?})")
        }
    }
}

/// An empty equation parses as `Ok(None)` (no AST), which is also a
/// failure for partial-equation purposes -- there is nothing to wrap and
/// returning the empty text would feed `(() - PREVIOUS(target))` into the
/// guard form. It must be a loud `Err`.
#[test]
fn build_partial_equation_shaped_empty_equation_is_err() {
    let deps = deps_set(&["pop"]);
    let live = Ident::<Canonical>::new("pop");
    let shape = RefShape::Bare;
    let dims = region_dim_elements();

    for empty in ["", "   ", "\t\n"] {
        let result = build_partial_equation_shaped(empty, &deps, &live, &shape, &dims, None, None);
        assert!(
            result.is_err(),
            "an empty/whitespace equation must be a loud Err; got {result:?} for {empty:?}"
        );
    }
}

/// The error path must be distinguished from the *legitimate*
/// "successfully parsed, but no other deps to wrap" case. A constant
/// equation (or one whose only reference is the live source) is its own
/// ceteris-paribus partial and must return `Ok` with the re-printed text
/// unchanged -- NOT an error. This is the line the bug blurred: a
/// text-unchanged result is only correct when it came from a successful
/// parse.
#[test]
fn build_partial_equation_shaped_no_deps_to_wrap_is_ok() {
    let deps = deps_set(&["pop"]);
    let live = Ident::<Canonical>::new("pop");
    let shape = RefShape::Bare;
    let dims = region_dim_elements();

    // A bare constant: parses fine, nothing to wrap.
    let constant = build_partial_equation_shaped("42", &deps, &live, &shape, &dims, None, None)
        .expect("a constant equation parses and is its own partial");
    assert_eq!(constant, "42");

    // The live source alone: parses fine, the live ref stays live, no
    // other deps to wrap.
    let live_only = build_partial_equation_shaped("pop", &deps, &live, &shape, &dims, None, None)
        .expect("the live source alone parses and stays live");
    assert_eq!(live_only, "pop");
    assert!(
        !live_only.contains("PREVIOUS"),
        "the live source must not be PREVIOUS-wrapped; got {live_only}"
    );
}

/// `subscript_idents_at_element` shares the same loud-failure contract:
/// an unparseable (already-partial) equation returns `Err`, while an
/// empty `idents` set is a legitimate no-op that returns the text
/// unchanged.
#[test]
fn subscript_idents_at_element_parse_error_is_err() {
    let idents = deps_set(&["dep"]);
    let bad = "dep * * other";
    let result = subscript_idents_at_element(bad, &idents, "region·nyc");
    match result {
        Err(err) => assert_eq!(err.equation_text, bad),
        Ok(out) => panic!("a parse failure must be a loud Err; got Ok({out:?})"),
    }

    // Empty idents: nothing to pin, returns the text verbatim (even text
    // that would not parse is irrelevant -- the function short-circuits).
    let noop = subscript_idents_at_element(bad, &deps_set(&[]), "region·nyc")
        .expect("empty idents is a no-op, not a parse attempt");
    assert_eq!(noop, bad);
}

// -- link_score_var_name: per-shape naming convention --
//
// The naming helper produces a stable name for each `(from, to, shape)`
// tuple regardless of which other shapes coexist in the same model.
// Bare uses the legacy canonical form; FixedIndex prefixes the source
// with the bracketed element name(s); Wildcard and DynamicIndex always
// append a stable suffix on the target side. The discovery parser
// (Phase 3 Task 7) strips the suffix before looking up offsets.

#[test]
fn link_score_name_bare_canonical() {
    assert_eq!(
        link_score_var_name("pop", "births", &RefShape::Bare),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births"
    );
}

#[test]
fn link_score_name_fixed_index() {
    let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
    assert_eq!(
        link_score_var_name("pop", "rel_pop", &shape),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"
    );
}

#[test]
fn link_score_name_wildcard_dynamic_collapse_to_bare() {
    // The `⁚wildcard` / `⁚dynamic` per-shape suffix was retired:
    // a maximal inlined reducer is hoisted into a `$⁚ltm⁚agg⁚{n}`
    // node, and the rare conservative-slice reducer collapses onto
    // the canonical Bare name (the emitter dedups by resulting name).
    let bare = link_score_var_name("pop", "share", &RefShape::Bare);
    assert_eq!(
        link_score_var_name("pop", "share", &RefShape::Wildcard),
        bare
    );
    assert_eq!(
        link_score_var_name("pop", "share", &RefShape::DynamicIndex),
        bare
    );
}

// -- generate_loop_score_equation: per-element link names --
//
// The per-element distinction lives in `link.from` itself (e.g.,
// `"pop[nyc]"` for cross-dimensional edges in mixed/scalar loops).
// generate_loop_score_equation uses Bare naming uniformly, so the
// bracketed `from` flows through verbatim and the resulting
// reference matches the per-element link score that
// try_cross_dimensional_link_scores emits.
#[test]
fn loop_score_equation_uses_element_level_from_for_per_element_links() {
    use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![
            Link {
                from: Ident::<Canonical>::new("pop[nyc]"),
                to: Ident::<Canonical>::new("rel_pop"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::<Canonical>::new("rel_pop"),
                to: Ident::<Canonical>::new("pop"),
                polarity: LinkPolarity::Positive,
            },
        ],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
        slot_links: vec![],
    };

    // Pretend both candidates were emitted as Bare; the resolver
    // will pick the canonical form via Bare naming, so the
    // bracketed from flows through verbatim.
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop".to_string());
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop".to_string());
    let eq = generate_loop_score_equation(&loop_item, &emitted, &Default::default());

    // Element-level from flows through Bare naming verbatim.
    assert!(
        eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop\""),
        "expected per-element link-score reference; got: {eq}"
    );
    // The closing link uses canonical Bare naming.
    assert!(
        eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop\""),
        "expected Bare link-score reference (closing link); got: {eq}"
    );
    // Loop score is the product of the two references.
    assert!(eq.contains(" * "), "expected product join; got: {eq}");
}

/// Regression test: when only a `FixedIndex` variant is in `emitted`
/// (e.g., `share[r] = pop[NYC]` -- only `pop[nyc]→share` is emitted),
/// the resolver must pick that variant rather than fall back to the
/// never-emitted Bare canonical name.
#[test]
fn resolver_picks_fixed_index_when_bare_not_emitted() {
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

    let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
    assert_eq!(
        chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share",
        "resolver should pick the FixedIndex variant when Bare is not emitted",
    );
}

/// Regression test: when multiple FixedIndex variants exist (e.g.,
/// `share[r] = pop[NYC] + pop[BOSTON]`), the resolver picks
/// deterministically (lexicographically first). This documents the
/// edge-aliasing limitation: only one variant contributes to the
/// loop score.
#[test]
fn resolver_picks_fixed_index_deterministically_with_multiple_variants() {
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share".to_string());

    let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
    // Lexicographic sort: "pop[boston]→share" < "pop[nyc]→share".
    assert_eq!(
        chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share",
        "resolver should pick the lexicographically first FixedIndex variant",
    );
}

/// Regression test: Bare must win when both a Bare and a FixedIndex
/// per-element link score exist for the same `(from, to)` edge -- the
/// documented Bare-beats-FixedIndex edge-aliasing tie-break.
#[test]
fn resolver_prefers_bare_over_fixed_index() {
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share".to_string());
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

    let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
    assert_eq!(
        chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share",
        "Bare must win when present, regardless of any FixedIndex variant",
    );
}

/// Regression test: bracketed `from` (cross-dimensional case) flows
/// through Bare naming verbatim and must resolve to the matching
/// per-element name emitted by `try_cross_dimensional_link_scores`.
#[test]
fn resolver_resolves_cross_dim_bracketed_from() {
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total".to_string());

    let chosen = resolve_link_score_name_for_loop("pop[nyc]", "total", &emitted, None);
    assert_eq!(
        chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total",
        "bracketed from + Bare should match the emitted per-element name",
    );
}

// -- Task 1 (ltm-503-cross-element-agg Phase 2): target_element-aware
//    loop-score link-score reference resolution --

/// Regression guard: `generate_loop_score_equation` is byte-identical
/// to the pre-Phase-2 behavior when no `Link.to` carries an element
/// subscript (which is the case for every pure-scalar / pure-A2A /
/// mixed loop the loop builder produces today, and stays the case for
/// pure-A2A loops after the cross-element rewrite).
#[test]
fn loop_score_equation_unsubscripted_to_unchanged() {
    use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![
            Link {
                from: Ident::<Canonical>::new("pop"),
                to: Ident::<Canonical>::new("births"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::<Canonical>::new("births"),
                to: Ident::<Canonical>::new("pop"),
                polarity: LinkPolarity::Positive,
            },
        ],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births".to_string());
    emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}births\u{2192}pop".to_string());

    let eq = generate_loop_score_equation(&loop_item, &emitted, &Default::default());
    assert_eq!(
        eq,
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births\" * \
         \"$\u{205A}ltm\u{205A}link_score\u{205A}births\u{2192}pop\"",
        "unsubscripted loop-score equation must be byte-identical to pre-Phase-2 output",
    );
}

/// When `target_element = None` the resolver is unchanged: it picks
/// the lexicographically-first FixedIndex variant.
#[test]
fn resolver_fixed_index_no_target_element_unchanged() {
    let mut emitted = HashSet::new();
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
            .to_string(),
    );
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure"
            .to_string(),
    );

    let chosen =
        resolve_link_score_name_for_loop("population", "migration_pressure", &emitted, None);
    // Lexicographic: "population[boston]..." < "population[nyc]...".
    assert_eq!(
        chosen,
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure",
        "with target_element=None the resolver keeps the alphabetical heuristic",
    );
}

/// `target_element = Some(e)` makes the resolver prefer the FixedIndex
/// variant whose source element matches `e` (an exact match), rather
/// than guessing alphabetically.
#[test]
fn resolver_fixed_index_target_element_exact_match() {
    let mut emitted = HashSet::new();
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
            .to_string(),
    );
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure"
            .to_string(),
    );

    let chosen =
        resolve_link_score_name_for_loop("population", "migration_pressure", &emitted, Some("nyc"));
    assert_eq!(
        chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure",
        "target_element=Some(\"nyc\") should select the nyc-source FixedIndex variant",
    );
}

/// A cross-element loop edge `population[nyc] -> migration_pressure[boston]`
/// where the emitted A2A link score is the per-source-element FixedIndex
/// form `population[nyc]->migration_pressure` (dimensioned over Region):
/// the loop-score equation references it subscripted at the visited
/// target element -- `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`.
#[test]
fn loop_score_equation_subscripts_a2a_fixed_index_link_at_visited_element() {
    use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

    let loop_item = Loop {
        id: "u1".to_string(),
        links: vec![Link {
            from: Ident::<Canonical>::new("population[nyc]"),
            to: Ident::<Canonical>::new("migration_pressure[boston]"),
            polarity: LinkPolarity::Positive,
        }],
        stocks: vec![],
        polarity: LoopPolarity::Undetermined,
        dimensions: vec![],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
            .to_string(),
    );

    let eq = generate_loop_score_equation(&loop_item, &emitted, &Default::default());
    assert_eq!(
        eq,
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston]",
        "A2A FixedIndex link score visited at element 'boston' must be subscripted [boston]",
    );
}

/// A cross-element loop edge `migration_in[nyc] -> population[nyc]` (a
/// structural flow->stock edge): the emitted link score uses the
/// *variable-level* `from` (`migration_in->population`, dimensioned
/// over Region), so the resolver must strip the subscript off
/// `Link.from` to find it, and the loop-score equation subscripts the
/// reference at the visited element -- `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]`.
#[test]
fn loop_score_equation_strips_from_for_variable_level_a2a_link() {
    use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

    let loop_item = Loop {
        id: "u1".to_string(),
        links: vec![Link {
            from: Ident::<Canonical>::new("migration_in[nyc]"),
            to: Ident::<Canonical>::new("population[nyc]"),
            polarity: LinkPolarity::Positive,
        }],
        stocks: vec![Ident::<Canonical>::new("population[nyc]")],
        polarity: LoopPolarity::Undetermined,
        dimensions: vec![],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted
        .insert("$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population".to_string());

    let eq = generate_loop_score_equation(&loop_item, &emitted, &Default::default());
    assert_eq!(
        eq, "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]",
        "variable-level-from A2A link score visited at 'nyc' must resolve via stripped-from \
         and be subscripted [nyc]",
    );
}

/// Full cross-element migration loop: three edges, three subscripted
/// references, all distinct A2A link scores, joined by ` * `.
#[test]
fn loop_score_equation_cross_element_migration_loop_full() {
    use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

    let loop_item = Loop {
        id: "u1".to_string(),
        links: vec![
            Link {
                from: Ident::<Canonical>::new("population[nyc]"),
                to: Ident::<Canonical>::new("migration_pressure[boston]"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::<Canonical>::new("migration_pressure[boston]"),
                to: Ident::<Canonical>::new("migration_in[nyc]"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::<Canonical>::new("migration_in[nyc]"),
                to: Ident::<Canonical>::new("population[nyc]"),
                polarity: LinkPolarity::Positive,
            },
        ],
        stocks: vec![Ident::<Canonical>::new("population[nyc]")],
        polarity: LoopPolarity::Undetermined,
        dimensions: vec![],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
            .to_string(),
    );
    emitted.insert(
        "$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in"
            .to_string(),
    );
    emitted
        .insert("$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population".to_string());

    let eq = generate_loop_score_equation(&loop_item, &emitted, &Default::default());
    assert_eq!(
        eq,
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston] * \
         \"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in\"[nyc] * \
         \"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]",
        "cross-element migration loop score must walk the element-level path",
    );
    // It must NOT reference the unsubscripted A2A diagonal names where
    // the loop visits a specific element.
    assert!(
        !eq.contains(
            "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure\u{2192}migration_out\""
        ),
        "must not reference the diagonal migration_out link score; got: {eq}",
    );
}

/// Regression test: a `DynamicIndex` live reference must still wrap
/// inner index expressions that reference other deps. The buggy
/// version skipped recursion for ANY live-shape match, which is
/// correct for `FixedIndex` (literal element indices are dimension
/// references, not deps) but wrong for `DynamicIndex` (the index is
/// an expression that may reference deps which must be held at
/// PREVIOUS for ceteris-paribus).
#[test]
fn partial_equation_dynamic_index_wraps_inner_deps() {
    // arr[idx + helper] with live_source=arr, live_shape=DynamicIndex.
    // The OUTER subscript is the live reference; idx and helper
    // inside the index expression are other deps and must be wrapped
    // in PREVIOUS for ceteris-paribus.
    let dims: Vec<Vec<String>> = vec![];
    let deps = deps_set(&["arr", "idx", "helper"]);
    let live = Ident::new("arr");
    let shape = RefShape::DynamicIndex;

    let partial =
        build_partial_equation_shaped("arr[idx + helper]", &deps, &live, &shape, &dims, None, None)
            .unwrap();

    assert!(
        partial.contains("PREVIOUS(idx)"),
        "idx must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
    );
    assert!(
        partial.contains("PREVIOUS(helper)"),
        "helper must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
    );
    // The outer arr[...] reference must stay live (no PREVIOUS wrap
    // around the whole subscript).
    assert!(
        !partial.contains("PREVIOUS(arr["),
        "live arr ref must not be wrapped; got: {partial}",
    );
}

/// Regression test: a literal-element subscript like `pop[NYC]` must
/// classify as `FixedIndex(["nyc"])` even when a user variable named
/// `nyc` exists and is in `other_deps`. The buggy implementation
/// recursed into the indices first (wrapping `Var(NYC)` as
/// `App(PREVIOUS, [Var(NYC)])`) and then classified the transformed
/// indices, which fell through to `DynamicIndex` and broke the live
/// FixedIndex match -- so the live reference got wrapped too.
#[test]
fn partial_equation_dimension_element_collides_with_variable_name() {
    let dims = region_dim_elements();
    // Both `pop` (live source) and `nyc` (user variable) are deps.
    // The literal subscript [NYC] must still classify as a dimension
    // element, not a wrapped variable reference.
    let deps = deps_set(&["pop", "nyc"]);
    let live = Ident::new("pop");
    let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);

    let partial =
        build_partial_equation_shaped("pop[NYC]", &deps, &live, &shape, &dims, None, None).unwrap();

    // The live reference must remain unwrapped.
    assert!(
        !partial.contains("PREVIOUS(pop"),
        "live FixedIndex reference unexpectedly wrapped; got: {partial}",
    );
    // The literal element subscript must remain unwrapped (NYC is a
    // dimension element here, not a runtime variable reference).
    assert!(
        !partial.contains("PREVIOUS(nyc)"),
        "literal element subscript wrongly treated as variable; got: {partial}",
    );
}

/// GH #587: an index identifier that names a dimension element must NOT
/// be PREVIOUS-wrapped when its enclosing (non-live) subscripted
/// reference is wrapped for ceteris-paribus -- it is an element selector,
/// not a causal reference. Wrapping it produced `dep[PREVIOUS(elem)]`,
/// whose helper-aux chain cannot compile (PREVIOUS of a bare element name
/// is meaningless), so the link score silently stubbed to zero.
///
/// The element index is also *qualified* (`dimension·element`) so the
/// PREVIOUS-wrapped reference stays statically resolvable: the
/// builtins-visitor compiles `PREVIOUS(dep[dim·elem])` to a direct
/// LoadPrev at the element's slot instead of synthesizing a helper aux.
#[test]
fn partial_equation_element_index_in_wrapped_dep_not_wrapped() {
    // Equation: `gwp_of_hfc[hfc134a] * input`, live source `input`
    // (scalar). `gwp_of_hfc` is an other-dep (must be wrapped); its
    // index `hfc134a` is an element of the `hfc_type` dimension and --
    // because `identifier_set` over-collects subscript identifiers --
    // also lands in the dep set.
    let deps = deps_set(&["gwp_of_hfc", "hfc134a", "input"]);
    let live = Ident::new("input");
    let shape = RefShape::Bare;

    let dm_dims = vec![crate::datamodel::Dimension::named(
        "hfc_type".to_string(),
        vec!["hfc134a".to_string(), "hfc125".to_string()],
    )];
    let dims_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());

    let partial = build_partial_equation_shaped(
        "gwp_of_hfc[hfc134a] * input",
        &deps,
        &live,
        &shape,
        &[],
        None,
        Some(&dims_ctx),
    )
    .unwrap();

    // The dep itself is wrapped for ceteris-paribus...
    assert!(
        partial.contains("PREVIOUS("),
        "the gwp_of_hfc dep must be PREVIOUS-wrapped; got: {partial}",
    );
    // ...but its element-name index must NOT be wrapped...
    assert!(
        !partial.to_lowercase().contains("previous(hfc134a)"),
        "element-name index must not be PREVIOUS-wrapped (GH #587); got: {partial}",
    );
    // ...and is qualified to the unambiguous `dimension·element` form.
    assert!(
        partial.contains("hfc_type\u{B7}hfc134a"),
        "element index should be qualified as dimension·element; got: {partial}",
    );
    // The live source stays live.
    assert!(
        !partial.to_lowercase().contains("previous(input)"),
        "live ref must stay live; got: {partial}",
    );
}

/// An element name declared by multiple dimensions at DIFFERENT positions
/// is genuinely ambiguous (qualification through different dimensions
/// would resolve to different indices), so it keeps the conservative
/// wrapping behavior (no qualification, PREVIOUS-wrapped when in the dep
/// set).
#[test]
fn partial_equation_ambiguous_element_index_left_verbatim() {
    let deps = deps_set(&["arr", "shared", "input"]);
    let live = Ident::new("input");
    let shape = RefShape::Bare;

    // `shared` is an element of BOTH dimensions, at different positions
    // (index 1 in dim_a, index 2 in dim_b).
    let dm_dims = vec![
        crate::datamodel::Dimension::named(
            "dim_a".to_string(),
            vec!["shared".to_string(), "a2".to_string()],
        ),
        crate::datamodel::Dimension::named(
            "dim_b".to_string(),
            vec!["b1".to_string(), "shared".to_string()],
        ),
    ];
    let dims_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());

    let partial = build_partial_equation_shaped(
        "arr[shared] * input",
        &deps,
        &live,
        &shape,
        &[],
        None,
        Some(&dims_ctx),
    )
    .unwrap();

    // Ambiguous element index: it cannot be qualified (the declaring
    // dimensions disagree on its index), but it is still an element
    // selector, never a causal reference -- so it is left verbatim, NOT
    // PREVIOUS-wrapped (GH #654). The downstream parse decides whether it
    // compiles to a static subscript (non-shadowed element) or needs a
    // helper aux (genuinely-dynamic index), with single-lag semantics
    // either way.
    assert!(
        !partial.contains('\u{B7}'),
        "ambiguous element must not be qualified; got: {partial}",
    );
    assert!(
        partial.to_lowercase().contains("previous(arr[shared])"),
        "ambiguous element index is left verbatim inside the wrapped dep; got: {partial}",
    );
}

/// An element name declared by multiple dimensions at the SAME position
/// is still unambiguous: qualification through any of the declaring
/// dimensions resolves to the same constant index. Models commonly
/// declare several same-shaped region/category dimensions, so this case
/// must qualify (C-LEARN's region dimensions are exactly this shape).
#[test]
fn partial_equation_same_position_shared_element_qualifies() {
    let deps = deps_set(&["arr", "shared", "input"]);
    let live = Ident::new("input");
    let shape = RefShape::Bare;

    // `shared` is the FIRST element of both dimensions.
    let dm_dims = vec![
        crate::datamodel::Dimension::named(
            "dim_a".to_string(),
            vec!["shared".to_string(), "a2".to_string()],
        ),
        crate::datamodel::Dimension::named(
            "dim_b".to_string(),
            vec!["shared".to_string(), "b2".to_string()],
        ),
    ];
    let dims_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());

    let partial = build_partial_equation_shaped(
        "arr[shared] * input",
        &deps,
        &live,
        &shape,
        &[],
        None,
        Some(&dims_ctx),
    )
    .unwrap();

    // Same-position sharing: qualified (deterministically against the
    // lexicographically smallest dimension name) and never wrapped.
    assert!(
        partial.contains("dim_a\u{B7}shared"),
        "same-position shared element should be qualified against dim_a; got: {partial}",
    );
    assert!(
        !partial.to_lowercase().contains("previous(shared)"),
        "same-position shared element must not be wrapped; got: {partial}",
    );
}

/// GH #759: a subscript index that names a project DIMENSION (`matrix[D1,
/// c1]`'s `D1` -- the iterated-dim reference form) is a dimension selector,
/// never a causal reference. The #587 guards cover dimension *elements*; a
/// dimension *name* is neither an element nor qualifiable, so it fell
/// through to the recursive wrap whenever the caller's (over-collected) dep
/// set contained it: the frozen co-source became
/// `PREVIOUS(matrix[PREVIOUS(d1), d2·c1])`, whose PREVIOUS-capture helper
/// cannot compile, and the link score read constant garbage off the
/// 0-stubbed helper (-40 for the GH #759 probe constants).
#[test]
fn partial_equation_dimension_name_index_not_wrapped() {
    // The Bare-shape pinned-index repro: `growth[D1] = matrix[D1, c1] *
    // frac[D1]`, building the changed-first partial for the `frac -> growth`
    // edge. `d1` and `c1` are deliberately included in the dep set -- the
    // wrapper-side guard must hold even when a caller over-collects.
    let deps = deps_set(&["matrix", "frac", "d1", "c1"]);
    let live = Ident::new("frac");
    let shape = RefShape::Bare;

    let dm_dims = vec![
        crate::datamodel::Dimension::named(
            "D1".to_string(),
            vec!["r1".to_string(), "r2".to_string()],
        ),
        crate::datamodel::Dimension::named(
            "D2".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        ),
    ];
    let dims_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());

    let partial = build_partial_equation_shaped(
        "matrix[D1, c1] * frac",
        &deps,
        &live,
        &shape,
        &[],
        None,
        Some(&dims_ctx),
    )
    .unwrap();

    assert!(
        !partial.to_lowercase().contains("previous(d1)"),
        "a dimension-name index must not be PREVIOUS-wrapped (GH #759); got: {partial}",
    );
    assert_eq!(
        partial, "PREVIOUS(matrix[d1, d2·c1]) * frac",
        "the co-source freezes wholesale with its iterated-dim index verbatim",
    );
}

/// GH #759, the original Wildcard-path filing: a live `Wildcard` reference
/// whose non-wildcard index is an iterated-dimension NAME
/// (`SUM(matrix[State, *])`) must keep that index verbatim --
/// `matrix[PREVIOUS(state), *]` is meaningless and dooms the fragment.
#[test]
fn partial_equation_wildcard_live_iterated_dim_index_not_wrapped() {
    let deps = deps_set(&["matrix", "state"]);
    let live = Ident::new("matrix");
    let shape = RefShape::Wildcard;

    let dm_dims = vec![
        crate::datamodel::Dimension::named(
            "State".to_string(),
            vec!["ca".to_string(), "ny".to_string()],
        ),
        crate::datamodel::Dimension::named(
            "D2".to_string(),
            vec!["x".to_string(), "y".to_string()],
        ),
    ];
    let dims_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    // The live source's declared dims (mirroring the #534 counterfactual:
    // `matrix` over State x D2).
    let source_dims = vec![
        vec!["ca".to_string(), "ny".to_string()],
        vec!["x".to_string(), "y".to_string()],
    ];

    let partial = build_partial_equation_shaped(
        "SUM(matrix[State, *])",
        &deps,
        &live,
        &shape,
        &source_dims,
        None,
        Some(&dims_ctx),
    )
    .unwrap();

    assert!(
        !partial.to_lowercase().contains("previous(state)"),
        "a dimension-name index in a live Wildcard reference must stay verbatim (GH #759); \
         got: {partial}",
    );
    assert_eq!(partial, "sum(matrix[state, *])");
}

/// References that are already inside a PREVIOUS() call's argument are
/// already lagged: their value is fixed at the prior step and cannot be
/// affected by the current-step ceteris-paribus perturbation, so the
/// wrapper must NOT wrap them again. Double-wrapping reads the value from
/// two steps ago (semantically wrong) and forces a nested-PREVIOUS helper
/// chain (one synthesized helper variable per occurrence -- the dominant
/// remaining helper source on SAMPLE-IF-TRUE-heavy models like C-LEARN).
#[test]
fn partial_equation_does_not_rewrap_inside_previous() {
    // Target equation shape: `if cond then input else previous(self, input)`
    // -- the SAMPLE IF TRUE pattern. `target` (the self-reference) and
    // `cond` are other-deps; `input` is the live source.
    let deps = deps_set(&["target", "cond", "input"]);
    let live = Ident::new("input");
    let shape = RefShape::Bare;

    let partial = build_partial_equation_shaped(
        "if cond then input else PREVIOUS(target, input)",
        &deps,
        &live,
        &shape,
        &[],
        None,
        None,
    )
    .unwrap();

    // The dep `cond` (outside any PREVIOUS) is wrapped...
    assert!(
        partial.to_lowercase().contains("previous(cond)"),
        "cond must be wrapped for ceteris-paribus; got: {partial}",
    );
    // ...but `target` inside the original PREVIOUS call is already
    // lagged and must NOT be double-wrapped.
    assert!(
        !partial.to_lowercase().contains("previous(previous(target)"),
        "reference inside PREVIOUS must not be double-wrapped; got: {partial}",
    );
    // The original previous(target, input) call survives intact.
    assert!(
        partial.to_lowercase().contains("previous(target,"),
        "the original lagged self-reference must survive; got: {partial}",
    );
}

// -- Arrayed-target link scores: per-element partial equations --
//
// For a per-element-equation (`Ast::Arrayed`) target, the link score
// must be an `Equation::Arrayed` whose per-element slot equation is the
// standard link-score guard form built around *that element's own*
// partial equation -- not a `"0"` placeholder. The tests below build a
// 2-region per-element-equation aux (`migration_pressure`) and verify
// that the `population -> migration_pressure` link-score equation for
// each `FixedIndex` shape carries the right partial in every slot.

/// Build a stage-1 `Variable` (lowered, `Expr2`) for a per-element-
/// equation (`Equation::Arrayed`) variable from raw element equation
/// text. Routes through the same `datamodel::Variable` -> parse -> lower
/// path production uses, so the result carries both `ast: Some(Ast::Arrayed)`
/// and `eqn: Some(Equation::Arrayed)`.
fn arrayed_var_from_text(
    ident: &str,
    dims: &[crate::datamodel::Dimension],
    elements: &[(&str, &str)],
    is_flow: bool,
) -> Variable {
    use crate::datamodel::{Aux, Equation as DmEquation, Flow, Variable as DmVariable};

    let equation = DmEquation::Arrayed(
        dims.iter().map(|d| d.name().to_string()).collect(),
        elements
            .iter()
            .map(|(e, eq)| ((*e).to_string(), (*eq).to_string(), None, None))
            .collect(),
        None,
        false,
    );
    let dm_var = if is_flow {
        DmVariable::Flow(Flow {
            ident: ident.to_string(),
            equation,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        })
    } else {
        DmVariable::Aux(Aux {
            ident: ident.to_string(),
            equation,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        })
    };

    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let mut implicit_vars = Vec::new();
    let stage0 = crate::variable::parse_var::<crate::datamodel::ModuleReference, _>(
        dims,
        &dm_var,
        &mut implicit_vars,
        &units_ctx,
        |mi| Ok(Some(mi.clone())),
    );
    let dim_ctx = crate::dimensions::DimensionsContext::from(dims);
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_ctx,
        model_name: "test",
    };
    crate::model::lower_variable(&scope, &stage0)
}

/// Look up the slot equation for `element` in an `Equation::Arrayed`,
/// failing the test loudly if the equation isn't `Arrayed` or the slot
/// is missing.
fn arrayed_slot<'a>(equation: &'a Equation, element: &str) -> &'a str {
    match equation {
        Equation::Arrayed(_, elements, _, _) => elements
            .iter()
            .find(|(e, _, _, _)| e == element)
            .map(|(_, eqn, _, _)| eqn.as_str())
            .unwrap_or_else(|| {
                panic!("no slot for element {element:?} in arrayed equation: {equation:?}")
            }),
        other => panic!("expected Equation::Arrayed, got: {other:?}"),
    }
}

fn region_dm_dimension() -> crate::datamodel::Dimension {
    crate::datamodel::Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string()],
    )
}

/// Build a scalar `Aux` variable from its equation text, lowering it the
/// same way the production path does so the generated `Variable` carries a
/// real (or, for empty text, absent) AST.
fn scalar_aux_from_text(ident: &str, eqn_text: &str) -> Variable {
    use crate::datamodel::{Aux, Equation as DmEquation, Variable as DmVariable};
    let dm_var = DmVariable::Aux(Aux {
        ident: ident.to_string(),
        equation: DmEquation::Scalar(eqn_text.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    });
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let mut implicit_vars = Vec::new();
    let stage0 = crate::variable::parse_var::<crate::datamodel::ModuleReference, _>(
        &[],
        &dm_var,
        &mut implicit_vars,
        &units_ctx,
        |mi| Ok(Some(mi.clone())),
    );
    let dim_ctx = crate::dimensions::DimensionsContext::from(&[][..]);
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_ctx,
        model_name: "test",
    };
    crate::model::lower_variable(&scope, &stage0)
}

/// GH #311 end-to-end through the generator chain: a target whose
/// equation text is empty (an `Ok(None)` parse) must make the whole
/// `generate_link_score_equation_for_link` chain return `Err` carrying
/// the (empty) equation text -- so the db-bearing caller skips the
/// variable and warns, rather than emitting a non-ceteris-paribus score.
#[test]
fn generate_link_score_equation_for_link_empty_target_is_err() {
    let from = Ident::<Canonical>::new("source");
    let to = Ident::<Canonical>::new("target");
    // An empty scalar equation: parses as `Ok(None)`, so there is no AST
    // to build a ceteris-paribus partial from.
    let to_var = scalar_aux_from_text("target", "");
    let from_var = scalar_aux_from_text("source", "1");

    let mut all_vars = HashMap::new();
    all_vars.insert(from.clone(), from_var);
    all_vars.insert(to.clone(), to_var.clone());

    let result = generate_link_score_equation_for_link(
        &from,
        &to,
        &RefShape::Bare,
        &[],
        &to_var,
        &all_vars,
        None,
        None,
    );
    assert!(
        result.is_err(),
        "an empty-equation target must yield a loud Err, not a magnitude-1 \
         link score; got {result:?}"
    );
}

/// A normal scalar target with a real equation still produces a valid
/// `Ok(Equation)` whose partial freezes the non-source dep at PREVIOUS --
/// the success path must be unaffected by the GH #311 error plumbing.
#[test]
fn generate_link_score_equation_for_link_normal_target_is_ok() {
    let from = Ident::<Canonical>::new("source");
    let to = Ident::<Canonical>::new("target");
    let to_var = scalar_aux_from_text("target", "source * other");
    let from_var = scalar_aux_from_text("source", "1");
    let other_var = scalar_aux_from_text("other", "2");

    let mut all_vars = HashMap::new();
    all_vars.insert(from.clone(), from_var);
    all_vars.insert(Ident::<Canonical>::new("other"), other_var);
    all_vars.insert(to.clone(), to_var.clone());

    let equation = generate_link_score_equation_for_link(
        &from,
        &to,
        &RefShape::Bare,
        &[],
        &to_var,
        &all_vars,
        None,
        None,
    )
    .expect("a normal scalar target must produce a valid link-score equation");
    let text = match &equation {
        Equation::Scalar(t) => t.clone(),
        other => panic!("expected a scalar link score, got {other:?}"),
    };
    // The non-source dep is frozen; the source stays live -- the partial
    // is genuinely ceteris-paribus, NOT identical to the full equation.
    assert!(
        text.contains("PREVIOUS(other)"),
        "the non-source dep must be PREVIOUS-frozen; got {text}"
    );
    assert!(
        text.contains("source * PREVIOUS(other)"),
        "the partial must keep source live and freeze other; got {text}"
    );
}

#[test]
fn test_arrayed_link_score_population_to_migration_pressure_fixed_nyc() {
    // ltm-503-cross-element-agg.AC1.1
    // migration_pressure is a per-element-equation aux:
    //   migration_pressure[NYC]    = (population[NYC] - population[Boston]) * 0.01
    //   migration_pressure[Boston] = (population[Boston] - population[NYC]) * 0.01
    // For the `population -> migration_pressure` link with shape
    // FixedIndex(["nyc"]), the `population[nyc]` ref stays live in every
    // slot and the other-element refs are frozen at PREVIOUS().
    let dims = vec![region_dm_dimension()];
    let to_var = arrayed_var_from_text(
        "migration_pressure",
        &dims,
        &[
            ("NYC", "(population[NYC] - population[Boston]) * 0.01"),
            ("Boston", "(population[Boston] - population[NYC]) * 0.01"),
        ],
        false,
    );

    let from = Ident::<Canonical>::new("population");
    let to = Ident::<Canonical>::new("migration_pressure");
    let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
    let source_dim_elements = region_dim_elements();

    let equation = generate_auxiliary_to_auxiliary_equation(
        &from,
        &to,
        &shape,
        &source_dim_elements,
        &[],
        &to_var,
        None,
        None,
    )
    .unwrap();

    match &equation {
        Equation::Arrayed(eq_dims, _, default, _) => {
            assert_eq!(eq_dims, &["Region".to_string()]);
            assert!(default.is_none(), "no EXCEPT default expected");
        }
        other => panic!("expected Equation::Arrayed, got: {other:?}"),
    }

    let nyc_slot = arrayed_slot(&equation, "nyc");
    let boston_slot = arrayed_slot(&equation, "boston");

    // No slot may carry the `(0)` placeholder partial that the pre-fix
    // `_ => "0"` fall-through produced.
    assert!(
        !nyc_slot.contains("((0) -"),
        "nyc slot must not use a '0' partial; got: {nyc_slot}"
    );
    assert!(
        !boston_slot.contains("((0) -"),
        "boston slot must not use a '0' partial; got: {boston_slot}"
    );

    // The `{partial}` substring is the canonical per-element equation
    // with the live-shape ref kept live and the rest frozen.
    assert!(
        nyc_slot.contains("(population[nyc] - PREVIOUS(population[boston])) * 0.01"),
        "nyc slot partial mismatch; got: {nyc_slot}"
    );
    assert!(
        boston_slot.contains("(PREVIOUS(population[boston]) - population[nyc]) * 0.01"),
        "boston slot partial mismatch; got: {boston_slot}"
    );

    // The guard form references the target element-wise (bare name) and
    // the shape-aware source subscript.
    assert!(
        nyc_slot.contains("(migration_pressure - PREVIOUS(migration_pressure))"),
        "nyc slot target ref mismatch; got: {nyc_slot}"
    );
    assert!(
        nyc_slot.contains("(population[nyc] - PREVIOUS(population[nyc]))"),
        "nyc slot source ref mismatch; got: {nyc_slot}"
    );
}

#[test]
fn test_arrayed_link_score_population_to_migration_pressure_fixed_boston() {
    // ltm-503-cross-element-agg.AC1.2
    // Same model; shape FixedIndex(["boston"]) keeps `population[boston]`
    // live and freezes `population[nyc]`.
    let dims = vec![region_dm_dimension()];
    let to_var = arrayed_var_from_text(
        "migration_pressure",
        &dims,
        &[
            ("NYC", "(population[NYC] - population[Boston]) * 0.01"),
            ("Boston", "(population[Boston] - population[NYC]) * 0.01"),
        ],
        false,
    );

    let from = Ident::<Canonical>::new("population");
    let to = Ident::<Canonical>::new("migration_pressure");
    let shape = RefShape::FixedIndex(vec!["boston".to_string()]);
    let source_dim_elements = region_dim_elements();

    let equation = generate_auxiliary_to_auxiliary_equation(
        &from,
        &to,
        &shape,
        &source_dim_elements,
        &[],
        &to_var,
        None,
        None,
    )
    .unwrap();

    let nyc_slot = arrayed_slot(&equation, "nyc");
    let boston_slot = arrayed_slot(&equation, "boston");

    assert!(
        !nyc_slot.contains("((0) -") && !boston_slot.contains("((0) -"),
        "no slot may use a '0' partial; nyc={nyc_slot} boston={boston_slot}"
    );
    assert!(
        nyc_slot.contains("(PREVIOUS(population[nyc]) - population[boston]) * 0.01"),
        "nyc slot partial mismatch; got: {nyc_slot}"
    );
    assert!(
        boston_slot.contains("(population[boston] - PREVIOUS(population[nyc])) * 0.01"),
        "boston slot partial mismatch; got: {boston_slot}"
    );
    // Source ref is the FixedIndex(boston) subscript, constant across slots.
    assert!(
        boston_slot.contains("(population[boston] - PREVIOUS(population[boston]))"),
        "boston slot source ref mismatch; got: {boston_slot}"
    );
}

#[test]
fn test_arrayed_link_score_stock_to_flow_per_element_partials() {
    // ltm-503-cross-element-agg.AC1.3 (unit-level): a stock-to-flow link
    // score into a per-element-equation arrayed flow yields per-element
    // partials referencing the flow's actual equation contents.
    let dims = vec![crate::datamodel::Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
    )];
    // `births[Region]` per-element flow referencing the `population` stock.
    let births = arrayed_var_from_text(
        "births",
        &dims,
        &[
            ("NYC", "population[NYC] * 0.03"),
            ("Boston", "population[Boston] * 0.02"),
            ("LA", "population[LA] * 0.01"),
        ],
        true,
    );

    let stock = Ident::<Canonical>::new("population");
    let flow = Ident::<Canonical>::new("births");
    // Each `births[e]` references `population[e]` -- a FixedIndex ref.
    let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
    let source_dim_elements = vec![vec![
        "nyc".to_string(),
        "boston".to_string(),
        "la".to_string(),
    ]];

    let equation = generate_stock_to_flow_equation(
        &stock,
        &flow,
        &shape,
        &source_dim_elements,
        &[],
        &births,
        None,
        None,
    )
    .unwrap();

    let nyc_slot = arrayed_slot(&equation, "nyc");
    let boston_slot = arrayed_slot(&equation, "boston");
    // The NYC slot keeps population[nyc] live (shape match); the other
    // slots freeze their population refs but still reference
    // `population` -- never a bare `(0)` partial.
    assert!(
        nyc_slot.contains("population[nyc] * 0.03"),
        "nyc slot partial should keep population[nyc] live; got: {nyc_slot}"
    );
    assert!(
        boston_slot.contains("population"),
        "boston slot should reference population; got: {boston_slot}"
    );
    assert!(
        !nyc_slot.contains("((0) -") && !boston_slot.contains("((0) -"),
        "no slot may use a '0' partial; nyc={nyc_slot} boston={boston_slot}"
    );
}

#[test]
fn test_scalar_and_a2a_link_scores_keep_their_shapes() {
    // Guard: the Arrayed-target path must not regress scalar or
    // ApplyToAll targets. A scalar aux target -> Equation::Scalar; an
    // ApplyToAll arrayed aux target -> Equation::ApplyToAll.
    let scalar_to = Variable::Var {
        ident: Ident::new("scalar_target"),
        ast: Some(Ast::Scalar(var_ref("driver"))),
        init_ast: None,
        eqn: Some(Equation::Scalar("driver".to_string())),
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };
    let from = Ident::<Canonical>::new("driver");
    let to = Ident::<Canonical>::new("scalar_target");
    let equation = generate_auxiliary_to_auxiliary_equation(
        &from,
        &to,
        &RefShape::Bare,
        &[],
        &[],
        &scalar_to,
        None,
        None,
    )
    .unwrap();
    assert!(
        matches!(equation, Equation::Scalar(_)),
        "scalar target must yield Equation::Scalar; got: {equation:?}"
    );

    // ApplyToAll target.
    let dims = vec![region_dm_dimension()];
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let mut implicit = Vec::new();
    let a2a_dm = crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: "a2a_target".to_string(),
        equation: Equation::ApplyToAll(vec!["Region".to_string()], "driver * 0.5".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    });
    let stage0 = crate::variable::parse_var::<crate::datamodel::ModuleReference, _>(
        &dims,
        &a2a_dm,
        &mut implicit,
        &units_ctx,
        |mi| Ok(Some(mi.clone())),
    );
    let dim_ctx = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_ctx,
        model_name: "test",
    };
    let a2a_to = crate::model::lower_variable(&scope, &stage0);
    let to_a2a = Ident::<Canonical>::new("a2a_target");
    let equation = generate_auxiliary_to_auxiliary_equation(
        &from,
        &to_a2a,
        &RefShape::Bare,
        &[],
        &[],
        &a2a_to,
        None,
        None,
    )
    .unwrap();
    match equation {
        Equation::ApplyToAll(d, _) => assert_eq!(d, vec!["Region".to_string()]),
        other => panic!("ApplyToAll target must yield Equation::ApplyToAll; got: {other:?}"),
    }
}

/// Build a `Variable::Stock` for the flow-to-stock generator tests.
/// Only `ident`, `eqn` (dimension source via `target_equation_dims`),
/// and `inflows`/`outflows` (inflow/outflow sign) matter to the
/// generator; the rest are defaulted.
fn flow_to_stock_test_stock(
    ident: &str,
    eqn: Equation,
    inflows: &[&str],
    outflows: &[&str],
) -> Variable {
    Variable::Stock {
        ident: Ident::new(ident),
        init_ast: None,
        eqn: Some(eqn),
        units: None,
        inflows: inflows.iter().map(|f| Ident::new(f)).collect(),
        outflows: outflows.iter().map(|f| Ident::new(f)).collect(),
        non_negative: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

/// Build a `Variable::Var` flow for the flow-to-stock generator tests.
/// Only `ident` and `eqn` (dimension source) matter to the generator.
fn flow_to_stock_test_flow(ident: &str, eqn: Equation) -> Variable {
    Variable::Var {
        ident: Ident::new(ident),
        ast: None,
        init_ast: None,
        eqn: Some(eqn),
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: true,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

/// LTM deep-review Finding 2: for an *arrayed* stock the flow-to-stock
/// link-score equation must reference the stock and flow with explicit
/// dimension subscripts. A *bare* arrayed name nested inside
/// `PREVIOUS(PREVIOUS(...))` is routed through a synthesized *scalar*
/// helper aux (see `builtins_visitor`) that cannot hold an arrayed
/// value -- the fragment then fails to compile and the LTM compiler
/// silently stubs it to 0, collapsing the score to a wrong constant
/// (`1/9` for the canonical pop/growth model instead of the
/// isolated-loop invariant `1`).
#[test]
fn test_flow_to_stock_arrayed_subscripts_references() {
    let stock = flow_to_stock_test_stock(
        "pop",
        Equation::ApplyToAll(vec!["region".to_string()], "100".to_string()),
        &["growth"],
        &[],
    );
    let flow = flow_to_stock_test_flow(
        "growth",
        Equation::ApplyToAll(vec!["region".to_string()], "pop[region] * 0.1".to_string()),
    );

    let equation = generate_flow_to_stock_equation("growth", "pop", &flow, &stock);
    let text = match &equation {
        Equation::ApplyToAll(dims, text) => {
            assert_eq!(dims, &vec!["region".to_string()]);
            text
        }
        other => panic!("arrayed stock must yield Equation::ApplyToAll; got: {other:?}"),
    };

    // Every stock/flow occurrence carries the dimension subscript --
    // including the nested-PREVIOUS terms, which are exactly the ones
    // that break with a bare arrayed name.
    assert!(
        text.contains("PREVIOUS(PREVIOUS(growth[region]))"),
        "nested-PREVIOUS flow term must be subscripted; got: {text}"
    );
    assert!(
        text.contains("PREVIOUS(PREVIOUS(pop[region]))"),
        "nested-PREVIOUS stock term must be subscripted; got: {text}"
    );
    // ...and no bare arrayed name survives as a PREVIOUS argument.
    assert!(
        !text.contains("PREVIOUS(growth)") && !text.contains("PREVIOUS(growth,"),
        "no bare arrayed flow reference may remain; got: {text}"
    );
    assert!(
        !text.contains("PREVIOUS(pop)") && !text.contains("PREVIOUS(pop,"),
        "no bare arrayed stock reference may remain; got: {text}"
    );
}

/// Guard: a *scalar* stock's flow-to-stock equation must NOT gain
/// subscripts -- it stays the bare-name `Equation::Scalar` form so the
/// scalar isolated-loop invariant (pinned by `ltm_dt_invariance.rs`)
/// is unaffected.
#[test]
fn test_flow_to_stock_scalar_stays_bare() {
    let stock =
        flow_to_stock_test_stock("s", Equation::Scalar("100".to_string()), &["births"], &[]);
    let flow = flow_to_stock_test_flow("births", Equation::Scalar("s * 0.1".to_string()));

    let equation = generate_flow_to_stock_equation("births", "s", &flow, &stock);
    let text = match &equation {
        Equation::Scalar(text) => text,
        other => panic!("scalar stock must yield Equation::Scalar; got: {other:?}"),
    };

    assert!(
        text.contains("PREVIOUS(PREVIOUS(births))"),
        "scalar flow term must stay bare; got: {text}"
    );
    assert!(
        text.contains("PREVIOUS(PREVIOUS(s))"),
        "scalar stock term must stay bare; got: {text}"
    );
    assert!(
        !text.contains('['),
        "scalar flow-to-stock equation must have no subscripts; got: {text}"
    );
}

/// An arrayed *outflow* keeps the negative structural sign while still
/// being subscripted: the sign is applied outside `ABS()`, independent
/// of the subscripting.
#[test]
fn test_flow_to_stock_arrayed_outflow_sign() {
    let stock = flow_to_stock_test_stock(
        "pop",
        Equation::ApplyToAll(vec!["region".to_string()], "100".to_string()),
        &[],
        &["deaths"],
    );
    let flow = flow_to_stock_test_flow(
        "deaths",
        Equation::ApplyToAll(vec!["region".to_string()], "pop[region] * 0.05".to_string()),
    );

    let equation = generate_flow_to_stock_equation("deaths", "pop", &flow, &stock);
    let text = match &equation {
        Equation::ApplyToAll(_, text) => text,
        other => panic!("arrayed stock must yield Equation::ApplyToAll; got: {other:?}"),
    };

    assert!(
        text.contains("-ABS(SAFEDIV("),
        "outflow link score must carry the negative structural sign; got: {text}"
    );
    assert!(
        text.contains("PREVIOUS(PREVIOUS(deaths[region]))"),
        "outflow must still be subscripted; got: {text}"
    );
}

// -- GH #653 Phase 1: dimension-aware loop-score equation generation --
//
// `generate_loop_score_variables` returns the real `datamodel::Equation`
// each loop score should carry: Scalar for scalar loops, ApplyToAll for
// dimensioned loops whose links resolve through Bare A2A names, and
// Arrayed (per-slot equations) for dimensioned loops backed by per-element
// circuits (`Loop.slot_links`).

/// Helper: a `datamodel::Dimension` for the dimension-aware loop-score
/// tests (these need datamodel dims, not `dimensions::Dimension`, because
/// the Arrayed slot enumeration reads the project's declared element
/// order).
fn dm_named_dimension(name: &str, elements: &[&str]) -> crate::datamodel::Dimension {
    crate::datamodel::Dimension::named(
        name.to_string(),
        elements.iter().map(|s| s.to_string()).collect(),
    )
}

fn ls_name(from: &str, to: &str) -> String {
    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")
}

fn make_link(from: &str, to: &str) -> crate::ltm::Link {
    crate::ltm::Link {
        from: Ident::<Canonical>::new(from),
        to: Ident::<Canonical>::new(to),
        polarity: crate::ltm::LinkPolarity::Positive,
    }
}

/// A scalar loop produces an `Equation::Scalar` whose text is the plain
/// product of its (Bare-resolved) link-score references.
#[test]
fn loop_score_variables_scalar_loop_yields_scalar_equation() {
    use crate::ltm::{Loop, LoopPolarity};

    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![make_link("pop", "births"), make_link("births", "pop")],
        stocks: vec![Ident::<Canonical>::new("pop")],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("pop", "births"));
    emitted.insert(ls_name("births", "pop"));

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &[],
        &Default::default(),
    );
    assert_eq!(vars.len(), 1);
    let (name, equation) = &vars[0];
    assert_eq!(name, "$\u{205A}ltm\u{205A}loop_score\u{205A}r1");
    match equation {
        Equation::Scalar(text) => {
            assert_eq!(
                text,
                &format!(
                    "\"{}\" * \"{}\"",
                    ls_name("pop", "births"),
                    ls_name("births", "pop")
                ),
                "scalar loop score must be the plain product of Bare link-score refs"
            );
        }
        other => panic!("scalar loop must yield Equation::Scalar; got: {other:?}"),
    }
}

/// A dimensioned loop with no `slot_links` (the Bare-A2A fast path)
/// produces the compact `Equation::ApplyToAll` form -- byte-identical
/// reference text to the scalar case, tagged with the loop's dimensions.
#[test]
fn loop_score_variables_a2a_without_slot_links_yields_apply_to_all() {
    use crate::ltm::{Loop, LoopPolarity};

    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![make_link("pop", "births"), make_link("births", "pop")],
        stocks: vec![
            Ident::<Canonical>::new("pop[nyc]"),
            Ident::<Canonical>::new("pop[boston]"),
        ],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["region".to_string()],
        slot_links: vec![],
    };
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("pop", "births"));
    emitted.insert(ls_name("births", "pop"));
    let dims = vec![dm_named_dimension("region", &["nyc", "boston"])];

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &dims,
        &Default::default(),
    );
    assert_eq!(vars.len(), 1);
    let (_, equation) = &vars[0];
    match equation {
        Equation::ApplyToAll(eq_dims, text) => {
            assert_eq!(eq_dims, &vec!["region".to_string()]);
            assert_eq!(
                text,
                &format!(
                    "\"{}\" * \"{}\"",
                    ls_name("pop", "births"),
                    ls_name("births", "pop")
                ),
                "Bare-A2A loop score must keep the compact ApplyToAll product form"
            );
        }
        other => panic!("Bare-A2A loop must yield Equation::ApplyToAll; got: {other:?}"),
    }
}

/// A dimensioned loop backed by per-element circuits (`slot_links`)
/// produces an `Equation::Arrayed` whose slot equations reference each
/// element's own per-element link-score names: the FixedIndex form
/// (`heat[det]→temp`, an arrayed var) is referenced subscripted at the
/// slot's element, and the per-target-element scalar form
/// (`pressure→production[det]`) is referenced bare.
#[test]
fn loop_score_variables_slot_links_yield_arrayed_per_slot_equations() {
    use crate::ltm::{Loop, LoopPolarity};

    // Two-element scenario dimension; per-element circuits reference
    // FixedIndex link scores (the C-LEARN / MDL-importer shape).
    let slot_links = vec![
        (
            "det".to_string(),
            vec![
                make_link("heat[det]", "temp[det]"),
                make_link("temp[det]", "heat[det]"),
            ],
        ),
        (
            "low".to_string(),
            vec![
                make_link("heat[low]", "temp[low]"),
                make_link("temp[low]", "heat[low]"),
            ],
        ),
    ];
    let loop_item = Loop {
        id: "pin1".to_string(),
        links: vec![make_link("heat", "temp"), make_link("temp", "heat")],
        stocks: vec![
            Ident::<Canonical>::new("heat[det]"),
            Ident::<Canonical>::new("heat[low]"),
        ],
        polarity: LoopPolarity::Balancing,
        dimensions: vec!["scenario".to_string()],
        slot_links,
    };
    // The emitted link scores are the per-element FixedIndex forms (each
    // an arrayed var over [scenario]); the Bare names do NOT exist.
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("heat[det]", "temp"));
    emitted.insert(ls_name("heat[low]", "temp"));
    emitted.insert(ls_name("temp[det]", "heat"));
    emitted.insert(ls_name("temp[low]", "heat"));
    let dims = vec![dm_named_dimension("scenario", &["det", "low"])];

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &dims,
        &Default::default(),
    );
    assert_eq!(vars.len(), 1);
    let (name, equation) = &vars[0];
    assert_eq!(name, "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1");
    match equation {
        Equation::Arrayed(eq_dims, elements, default, _) => {
            assert_eq!(eq_dims, &vec!["scenario".to_string()]);
            assert!(default.is_none());
            assert_eq!(
                elements.len(),
                2,
                "one slot equation per scenario element; got: {elements:?}"
            );
            // Slot order follows the dimension's declared element order.
            assert_eq!(elements[0].0, "det");
            assert_eq!(elements[1].0, "low");
            // The det slot references det's FixedIndex link scores
            // subscripted at det (they are arrayed vars), and must not
            // reference low's.
            let det_eq = &elements[0].1;
            assert!(
                det_eq.contains(&format!("\"{}\"[det]", ls_name("heat[det]", "temp"))),
                "det slot must reference heat[det]→temp subscripted at [det]; got: {det_eq}"
            );
            assert!(
                !det_eq.contains("low"),
                "det slot must not reference the low element's link scores; got: {det_eq}"
            );
            let low_eq = &elements[1].1;
            assert!(
                low_eq.contains(&format!("\"{}\"[low]", ls_name("heat[low]", "temp"))),
                "low slot must reference heat[low]→temp subscripted at [low]; got: {low_eq}"
            );
        }
        other => panic!("slot_links loop must yield Equation::Arrayed; got: {other:?}"),
    }
}

/// A dimension element with no backing circuit (a structurally absent
/// per-element instance) gets a constant-0 slot equation so the Arrayed
/// equation stays total over the dimension's element space.
#[test]
fn loop_score_variables_missing_slot_scores_zero() {
    use crate::ltm::{Loop, LoopPolarity};

    let slot_links = vec![(
        "det".to_string(),
        vec![
            make_link("heat[det]", "temp[det]"),
            make_link("temp[det]", "heat[det]"),
        ],
    )];
    let loop_item = Loop {
        id: "pin1".to_string(),
        links: vec![make_link("heat", "temp"), make_link("temp", "heat")],
        stocks: vec![Ident::<Canonical>::new("heat[det]")],
        polarity: LoopPolarity::Balancing,
        dimensions: vec!["scenario".to_string()],
        slot_links,
    };
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("heat[det]", "temp"));
    emitted.insert(ls_name("temp[det]", "heat"));
    // Three declared elements; only `det` has a circuit.
    let dims = vec![dm_named_dimension("scenario", &["det", "low", "high"])];

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &dims,
        &Default::default(),
    );
    let (_, equation) = &vars[0];
    match equation {
        Equation::Arrayed(_, elements, _, _) => {
            assert_eq!(elements.len(), 3, "every declared element gets a slot");
            let by_elem: std::collections::HashMap<&str, &str> = elements
                .iter()
                .map(|(e, eq, _, _)| (e.as_str(), eq.as_str()))
                .collect();
            assert!(by_elem["det"].contains("link_score"));
            assert_eq!(
                by_elem["low"], "0",
                "a slot with no backing circuit must score a constant 0"
            );
            assert_eq!(by_elem["high"], "0");
        }
        other => panic!("expected Equation::Arrayed; got: {other:?}"),
    }
}

/// Multi-dimensional slot tuples: a loop over `[region, age]` keys its
/// slots by the comma-joined element tuple, in row-major declared order.
#[test]
fn loop_score_variables_multi_dim_slot_tuples() {
    use crate::ltm::{Loop, LoopPolarity};

    let slot_links = vec![
        (
            "nyc,young".to_string(),
            vec![
                make_link("pop[nyc,young]", "births[nyc,young]"),
                make_link("births[nyc,young]", "pop[nyc,young]"),
            ],
        ),
        (
            "boston,old".to_string(),
            vec![
                make_link("pop[boston,old]", "births[boston,old]"),
                make_link("births[boston,old]", "pop[boston,old]"),
            ],
        ),
    ];
    let loop_item = Loop {
        id: "pin1".to_string(),
        links: vec![make_link("pop", "births"), make_link("births", "pop")],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["region".to_string(), "age".to_string()],
        slot_links,
    };
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("pop[nyc,young]", "births"));
    emitted.insert(ls_name("births[nyc,young]", "pop"));
    emitted.insert(ls_name("pop[boston,old]", "births"));
    emitted.insert(ls_name("births[boston,old]", "pop"));
    let dims = vec![
        dm_named_dimension("region", &["nyc", "boston"]),
        dm_named_dimension("age", &["young", "old"]),
    ];

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &dims,
        &Default::default(),
    );
    let (_, equation) = &vars[0];
    match equation {
        Equation::Arrayed(_, elements, _, _) => {
            // Row-major over declared order: nyc,young / nyc,old /
            // boston,young / boston,old.
            let keys: Vec<&str> = elements.iter().map(|(e, _, _, _)| e.as_str()).collect();
            assert_eq!(
                keys,
                vec!["nyc,young", "nyc,old", "boston,young", "boston,old"]
            );
            let by_elem: std::collections::HashMap<&str, &str> = elements
                .iter()
                .map(|(e, eq, _, _)| (e.as_str(), eq.as_str()))
                .collect();
            assert!(by_elem["nyc,young"].contains("link_score"));
            assert_eq!(by_elem["nyc,old"], "0");
            assert_eq!(by_elem["boston,young"], "0");
            assert!(by_elem["boston,old"].contains("link_score"));
        }
        other => panic!("expected Equation::Arrayed; got: {other:?}"),
    }
}

/// When every link of a dimensioned loop resolves to an emitted Bare A2A
/// link-score name, the compact `ApplyToAll` form is preferred even when
/// per-slot circuit info (`slot_links`) is available -- the diagonal A2A
/// read is correct, and the compact form keeps Bare-shaped models'
/// equations byte-identical to the pre-#653 output.
#[test]
fn loop_score_variables_prefer_apply_to_all_when_all_links_bare() {
    use crate::ltm::{Loop, LoopPolarity};

    let slot_links = vec![
        (
            "nyc".to_string(),
            vec![
                make_link("pop[nyc]", "births[nyc]"),
                make_link("births[nyc]", "pop[nyc]"),
            ],
        ),
        (
            "boston".to_string(),
            vec![
                make_link("pop[boston]", "births[boston]"),
                make_link("births[boston]", "pop[boston]"),
            ],
        ),
    ];
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![make_link("pop", "births"), make_link("births", "pop")],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["region".to_string()],
        slot_links,
    };
    // Both Bare A2A names are emitted -> ApplyToAll wins over slot_links.
    let mut emitted = HashSet::new();
    emitted.insert(ls_name("pop", "births"));
    emitted.insert(ls_name("births", "pop"));
    let dims = vec![dm_named_dimension("region", &["nyc", "boston"])];

    let vars = generate_loop_score_variables(
        std::slice::from_ref(&loop_item),
        &emitted,
        &dims,
        &Default::default(),
    );
    let (_, equation) = &vars[0];
    match equation {
        Equation::ApplyToAll(eq_dims, text) => {
            assert_eq!(eq_dims, &vec!["region".to_string()]);
            assert_eq!(
                text,
                &format!(
                    "\"{}\" * \"{}\"",
                    ls_name("pop", "births"),
                    ls_name("births", "pop")
                ),
            );
        }
        other => panic!(
            "all-Bare dimensioned loop must keep the compact ApplyToAll form; got: {other:?}"
        ),
    }
}

/// GH #737: the scalar-feeder → agg link-score equation freezes ONLY the
/// feeder (changed-last attribution): the reducer's array references stay
/// live exactly as in the agg's own equation (the changed-first form would
/// need a lagged whole-array read, which does not compile), and the guard
/// structure matches `link_score_guard_form`.
#[test]
fn test_generate_scalar_feeder_to_agg_equation_freezes_only_feeder() {
    let eq = generate_scalar_feeder_to_agg_equation(
        "scale",
        "$\u{205A}ltm\u{205A}agg\u{205A}0",
        "sum(pop[*] * scale)",
    )
    .expect("the agg equation text must parse");
    // The frozen evaluation wraps the feeder, not the array reference.
    assert!(
        eq.contains("sum(pop[*] * PREVIOUS(scale))"),
        "the frozen partial must wrap only the scalar feeder; got: {eq}"
    );
    assert!(
        !eq.contains("PREVIOUS(pop"),
        "the array reference must stay live (a lagged whole-array read does not compile); \
         got: {eq}"
    );
    // The numerator is changed-last: agg minus the feeder-frozen evaluation.
    assert!(
        eq.contains("\"$\u{205A}ltm\u{205A}agg\u{205A}0\" - (sum(pop[*] * PREVIOUS(scale)))"),
        "the numerator must subtract the feeder-frozen evaluation from the agg; got: {eq}"
    );
    // Standard guard structure: initial-step zero, zero-delta zero, SAFEDIV.
    assert!(
        eq.starts_with("if (TIME = INITIAL_TIME) then 0"),
        "got: {eq}"
    );
    assert!(eq.contains("SAFEDIV("), "got: {eq}");
    assert!(eq.contains("SIGN((scale - PREVIOUS(scale)))"), "got: {eq}");
}

/// GH #767 (T5): the iterated-dim projection-feeder per-`(row, slot)`
/// equation pins the reducer text's iterated-dim indices to the slot
/// (every reference, co-source and feeder alike), freezes ONLY the
/// feeder's slot-pinned reference (changed-last -- the co-source's
/// wildcard slice stays verbatim, exactly like the scalar feeder), and
/// carries the slot subscript on the agg/target and feeder guard
/// references.
#[test]
fn test_generate_iterated_feeder_to_agg_equation_pins_slot_and_freezes_feeder() {
    let eq = generate_iterated_feeder_to_agg_equation(
        "frac",
        "growth",
        "sum(matrix[d1, *] * frac[d1])",
        &["d1".to_string()],
        &["d1\u{B7}r1".to_string()],
    )
    .expect("the agg equation text must parse");
    assert_eq!(
        eq,
        "if (TIME = INITIAL_TIME) then 0 else if ((growth[d1\u{B7}r1] - \
         PREVIOUS(growth[d1\u{B7}r1])) = 0) OR ((frac[d1\u{B7}r1] - \
         PREVIOUS(frac[d1\u{B7}r1])) = 0) then 0 else \
         SAFEDIV((growth[d1\u{B7}r1] - (sum(matrix[d1\u{B7}r1, *] * \
         PREVIOUS(frac[d1\u{B7}r1])))), ABS((growth[d1\u{B7}r1] - \
         PREVIOUS(growth[d1\u{B7}r1]))), 0) * SIGN((frac[d1\u{B7}r1] - \
         PREVIOUS(frac[d1\u{B7}r1])))"
    );
}

/// GH #767 (T5): a feeder absent from the reducer text (no occurrence to
/// freeze) is the unfreezable loud-failure contract -- the score would be
/// a silent constant 0 otherwise.
#[test]
fn test_generate_iterated_feeder_to_agg_equation_unfreezable_without_occurrence() {
    let err = generate_iterated_feeder_to_agg_equation(
        "absent",
        "growth",
        "sum(matrix[d1, *] * frac[d1])",
        &["d1".to_string()],
        &["d1\u{B7}r1".to_string()],
    )
    .expect_err("a feeder with no occurrence must be unfreezable");
    assert_eq!(err.kind, PartialEquationErrorKind::UnfreezablePartial);
}

/// PR #784 review (P3): a REPEATED dim name among the iterated slot axes
/// (a degenerate square-source agg, `SUM(cube[d1,d1,*] * frac[d1,d1])`
/// with slot dims `[d1, d1]`) makes the by-name slot pin ambiguous --
/// pre-fix, every `d1` index pinned to the FIRST slot part, so the
/// off-diagonal slot `[r1, r2]` froze `frac[d1·r1, d1·r1]` (the wrong
/// source row, a silently wrong score). The generator must bail loudly
/// instead, mirroring `resolve_mismatched_index_position`'s uniqueness
/// defense.
#[test]
fn test_generate_iterated_feeder_to_agg_equation_bails_on_duplicate_dims() {
    let err = generate_iterated_feeder_to_agg_equation(
        "frac",
        "growth",
        "sum(cube[d1, d1, *] * frac[d1, d1])",
        &["d1".to_string(), "d1".to_string()],
        &["d1\u{B7}r1".to_string(), "d1\u{B7}r2".to_string()],
    )
    .expect_err("an ambiguous duplicate-dim slot pin must fail loudly");
    assert_eq!(err.kind, PartialEquationErrorKind::UnfreezablePartial);
}

/// `wrap_matching_in_previous` must not double-lag references that are
/// already inside a `PREVIOUS(...)`/`INIT(...)` call, and must wrap every
/// other occurrence of the target (including inside nested calls and
/// subscript index expressions).
#[test]
fn test_wrap_matching_in_previous_skips_already_lagged() {
    let ast = Expr0::new("sum(arr[*] * scale) + PREVIOUS(scale) + abs(scale)", {
        crate::lexer::LexerType::Equation
    })
    .unwrap()
    .unwrap();
    let wrapped = wrap_matching_in_previous(ast, &Ident::<Canonical>::new("scale"));
    let text = print_eqn(&wrapped);
    // (The parse/print roundtrip lowercases the pre-existing `PREVIOUS` call
    // name; the newly-inserted wrappers keep the uppercase spelling. Both
    // parse identically.)
    assert_eq!(
        text, "sum(arr[*] * PREVIOUS(scale)) + previous(scale) + abs(PREVIOUS(scale))",
        "only un-lagged occurrences of the target are wrapped"
    );
}

/// GH #744 review I1: a FIXED-literal reference to the live source inside
/// the body (`SUM(pop[*] * pop[nyc])` w.r.t. `pop`, row `nyc`) must NOT be
/// row-pinned: the other co-reduced rows' bodies (`pop[i] * pop[nyc]`) also
/// reference the live element, so they do not cancel against
/// PREVIOUS(target) and the single-row partial drops their contribution
/// (`Σ_{i≠e} pop_i_prev * Δpop[e]`). The row must fall back to the
/// delta-ratio form -- consistently with the rows whose literal does not
/// match (which already fell back).
#[test]
fn test_body_aware_fixed_literal_self_reference_falls_back() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new("pop[*] * pop[nyc]", "pop", &[("pop", 1)], &[], &["region"]);
    let eq = generate_element_to_scalar_equation(
        "pop",
        "total",
        "region·nyc",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("SAFEDIV((total - PREVIOUS(total))"),
        "the nyc row must use the delta-ratio fallback: {eq}"
    );
    assert!(
        !eq.contains("pop[region·nyc] * pop[region·nyc]"),
        "the broken pinned self-product must not be emitted: {eq}"
    );
}

/// The same shape with a MOVING axis alongside the literal one
/// (`SUM(matrix[nyc, *])`, the pinned-slice shape) stays on the body
/// partial: each row's live reference moves with the row, so the other
/// co-reduced rows never reference the live element and cancellation
/// holds.
#[test]
fn test_body_aware_pinned_slice_self_reference_still_pins() {
    let elements = vec!["region·nyc,d2·x".to_string(), "region·nyc,d2·y".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[nyc, *]",
        "matrix",
        &[("matrix", 2)],
        &[],
        &["region", "d2"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "total",
        "region·nyc,d2·x",
        &elements,
        &ReducerKind::Linear,
        "SUM",
        true,
        Some(&fixture.ctx()),
    );
    // The bare fast path fires: legacy shortcut, not delta-ratio.
    assert!(
        eq.contains(
            "PREVIOUS(total) + (matrix[region·nyc,d2·x] - PREVIOUS(matrix[region·nyc,d2·x]))"
        ),
        "equation: {eq}"
    );
}

// -- GH #762: body-aware nonlinear (MIN/MAX/STDDEV) partial tests --

/// A bare-source body must keep the legacy nonlinear emission
/// byte-identically (MIN and STDDEV; same `None`-context comparison the
/// linear arm pins).
#[test]
fn test_body_aware_nonlinear_bare_matches_legacy() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new("pop[*]", "pop", &[("pop", 1)], &[], &["region"]);
    for name in ["MIN", "MAX", "STDDEV"] {
        let with_body = generate_element_to_scalar_equation(
            "pop",
            "agg",
            "region·nyc",
            &elements,
            &ReducerKind::Nonlinear,
            name,
            true,
            Some(&fixture.ctx()),
        );
        let legacy = generate_element_to_scalar_equation(
            "pop",
            "agg",
            "region·nyc",
            &elements,
            &ReducerKind::Nonlinear,
            name,
            true,
            None,
        );
        assert_eq!(with_body, legacy, "{name}: bare body must keep legacy form");
    }
}

/// A coefficient body (`pop[*] * scale` w.r.t. `pop`) must build each
/// MIN term from the row-pinned body: the scored row's term live (with
/// the feeder frozen), every other row's term fully frozen.
#[test]
fn test_body_aware_min_coefficient_terms() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "agg",
        "region·nyc",
        &elements,
        &ReducerKind::Nonlinear,
        "MIN",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains(
            "MIN((pop[region·nyc] * PREVIOUS(scale)), \
             (PREVIOUS(pop[region·boston]) * PREVIOUS(scale)))"
        ),
        "equation: {eq}"
    );
    // The raw bare-element form (the GH #762 garbage) must be gone.
    assert!(
        !eq.contains("MIN(pop[region·nyc], PREVIOUS(pop[region·boston]))"),
        "equation: {eq}"
    );
}

/// MAX shares the nested-binary builder; spot-check the term shape.
#[test]
fn test_body_aware_max_coefficient_terms() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "agg",
        "region·boston",
        &elements,
        &ReducerKind::Nonlinear,
        "MAX",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains(
            "MAX((PREVIOUS(pop[region·nyc]) * PREVIOUS(scale)), \
             (pop[region·boston] * PREVIOUS(scale)))"
        ),
        "equation: {eq}"
    );
}

/// STDDEV builds the unrolled population-variance form (divisor N,
/// inlined mean -- the GH #483 shape) over the row-pinned body terms.
#[test]
fn test_body_aware_stddev_coefficient_terms() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "agg",
        "region·nyc",
        &elements,
        &ReducerKind::Nonlinear,
        "STDDEV",
        true,
        Some(&fixture.ctx()),
    );
    let live = "(pop[region·nyc] * PREVIOUS(scale))";
    let frozen = "(PREVIOUS(pop[region·boston]) * PREVIOUS(scale))";
    assert!(eq.contains("sqrt("), "equation: {eq}");
    assert!(
        eq.contains(&format!("(({live} + {frozen}) / 2)")),
        "the inlined mean must use the body terms: {eq}"
    );
    assert!(eq.contains(" / 2)"), "divisor must stay N (GH #483): {eq}");
}

/// GH #767 (T5 flip, nonlinear sibling): the projection-feeder dep pins by
/// dim name in the nonlinear per-term expansion too -- each MIN term is the
/// row-pinned body with `PREVIOUS(frac[d1·…])` frozen, live at the scored
/// row only.
#[test]
fn test_body_aware_nonlinear_projection_feeder_dep_pins_by_dim_name() {
    let elements = vec!["d1·a,d2·x".to_string(), "d1·a,d2·y".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[d1, *] * frac[d1]",
        "matrix",
        &[("matrix", 2), ("frac", 1)],
        &[],
        &["d1", "d2"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "agg",
        "d1·a,d2·x",
        &elements,
        &ReducerKind::Nonlinear,
        "MIN",
        true,
        Some(&fixture.ctx()),
    );
    let live = "(matrix[d1·a, d2·x] * PREVIOUS(frac[d1·a]))";
    let frozen = "(PREVIOUS(matrix[d1·a, d2·y]) * PREVIOUS(frac[d1·a]))";
    assert!(
        eq.contains(&format!("MIN({live}, {frozen})")),
        "the nonlinear terms must pin the feeder by dim name: {eq}"
    );
}

/// A genuinely un-pinnable nonlinear body (a mismatched-axis dep indexed
/// outside the row's axes) must degrade to the delta-ratio fallback, the
/// same contract as the linear arm.
#[test]
fn test_body_aware_nonlinear_unpinnable_falls_back() {
    let elements = vec!["d1·a,d2·x".to_string(), "d1·a,d2·y".to_string()];
    let fixture = BodyCtxFixture::new(
        "matrix[d1, *] * q[d9]",
        "matrix",
        &[("matrix", 2), ("q", 1)],
        &[],
        &["d1", "d2"],
    );
    let eq = generate_element_to_scalar_equation(
        "matrix",
        "agg",
        "d1·a,d2·x",
        &elements,
        &ReducerKind::Nonlinear,
        "MIN",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("SAFEDIV((agg - PREVIOUS(agg))"),
        "equation: {eq}"
    );
    assert!(!eq.contains("q["), "equation: {eq}");
}

/// RANK keeps its documented delta-ratio stand-in regardless of the body
/// (companion to `test_generate_rank_keeps_delta_ratio`).
#[test]
fn test_body_aware_rank_keeps_delta_ratio() {
    let elements = vec!["region·nyc".to_string(), "region·boston".to_string()];
    let fixture = BodyCtxFixture::new(
        "pop[*] * scale",
        "pop",
        &[("pop", 1)],
        &["scale"],
        &["region"],
    );
    let eq = generate_element_to_scalar_equation(
        "pop",
        "agg",
        "region·nyc",
        &elements,
        &ReducerKind::Nonlinear,
        "RANK",
        true,
        Some(&fixture.ctx()),
    );
    assert!(
        eq.contains("SAFEDIV((agg - PREVIOUS(agg))"),
        "equation: {eq}"
    );
}

// -- shaped_guard_form_text: attribution-convention chooser (GH #743) --
//
// The chooser builds the standard changed-first guard form, but when the
// changed-first partial would embed `PREVIOUS` of an array slice (a
// wildcard/star-range-subscripted reference -- no LoadPrev-of-array-view
// codegen path exists, so the equation can only silently stub or hard-fail),
// it falls back to the changed-last attribution (only the live source
// frozen), and errors loudly when both conventions are unfreezable.

/// The GH #743 shape: live `frac` (Bare, iterated-dim feeder) inside a
/// reducer whose co-source is a wildcard slice. Changed-first would freeze
/// `matrix[d1,*]` as `PREVIOUS(matrix[..],*])` -- unfreezable -- so the
/// chooser must emit the changed-last form: only `frac` frozen, the
/// wildcard slice left verbatim (it compiles exactly like the target's own
/// equation), numerator `(target - frozen)`.
#[test]
fn shaped_guard_form_falls_back_to_changed_last_for_unfreezable_co_source() {
    let deps = deps_set(&["matrix", "frac"]);
    let live = Ident::<Canonical>::new("frac");
    let source_dims = vec![vec!["r1".to_string(), "r2".to_string()]];
    let source_dim_names = vec!["d1".to_string()];
    let target_iterated = vec!["d1".to_string()];
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated,
        dim_ctx: None,
        dep_dims: None,
    };
    let text = shaped_guard_form_text(
        "SUM(matrix[D1, *] * frac[D1])",
        &deps,
        &live,
        &RefShape::Bare,
        &source_dims,
        &source_dim_names,
        Some(&iter_ctx),
        None,
        "growth",
    )
    .unwrap();
    assert_eq!(
        text,
        "if (TIME = INITIAL_TIME) then 0 \
         else if ((growth - PREVIOUS(growth)) = 0) OR ((frac - PREVIOUS(frac)) = 0) then 0 \
         else SAFEDIV((growth - (sum(matrix[d1, *] * PREVIOUS(frac)))), \
         ABS((growth - PREVIOUS(growth))), 0) * SIGN((frac - PREVIOUS(frac)))"
    );
}

/// A shape where the changed-first partial is freezable stays byte-identical
/// to the historical output: the chooser is a pure pass-through to the
/// changed-first guard form whenever that form compiles.
#[test]
fn shaped_guard_form_keeps_changed_first_when_freezable() {
    let deps = deps_set(&["population"]);
    let live = Ident::<Canonical>::new("population");
    // `population / SUM(population[*])`: the live ref is OUTSIDE the
    // reducer occurrence that matters, and the whole reducer is frozen as
    // `PREVIOUS(sum(population[*]))` -- PREVIOUS of a scalar, freezable.
    let text = shaped_guard_form_text(
        "population / SUM(population[*])",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        &[],
        None,
        None,
        "share",
    )
    .unwrap();
    let expected_partial = build_partial_equation_shaped(
        "population / SUM(population[*])",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        expected_partial,
        "population / PREVIOUS(sum(population[*]))"
    );
    assert_eq!(
        text,
        format!(
            "if (TIME = INITIAL_TIME) then 0 \
             else if ((share - PREVIOUS(share)) = 0) OR ((population - PREVIOUS(population)) = 0) then 0 \
             else SAFEDIV((({expected_partial}) - PREVIOUS(share)), \
             ABS((share - PREVIOUS(share))), 0) * SIGN((population - PREVIOUS(population)))"
        )
    );
}

/// GH #742 x GH #743: a frozen RANK subtree whose argument carries an
/// uncollapsed array slice (`PREVIOUS(rank(matrix[d1, *], 1))`) is
/// unfreezable -- RANK is array-valued, so it does NOT collapse the slice
/// the way SUM does, and the slice-bearing capture would land in an
/// ill-typed scalar helper. The chooser must detect that (RANK is
/// transparent to `expr_is_array_slice_valued`) and fall back to the
/// changed-last attribution, where the RANK subtree stays verbatim
/// (compiling exactly like the target's own equation) and only the live
/// feeder is frozen.
#[test]
fn shaped_guard_form_rank_slice_arg_falls_back_to_changed_last() {
    let deps = deps_set(&["matrix", "frac"]);
    let live = Ident::<Canonical>::new("frac");
    let text = shaped_guard_form_text(
        "frac * RANK(matrix[d1, *], 1)",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        &[],
        None,
        None,
        "growth",
    )
    .unwrap();
    assert!(
        text.contains("(growth - (PREVIOUS(frac) * rank(matrix[d1, *], 1)))"),
        "the chooser must emit the changed-last numerator with the RANK subtree \
         verbatim; got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("previous(rank"),
        "the slice-bearing RANK subtree must not be frozen (unfreezable, GH #742); got: {text}"
    );
}

/// When BOTH conventions are unfreezable (the live occurrence is itself a
/// wildcard slice, so changed-last would freeze `PREVIOUS(arr[*])`), the
/// chooser fails loudly: the caller skips the score and warns, instead of
/// emitting an equation whose helper silently stubs to 0 and poisons the
/// score (the GH #743 -250-class garbage).
#[test]
fn shaped_guard_form_errs_when_both_conventions_unfreezable() {
    let deps = deps_set(&["arr", "brr"]);
    let live = Ident::<Canonical>::new("arr");
    let err = shaped_guard_form_text(
        "SUM(arr[*] * brr[d1, *])",
        &deps,
        &live,
        &RefShape::Wildcard,
        &[],
        &[],
        None,
        None,
        "total",
    )
    .unwrap_err();
    assert_eq!(err.kind, PartialEquationErrorKind::UnfreezablePartial);
}

/// Defensive: when the changed-first partial is unfreezable and the live
/// source has NO matching occurrence to freeze, changed-last would emit the
/// target's own equation verbatim (a silent constant-0 score). The chooser
/// must error loudly instead.
#[test]
fn shaped_guard_form_errs_when_no_live_occurrence_to_freeze() {
    let deps = deps_set(&["matrix"]);
    let live = Ident::<Canonical>::new("frac");
    let err = shaped_guard_form_text(
        // A naked (non-reducer-enclosed) wildcard slice; `frac` absent.
        // Not a compilable model equation, but exercises the guard.
        "matrix[d1, *] * 2",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        &[],
        None,
        None,
        "growth",
    )
    .unwrap_err();
    assert_eq!(err.kind, PartialEquationErrorKind::UnfreezablePartial);
}

/// GH #526: a TRANSPOSED non-live array dep (`arr[D2,D1]` for `arr`
/// declared `[D1,D2]` -- a genuine positional transposition in the
/// executed simulation) with its declared dims THREADED must not be
/// collapsed to a bare `PREVIOUS(arr)` (which freezes the WRONG element,
/// a silent magnitude error). The changed-first-only builder fails with
/// the loud `UnfreezablePartial`; `shaped_guard_form_text` callers fall
/// back to the changed-last convention instead (pinned end-to-end by
/// `ltm_array_agg::gh526_transposed_dep_partial_takes_changed_last`).
#[test]
fn gh526_transposed_other_dep_with_threaded_dims_is_unfreezable() {
    let equation = "pop[d1, d2] * 0.1 + arr[d2, d1] * 0.001";
    let deps = deps_set(&["pop", "arr"]);
    let live = Ident::<Canonical>::new("pop");
    let target_iterated_dims = vec!["d1".to_string(), "d2".to_string()];
    let source_dim_names = vec!["d1".to_string(), "d2".to_string()];
    let dep_dims: HashMap<String, Vec<Dimension>> = std::iter::once((
        "arr".to_string(),
        vec![
            make_named_dimension("d1", &["a", "b"]),
            make_named_dimension("d2", &["x", "y"]),
        ],
    ))
    .collect();
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx: None,
        dep_dims: Some(&dep_dims),
    };
    let result = build_partial_equation_shaped(
        equation,
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        Some(&iter_ctx),
        None,
    );
    assert!(
        matches!(
            result,
            Err(PartialEquationError {
                kind: PartialEquationErrorKind::UnfreezablePartial,
                ..
            })
        ),
        "a known-transposed other-dep must doom the changed-first partial loudly; got: {result:?}"
    );
}

// -- GH #779: bare-spelled feeder of an un-hoisted reducer declines loudly --

/// The detector keys on a BARE `Var` reference to the source nested inside
/// an array-reducer argument. It must fire for the bare reference and stay
/// silent for the adjacent shapes (subscripted, outside-reducer, inside
/// PREVIOUS, and other reducers) so the decline is precise.
#[test]
fn references_bare_source_inside_reducer_detects_only_the_dangerous_shape() {
    let frac = Ident::<Canonical>::new("frac");
    let parse = |eqn: &str| Expr0::new(eqn, LexerType::Equation).unwrap().unwrap();

    // The GH #779 shape: bare `frac` inside SUM -> fires.
    assert!(references_bare_source_inside_reducer(
        &parse("SUM(matrix[D1, *] * frac)"),
        &frac,
        false
    ));
    // The whole reducer class is covered.
    for reducer in ["MEAN", "MIN", "MAX", "STDDEV"] {
        assert!(
            references_bare_source_inside_reducer(
                &parse(&format!("{reducer}(matrix[D1, *] * frac)")),
                &frac,
                false
            ),
            "{reducer}: bare feeder inside reducer must be detected"
        );
    }

    // The SUBSCRIPTED feeder spelling is NOT the bare shape: it is hoisted
    // and scored correctly elsewhere (GH #767/T5).
    assert!(!references_bare_source_inside_reducer(
        &parse("SUM(matrix[D1, *] * frac[D1])"),
        &frac,
        false
    ));
    // A bare `frac` OUTSIDE any reducer is the bread-and-butter Bare A2A
    // case (its changed-first partial compiles), and must not fire.
    assert!(!references_bare_source_inside_reducer(
        &parse("frac * 2 + SUM(matrix[D1, *])"),
        &frac,
        false
    ));
    // A bare `frac` already inside PREVIOUS is lagged, not a live read the
    // partial must account for.
    assert!(!references_bare_source_inside_reducer(
        &parse("SUM(matrix[D1, *] * PREVIOUS(frac))"),
        &frac,
        false
    ));
    // RANK is array-valued and never hoisted (GH #771/#742): its bare arg is
    // a genuine Bare diagonal reference, not the feeder shape.
    assert!(!references_bare_source_inside_reducer(
        &parse("RANK(frac, 1)"),
        &frac,
        false
    ));
    // A different variable inside the reducer is irrelevant.
    assert!(!references_bare_source_inside_reducer(
        &parse("SUM(matrix[D1, *] * other)"),
        &frac,
        false
    ));
}

/// End-to-end through the chooser: a bare ARRAYED feeder inside an un-hoisted
/// reducer is DECLINED loudly (`BareReducerFeeder`, whose diagnostic names
/// the shape and the subscripted-spelling workaround) instead of given the
/// silently-wrong changed-last per-element partial -- the GH #779 fix. The
/// SUBSCRIPTED sibling (`frac[D1]`) keeps the changed-last score (pinned by
/// `shaped_guard_form_falls_back_to_changed_last_for_unfreezable_co_source`).
#[test]
fn shaped_guard_form_declines_bare_arrayed_feeder_of_unhoisted_reducer() {
    let deps = deps_set(&["matrix", "frac"]);
    let live = Ident::<Canonical>::new("frac");
    // `frac` arrayed over `d1` -> non-empty source dims.
    let source_dims = vec![vec!["r1".to_string(), "r2".to_string()]];
    let source_dim_names = vec!["d1".to_string()];
    let target_iterated = vec!["d1".to_string()];
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated,
        dim_ctx: None,
        dep_dims: None,
    };
    let err = shaped_guard_form_text(
        "SUM(matrix[D1, *] * frac)",
        &deps,
        &live,
        &RefShape::Bare,
        &source_dims,
        &source_dim_names,
        Some(&iter_ctx),
        None,
        "growth",
    )
    .unwrap_err();
    assert_eq!(
        err.kind,
        PartialEquationErrorKind::BareReducerFeeder,
        "the bare arrayed feeder of an un-hoisted reducer must decline loudly \
         with the shape-specific diagnostic, not score the silent wrong number"
    );
}

/// Precision control: a bare SCALAR source inside a reducer does NOT trigger
/// the GH #779 decline -- the gate requires an ARRAYED source
/// (`source_dim_names` non-empty), so it is inert here and the changed-last
/// convention governs as before: the changed-first leg is doomed (the
/// co-source's wildcard slice cannot be frozen), the gate is skipped (scalar
/// source), and the changed-last leg freezes the scalar `scale` occurrence
/// (a plain `LoadPrev`) and scores. (Hoisted instances of this feeder are
/// normally routed `ThroughAgg` to `generate_scalar_feeder_to_agg_equation`
/// before this chooser is reached; the whole-RHS spelling's own emission
/// defect is tracked separately as GH #790.)
#[test]
fn shaped_guard_form_scalar_feeder_inside_reducer_not_declined_by_gh779() {
    let deps = deps_set(&["matrix", "scale"]);
    let live = Ident::<Canonical>::new("scale");
    // `scale` SCALAR -> empty source dims, so the GH #779 gate is inert.
    let result = shaped_guard_form_text(
        "SUM(matrix[D1, *] * scale)",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        &[],
        None,
        None,
        "growth",
    );
    assert!(
        result.is_ok(),
        "a scalar feeder inside a reducer is not the GH #779 arrayed shape and \
         must keep its changed-last score; got: {result:?}"
    );
}

/// GH #526 control: the NATURAL-position dep (`arr[D1,D2]` matching its
/// declared order) keeps the historical collapse to `PREVIOUS(arr)` even
/// with dims threaded -- the bare freeze reads the same element, so the
/// collapse is exact. And with dims UN-threadable (the dep absent from the
/// map), the transposed spelling keeps the permissive legacy collapse, as
/// the design's GH #526 fallback clause requires.
#[test]
fn gh526_natural_and_unthreadable_other_deps_keep_collapse() {
    let deps = deps_set(&["pop", "arr"]);
    let live = Ident::<Canonical>::new("pop");
    let target_iterated_dims = vec!["d1".to_string(), "d2".to_string()];
    let source_dim_names = vec!["d1".to_string(), "d2".to_string()];
    let dep_dims: HashMap<String, Vec<Dimension>> = std::iter::once((
        "arr".to_string(),
        vec![
            make_named_dimension("d1", &["a", "b"]),
            make_named_dimension("d2", &["x", "y"]),
        ],
    ))
    .collect();
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx: None,
        dep_dims: Some(&dep_dims),
    };
    let partial = build_partial_equation_shaped(
        "pop[d1, d2] * 0.1 + arr[d1, d2] * 0.001",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        Some(&iter_ctx),
        None,
    )
    .unwrap();
    assert!(
        partial.contains("PREVIOUS(arr)") && !partial.contains("PREVIOUS(arr["),
        "the natural-position dep keeps the exact bare-PREVIOUS collapse; got: {partial}"
    );

    // Un-threadable dims (dep absent from the map): permissive legacy
    // collapse even for the transposed spelling.
    let empty_dep_dims: HashMap<String, Vec<Dimension>> = HashMap::new();
    let iter_ctx_unthreaded = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx: None,
        dep_dims: Some(&empty_dep_dims),
    };
    let partial = build_partial_equation_shaped(
        "pop[d1, d2] * 0.1 + arr[d2, d1] * 0.001",
        &deps,
        &live,
        &RefShape::Bare,
        &[],
        Some(&iter_ctx_unthreaded),
        None,
    )
    .unwrap();
    assert!(
        partial.contains("PREVIOUS(arr)") && !partial.contains("PREVIOUS(arr["),
        "un-threadable dep dims keep the permissive legacy collapse; got: {partial}"
    );
}
