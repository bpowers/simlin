// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static link-polarity analysis tests: the polarity helpers (`flip_polarity`,
//! the constant-sign predicates), `analyze_link_polarity` / the
//! `analyze_expr_polarity` expression forms (if-then-else, unary NOT, division,
//! array reducers, subscripts), the graphical-function monotonicity classifier,
//! and lookup-table polarity propagation through links.
//!
//! Split out of `ltm/tests.rs` to keep that file under the project line-count
//! lint; included via `#[path]` as a child of it, so `use super::*` resolves
//! that file's imports and private helpers.

use super::*;

#[test]
fn test_flip_polarity() {
    // Test flip_polarity function (covers lines 1049-1054)
    assert_eq!(
        flip_polarity(LinkPolarity::Positive),
        LinkPolarity::Negative
    );
    assert_eq!(
        flip_polarity(LinkPolarity::Negative),
        LinkPolarity::Positive
    );
    assert_eq!(flip_polarity(LinkPolarity::Unknown), LinkPolarity::Unknown);
}

#[test]
fn test_literal_sign() {
    use crate::ast::{Expr2, Loc, UnaryOp};

    let cnst = |text: &str, v: f64| {
        Expr2::Const(
            text.to_string(),
            crate::ast::Literal::new(v),
            Loc::default(),
        )
    };
    let neg = |e: Expr2| Expr2::Op1(UnaryOp::Negative, Box::new(e), None, Loc::default());

    assert_eq!(literal_sign(&cnst("5", 5.0)), Some(true), "5 is positive");
    assert_eq!(literal_sign(&cnst("0", 0.0)), None, "0 has no sign");
    assert_eq!(
        literal_sign(&Expr2::Var(Ident::new("x"), None, Loc::default())),
        None,
        "a variable is not a literal"
    );

    // The lexer takes no leading sign, so a model equation `-5` parses as
    // Op1(Negative, Const(5)) -- the shape a Const-only predicate is blind
    // to. literal_sign must see through the negation (and chains of it).
    assert_eq!(
        literal_sign(&neg(cnst("5", 5.0))),
        Some(false),
        "-5 (parsed shape) is negative"
    );
    assert_eq!(
        literal_sign(&neg(neg(cnst("3", 3.0)))),
        Some(true),
        "--3 is positive"
    );
    assert_eq!(literal_sign(&neg(cnst("0", 0.0))), None, "-0 has no sign");

    // A hand-built Const carrying a negative value directly (the shape
    // constant folding could one day produce) is negative too.
    assert_eq!(
        literal_sign(&cnst("-3", -3.0)),
        Some(false),
        "Const(-3) is negative"
    );
}

#[test]
fn test_provable_value_sign() {
    use crate::ast::{Ast, Expr2, Loc, UnaryOp};

    let cnst = |v: f64| Expr2::Const(format!("{v}"), crate::ast::Literal::new(v), Loc::default());
    let neg = |e: Expr2| Expr2::Op1(UnaryOp::Negative, Box::new(e), None, Loc::default());
    let var = |n: &str| Expr2::Var(Ident::new(n), None, Loc::default());
    use crate::variable::VarKind;
    let scalar_var = |ident: &str, eq: Expr2| Variable {
        ident: Ident::new(ident),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Aux {
            ast: Some(Ast::Scalar(eq)),
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
        },
    };

    // `k_neg = -5` hand-builds the Op1(Negative, Const(5)) equation shape,
    // which IS the production shape: the lexer takes no leading sign, so a
    // parsed `-5` is a negation of the literal 5. The end-to-end twin
    // (`test_mul_negative_constant_valued_cofactor_flips`, which goes
    // through the real parse) is what pins that correspondence.
    let mut variables: HashMap<Ident<Canonical>, Variable> = HashMap::new();
    variables.insert(Ident::new("k_neg"), scalar_var("k_neg", neg(cnst(5.0))));
    variables.insert(Ident::new("k_pos"), scalar_var("k_pos", cnst(5.0)));
    variables.insert(
        Ident::new("k_dyn"),
        scalar_var(
            "k_dyn",
            Expr2::Op2(
                BinaryOp::Mul,
                Box::new(var("q")),
                Box::new(cnst(2.0)),
                None,
                Loc::default(),
            ),
        ),
    );

    let vars = Some(&variables);
    assert_eq!(provable_value_sign(&cnst(2.0), vars), Some(true));
    assert_eq!(provable_value_sign(&var("k_pos"), vars), Some(true));
    assert_eq!(
        provable_value_sign(&var("k_neg"), vars),
        Some(false),
        "a variable whose equation is -5 is provably negative"
    );
    assert_eq!(
        provable_value_sign(&neg(var("k_neg")), vars),
        Some(true),
        "-k_neg is provably positive"
    );
    assert_eq!(
        provable_value_sign(&var("k_dyn"), vars),
        None,
        "a non-constant equation proves nothing"
    );
    assert_eq!(
        provable_value_sign(&var("missing"), vars),
        None,
        "an unknown ident proves nothing"
    );
    assert_eq!(
        provable_value_sign(&var("k_pos"), None),
        None,
        "no variables map -> a bare reference proves nothing"
    );
}

#[test]
fn test_analyze_link_polarity_arrayed() {
    // Test analyze_link_polarity with Arrayed AST (covers lines 935-947)
    use crate::ast::{Ast, Expr2, Loc};
    use crate::common::CanonicalElementName;
    use std::collections::HashMap;

    let x_var = Ident::new("x");

    // Create arrayed AST with consistent positive polarity
    let mut elements = HashMap::new();
    elements.insert(
        CanonicalElementName::from_raw("dim1"),
        Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const(
                "2".to_string(),
                crate::ast::Literal::new(2.0),
                Loc::default(),
            )),
            None,
            Loc::default(),
        ),
    );
    elements.insert(
        CanonicalElementName::from_raw("dim2"),
        Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const(
                "10".to_string(),
                crate::ast::Literal::new(10.0),
                Loc::default(),
            )),
            None,
            Loc::default(),
        ),
    );

    let ast = Ast::Arrayed(vec![], elements, None, false);
    let empty_vars = HashMap::new();
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(
        polarity,
        LinkPolarity::Positive,
        "Consistent positive elements should be positive"
    );

    // Test with mixed polarities
    let mut mixed_elements = HashMap::new();
    mixed_elements.insert(
        CanonicalElementName::from_raw("dim1"),
        Expr2::Var(x_var.clone(), None, Loc::default()),
    );
    mixed_elements.insert(
        CanonicalElementName::from_raw("dim2"),
        Expr2::Op1(
            crate::ast::UnaryOp::Negative,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        ),
    );

    let mixed_ast = Ast::Arrayed(vec![], mixed_elements, None, false);
    let mixed_polarity = analyze_link_polarity(&mixed_ast, &x_var, &empty_vars);
    assert_eq!(
        mixed_polarity,
        LinkPolarity::Unknown,
        "Mixed polarities should be Unknown"
    );
}

#[test]
fn test_analyze_expr_polarity_if_then_else() {
    // Test analyze_expr_polarity with If-Then-Else (covers lines 1033-1042)
    use crate::ast::{Expr2, Loc};

    let x_var = Ident::new("x");

    // If with same polarity in both branches
    let if_expr = Expr2::If(
        Box::new(Expr2::Const(
            "1".to_string(),
            crate::ast::Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
        Box::new(Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const(
                "2".to_string(),
                crate::ast::Literal::new(2.0),
                Loc::default(),
            )),
            None,
            Loc::default(),
        )),
        None,
        Loc::default(),
    );

    let polarity =
        analyze_expr_polarity_with_context(&if_expr, &x_var, LinkPolarity::Positive, None);
    assert_eq!(
        polarity,
        LinkPolarity::Positive,
        "Same polarity branches should return that polarity"
    );

    // If with different polarities in branches
    let mixed_if = Expr2::If(
        Box::new(Expr2::Const(
            "1".to_string(),
            crate::ast::Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
        Box::new(Expr2::Op1(
            crate::ast::UnaryOp::Negative,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        )),
        None,
        Loc::default(),
    );

    let mixed_polarity =
        analyze_expr_polarity_with_context(&mixed_if, &x_var, LinkPolarity::Positive, None);
    assert_eq!(
        mixed_polarity,
        LinkPolarity::Unknown,
        "Different polarity branches should be Unknown"
    );
}

#[test]
fn test_analyze_expr_polarity_unary_not() {
    // Test analyze_expr_polarity with unary NOT operator (covers lines 1026-1031)
    use crate::ast::{Expr2, Loc, UnaryOp};

    let x_var = Ident::new("x");

    let not_expr = Expr2::Op1(
        UnaryOp::Not,
        Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
        None,
        Loc::default(),
    );

    let polarity =
        analyze_expr_polarity_with_context(&not_expr, &x_var, LinkPolarity::Positive, None);
    assert_eq!(
        polarity,
        LinkPolarity::Negative,
        "NOT should flip polarity from positive to negative"
    );
}

#[test]
fn test_analyze_expr_polarity_division_edge_cases() {
    // Test division polarity analysis edge cases (covers lines 1013-1022)
    use crate::ast::{Expr2, Loc};

    let x_var = Ident::new("x");
    let y_var = Ident::new("y");

    // Division with variable in numerator
    let div_num = Expr2::Op2(
        BinaryOp::Div,
        Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
        Box::new(Expr2::Const(
            "10".to_string(),
            crate::ast::Literal::new(10.0),
            Loc::default(),
        )),
        None,
        Loc::default(),
    );

    let pol_num =
        analyze_expr_polarity_with_context(&div_num, &x_var, LinkPolarity::Positive, None);
    assert_eq!(
        pol_num,
        LinkPolarity::Positive,
        "Variable in numerator should keep polarity"
    );

    // Division with different variable in denominator (not the one we're tracking)
    let div_other = Expr2::Op2(
        BinaryOp::Div,
        Box::new(Expr2::Const(
            "100".to_string(),
            crate::ast::Literal::new(100.0),
            Loc::default(),
        )),
        Box::new(Expr2::Var(y_var.clone(), None, Loc::default())),
        None,
        Loc::default(),
    );

    let pol_other =
        analyze_expr_polarity_with_context(&div_other, &x_var, LinkPolarity::Positive, None);
    assert_eq!(
        pol_other,
        LinkPolarity::Unknown,
        "Unrelated variable should give Unknown"
    );
}

#[test]
fn test_analyze_expr_polarity_array_reducers() {
    // Array reducers SUM, MEAN, MAX (single-arg), MIN (single-arg) are monotone
    // in their argument: their polarity equals the inner expression's polarity.
    // STDDEV and RANK are not monotone: they must return Unknown even when the
    // argument has a known polarity.
    use crate::ast::{Expr2, Loc, UnaryOp};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let pos_inner = || Expr2::Var(x_var.clone(), None, Loc::default());
    let neg_inner = || {
        Expr2::Op1(
            UnaryOp::Negative,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        )
    };

    // SUM passes through positive polarity.
    let sum_pos = Expr2::App(BuiltinFn::Sum(Box::new(pos_inner())), None, Loc::default());
    assert_eq!(
        analyze_expr_polarity_with_context(&sum_pos, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Positive,
        "SUM(x) should pass through positive polarity",
    );

    // SUM passes through negative polarity (e.g. SUM(-x)).
    let sum_neg = Expr2::App(BuiltinFn::Sum(Box::new(neg_inner())), None, Loc::default());
    assert_eq!(
        analyze_expr_polarity_with_context(&sum_neg, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Negative,
        "SUM(-x) should pass through negative polarity",
    );

    // MEAN with a single (array) argument passes through positive polarity.
    let mean_pos = Expr2::App(BuiltinFn::Mean(vec![pos_inner()]), None, Loc::default());
    assert_eq!(
        analyze_expr_polarity_with_context(&mean_pos, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Positive,
        "MEAN(x) should pass through positive polarity",
    );

    // MEAN with a single (array) argument passes through negative polarity.
    let mean_neg = Expr2::App(BuiltinFn::Mean(vec![neg_inner()]), None, Loc::default());
    assert_eq!(
        analyze_expr_polarity_with_context(&mean_neg, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Negative,
        "MEAN(-x) should pass through negative polarity",
    );

    // Array MAX (no second argument) passes through inner polarity.
    let max_array_pos = Expr2::App(
        BuiltinFn::Max(Box::new(pos_inner()), None),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&max_array_pos, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Positive,
        "MAX(x) (array form) should pass through positive polarity",
    );
    let max_array_neg = Expr2::App(
        BuiltinFn::Max(Box::new(neg_inner()), None),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&max_array_neg, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Negative,
        "MAX(-x) (array form) should pass through negative polarity",
    );

    // Array MIN (no second argument) passes through inner polarity.
    let min_array_pos = Expr2::App(
        BuiltinFn::Min(Box::new(pos_inner()), None),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&min_array_pos, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Positive,
        "MIN(x) (array form) should pass through positive polarity",
    );
    let min_array_neg = Expr2::App(
        BuiltinFn::Min(Box::new(neg_inner()), None),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&min_array_neg, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Negative,
        "MIN(-x) (array form) should pass through negative polarity",
    );

    // STDDEV is non-monotone: even with a positive-polarity argument, the result
    // is Unknown because variance has no fixed sign w.r.t. its inputs.
    let stddev = Expr2::App(
        BuiltinFn::Stddev(Box::new(pos_inner())),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&stddev, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Unknown,
        "STDDEV must always return Unknown polarity",
    );

    // RANK depends on the rest of the array, so polarity is undefined.
    let direction = Expr2::Const(
        "1".to_string(),
        crate::ast::Literal::new(1.0),
        Loc::default(),
    );
    let rank = Expr2::App(
        BuiltinFn::Rank(Box::new(pos_inner()), Box::new(direction)),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&rank, &x_var, LinkPolarity::Positive, None),
        LinkPolarity::Unknown,
        "RANK must always return Unknown polarity",
    );
}

/// Verify reducer polarity propagates through the actual parsed shape:
/// `SUM(x[*])` lowers to `Sum(Box::new(Subscript(x, [Wildcard], _, _)))`,
/// not `Sum(Box::new(Var(x, ...)))`. Without an `Expr2::Subscript` arm in
/// `analyze_expr_polarity_with_context`, the new reducer arms fall through
/// to Unknown for the production case the issue actually targets.
#[test]
fn test_analyze_expr_polarity_array_reducers_subscript_wildcard() {
    use crate::ast::{Expr2, IndexExpr2, Loc, UnaryOp};
    use crate::builtins::BuiltinFn;
    use LinkPolarity::{Negative, Positive, Unknown};

    let x = Ident::new("x");
    let y = Ident::new("y");
    let sub = |id: &Ident<Canonical>| {
        Expr2::Subscript(
            id.clone(),
            vec![IndexExpr2::Wildcard(Loc::default())],
            None,
            Loc::default(),
        )
    };
    let app = |b: BuiltinFn<Expr2>| Expr2::App(b, None, Loc::default());
    let neg = |e: Expr2| Expr2::Op1(UnaryOp::Negative, Box::new(e), None, Loc::default());
    let one = || {
        Expr2::Const(
            "1".to_string(),
            crate::ast::Literal::new(1.0),
            Loc::default(),
        )
    };

    // (label, expression, context_polarity, expected_polarity)
    let cases: Vec<(&str, Expr2, LinkPolarity, LinkPolarity)> = vec![
        (
            "SUM(x[*]) +",
            app(BuiltinFn::Sum(Box::new(sub(&x)))),
            Positive,
            Positive,
        ),
        (
            "SUM(x[*]) -",
            app(BuiltinFn::Sum(Box::new(sub(&x)))),
            Negative,
            Negative,
        ),
        (
            "SUM(-x[*])",
            app(BuiltinFn::Sum(Box::new(neg(sub(&x))))),
            Positive,
            Negative,
        ),
        (
            "SUM(y[*])",
            app(BuiltinFn::Sum(Box::new(sub(&y)))),
            Positive,
            Unknown,
        ),
        (
            "MEAN(x[*])",
            app(BuiltinFn::Mean(vec![sub(&x)])),
            Positive,
            Positive,
        ),
        (
            "MAX(x[*])",
            app(BuiltinFn::Max(Box::new(sub(&x)), None)),
            Positive,
            Positive,
        ),
        (
            "MIN(x[*])",
            app(BuiltinFn::Min(Box::new(sub(&x)), None)),
            Positive,
            Positive,
        ),
        (
            "STDDEV(x[*])",
            app(BuiltinFn::Stddev(Box::new(sub(&x)))),
            Positive,
            Unknown,
        ),
        (
            "RANK(x[*], 1)",
            app(BuiltinFn::Rank(Box::new(sub(&x)), Box::new(one()))),
            Positive,
            Unknown,
        ),
    ];

    for (label, expr, ctx, want) in &cases {
        assert_eq!(
            analyze_expr_polarity_with_context(expr, &x, *ctx, None),
            *want,
            "{label}",
        );
    }
}

/// The Subscript arm must distinguish between indices that are independent
/// of `from_var` (literal, wildcard, expressions over other variables) and
/// indices that themselves reference `from_var`. In the latter case the
/// relationship between `from_var` and the subscripted result is non-monotone:
/// changing `from_var` shifts both the lookup target AND the index, so no
/// single polarity describes the result. The dominant cases (`SUM(arr[*])`,
/// `arr[Region]`, indices over OTHER variables) keep their original behavior
/// of returning `current_polarity` because their indices don't reference
/// `from_var`.
#[test]
fn test_analyze_expr_polarity_subscript_self_indexing() {
    use crate::ast::{Expr2, IndexExpr2, Loc};
    use crate::builtins::BuiltinFn;
    use LinkPolarity::{Positive, Unknown};

    let arr = Ident::new("arr");
    let other = Ident::new("other");
    let i = Ident::new("i");

    let var = |id: &Ident<Canonical>| Expr2::Var(id.clone(), None, Loc::default());
    let lit = |n: f64| Expr2::Const(format!("{n}"), crate::ast::Literal::new(n), Loc::default());

    // arr[*] -- wildcard index, no reference to arr in the index.
    let arr_wildcard = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Wildcard(Loc::default())],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&arr_wildcard, &arr, Positive, None),
        Positive,
        "arr[*] preserves current_polarity",
    );

    // arr[3] -- literal index, no reference to arr in the index.
    let arr_literal = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Expr(lit(3.0))],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&arr_literal, &arr, Positive, None),
        Positive,
        "arr[3] preserves current_polarity",
    );

    // arr[i] where i is a different variable -- index references some OTHER
    // variable, but not from_var (= arr). Polarity contract still holds.
    let arr_other_index = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Expr(var(&i))],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&arr_other_index, &arr, Positive, None),
        Positive,
        "arr[i] (i != from_var) preserves current_polarity",
    );

    // arr[arr] -- index trivially references arr. Result is non-monotone
    // because shifting arr shifts both the lookup target and the index.
    let arr_self_var = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Expr(var(&arr))],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&arr_self_var, &arr, Positive, None),
        Unknown,
        "arr[arr] is non-monotone",
    );

    // arr[INT(arr[i])] -- the canonical self-indexing case. Index references
    // arr through a nested subscript; relationship is non-monotone.
    let inner = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Expr(var(&i))],
        None,
        Loc::default(),
    );
    let int_inner = Expr2::App(BuiltinFn::Int(Box::new(inner)), None, Loc::default());
    let arr_self_nested = Expr2::Subscript(
        arr.clone(),
        vec![IndexExpr2::Expr(int_inner)],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&arr_self_nested, &arr, Positive, None),
        Unknown,
        "arr[INT(arr[i])] is non-monotone",
    );

    // other[*] where from_var is arr -- subscripted array is not from_var.
    // Existing behavior: contributes Unknown because the arm conservatively
    // can't classify references through other arrays.
    let other_wildcard = Expr2::Subscript(
        other.clone(),
        vec![IndexExpr2::Wildcard(Loc::default())],
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_expr_polarity_with_context(&other_wildcard, &arr, Positive, None),
        Unknown,
        "other[*] (other != from_var) returns Unknown",
    );
}

#[test]
fn test_graphical_function_polarity() {
    use crate::variable::Table;

    // Test 1: Monotonically increasing function (positive polarity)
    let increasing_table =
        Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 2.0, 4.0, 6.0, 8.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&increasing_table),
        LinkPolarity::Positive,
        "Monotonically increasing function should have positive polarity"
    );

    // Test 2: Monotonically decreasing function (negative polarity)
    let decreasing_table = Table::new_for_test(
        vec![0.0, 1.0, 2.0, 3.0, 4.0],
        vec![10.0, 8.0, 6.0, 4.0, 2.0],
    );
    assert_eq!(
        analyze_graphical_function_polarity(&decreasing_table),
        LinkPolarity::Negative,
        "Monotonically decreasing function should have negative polarity"
    );

    // Test 3: Non-monotonic function (unknown polarity)
    let non_monotonic_table =
        Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 5.0, 3.0, 7.0, 2.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&non_monotonic_table),
        LinkPolarity::Unknown,
        "Non-monotonic function should have unknown polarity"
    );

    // Test 4: Constant function (unknown polarity - no change)
    let constant_table = Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0, 5.0, 5.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&constant_table),
        LinkPolarity::Unknown,
        "Constant function should have unknown polarity"
    );

    // Test 5: Single point (edge case)
    let single_point_table = Table::new_for_test(vec![1.0], vec![2.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&single_point_table),
        LinkPolarity::Unknown,
        "Single point should have unknown polarity"
    );

    // Test 6: Nearly constant with small variations (testing tolerance)
    let nearly_constant_table =
        Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0001, 5.0002, 5.0003]);
    assert_eq!(
        analyze_graphical_function_polarity(&nearly_constant_table),
        LinkPolarity::Positive,
        "Nearly constant but increasing should have positive polarity"
    );
}

#[test]
fn test_graphical_function_polarity_tolerates_import_noise() {
    use crate::variable::Table;

    // A table that is monotone non-decreasing modulo round-trip numeric-import
    // noise: the second segment dips by ~2e-7 against a 1.5-wide y-range. The
    // y-range-relative epsilon (1e-6 * 1.5 = 1.5e-6) absorbs the dip, so the
    // table reads as Positive. With the old absolute 1e-10 epsilon this dip
    // broke monotonicity and the table read as Unknown (#492).
    let import_noise_table = Table::new_for_test(
        vec![0.0, 1.0, 2.0, 3.0, 4.0],
        vec![0.0, 0.5000001, 0.4999999, 1.0, 1.5],
    );
    assert_eq!(
        analyze_graphical_function_polarity(&import_noise_table),
        LinkPolarity::Positive,
        "A monotone-modulo-import-noise table should read as Positive"
    );

    // A genuine ~0.7 reversal against a 1.0 y-range: far larger than the
    // relative epsilon (1e-6), so the table is correctly Unknown.
    let real_reversal_table =
        Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![0.0, 1.0, 0.3, 0.7]);
    assert_eq!(
        analyze_graphical_function_polarity(&real_reversal_table),
        LinkPolarity::Unknown,
        "A genuinely non-monotone table is still Unknown"
    );

    // A perfectly constant table: y_max - y_min == 0 so epsilon clamps to
    // 1e-12; every dy == 0 is a plateau (not > 1e-12, not < -1e-12), so the
    // table is still classified constant and reads as Unknown.
    let constant_table = Table::new_for_test(vec![0.0, 1.0, 2.0], vec![5.0, 5.0, 5.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&constant_table),
        LinkPolarity::Unknown,
        "A constant table is still Unknown"
    );
}

#[test]
fn test_graphical_function_polarity_uses_slope_not_y_delta() {
    use crate::variable::Table;

    // Non-uniform x-spacing where the y-delta heuristic and the slope heuristic
    // DISAGREE on the verdict (#536). The middle segment is a genuine, steep,
    // local DECREASE over a very narrow x-interval (dx = 0.001), so the table is
    // NOT monotone -- the correct answer is Unknown.
    //
    //   x = [0, 100, 100.001, 200]
    //   y = [0,  10,  9.99999,  20]
    //
    //   y-range = 20, so the y-range-relative dy epsilon is 1e-6 * 20 = 2e-5.
    //   The middle dy is -1e-5, which is *below* that epsilon, so the y-delta
    //   heuristic SUPPRESSES the dip as a plateau and -- seeing only the two
    //   surrounding increases -- wrongly classifies the table as Positive.
    //
    //   The middle segment's SLOPE, however, is -1e-5 / 0.001 = -0.01, far
    //   steeper (in magnitude) than the table's average slope (20/200 = 0.1)
    //   times the relative tolerance, so the slope heuristic correctly sees a
    //   real local decrease and returns Unknown.
    let narrow_dip_table = Table::new_for_test(
        vec![0.0, 100.0, 100.001, 200.0],
        vec![0.0, 10.0, 9.99999, 20.0],
    );
    assert_eq!(
        analyze_graphical_function_polarity(&narrow_dip_table),
        LinkPolarity::Unknown,
        "a steep narrow local decrease must be caught by the slope heuristic, not \
         smoothed over by the y-delta heuristic"
    );

    // The mirror case: a steep narrow local INCREASE in an otherwise-decreasing
    // table must likewise flip a naive Negative to Unknown.
    let narrow_bump_table = Table::new_for_test(
        vec![0.0, 100.0, 100.001, 200.0],
        vec![20.0, 10.0, 10.00001, 0.0],
    );
    assert_eq!(
        analyze_graphical_function_polarity(&narrow_bump_table),
        LinkPolarity::Unknown,
        "a steep narrow local increase must flip an otherwise-decreasing table to Unknown"
    );

    // A genuinely monotone table with non-uniform x-spacing (no sign change in
    // slope) must still classify correctly: every segment slopes up, so the
    // verdict is Positive even though the dx values vary by orders of magnitude.
    let monotone_nonuniform_table =
        Table::new_for_test(vec![0.0, 0.001, 100.0, 200.0], vec![0.0, 5.0, 10.0, 20.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&monotone_nonuniform_table),
        LinkPolarity::Positive,
        "a monotone table with non-uniform x-spacing stays Positive"
    );

    // Degenerate vertical segment (x[i] == x[i-1]) with differing y: two outputs
    // for one input is an ambiguous lookup with undefined slope, so bail to
    // Unknown.
    let vertical_segment_table =
        Table::new_for_test(vec![0.0, 1.0, 1.0, 2.0], vec![0.0, 1.0, 5.0, 6.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&vertical_segment_table),
        LinkPolarity::Unknown,
        "a vertical (dx == 0, dy != 0) segment has undefined slope and must read as Unknown"
    );

    // A duplicated point (x[i] == x[i-1] AND y[i] == y[i-1]) is redundant, not
    // ambiguous: it must be skipped as non-determining and not poison an
    // otherwise-monotone verdict.
    let duplicate_point_table =
        Table::new_for_test(vec![0.0, 1.0, 1.0, 2.0], vec![0.0, 1.0, 1.0, 2.0]);
    assert_eq!(
        analyze_graphical_function_polarity(&duplicate_point_table),
        LinkPolarity::Positive,
        "a redundant duplicate point must not flip a monotone-increasing table"
    );
}

/// GH #536 tolerance regression: a many-point uniformly-spaced monotone table
/// with a single small import-noise dip must still classify Positive even after
/// the slope-based tolerance was introduced (#536).
///
/// Root cause: the original #536 fix set `slope_epsilon = 1e-6 * (y_max -
/// y_min) / x_span`, which is `1e-6 * avg_slope_mag`. For a uniformly-spaced
/// n-point table `dx = x_span / (n - 1)`, so the effective per-segment dy
/// threshold becomes `slope_epsilon * dx = 1e-6 * (y_max - y_min) / (n - 1)` --
/// which is `(n - 1)x` tighter than the old #492 threshold `1e-6 * (y_max -
/// y_min)`. A 50-point table therefore has a threshold ~49x tighter, and a
/// noise dip that sat safely inside the #492 window now crosses it and flips the
/// classification to Unknown.
///
/// The correct fix scales the tolerance by `avg_dx = x_span / (n - 1)`, giving
/// `slope_epsilon = 1e-6 * (y_max - y_min) / avg_dx`. Then for uniform spacing
/// `dx == avg_dx` and the per-segment dy threshold is `slope_epsilon * dx =
/// 1e-6 * (y_max - y_min)` -- exactly the old #492 behavior. For non-uniform
/// spacing the threshold still scales by `dx / avg_dx`, so narrow steep segments
/// still trip correctly (the #536 motivation is preserved).
#[test]
fn test_graphical_function_polarity_uniform_50pt_import_noise() {
    use crate::variable::Table;

    // Build a 50-point uniformly-spaced monotone-increasing table on [0, 49]
    // with y rising from 0.0 to 1.0. One interior point (index 25) has a
    // downward import-noise dip of -5e-8.
    //
    // With 50 points: y_range = 1.0, x_span = 49.0, avg_dx = 1.0.
    // Old #492 threshold: 1e-6 * 1.0 = 1e-6. The dip (-5e-8) is far below that
    // threshold in magnitude, so the old code classified Positive.
    //
    // Buggy #536 slope_epsilon = 1e-6 * 1.0 / 49.0 ≈ 2.04e-8.
    // The dip's slope = -5e-8 / 1.0 = -5e-8, which is < -2.04e-8, so the
    // buggy code classifies Unknown (the #492 regression).
    //
    // Fixed #536 slope_epsilon = 1e-6 * 1.0 / avg_dx = 1e-6 * 1.0 / 1.0 = 1e-6.
    // The dip's slope (-5e-8) is within [-1e-6, 1e-6], so the fixed code
    // correctly classifies Positive.
    let n = 50usize;
    let x_span = (n - 1) as f64; // 49.0
    let noise_dip_idx = 25usize;
    // The noise magnitude must be:
    //   - within the old #492 threshold: 1e-6 * y_range = 1e-6 * 1.0 = 1e-6
    //   - outside the buggy #536 threshold: 1e-6 * y_range / (n-1) = 1e-6/49 ≈ 2.04e-8
    // Choosing 5e-8: between 2.04e-8 and 1e-6, so buggy code fails and correct
    // code passes.
    let noise_magnitude = 5e-8_f64;

    // Build a strictly-increasing base curve and then pull y[noise_dip_idx]
    // BELOW y[noise_dip_idx - 1] by noise_magnitude.  This makes the segment
    // (noise_dip_idx - 1) -> noise_dip_idx have a genuinely negative slope:
    //   dy = -noise_magnitude, dx = 1.0, slope = -5e-8.
    // The following segment noise_dip_idx -> (noise_dip_idx + 1) is then
    // extra-positive (+2/49 + noise_magnitude), so the table overall is nearly
    // monotone.
    let xs: Vec<f64> = (0..n).map(|i| i as f64).collect();
    let base_y: Vec<f64> = (0..n).map(|i| i as f64 / x_span).collect();
    let ys: Vec<f64> = (0..n)
        .map(|i| {
            if i == noise_dip_idx {
                // pull this point below its predecessor: slope (i-1)->i is -5e-8
                base_y[noise_dip_idx - 1] - noise_magnitude
            } else {
                base_y[i]
            }
        })
        .collect();

    let table = Table::new_for_test(xs, ys);
    assert_eq!(
        analyze_graphical_function_polarity(&table),
        LinkPolarity::Positive,
        "a 50-point uniformly-spaced monotone table with a single {noise_magnitude:.0e} \
         import-noise dip (within the #492 tolerance 1e-6 * y_range = 1e-6) must \
         classify Positive; slope_epsilon must scale by avg_dx so uniform-spacing \
         behavior matches the pre-#536 y-delta threshold (GH #536 regression)"
    );
}

#[test]
fn test_lookup_table_polarity_in_links() {
    use crate::datamodel;

    // Create a model with a lookup table. The flow scales the lookup result
    // by a positive CONSTANT (not by `water` itself): with `water` appearing
    // in both factors of a product, the partial `L(w) + w*L'(w)` is
    // sign-indefinite without value reasoning, so the sound answer would be
    // Unknown -- this test isolates the table-monotonicity propagation.
    let mut model_vars = vec![
        x_stock("water", "100", &[], &["outflow"], None),
        x_flow("outflow", "0.5 * lookup(lookup, water)", None),
    ];

    // Create the lookup table auxiliary
    let mut lookup_var = x_aux("lookup", "0", None);
    if let datamodel::Variable::Aux(aux) = &mut lookup_var {
        aux.gf = Some(datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 50.0, 100.0, 150.0]),
            y_points: vec![0.1, 0.2, 0.3, 0.4],
            x_scale: datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 150.0,
            },
            y_scale: datamodel::GraphicalFunctionScale { min: 0.1, max: 0.4 },
        });
    }
    model_vars.push(lookup_var);

    let model = x_model("main", model_vars);
    let sim_specs = sim_specs_with_units("months");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    // Check per-link polarity via compute_link_polarities
    let polarities = compute_link_polarities(&db, model, result.project);
    let water_to_outflow_key = ("water".to_string(), "outflow".to_string());
    assert_eq!(
        polarities[&water_to_outflow_key],
        LinkPolarity::Positive,
        "Monotonically increasing lookup table should preserve positive polarity"
    );

    // Verify loop polarity via model_detected_loops
    let detected = model_detected_loops(&db, model, result.project);
    assert_eq!(detected.loops.len(), 1, "Should have one loop");
    // water -> outflow: Positive (increasing lookup), outflow -> water: Negative (outflow)
    assert_eq!(
        detected.loops[0].polarity,
        DetectedLoopPolarity::Balancing,
        "Loop with one negative link should be balancing"
    );
}

/// Build a project from `(ident, kind)` variables, sync it, and return the
/// link polarities plus detected loops -- the shared production path
/// (parse -> lower -> `compute_link_polarities` / `model_detected_loops`)
/// for the Mul/Div co-factor convention tests below. Using the real parse
/// matters: a negative literal like `-5` parses as `Op1(Negative, Const(5))`
/// (the lexer takes no leading sign), a shape hand-built `Const(-5.0)`
/// fixtures never exercise.
fn link_polarities_for(
    variables: Vec<crate::datamodel::Variable>,
) -> (HashMap<(String, String), LinkPolarity>, Vec<DetectedLoop>) {
    let model = x_model("main", variables);
    let sim_specs = sim_specs_with_units("months");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let polarities = compute_link_polarities(&db, model, result.project);
    let detected = model_detected_loops(&db, model, result.project)
        .loops
        .clone();
    (polarities, detected)
}

fn link(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

/// The pysimlin README quickstart model: logistic growth with the growth
/// fraction split into its own aux. The compounding link
/// `population -> net_growth` has a bare named co-factor
/// (`fractional_growth`), which the SD positive-value labeling convention
/// signs Positive -- the same convention the Div one-side arm has always
/// applied to `share = pop / total`. The compounding loop is therefore
/// named `r1`, not `u1`.
#[test]
fn test_mul_bare_var_cofactor_is_positive_by_convention() {
    let (polarities, loops) = link_polarities_for(vec![
        x_stock("population", "50", &["net_growth"], &[], None),
        x_flow("net_growth", "population * fractional_growth", None),
        x_aux(
            "fractional_growth",
            "max_growth_rate * (1 - population / carrying_capacity)",
            None,
        ),
        x_aux("max_growth_rate", "0.08", None),
        x_aux("carrying_capacity", "10000", None),
    ]);

    assert_eq!(
        polarities[&link("population", "net_growth")],
        LinkPolarity::Positive,
        "bare named co-factor (fractional_growth) is positive by convention",
    );
    assert_eq!(
        polarities[&link("fractional_growth", "net_growth")],
        LinkPolarity::Positive,
        "bare named co-factor (population) is positive by convention",
    );
    assert_eq!(
        polarities[&link("population", "fractional_growth")],
        LinkPolarity::Negative,
        "1 - population/K is provably decreasing in population",
    );
    // The co-factor of max_growth_rate is `(1 - population / carrying_capacity)`,
    // a compound expression whose value sign is derived, not conventional: it
    // genuinely flips when population crosses the carrying capacity.
    assert_eq!(
        polarities[&link("max_growth_rate", "fractional_growth")],
        LinkPolarity::Unknown,
        "compound co-factor stays Unknown",
    );

    assert_eq!(loops.len(), 2, "logistic growth has two loops");
    let compounding = loops
        .iter()
        .find(|l| l.variables.len() == 2)
        .expect("population <-> net_growth loop");
    assert_eq!(compounding.id, "r1");
    assert_eq!(compounding.polarity, DetectedLoopPolarity::Reinforcing);
    let crowding = loops
        .iter()
        .find(|l| l.variables.len() == 3)
        .expect("crowding loop through fractional_growth");
    assert_eq!(crowding.id, "b1");
    assert_eq!(crowding.polarity, DetectedLoopPolarity::Balancing);
}

/// The dangerous class the convention must NOT touch: a compound co-factor
/// (`a - b`, `1 - pop/K`, ...) whose value sign is derived rather than
/// conventional. Both the one-side arm (independent compound co-factor) and
/// the both-sides arm (single-equation logistic, whose link partial really
/// does flip sign at K/2) must stay Unknown.
#[test]
fn test_mul_compound_cofactor_stays_unknown() {
    // One-side: co-factor `(a - b)` is independent of x but compound.
    let (polarities, loops) = link_polarities_for(vec![
        x_stock("x", "1", &["growth"], &[], None),
        x_flow("growth", "x * (a - b)", None),
        x_aux("a", "3", None),
        x_aux("b", "1", None),
    ]);
    assert_eq!(
        polarities[&link("x", "growth")],
        LinkPolarity::Unknown,
        "compound independent co-factor must stay Unknown",
    );
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].id, "u1");
    assert_eq!(loops[0].polarity, DetectedLoopPolarity::Undetermined);

    // Both-sides: single-equation logistic. Its true partial
    // r*(1 - 2*pop/K) flips sign at K/2, so no value convention rescues it.
    let (polarities, _) = link_polarities_for(vec![
        x_stock("population", "50", &["net_growth"], &[], None),
        x_flow(
            "net_growth",
            "population * max_growth_rate * (1 - population / carrying_capacity)",
            None,
        ),
        x_aux("max_growth_rate", "0.08", None),
        x_aux("carrying_capacity", "10000", None),
    ]);
    assert_eq!(
        polarities[&link("population", "net_growth")],
        LinkPolarity::Unknown,
        "single-equation logistic link partial is sign-indefinite",
    );
}

/// A co-factor with a PROVABLE sign must beat the positive-value convention:
/// `y = x * k` with `k = -5` is decreasing in x. The negative literal
/// parses as `Op1(Negative, Const(5))`, so the constant-sign predicates
/// must see through unary negation -- a naive convention extension would
/// confidently mislabel this link Positive.
#[test]
fn test_mul_negative_constant_valued_cofactor_flips() {
    let (polarities, _) = link_polarities_for(vec![
        x_aux("x", "1", None),
        x_aux("k", "-5", None),
        x_aux("y", "x * k", None),
    ]);
    assert_eq!(
        polarities[&link("x", "y")],
        LinkPolarity::Negative,
        "provably negative co-factor flips polarity",
    );
    assert_eq!(
        polarities[&link("k", "y")],
        LinkPolarity::Positive,
        "co-factor x is positive by convention",
    );
}

/// An inline negated bare co-factor is negative by the same convention:
/// `y = x * (-z)` labels `x -> y` Negative (z positive-by-convention,
/// negation flips).
#[test]
fn test_mul_negated_bare_cofactor_flips() {
    let (polarities, _) = link_polarities_for(vec![
        x_aux("x", "1", None),
        x_aux("q", "1", None),
        x_aux("z", "q * 2", None),
        x_aux("y", "x * (-z)", None),
    ]);
    assert_eq!(
        polarities[&link("x", "y")],
        LinkPolarity::Negative,
        "negated bare co-factor flips polarity by convention",
    );
}

/// A co-factor that references from_var non-monotonically must stay Unknown:
/// the convention only applies to co-factors INDEPENDENT of the link source.
#[test]
fn test_mul_cofactor_referencing_from_var_stays_unknown() {
    let (polarities, _) =
        link_polarities_for(vec![x_aux("x", "1", None), x_aux("y", "x * ABS(x)", None)]);
    assert_eq!(
        polarities[&link("x", "y")],
        LinkPolarity::Unknown,
        "co-factor depending non-monotonically on from_var poisons the product",
    );
}

/// A subscripted reference to a named quantity is positive by convention,
/// exactly like a bare Var (mirroring `operand_positive_by_convention`).
#[test]
fn test_mul_subscript_cofactor_is_positive_by_convention() {
    use crate::datamodel::{self, Dimension, DimensionElements};

    let arr = datamodel::Variable::Aux(datamodel::Aux {
        ident: "arr".to_string(),
        equation: datamodel::Equation::ApplyToAll(vec!["dim_d".to_string()], "2".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let model = x_model(
        "main",
        vec![x_aux("x", "1", None), arr, x_aux("y", "x * arr[d1]", None)],
    );
    let sim_specs = sim_specs_with_units("months");
    let mut datamodel_project = x_project(sim_specs, &[model]);
    datamodel_project.dimensions = vec![Dimension {
        name: "dim_d".to_string(),
        elements: DimensionElements::Named(vec!["d1".to_string(), "d2".to_string()]),
        mappings: vec![],
        parent: None,
    }];
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let polarities = compute_link_polarities(&db, model, result.project);
    assert_eq!(
        polarities[&link("x", "y")],
        LinkPolarity::Positive,
        "subscripted named co-factor is positive by convention",
    );
}

/// The Div arms' provable-sign escape hatch must also see through the
/// production parse of a negative literal: `k = -5` makes `x / k`
/// decreasing in x and `k / x` INCREASING in x (d(k/x)/dx = -k/x^2 > 0).
/// Before the sign predicates handled unary negation both were labeled by
/// the positive-value convention -- the exact mislabel the Div arm's
/// comment describes for `-5/y`.
#[test]
fn test_div_negative_constant_valued_operand_is_provable() {
    let (polarities, _) = link_polarities_for(vec![
        x_aux("x", "1", None),
        x_aux("k", "-5", None),
        x_aux("y", "x / k", None),
        x_aux("y2", "k / x", None),
    ]);
    assert_eq!(
        polarities[&link("x", "y")],
        LinkPolarity::Negative,
        "provably negative denominator flips the numerator pass-through",
    );
    assert_eq!(
        polarities[&link("x", "y2")],
        LinkPolarity::Positive,
        "provably negative numerator inverts the conventional denominator flip",
    );
}
