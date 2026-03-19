// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::ast::{Expr0, IndexExpr0, Loc};
use crate::common::RawIdent;
use crate::datamodel::{
    Aux, Compat, Equation, Flow, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale,
    Rect, SimMethod, Stock, StockFlow, Unit, Variable, view_element,
};
use crate::lexer::LexerType;
use crate::mdl::LOOKUP_SENTINEL;

/// Parse XMILE equation text to Expr0, then convert to MDL and assert.
fn assert_mdl(xmile_eqn: &str, expected_mdl: &str) {
    let ast = Expr0::new(xmile_eqn, LexerType::Equation)
        .expect("parse should succeed")
        .expect("expression should not be empty");
    let mdl = expr0_to_mdl(&ast);
    assert_eq!(
        expected_mdl, &mdl,
        "MDL mismatch for XMILE input: {xmile_eqn:?}"
    );
}

#[test]
fn constants() {
    assert_mdl("5", "5");
    assert_mdl("3.14", "3.14");
    assert_mdl("1e3", "1e3");
}

#[test]
fn nan_constant() {
    let ast = Expr0::new("NAN", LexerType::Equation).unwrap().unwrap();
    let mdl = expr0_to_mdl(&ast);
    assert_eq!("NaN", &mdl);
}

#[test]
fn variable_references() {
    assert_mdl("population_growth_rate", "population growth rate");
    assert_mdl("x", "x");
    assert_mdl("a_b_c", "a b c");
}

#[test]
fn variable_references_quote_special_identifiers() {
    let special = Expr0::Var(RawIdent::new_from_str("$_euro"), Loc::default());
    assert_eq!(expr0_to_mdl(&special), "\"$ euro\"");

    let expr = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Var(RawIdent::new_from_str("$_euro"), Loc::default())),
        Box::new(Expr0::Var(
            RawIdent::new_from_str("revenue"),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(expr0_to_mdl(&expr), "\"$ euro\" + revenue");
}

#[test]
fn quoted_identifiers_escape_embedded_quotes_and_backslashes() {
    assert_eq!(escape_mdl_quoted_ident(r#"it"s"#), r#"it\"s"#);
    assert_eq!(escape_mdl_quoted_ident(r"back\slash"), r"back\\slash");
    assert_eq!(escape_mdl_quoted_ident(r#"a"b\c"#), r#"a\"b\\c"#,);

    assert_eq!(format_mdl_ident(r#"it"s_a_test"#), r#""it\"s a test""#,);
}

#[test]
fn quoted_identifiers_handle_newlines() {
    // Literal newline chars become the two-character escape \n
    assert_eq!(
        escape_mdl_quoted_ident("Maximum\nfishery size"),
        r"Maximum\nfishery size"
    );
    // Already-escaped \n (two chars: backslash + n) stays as-is
    assert_eq!(
        escape_mdl_quoted_ident(r"Maximum\nfishery size"),
        r"Maximum\nfishery size"
    );
    // Full round through format_mdl_ident: name with literal newline
    assert_eq!(
        format_mdl_ident("Maximum\nfishery_size"),
        r#""Maximum\nfishery size""#
    );
}

#[test]
fn needs_mdl_quoting_edge_cases() {
    assert!(needs_mdl_quoting(""));
    assert!(needs_mdl_quoting(" leading"));
    assert!(needs_mdl_quoting("trailing "));
    assert!(needs_mdl_quoting("1starts_with_digit"));
    assert!(!needs_mdl_quoting("normal name"));
    assert!(!needs_mdl_quoting("_private"));
    assert!(needs_mdl_quoting("has/slash"));
    assert!(needs_mdl_quoting("has|pipe"));
}

#[test]
fn arithmetic_operators() {
    assert_mdl("a + b", "a + b");
    assert_mdl("a - b", "a - b");
    assert_mdl("a * b", "a * b");
    assert_mdl("a / b", "a / b");
    assert_mdl("a ^ b", "a ^ b");
}

#[test]
fn precedence_no_extra_parens() {
    assert_mdl("a + b * c", "a + b * c");
}

#[test]
fn right_child_same_precedence_non_commutative() {
    // a - (b - c) must preserve parens: subtraction is not associative
    assert_mdl("a - (b - c)", "a - (b - c)");
    // a / (b / c) must preserve parens: division is not associative
    assert_mdl("a / (b / c)", "a / (b / c)");
    // a - (b + c) must preserve parens: + has same precedence as -
    assert_mdl("a - (b + c)", "a - (b + c)");
}

#[test]
fn left_child_same_precedence_no_extra_parens() {
    // (a - b) - c should NOT get extra parens: left-to-right is natural
    assert_mdl("a - b - c", "a - b - c");
    // (a / b) / c should NOT get extra parens
    assert_mdl("a / b / c", "a / b / c");
    // (a + b) + c should NOT get extra parens: + is commutative anyway
    assert_mdl("a + b + c", "a + b + c");
}

#[test]
fn precedence_parens_emitted() {
    assert_mdl("(a + b) * c", "(a + b) * c");
}

#[test]
fn nested_precedence() {
    assert_mdl("a * (b + c) / d", "a * (b + c) / d");
}

#[test]
fn unary_operators() {
    assert_mdl("-a", "-a");
    assert_mdl("+a", "+a");
    // XMILE uses `not` keyword; MDL uses `:NOT:` with a trailing space before the operand
    assert_mdl("not a", ":NOT: a");
}

#[test]
fn logical_operators_and() {
    // XMILE uses `and` keyword; MDL uses `:AND:` infix operator
    assert_mdl("a and b", "a :AND: b");
}

#[test]
fn logical_operators_or() {
    // XMILE uses `or` keyword; MDL uses `:OR:` infix operator
    assert_mdl("a or b", "a :OR: b");
}

#[test]
fn function_rename_smooth() {
    assert_mdl("smth1(x, 5)", "SMOOTH(x, 5)");
}

#[test]
fn function_rename_smooth3() {
    assert_mdl("smth3(x, 5)", "SMOOTH3(x, 5)");
}

#[test]
fn function_rename_safediv() {
    assert_mdl("safediv(a, b)", "ZIDZ(a, b)");
}

#[test]
fn function_rename_safediv_three_args_emits_xidz() {
    assert_mdl("safediv(a, b, x)", "XIDZ(a, b, x)");
}

#[test]
fn function_rename_init() {
    assert_mdl("init(x, 10)", "ACTIVE INITIAL(x, 10)");
}

#[test]
fn function_rename_int() {
    assert_mdl("int(x)", "INTEGER(x)");
}

#[test]
fn function_rename_uniform() {
    assert_mdl("uniform(0, 10)", "RANDOM UNIFORM(0, 10)");
}

#[test]
fn function_rename_forcst() {
    assert_mdl("forcst(x, 5, 0)", "FORECAST(x, 5, 0)");
}

#[test]
fn function_rename_delay() {
    assert_mdl("delay(x, 5, 0)", "DELAY FIXED(x, 5, 0)");
}

#[test]
fn function_rename_delay1() {
    assert_mdl("delay1(x, 5)", "DELAY1(x, 5)");
}

#[test]
fn function_rename_delay3() {
    assert_mdl("delay3(x, 5)", "DELAY3(x, 5)");
}

#[test]
fn function_rename_integ() {
    assert_mdl(
        "integ(inflow - outflow, 100)",
        "INTEG(inflow - outflow, 100)",
    );
}

#[test]
fn function_rename_lookupinv() {
    assert_mdl("lookupinv(tbl, 0.5)", "LOOKUP INVERT(tbl, 0.5)");
}

#[test]
fn function_rename_normalpink() {
    assert_mdl("normalpink(x, 5)", "RANDOM PINK NOISE(x, 5)");
}

#[test]
fn lookup_call_native_vensim_syntax() {
    // Vensim uses `table ( input )` syntax for lookup calls
    assert_mdl("lookup(tbl, x)", "tbl ( x )");
}

#[test]
fn function_unknown_uppercased() {
    assert_mdl("abs(x)", "ABS(x)");
    assert_mdl("ln(x)", "LN(x)");
    assert_mdl("max(a, b)", "MAX(a, b)");
}

#[test]
fn arg_reorder_delay_n() {
    // XMILE: delayn(input, delay_time, n, init) -> MDL: DELAY N(input, delay_time, init, n)
    assert_mdl(
        "delayn(input, delay_time, 3, init_val)",
        "DELAY N(input, delay time, init val, 3)",
    );
}

#[test]
fn arg_reorder_smooth_n() {
    // XMILE: smthn(input, delay_time, n, init) -> MDL: SMOOTH N(input, delay_time, init, n)
    assert_mdl(
        "smthn(input, delay_time, 3, init_val)",
        "SMOOTH N(input, delay time, init val, 3)",
    );
}

#[test]
fn arg_reorder_random_normal() {
    // XMILE: normal(mean, sd, seed, min, max) -> MDL: RANDOM NORMAL(min, max, mean, sd, seed)
    assert_mdl(
        "normal(mean, sd, seed, min_val, max_val)",
        "RANDOM NORMAL(min val, max val, mean, sd, seed)",
    );
}

// -- pattern recognizer tests (Task 2) --

#[test]
fn pattern_random_0_1() {
    // XMILE: uniform(0, 1) -> MDL: RANDOM 0 1()
    assert_mdl("uniform(0, 1)", "RANDOM 0 1()");
}

#[test]
fn pattern_random_0_1_not_matched_different_args() {
    // uniform with non-(0,1) args should NOT match the RANDOM 0 1 pattern
    assert_mdl("uniform(0, 10)", "RANDOM UNIFORM(0, 10)");
}

#[test]
fn pattern_log_2arg() {
    // XMILE: ln(x) / ln(base) -> MDL: LOG(x, base)
    assert_mdl("ln(x) / ln(base)", "LOG(x, base)");
}

#[test]
fn pattern_quantum() {
    // XMILE: q * int(x / q)  ->  MDL: QUANTUM(x, q)
    assert_mdl("q * int(x / q)", "QUANTUM(x, q)");
}

#[test]
fn pattern_quantum_not_matched_different_q() {
    // q1 * int(x / q2) should NOT match QUANTUM when q1 != q2
    assert_mdl("q1 * int(x / q2)", "q1 * INTEGER(x / q2)");
}

#[test]
fn pattern_pulse() {
    // XMILE expansion of PULSE(start, width):
    // IF TIME >= start AND TIME < (start + MAX(DT, width)) THEN 1 ELSE 0
    assert_mdl(
        "if time >= start and time < (start + max(dt, width)) then 1 else 0",
        "PULSE(start, width)",
    );
}

#[test]
fn pattern_pulse_not_matched_missing_lt() {
    // Missing the Lt branch -- should fall through to mechanical conversion
    assert_mdl(
        "if time >= start then 1 else 0",
        "IF THEN ELSE(Time >= start, 1, 0)",
    );
}

#[test]
fn pattern_pulse_train() {
    // XMILE expansion of PULSE TRAIN(start, width, interval, end_val):
    // IF TIME >= start AND TIME <= end_val AND (TIME - start) MOD interval < width THEN 1 ELSE 0
    assert_mdl(
        "if time >= start and time <= end_val and (time - start) mod interval < width then 1 else 0",
        "PULSE TRAIN(start, width, interval, end val)",
    );
}

#[test]
fn mod_emits_modulo() {
    assert_mdl("a mod b", "MODULO(a, b)");
    assert_mdl("(time) mod (5)", "MODULO(Time, 5)");
}

#[test]
fn pattern_sample_if_true() {
    // XMILE expansion of SAMPLE IF TRUE(cond, input, init):
    // IF cond THEN input ELSE PREVIOUS(SELF, init)
    assert_mdl(
        "if cond then input else previous(self, init_val)",
        "SAMPLE IF TRUE(cond, input, init val)",
    );
}

#[test]
fn pattern_time_base() {
    // XMILE expansion of TIME BASE(t, delta):
    // t + delta * TIME
    assert_mdl("t_val + delta * time", "TIME BASE(t val, delta)");
}

#[test]
fn pattern_random_poisson() {
    // XMILE expansion of RANDOM POISSON(min, max, mean, sdev, factor, seed):
    // poisson(mean / dt, seed, min, max) * factor + sdev
    assert_mdl(
        "poisson(mean / dt, seed, min_val, max_val) * factor + sdev",
        "RANDOM POISSON(min val, max val, mean, sdev, factor, seed)",
    );
}

#[test]
fn pattern_fallthrough_no_match() {
    // An If expression that doesn't match any pattern should use mechanical conversion
    assert_mdl("if a > b then c else d", "IF THEN ELSE(a > b, c, d)");
}

#[test]
fn pattern_allocate_by_priority() {
    // XMILE expansion of ALLOCATE BY PRIORITY(demand[region], priority, ignore, width, supply):
    // allocate(supply, region, demand[region.*], priority, width)
    //
    // The last subscript (region.*) is replaced with the dimension name, yielding demand[region].
    // The arguments are reordered: demand first, then priority, then 0 (ignore), width, supply.
    assert_mdl(
        "allocate(supply, region, demand[region.*], priority, width)",
        "ALLOCATE BY PRIORITY(demand[region], priority, 0, width, supply)",
    );
}

#[test]
fn pattern_allocate_by_priority_no_subscript() {
    // When the demand argument has no subscript (simple variable), it passes through as-is.
    assert_mdl(
        "allocate(supply, region, demand, priority, width)",
        "ALLOCATE BY PRIORITY(demand, priority, 0, width, supply)",
    );
}

#[test]
fn pattern_allocate_by_priority_native() {
    // Native XMILE form: allocate_by_priority(request, priority, size, width, supply)
    // Args are already in MDL order -- no reordering needed.
    assert_mdl(
        "allocate_by_priority(demand[region], priority, 0, width, supply)",
        "ALLOCATE BY PRIORITY(demand[region], priority, 0, width, supply)",
    );
}

// ---- lookup call syntax tests ----

#[test]
fn lookup_call_with_spaced_table_name() {
    // Multi-word table ident should be space-separated in output
    assert_mdl(
        "lookup(federal_funds_rate_lookup, time)",
        "federal funds rate lookup ( Time )",
    );
}

#[test]
fn lookup_call_with_expression_input() {
    // The input argument can be an arbitrary expression
    assert_mdl("lookup(my_table, a + b)", "my table ( a + b )");
}

#[test]
fn lookup_non_var_first_arg_falls_through() {
    // When the first arg is not a bare variable (e.g. a subscripted
    // reference), the generic LOOKUP(...) path is used as a fallback.
    let table_sub = Expr0::Subscript(
        RawIdent::new_from_str("tbl"),
        vec![IndexExpr0::Expr(Expr0::Var(
            RawIdent::new_from_str("i"),
            Loc::default(),
        ))],
        Loc::default(),
    );
    let input = Expr0::Var(RawIdent::new_from_str("x"), Loc::default());
    let expr = Expr0::App(
        UntypedBuiltinFn("lookup".to_owned(), vec![table_sub, input]),
        Loc::default(),
    );
    let mdl = expr0_to_mdl(&expr);
    assert_eq!(mdl, "LOOKUP(tbl[i], x)");
}

#[test]
fn non_lookup_function_emits_normally() {
    // Other function calls should not be affected by the lookup special-case
    assert_mdl("max(a, b)", "MAX(a, b)");
    assert_mdl("min(x, y)", "MIN(x, y)");
}

// ---- Task 1: Variable entry formatting (scalar) ----

fn make_aux(ident: &str, eqn: &str, units: Option<&str>, doc: &str) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation: Equation::Scalar(eqn.to_owned()),
        documentation: doc.to_owned(),
        units: units.map(|u| u.to_owned()),
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn make_stock(ident: &str, eqn: &str, units: Option<&str>, doc: &str) -> Variable {
    Variable::Stock(Stock {
        ident: ident.to_owned(),
        equation: Equation::Scalar(eqn.to_owned()),
        documentation: doc.to_owned(),
        units: units.map(|u| u.to_owned()),
        inflows: vec![],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn make_gf() -> GraphicalFunction {
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 0.5, 1.0],
        x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

#[test]
fn scalar_aux_entry() {
    let var = make_aux("characteristic_time", "10", Some("Minutes"), "How long");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(
        buf,
        "characteristic time = 10\n\t~\tMinutes\n\t~\tHow long\n\t|"
    );
}

#[test]
fn scalar_aux_entry_quotes_special_identifier_name() {
    let var = make_aux("$_euro", "10", Some("Dmnl"), "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(buf, "\"$ euro\" = 10\n\t~\tDmnl\n\t~\t\n\t|");
}

#[test]
fn scalar_aux_no_units() {
    let var = make_aux("rate", "a + b", None, "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(buf, "rate = a + b\n\t~\t\n\t~\t\n\t|");
}

// ---- Inline vs multiline equation formatting ----

#[test]
fn short_equation_uses_inline_format() {
    let var = make_aux("average_repayment_rate", "0.03", Some("1/Year"), "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.starts_with("average repayment rate = 0.03\n"),
        "short equation should use inline format: {buf}"
    );
    assert!(
        !buf.contains("=\n\t0.03"),
        "short equation should not use multiline format: {buf}"
    );
}

#[test]
fn long_equation_uses_multiline_format() {
    // Build an equation that, combined with the name, exceeds 80 chars
    let long_eqn =
        "very_long_variable_a + very_long_variable_b + very_long_variable_c + very_long_variable_d";
    let var = make_aux("some_computed_value", long_eqn, None, "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("some computed value=\n\t"),
        "long equation should use multiline format: {buf}"
    );
}

#[test]
fn lookup_always_uses_multiline_format() {
    let gf = make_gf();
    let var = Variable::Aux(Aux {
        ident: "x".to_owned(),
        equation: Equation::Scalar("TIME".to_owned()),
        documentation: String::new(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.starts_with("x=\n\t"),
        "lookup equation should always use multiline format: {buf}"
    );
}

#[test]
fn data_equation_uses_data_equals_inline() {
    let var = make_aux(
        "small_data",
        "{GET_DIRECT_DATA('f.csv',',','A','B')}",
        None,
        "",
    );
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains(":="),
        "data equation should use := operator: {buf}"
    );
    assert!(
        buf.contains(" := "),
        "short data equation should use inline format with spaces: {buf}"
    );
}

// ---- Backslash line continuation tests ----

#[test]
fn wrap_short_equation_unchanged() {
    let eqn = "a + b";
    let wrapped = wrap_equation_with_continuations(eqn, 80);
    assert_eq!(wrapped, eqn);
    assert!(
        !wrapped.contains('\\'),
        "short equation should not be wrapped: {wrapped}"
    );
}

#[test]
fn wrap_long_equation_with_continuations() {
    // Build an equation >80 chars with multiple terms
    let eqn =
        "very long variable a + very long variable b + very long variable c + very long variable d";
    assert!(eqn.len() > 80, "test equation should exceed 80 chars");
    let wrapped = wrap_equation_with_continuations(eqn, 80);
    assert!(
        wrapped.contains("\\\n\t\t"),
        "long equation should contain continuation: {wrapped}"
    );
    // Verify the continuation produces valid content when joined
    let rejoined = wrapped.replace("\\\n\t\t", "");
    // The rejoined text should reconstruct the original (modulo trimmed trailing spaces)
    assert!(
        rejoined.contains("very long variable a"),
        "content should be preserved: {rejoined}"
    );
}

#[test]
fn wrap_equation_breaks_after_comma() {
    // A function call with many arguments
    let eqn = "IF THEN ELSE(very long condition variable > threshold value, very long true result, very long false result)";
    assert!(eqn.len() > 80);
    let wrapped = wrap_equation_with_continuations(eqn, 80);
    assert!(wrapped.contains("\\\n\t\t"), "should wrap: {wrapped}");
    // Verify breaks happen at reasonable points (after commas or before operators)
    let lines: Vec<&str> = wrapped.split("\\\n\t\t").collect();
    assert!(lines.len() >= 2, "should have at least 2 lines: {wrapped}");
}

#[test]
fn long_equation_variable_entry_uses_continuation() {
    let long_eqn =
        "very_long_variable_a + very_long_variable_b + very_long_variable_c + very_long_variable_d";
    let var = make_aux("some_computed_value", long_eqn, None, "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("some computed value=\n\t"),
        "long equation should use multiline format: {buf}"
    );
    // The equation body should have a continuation if the MDL form exceeds 80 chars
    let mdl_eqn = equation_to_mdl(long_eqn);
    if mdl_eqn.len() > 80 {
        assert!(
            buf.contains("\\\n\t\t"),
            "long MDL equation should use backslash continuation: {buf}"
        );
    }
}

#[test]
fn short_equation_variable_entry_no_continuation() {
    let var = make_aux("x", "42", None, "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        !buf.contains("\\\n\t\t"),
        "short equation should not have continuation: {buf}"
    );
}

#[test]
fn tokenize_preserves_equation_text() {
    let eqn = "IF THEN ELSE(a > b, c + d, e * f)";
    let tokens = tokenize_for_wrapping(eqn);
    let rejoined: String = tokens.concat();
    assert_eq!(
        rejoined, eqn,
        "concatenating tokens should reproduce the original"
    );
}

#[test]
fn tokenize_splits_at_operators_and_commas() {
    let eqn = "a + b, c * d";
    let tokens = tokenize_for_wrapping(eqn);
    // Should have splits at +, *, and after comma
    assert!(
        tokens.len() >= 5,
        "expected multiple tokens from operators/commas: {tokens:?}"
    );
}

#[test]
fn scalar_stock_integ() {
    // Real stocks from the MDL reader store only the initial value in
    // equation, with inflows/outflows in separate fields.  The writer
    // must reconstruct the INTEG(...) expression.
    let var = Variable::Stock(Stock {
        ident: "teacup_temperature".to_owned(),
        equation: Equation::Scalar("180".to_owned()),
        documentation: "Temperature of tea".to_owned(),
        units: Some("Degrees Fahrenheit".to_owned()),
        inflows: vec![],
        outflows: vec!["heat_loss_to_room".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(
        buf,
        "teacup temperature=\n\tINTEG(-heat loss to room, 180)\n\t~\tDegrees Fahrenheit\n\t~\tTemperature of tea\n\t|"
    );
}

#[test]
fn stock_with_inflows_and_outflows() {
    let var = Variable::Stock(Stock {
        ident: "population".to_owned(),
        equation: Equation::Scalar("1000".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec!["births".to_owned()],
        outflows: vec!["deaths".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("INTEG(births-deaths, 1000)"),
        "Expected INTEG with both inflow and outflow: {}",
        buf
    );
}

#[test]
fn arrayed_stock_apply_to_all_preserves_initial_value() {
    let var = Variable::Stock(Stock {
        ident: "inventory".to_owned(),
        equation: Equation::ApplyToAll(vec!["region".to_owned()], "100".to_owned()),
        documentation: "Stock by region".to_owned(),
        units: Some("widgets".to_owned()),
        inflows: vec!["inflow".to_owned()],
        outflows: vec!["outflow".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("inventory[region]=\n\tINTEG(inflow-outflow, 100)"),
        "ApplyToAll stock should emit arrayed INTEG with initial value: {}",
        buf
    );
}

#[test]
fn arrayed_stock_elements_preserve_each_initial_value() {
    let var = Variable::Stock(Stock {
        ident: "inventory".to_owned(),
        equation: Equation::Arrayed(
            vec!["region".to_owned()],
            vec![
                ("north".to_owned(), "100".to_owned(), None, None),
                ("south".to_owned(), "200".to_owned(), None, None),
            ],
            None,
            false,
        ),
        documentation: "Stock by region".to_owned(),
        units: Some("widgets".to_owned()),
        inflows: vec!["inflow".to_owned()],
        outflows: vec!["outflow".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("inventory[north]=\n\tINTEG(inflow-outflow, 100)"),
        "First arrayed stock element should retain initial value: {}",
        buf
    );
    assert!(
        buf.contains("inventory[south]=\n\tINTEG(inflow-outflow, 200)"),
        "Second arrayed stock element should retain initial value: {}",
        buf
    );
}

#[test]
fn active_initial_preserved_on_aux() {
    let var = Variable::Aux(Aux {
        ident: "x".to_owned(),
        equation: Equation::Scalar("y * 2".to_owned()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat {
            active_initial: Some("100".to_owned()),
            ..Compat::default()
        },
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains("ACTIVE INITIAL(y * 2, 100)"),
        "Expected ACTIVE INITIAL wrapper: {}",
        buf
    );
}

#[test]
fn compat_data_source_reconstructs_get_direct_constants() {
    let var = Variable::Aux(Aux {
        ident: "imported_constants".to_owned(),
        equation: Equation::Scalar("0".to_owned()),
        documentation: String::new(),
        units: None,
        gf: Some(make_gf()),
        ai_state: None,
        uid: None,
        compat: Compat {
            data_source: Some(crate::datamodel::DataSource {
                kind: crate::datamodel::DataSourceKind::Constants,
                file: "workbook.xlsx".to_owned(),
                tab_or_delimiter: "Sheet1".to_owned(),
                row_or_col: "A".to_owned(),
                cell: String::new(),
            }),
            ..Compat::default()
        },
    });

    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(
        buf.contains(":="),
        "GET DIRECT reconstruction should use := for data equations: {buf}"
    );
    assert!(
        buf.contains("GET DIRECT CONSTANTS('workbook.xlsx', 'Sheet1', 'A')"),
        "writer should reconstruct GET DIRECT CONSTANTS from compat metadata: {buf}"
    );
    assert!(
        !buf.contains("([(0,0)-(2,1)]"),
        "lookup table output must be suppressed when data_source metadata is present: {buf}"
    );
}

#[test]
fn scalar_aux_with_lookup() {
    let gf = make_gf();
    let var = Variable::Aux(Aux {
        ident: "effect_of_x".to_owned(),
        equation: Equation::Scalar("TIME".to_owned()),
        documentation: "Lookup effect".to_owned(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(
        buf,
        "effect of x=\n\tWITH LOOKUP(Time, ([(0,0)-(2,1)],(0,0),(1,0.5),(2,1)))\n\t~\t\n\t~\tLookup effect\n\t|"
    );
}

#[test]
fn lookup_without_explicit_x_points() {
    let gf = GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: None,
        y_points: vec![0.0, 0.5, 1.0],
        x_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let var = Variable::Aux(Aux {
        ident: "tbl".to_owned(),
        equation: Equation::Scalar(String::new()),
        documentation: String::new(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // Standalone lookup: name(\n\tbody)
    assert_eq!(
        buf,
        "tbl(\n\t[(0,0)-(10,1)],(0,0),(5,0.5),(10,1))\n\t~\t\n\t~\t\n\t|"
    );
}

// ---- Task 2: Subscripted equation formatting ----

#[test]
fn apply_to_all_entry() {
    let var = Variable::Aux(Aux {
        ident: "rate_a".to_owned(),
        equation: Equation::ApplyToAll(
            vec!["one_dimensional_subscript".to_owned()],
            "100".to_owned(),
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(
        buf,
        "rate a[one dimensional subscript] = 100\n\t~\t\n\t~\t\n\t|"
    );
}

#[test]
fn apply_to_all_multi_dim() {
    let var = Variable::Aux(Aux {
        ident: "matrix_a".to_owned(),
        equation: Equation::ApplyToAll(
            vec!["dim_a".to_owned(), "dim_b".to_owned()],
            "0".to_owned(),
        ),
        documentation: String::new(),
        units: Some("Dmnl".to_owned()),
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(buf, "matrix a[dim a,dim b] = 0\n\t~\tDmnl\n\t~\t\n\t|");
}

#[test]
fn arrayed_per_element() {
    let var = Variable::Aux(Aux {
        ident: "rate_a".to_owned(),
        equation: Equation::Arrayed(
            vec!["one_dimensional_subscript".to_owned()],
            vec![
                ("entry_1".to_owned(), "0.01".to_owned(), None, None),
                ("entry_2".to_owned(), "0.2".to_owned(), None, None),
                ("entry_3".to_owned(), "0.3".to_owned(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert_eq!(
        buf,
        "rate a[entry 1]=\n\t0.01\n\t~~|\nrate a[entry 2]=\n\t0.2\n\t~~|\nrate a[entry 3]=\n\t0.3\n\t~\t\n\t~\t\n\t|"
    );
}

#[test]
fn arrayed_subscript_names_with_underscores() {
    let var = Variable::Aux(Aux {
        ident: "demand".to_owned(),
        equation: Equation::Arrayed(
            vec!["region".to_owned()],
            vec![
                ("north_america".to_owned(), "100".to_owned(), None, None),
                ("south_america".to_owned(), "200".to_owned(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // Underscored element names should appear with spaces
    assert!(buf.contains("[north america]"));
    assert!(buf.contains("[south america]"));
}

#[test]
fn arrayed_multidimensional_element_keys_preserve_tuple_shape() {
    let var = Variable::Aux(Aux {
        ident: "power5".to_owned(),
        equation: Equation::Arrayed(
            vec!["subs2".to_owned(), "subs1".to_owned(), "subs3".to_owned()],
            vec![
                (
                    "c,a,f".to_owned(),
                    "power(var3[subs2, subs1], var4[subs2, subs3])".to_owned(),
                    None,
                    None,
                ),
                (
                    "d,b,g".to_owned(),
                    "power(var3[subs2, subs1], var4[subs2, subs3])".to_owned(),
                    None,
                    None,
                ),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());

    assert!(
        buf.contains("power5[c,a,f]="),
        "missing first tuple key: {buf}"
    );
    assert!(
        buf.contains("power5[d,b,g]="),
        "missing second tuple key: {buf}"
    );
    assert!(
        !buf.contains("power5[\"c,a,f\"]"),
        "tuple key must not be quoted as a single symbol: {buf}"
    );
}

#[test]
fn arrayed_with_per_element_lookup() {
    let gf = make_gf();
    let var = Variable::Aux(Aux {
        ident: "tbl".to_owned(),
        equation: Equation::Arrayed(
            vec!["dim".to_owned()],
            vec![
                ("a".to_owned(), String::new(), None, Some(gf.clone())),
                ("b".to_owned(), "5".to_owned(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // Element "a" has empty equation + gf → standalone lookup
    assert!(buf.contains("tbl[a](\n\t[(0,0)-(2,1)]"));
    assert!(buf.contains("tbl[b]=\n\t5"));
}

// ---- Task 3: Dimension definitions ----

#[test]
fn dimension_def_named() {
    let dim = datamodel::Dimension::named(
        "dim_a".to_owned(),
        vec!["a1".to_owned(), "a2".to_owned(), "a3".to_owned()],
    );
    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);
    assert_eq!(buf, "dim a:\n\ta1, a2, a3\n\t~~|\n");
}

#[test]
fn dimension_def_indexed() {
    let dim = datamodel::Dimension::indexed("dim_b".to_owned(), 5);
    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);
    assert_eq!(buf, "dim b:\n\t(1-5)\n\t~~|\n");
}

#[test]
fn dimension_def_with_mapping() {
    let mut dim = datamodel::Dimension::named(
        "dim_c".to_owned(),
        vec!["dc1".to_owned(), "dc2".to_owned(), "dc3".to_owned()],
    );
    dim.set_maps_to("dim_b".to_owned());
    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);
    assert_eq!(buf, "dim c:\n\tdc1, dc2, dc3 -> dim b\n\t~~|\n");
}

// ---- Task 3: Data equations ----

#[test]
fn data_equation_uses_data_equals() {
    let var = make_aux(
        "direct_data_down",
        "{GET_DIRECT_DATA('data_down.csv',',','A','B2')}",
        None,
        "",
    );
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // Data equations use := instead of =
    assert!(buf.contains(":="), "expected := in: {buf}");
}

#[test]
fn non_data_equation_uses_equals() {
    let var = make_aux("x", "42", None, "");
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(buf.starts_with("x = "), "expected = in: {buf}");
}

#[test]
fn is_data_equation_detection() {
    // Underscore-separated form (as might appear in some equation strings)
    assert!(is_data_equation("{GET_DIRECT_DATA('f',',','A','B')}"));
    assert!(is_data_equation("{GET_XLS_DATA('f','s','A','B')}"));
    assert!(is_data_equation("{GET_VDF_DATA('f','v')}"));
    assert!(is_data_equation("{GET_DATA_AT_TIME('v', 5)}"));
    assert!(is_data_equation("{GET_123_DATA('f','s','A','B')}"));

    // Space-separated form (as produced by the normalizer's SymbolClass::GetXls)
    assert!(is_data_equation("{GET DIRECT DATA('f',',','A','B')}"));
    assert!(is_data_equation("{GET XLS DATA('f','s','A','B')}"));
    assert!(is_data_equation("{GET VDF DATA('f','v')}"));
    assert!(is_data_equation("{GET DATA AT TIME('v', 5)}"));
    assert!(is_data_equation("{GET 123 DATA('f','s','A','B')}"));

    assert!(!is_data_equation("100"));
    assert!(!is_data_equation("integ(a, b)"));
    assert!(!is_data_equation(""));
}

#[test]
fn data_equation_preserves_raw_content() {
    // Data equations should not go through expr0_to_mdl() because
    // the GET XLS/DIRECT/etc. placeholders are not parseable as Expr0.
    // Verify the raw content is preserved (not mangled by underbar_to_space).
    let var = make_aux(
        "my_data",
        "{GET DIRECT DATA('data_file.csv',',','A','B2')}",
        None,
        "",
    );
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // The equation content must preserve underscores in quoted strings
    assert!(
        buf.contains("GET DIRECT DATA('data_file.csv',',','A','B2')"),
        "Data equation content mangled: {}",
        buf
    );
    // Must use := for data equations
    assert!(buf.contains(":="), "Expected := for data equation: {}", buf);
}

#[test]
fn format_f64_whole_numbers() {
    assert_eq!(format_f64(0.0), "0");
    assert_eq!(format_f64(1.0), "1");
    assert_eq!(format_f64(-5.0), "-5");
    assert_eq!(format_f64(100.0), "100");
}

#[test]
fn format_f64_fractional() {
    assert_eq!(format_f64(0.5), "0.5");
    assert_eq!(format_f64(2.71), "2.71");
}

#[test]
fn format_f64_infinity_uses_vensim_numeric_sentinels() {
    assert_eq!(format_f64(f64::INFINITY), "1e+38");
    assert_eq!(format_f64(f64::NEG_INFINITY), "-1e+38");
}

#[test]
fn flow_with_lookup() {
    let gf = make_gf();
    let var = Variable::Flow(Flow {
        ident: "flow_rate".to_owned(),
        equation: Equation::Scalar("TIME".to_owned()),
        documentation: "A flow".to_owned(),
        units: Some("widgets/year".to_owned()),
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    // Flow with equation "TIME" + gf → WITH LOOKUP
    assert!(buf.contains("flow rate=\n\tWITH LOOKUP(Time, ([(0,0)-(2,1)]"));
    assert!(buf.contains("~\twidgets/year"));
    assert!(buf.contains("~\tA flow"));
}

#[test]
fn module_variable_produces_no_output() {
    let var = Variable::Module(datamodel::Module {
        ident: "mod1".to_owned(),
        model_name: "model1".to_owned(),
        documentation: String::new(),
        units: None,
        references: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());
    assert!(buf.is_empty());
}

// ---- Phase 4 Task 1: Validation ----

fn make_project(models: Vec<datamodel::Model>) -> datamodel::Project {
    datamodel::Project {
        name: "test".to_owned(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models,
        source: None,
        ai_information: None,
    }
}

fn make_model(variables: Vec<Variable>) -> datamodel::Model {
    datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    }
}

#[test]
fn project_to_mdl_rejects_multiple_models() {
    let project = make_project(vec![make_model(vec![]), make_model(vec![])]);
    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("single model"),
        "error should mention single model, got: {}",
        err
    );
}

#[test]
fn project_to_mdl_rejects_module_variable() {
    let module_var = Variable::Module(datamodel::Module {
        ident: "submodel".to_owned(),
        model_name: "inner".to_owned(),
        documentation: String::new(),
        units: None,
        references: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let project = make_project(vec![make_model(vec![module_var])]);
    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("Module"),
        "error should mention Module, got: {}",
        err
    );
}

#[test]
fn project_to_mdl_succeeds_single_model() {
    let var = make_aux("x", "5", Some("Units"), "A constant");
    let project = make_project(vec![make_model(vec![var])]);
    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok(), "should succeed: {:?}", result);
    let mdl = result.unwrap();
    assert!(
        mdl.starts_with("{UTF-8}\r\n"),
        "MDL should start with UTF-8 marker, got: {:?}",
        mdl.lines().next()
    );
    assert!(mdl.contains("x = "));
    assert!(mdl.contains("\\\\\\---///"));
}

// ---- Phase 4 Task 2: Sim spec emission ----

#[test]
fn sim_specs_emission() {
    let sim_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: datamodel::Dt::Dt(0.5),
        save_step: Some(datamodel::Dt::Dt(1.0)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Month".to_owned()),
    };
    let mut writer = MdlWriter::new();
    writer.write_sim_specs(&sim_specs);
    let output = writer.buf;

    assert!(
        output.contains("INITIAL TIME  = \n\t0"),
        "should have INITIAL TIME, got: {output}"
    );
    assert!(
        output.contains("~\tMonth\n\t~\tThe initial time for the simulation."),
        "INITIAL TIME should have Month units"
    );
    assert!(
        output.contains("FINAL TIME  = \n\t100"),
        "should have FINAL TIME, got: {output}"
    );
    assert!(
        output.contains("TIME STEP  = \n\t0.5"),
        "should have TIME STEP = 0.5, got: {output}"
    );
    assert!(
        output.contains("Month [0,?]"),
        "TIME STEP should have units with range, got: {output}"
    );
    assert!(
        output.contains("SAVEPER  = \n\t1"),
        "should have SAVEPER = 1, got: {output}"
    );
}

#[test]
fn sim_specs_saveper_defaults_to_time_step() {
    let sim_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 50.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let mut writer = MdlWriter::new();
    writer.write_sim_specs(&sim_specs);
    let output = writer.buf;

    assert!(
        output.contains("SAVEPER  = \n\tTIME STEP"),
        "SAVEPER should reference TIME STEP when save_step is None, got: {output}"
    );
}

#[test]
fn sim_specs_reciprocal_dt() {
    let sim_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: datamodel::Dt::Reciprocal(4.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Year".to_owned()),
    };
    let mut writer = MdlWriter::new();
    writer.write_sim_specs(&sim_specs);
    let output = writer.buf;

    assert!(
        output.contains("TIME STEP  = \n\t1/4"),
        "reciprocal dt should emit 1/v, got: {output}"
    );
}

// ---- Phase 4 Task 3: Equations section assembly ----

#[test]
fn equations_section_full_assembly() {
    let var1 = make_aux("growth_rate", "0.05", Some("1/Year"), "Growth rate");
    let var2 = make_stock(
        "population",
        "integ(growth_rate * population, 100)",
        Some("People"),
        "Total population",
    );
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var1, var2],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = datamodel::Project {
        name: "test".to_owned(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Year".to_owned()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![model],
        source: None,
        ai_information: None,
    };

    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok(), "should succeed: {:?}", result);
    let mdl = result.unwrap();

    // Variable entries present
    assert!(
        mdl.contains("growth rate = "),
        "should contain growth rate variable"
    );
    assert!(
        mdl.contains("population="),
        "should contain population variable"
    );

    // Sim specs present
    assert!(mdl.contains("INITIAL TIME"), "should contain INITIAL TIME");
    assert!(mdl.contains("FINAL TIME"), "should contain FINAL TIME");
    assert!(mdl.contains("TIME STEP"), "should contain TIME STEP");
    assert!(mdl.contains("SAVEPER"), "should contain SAVEPER");

    // Terminator present
    assert!(
        mdl.contains("\\\\\\---/// Sketch information - do not modify anything except names"),
        "should contain section terminator"
    );

    // Ordering: variables before sim specs, sim specs before terminator
    let var_pos = mdl.find("growth rate = ").unwrap();
    let initial_pos = mdl.find("INITIAL TIME").unwrap();
    let terminator_pos = mdl.find("\\\\\\---///").unwrap();
    assert!(
        var_pos < initial_pos,
        "variables should come before sim specs"
    );
    assert!(
        initial_pos < terminator_pos,
        "sim specs should come before terminator"
    );
}

#[test]
fn equations_section_with_groups() {
    let var1 = make_aux("rate_a", "10", None, "");
    let var2 = make_aux("rate_b", "20", None, "");
    let var3 = make_aux("ungrouped_var", "30", None, "");
    let group = datamodel::ModelGroup {
        name: "my_group".to_owned(),
        doc: Some("Group docs".to_owned()),
        parent: None,
        members: vec!["rate_a".to_owned(), "rate_b".to_owned()],
        run_enabled: false,
    };
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var1, var2, var3],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![group],
    };
    let project = make_project(vec![model]);

    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok(), "should succeed: {:?}", result);
    let mdl = result.unwrap();

    // Group marker present
    assert!(
        mdl.contains(".my group"),
        "should contain group marker, got: {mdl}"
    );
    assert!(
        mdl.contains("Group docs"),
        "should contain group documentation"
    );

    // Grouped variables come before ungrouped
    let rate_a_pos = mdl.find("rate a = ").unwrap();
    let ungrouped_pos = mdl.find("ungrouped var = ").unwrap();
    assert!(
        rate_a_pos < ungrouped_pos,
        "grouped variables should come before ungrouped"
    );
}

#[test]
fn equations_section_with_dimensions() {
    let dim = datamodel::Dimension::named(
        "region".to_owned(),
        vec!["north".to_owned(), "south".to_owned()],
    );
    let var = make_aux("x", "1", None, "");
    let model = make_model(vec![var]);
    let mut project = make_project(vec![model]);
    project.dimensions.push(dim);

    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok(), "should succeed: {:?}", result);
    let mdl = result.unwrap();

    // Dimension def at the start, before variables
    assert!(
        mdl.contains("region:\r\n\tnorth, south\r\n\t~~|"),
        "should contain dimension def"
    );
    let dim_pos = mdl.find("region:").unwrap();
    let var_pos = mdl.find("x = ").unwrap();
    assert!(dim_pos < var_pos, "dimensions should come before variables");
}

// ---- Phase 5 Task 1: Sketch element serialization (types 10, 11, 12) ----

#[test]
fn sketch_aux_element() {
    let aux = view_element::Aux {
        name: "Growth_Rate".to_string(),
        uid: 1,
        x: 100.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut buf = String::new();
    write_aux_element(&mut buf, &aux);
    assert_eq!(buf, "10,1,Growth Rate,100,200,40,20,8,3,0,0,-1,0,0,0");
}

#[test]
fn sketch_stock_element() {
    let stock = view_element::Stock {
        name: "Population".to_string(),
        uid: 2,
        x: 300.0,
        y: 150.0,
        label_side: view_element::LabelSide::Top,
        compat: None,
    };
    let mut buf = String::new();
    write_stock_element(&mut buf, &stock);
    assert_eq!(buf, "10,2,Population,300,150,40,20,3,3,0,0,0,0,0,0");
}

#[test]
fn sketch_flow_element_produces_valve_and_variable() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 295.0,
        y: 191.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );
    // No flow points, so no pipe connectors; valve and label follow
    assert!(buf.contains("11,100,0,295,191,6,8,34,3,0,0,1,0,0,0"));
    assert!(buf.contains("10,6,Infection Rate,295,207,49,8,40,3,0,0,-1,0,0,0"));
}

#[test]
fn sketch_flow_element_emits_pipe_connectors_from_flow_points() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![
            view_element::FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            view_element::FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(2),
            },
        ],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );

    let connector_lines: Vec<&str> = buf.lines().filter(|line| line.starts_with("1,")).collect();
    assert_eq!(
        connector_lines.len(),
        2,
        "Expected two type-1 connector lines for flow endpoints: {}",
        buf
    );
    assert!(
        connector_lines.iter().any(|line| line.contains(",100,1,")),
        "Expected connector from valve uid 100 to endpoint uid 1: {}",
        buf
    );
    assert!(
        connector_lines.iter().any(|line| line.contains(",100,2,")),
        "Expected connector from valve uid 100 to endpoint uid 2: {}",
        buf
    );
}

#[test]
fn sketch_flow_element_derives_stock_connector_points_from_takeoffs() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![
            view_element::FlowPoint {
                x: 122.5,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            view_element::FlowPoint {
                x: 177.5,
                y: 100.0,
                attached_to_uid: Some(2),
            },
        ],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let elem_positions = HashMap::from([(1, (100, 100)), (2, (200, 100))]);
    let stock_uids = HashSet::from([1, 2]);
    let mut next_connector_uid = 200;
    write_flow_element_with_context(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
        SketchTransform::identity(),
        &elem_positions,
        &stock_uids,
        None,
    );

    assert!(
        buf.contains("1,200,100,2,4,0,0,22,0,0,0,-1--1--1,,1|(200,100)|"),
        "sink pipe connector should be reconstructed from the stock center: {buf}"
    );
    assert!(
        buf.contains("1,201,100,1,4,0,0,22,0,0,0,-1--1--1,,1|(100,100)|"),
        "source pipe connector should be reconstructed from the stock center: {buf}"
    );
    assert!(
        buf.contains("10,6,Infection Rate,150,116,49,8,40,3,0,0,-1,0,0,0"),
        "flow label should fall back to the canonical bottom label position: {buf}"
    );
}

#[test]
fn valve_uids_do_not_collide_with_existing_elements() {
    // stock uid=1, flow uid=2 -> valve must NOT get uid=1
    let elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "Population".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Flow(view_element::Flow {
            name: "Birth_Rate".to_string(),
            uid: 2,
            x: 200.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            points: vec![],
            compat: None,
            label_compat: None,
        }),
    ];

    let valve_uids = allocate_valve_uids(&elements);
    // The valve for flow uid=2 must not equal 1 (stock's uid)
    let valve_uid = valve_uids[&2];
    assert_ne!(valve_uid, 1, "Valve UID collides with stock UID");
    assert_ne!(valve_uid, 2, "Valve UID collides with flow UID");
}

#[test]
fn sketch_cloud_element() {
    let cloud = view_element::Cloud {
        uid: 7,
        flow_uid: 6,
        x: 479.0,
        y: 235.0,
        compat: None,
    };
    let mut buf = String::new();
    write_cloud_element(&mut buf, &cloud);
    assert_eq!(buf, "12,7,48,479,235,10,8,0,3,0,0,-1,0,0,0");
}

#[test]
fn sketch_alias_element() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Growth_Rate");
    let mut buf = String::new();
    write_alias_element(&mut buf, &alias, &name_map);
    assert!(buf.starts_with("10,10,Growth Rate,200,300,40,20,8,2,0,3,-1,0,0,0,"));
    assert!(buf.contains("128-128-128"));
}

#[test]
fn sketch_alias_element_offsets_stock_ghost_coordinates() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Population");
    let mut buf = String::new();
    write_alias_element_with_context(
        &mut buf,
        &alias,
        &name_map,
        &HashSet::from([1]),
        SketchTransform::identity(),
        None,
    );
    assert!(
        buf.starts_with("10,10,Population,222,317,40,20,8,2,0,3,-1,0,0,0,"),
        "stock ghosts should serialize using Vensim's stock-alias offset: {buf}"
    );
}

// ---- Phase 5 Task 2: Connector serialization (type 1) ----

#[test]
fn sketch_link_straight() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 200));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // Straight => control point (0,0), field 9 = 64 (influence connector)
    assert_eq!(buf, "1,3,1,2,0,0,0,0,0,64,0,-1--1--1,,1|(0,0)|");
}

#[test]
fn sketch_link_with_polarity_symbol() {
    let link = view_element::Link {
        uid: 5,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: Some(LinkPolarity::Positive),
    };
    let positions = HashMap::new();
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // polarity=43 ('+'), field 9 = 64
    assert!(buf.contains(",0,0,43,0,0,64,0,"));
}

#[test]
fn sketch_link_with_polarity_letter() {
    let link = view_element::Link {
        uid: 5,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: Some(LinkPolarity::Positive),
    };
    let positions = HashMap::new();
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, true);
    // polarity=83 ('S' for lettered positive), field 9 = 64
    assert!(buf.contains(",0,0,83,0,0,64,0,"));
}

#[test]
fn sketch_link_arc_produces_nonzero_control_point() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Arc(45.0),
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 100));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // Arc should produce a non-(0,0) control point
    assert!(
        !buf.contains("|(0,0)|"),
        "arc should not produce (0,0) control point"
    );
}

#[test]
fn sketch_link_with_field_hints_preserves_nonsemantic_flags() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: None,
    };
    let positions = HashMap::from([(1, (100, 100)), (2, (200, 116)), (100, (200, 100))]);
    let compat = view_element::LinkSketchCompat {
        uid: 3,
        field4: 1,
        field10: 7,
        from_attached_valve: false,
        to_attached_valve: true,
        control_x: 150.0,
        control_y: 80.0,
        from_x: 100.0,
        from_y: 100.0,
        to_x: 200.0,
        to_y: 100.0,
    };
    let mut buf = String::new();
    write_link_element_with_context(
        &mut buf,
        &link,
        &positions,
        false,
        Some(&compat),
        SketchTransform::identity(),
        None,
    );
    assert_eq!(buf, "1,3,1,2,1,0,0,0,0,64,7,-1--1--1,,1|(0,0)|");
}

#[test]
fn sketch_link_with_field_hints_still_uses_link_geometry() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Arc(45.0),
        polarity: None,
    };
    let positions = HashMap::from([(1, (110, 100)), (2, (210, 100))]);
    let compat = view_element::LinkSketchCompat {
        uid: 3,
        field4: 0,
        field10: 0,
        from_attached_valve: false,
        to_attached_valve: false,
        control_x: 160.0,
        control_y: 70.0,
        from_x: 100.0,
        from_y: 100.0,
        to_x: 200.0,
        to_y: 100.0,
    };
    let mut buf = String::new();
    write_link_element_with_context(
        &mut buf,
        &link,
        &positions,
        false,
        Some(&compat),
        SketchTransform::identity(),
        None,
    );
    let (ctrl_x, ctrl_y) = compute_control_point((110, 100), (210, 100), 45.0);
    assert_eq!(
        buf,
        format!("1,3,1,2,0,0,0,0,0,64,0,-1--1--1,,1|({ctrl_x},{ctrl_y})|")
    );
}

#[test]
fn sketch_link_multipoint_emits_all_points() {
    let points = vec![
        view_element::FlowPoint {
            x: 150.0,
            y: 120.0,
            attached_to_uid: None,
        },
        view_element::FlowPoint {
            x: 170.0,
            y: 140.0,
            attached_to_uid: None,
        },
        view_element::FlowPoint {
            x: 190.0,
            y: 160.0,
            attached_to_uid: None,
        },
    ];
    let link = view_element::Link {
        uid: 4,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::MultiPoint(points),
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 200));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    assert!(
        buf.contains("3|(150,120)|(170,140)|(190,160)|"),
        "multipoint should emit all three points: {buf}"
    );
}

// ---- Phase 5 Task 3: Complete sketch section assembly ----

#[test]
fn sketch_section_structure() {
    let elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "Population".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            compat: None,
        }),
        ViewElement::Aux(view_element::Aux {
            name: "Growth_Rate".to_string(),
            uid: 2,
            x: 200.0,
            y: 200.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Link(view_element::Link {
            uid: 3,
            from_uid: 2,
            to_uid: 1,
            shape: LinkShape::Straight,
            polarity: None,
        }),
    ];
    let sf = datamodel::StockFlow {
        name: None,
        elements,
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    // Header
    assert!(
        output.starts_with("V300  Do not put anything below this section"),
        "should start with V300 header"
    );
    // View title
    assert!(output.contains("*View 1\n"), "should have view title");
    // Font line
    assert!(
        output.contains("$192-192-192"),
        "should have font settings line"
    );
    // Elements
    assert!(
        output.contains("10,1,Population,"),
        "should have stock element"
    );
    assert!(
        output.contains("10,2,Growth Rate,"),
        "should have aux element"
    );
    assert!(output.contains("1,3,2,1,"), "should have link element");
    // Terminator
    assert!(
        output.ends_with("///---\\\\\\\n"),
        "should end with sketch terminator"
    );
}

#[test]
fn sketch_section_in_full_project() {
    let var = make_aux("x", "1", None, "");
    let elements = vec![ViewElement::Aux(view_element::Aux {
        name: "x".to_string(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })];
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: None,
            elements,
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);

    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok());
    let mdl = result.unwrap();

    // The sketch section should appear after the equations terminator
    let terminator_pos = mdl
        .find("\\\\\\---/// Sketch information")
        .expect("should have equations terminator");
    let v300_pos = mdl.find("V300").expect("should have V300 header");
    assert!(
        terminator_pos < v300_pos,
        "V300 should come after equations terminator"
    );

    // The sketch terminator should be at the end
    assert!(
        mdl.contains("///---\\\\\\"),
        "should have sketch terminator"
    );
}

#[test]
fn sketch_roundtrip_teacup() {
    // Read teacup.mdl, parse to Project, write sketch section, verify structure
    let mdl_contents = include_str!("../../../../test/test-models/samples/teacup/teacup.mdl");
    let project =
        crate::mdl::parse_mdl(mdl_contents).expect("teacup.mdl should parse successfully");

    let model = &project.models[0];
    assert!(
        !model.views.is_empty(),
        "teacup model should have at least one view"
    );

    // Write the sketch section
    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&model.views);
    let output = writer.buf;

    // Verify structural elements: the teacup model should have stocks, auxes,
    // flows (valve + attached variable), links, and clouds.
    assert!(output.contains("V300"), "output should contain V300 header");
    assert!(
        output.contains("*View 1"),
        "output should contain view title"
    );
    assert!(
        output.contains("///---\\\\\\"),
        "output should end with sketch terminator"
    );

    // The teacup model elements (after roundtrip through datamodel):
    // Stock: Teacup_Temperature -> type 10 with shape=3
    // Aux: Heat_Loss_to_Room flow -> type 11 valve + type 10 attached
    // Aux: Room_Temperature, Characteristic_Time -> type 10
    // Links -> type 1
    // Clouds -> type 12

    // Count element types in output
    let lines: Vec<&str> = output.lines().collect();
    let type10_count = lines.iter().filter(|l| l.starts_with("10,")).count();
    let type11_count = lines.iter().filter(|l| l.starts_with("11,")).count();
    let type12_count = lines.iter().filter(|l| l.starts_with("12,")).count();
    let type1_count = lines.iter().filter(|l| l.starts_with("1,")).count();

    // Teacup has: 1 stock (Teacup_Temperature), 3 auxes (Heat_Loss_to_Room,
    // Room_Temperature, Characteristic_Time), 1 flow (Heat_Loss_to_Room)
    // which produces valve+variable, plus 1 cloud.
    // The exact numbers depend on the MDL->datamodel conversion, but
    // we should have a reasonable set of elements.
    assert!(
        type10_count >= 2,
        "should have at least 2 type-10 elements (variables/stocks), got {type10_count}"
    );
    assert!(
        type11_count >= 1,
        "should have at least 1 type-11 element (valve), got {type11_count}"
    );
    assert!(
        type12_count >= 1,
        "should have at least 1 type-12 element (cloud/comment), got {type12_count}"
    );
    assert!(
        type1_count >= 1,
        "should have at least 1 type-1 element (connector), got {type1_count}"
    );
    // Verify no empty lines were introduced between elements
    let element_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| {
            l.starts_with("10,")
                || l.starts_with("11,")
                || l.starts_with("12,")
                || l.starts_with("1,")
        })
        .collect();
    assert!(
        !element_lines.is_empty(),
        "should have sketch elements in output"
    );

    // Verify the output can be re-parsed as a valid sketch section
    let reparsed = crate::mdl::view::parse_views(&output);
    assert!(
        reparsed.is_ok(),
        "re-serialized sketch should parse: {:?}",
        reparsed.err()
    );
    let views = reparsed.unwrap();
    assert!(
        !views.is_empty(),
        "re-parsed sketch should have at least one view"
    );

    // Verify all expected element types are present after re-parse
    let view = &views[0];
    let has_variable = view
        .iter()
        .any(|e| matches!(e, crate::mdl::view::VensimElement::Variable(_)));
    let has_connector = view
        .iter()
        .any(|e| matches!(e, crate::mdl::view::VensimElement::Connector(_)));
    assert!(has_variable, "re-parsed view should have variables");
    assert!(has_connector, "re-parsed view should have connectors");
}

#[test]
fn sketch_roundtrip_preserves_view_title() {
    let mdl_contents = r#"x = 5
~ ~|
\\\---/// Sketch information
V300  Do not put anything below this section - it will be ignored
*Overview
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
10,1,x,100,100,40,20,8,3,0,0,-1,0,0,0
///---\\\
"#;

    let project =
        crate::mdl::parse_mdl(mdl_contents).expect("source MDL should parse successfully");
    let mdl = crate::mdl::project_to_mdl(&project).expect("roundtrip MDL write should work");

    assert!(
        mdl.contains("*Overview\r\n"),
        "Roundtrip should preserve original view title: {}",
        mdl
    );
}

#[test]
fn sketch_roundtrip_sanitizes_multiline_view_title() {
    let var = make_aux("x", "5", Some("Units"), "A constant");
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: Some("Overview\r\nMain".to_owned()),
            elements: vec![ViewElement::Aux(view_element::Aux {
                name: "x".to_owned(),
                uid: 1,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            })],
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
    assert!(
        mdl.contains("*Overview Main\r\n"),
        "view title should be serialized as a single line: {mdl}",
    );

    let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
    let View::StockFlow(sf) = &reparsed.models[0].views[0];
    assert_eq!(
        sf.name.as_deref(),
        Some("Overview Main"),
        "sanitized title should roundtrip through MDL",
    );
}

#[test]
fn sketch_roundtrip_preserves_flow_endpoints_with_nonadjacent_valve_uid() {
    let stock_a = Variable::Stock(Stock {
        ident: "stock_a".to_owned(),
        equation: Equation::Scalar("100".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec![],
        outflows: vec!["flow_ab".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let stock_b = Variable::Stock(Stock {
        ident: "stock_b".to_owned(),
        equation: Equation::Scalar("0".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec!["flow_ab".to_owned()],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let flow = Variable::Flow(Flow {
        ident: "flow_ab".to_owned(),
        equation: Equation::Scalar("10".to_owned()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });

    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![stock_a, stock_b, flow],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: Some("View 1".to_owned()),
            elements: vec![
                ViewElement::Stock(view_element::Stock {
                    name: "Stock_A".to_owned(),
                    uid: 1,
                    x: 100.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Stock(view_element::Stock {
                    name: "Stock_B".to_owned(),
                    uid: 2,
                    x: 300.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Flow(view_element::Flow {
                    name: "Flow_AB".to_owned(),
                    uid: 6,
                    x: 200.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    points: vec![
                        view_element::FlowPoint {
                            x: 122.5,
                            y: 100.0,
                            attached_to_uid: Some(1),
                        },
                        view_element::FlowPoint {
                            x: 277.5,
                            y: 100.0,
                            attached_to_uid: Some(2),
                        },
                    ],
                    compat: None,
                    label_compat: None,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
    let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
    let View::StockFlow(sf) = &reparsed.models[0].views[0];

    let stock_uid_by_name: HashMap<&str, i32> = sf
        .elements
        .iter()
        .filter_map(|elem| {
            if let ViewElement::Stock(stock) = elem {
                Some((stock.name.as_str(), stock.uid))
            } else {
                None
            }
        })
        .collect();

    let flow = sf
        .elements
        .iter()
        .find_map(|elem| {
            if let ViewElement::Flow(flow) = elem {
                Some(flow)
            } else {
                None
            }
        })
        .expect("expected flow element after roundtrip");

    assert_eq!(
        flow.points.first().and_then(|pt| pt.attached_to_uid),
        stock_uid_by_name.get("Stock_A").copied(),
        "flow source attachment should roundtrip to Stock_A",
    );
    assert_eq!(
        flow.points.last().and_then(|pt| pt.attached_to_uid),
        stock_uid_by_name.get("Stock_B").copied(),
        "flow sink attachment should roundtrip to Stock_B",
    );
}

#[test]
fn sketch_roundtrip_preserves_causal_links_to_flows_without_sketch_compat() {
    let stock_a = Variable::Stock(Stock {
        ident: "stock_a".to_owned(),
        equation: Equation::Scalar("100".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec![],
        outflows: vec!["flow_ab".to_owned()],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let stock_b = Variable::Stock(Stock {
        ident: "stock_b".to_owned(),
        equation: Equation::Scalar("0".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec!["flow_ab".to_owned()],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let flow = Variable::Flow(Flow {
        ident: "flow_ab".to_owned(),
        equation: Equation::Scalar("driver".to_owned()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let driver = Variable::Aux(Aux {
        ident: "driver".to_owned(),
        equation: Equation::Scalar("1".to_owned()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });

    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![stock_a, stock_b, flow, driver],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: Some("View 1".to_owned()),
            elements: vec![
                ViewElement::Stock(view_element::Stock {
                    name: "Stock_A".to_owned(),
                    uid: 1,
                    x: 100.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Stock(view_element::Stock {
                    name: "Stock_B".to_owned(),
                    uid: 2,
                    x: 300.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Aux(view_element::Aux {
                    name: "Driver".to_owned(),
                    uid: 3,
                    x: 200.0,
                    y: 40.0,
                    label_side: view_element::LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Flow(view_element::Flow {
                    name: "Flow_AB".to_owned(),
                    uid: 6,
                    x: 200.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                    points: vec![
                        view_element::FlowPoint {
                            x: 122.5,
                            y: 100.0,
                            attached_to_uid: Some(1),
                        },
                        view_element::FlowPoint {
                            x: 277.5,
                            y: 100.0,
                            attached_to_uid: Some(2),
                        },
                    ],
                    compat: None,
                    label_compat: None,
                }),
                ViewElement::Link(view_element::Link {
                    uid: 7,
                    from_uid: 3,
                    to_uid: 6,
                    shape: LinkShape::Straight,
                    polarity: Some(LinkPolarity::Positive),
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
    let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
    let View::StockFlow(sf) = &reparsed.models[0].views[0];

    let uid_by_name: HashMap<&str, i32> = sf
        .elements
        .iter()
        .filter_map(|elem| match elem {
            ViewElement::Aux(aux) => Some((aux.name.as_str(), aux.uid)),
            ViewElement::Flow(flow) => Some((flow.name.as_str(), flow.uid)),
            _ => None,
        })
        .collect();

    let link = sf
        .elements
        .iter()
        .find_map(|elem| {
            if let ViewElement::Link(link) = elem {
                Some(link)
            } else {
                None
            }
        })
        .expect("expected link element after roundtrip");

    assert_eq!(
        link.from_uid,
        uid_by_name.get("Driver").copied().expect("driver uid"),
        "causal link source should roundtrip to Driver",
    );
    assert_eq!(
        link.to_uid,
        uid_by_name.get("Flow_AB").copied().expect("flow uid"),
        "causal link target should roundtrip to Flow_AB",
    );
}

#[test]
fn compute_control_point_straight_midpoint() {
    // For a nearly-straight arc angle, the control point should be near the midpoint
    let from = (100, 100);
    let to = (200, 100);
    // Canvas angle of 0 degrees = straight line along x-axis
    let (cx, cy) = compute_control_point(from, to, 0.0);
    // For a straight line, the midpoint should be returned
    assert_eq!(cx, 150);
    assert_eq!(cy, 100);
}

#[test]
fn compute_control_point_arc_off_center() {
    // A 45-degree arc should produce a control point off the midpoint
    let from = (100, 100);
    let to = (200, 100);
    let (_cx, cy) = compute_control_point(from, to, 45.0);
    // The control point should be above or below the line, not on it
    assert_ne!(cy, 100, "arc control point should be off the straight line");
}

// ---- Phase 6 Task 1: Settings section ----

#[test]
fn settings_section_starts_with_marker() {
    let project = make_project(vec![make_model(vec![])]);
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        output.starts_with(":L\x7F<%^E!@\n"),
        "settings section should start with marker (separator is in sketch section), got: {:?}",
        &output[..output.len().min(40)]
    );
}

#[test]
fn settings_section_contains_type_15_euler() {
    let project = make_project(vec![make_model(vec![])]);
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        output.contains("15:0,0,0,0,0,0\n"),
        "Euler method should emit method code 0, got: {:?}",
        output
    );
}

#[test]
fn settings_section_contains_type_15_rk4() {
    let mut project = make_project(vec![make_model(vec![])]);
    project.sim_specs.sim_method = SimMethod::RungeKutta4;
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        output.contains("15:0,0,0,1,0,0\n"),
        "RK4 method should emit method code 1, got: {:?}",
        output
    );
}

#[test]
fn settings_section_contains_type_15_rk2() {
    let mut project = make_project(vec![make_model(vec![])]);
    project.sim_specs.sim_method = SimMethod::RungeKutta2;
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        output.contains("15:0,0,0,3,0,0\n"),
        "RK2 method should emit method code 3, got: {:?}",
        output
    );
}

#[test]
fn settings_section_contains_type_22_units() {
    let mut project = make_project(vec![make_model(vec![])]);
    project.units = vec![
        Unit {
            name: "Dollar".to_owned(),
            equation: Some("$".to_owned()),
            disabled: false,
            aliases: vec!["Dollars".to_owned(), "$s".to_owned()],
        },
        Unit {
            name: "Hour".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["Hours".to_owned()],
        },
    ];
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        output.contains("22:$,Dollar,Dollars,$s\n"),
        "should contain Dollar unit equivalence, got: {:?}",
        output
    );
    assert!(
        output.contains("22:Hour,Hours\n"),
        "should contain Hour unit equivalence, got: {:?}",
        output
    );
}

#[test]
fn settings_section_skips_disabled_units() {
    let mut project = make_project(vec![make_model(vec![])]);
    project.units = vec![Unit {
        name: "Disabled".to_owned(),
        equation: None,
        disabled: true,
        aliases: vec![],
    }];
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    assert!(
        !output.contains("22:Disabled"),
        "disabled units should not appear in output"
    );
}

#[test]
fn settings_section_contains_common_defaults() {
    let project = make_project(vec![make_model(vec![])]);
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    let output = writer.buf;
    // Type 4 (Time), Type 19 (display), Type 24/25/26 (time bounds)
    assert!(output.contains("\n4:Time\n"), "should have Type 4 (Time)");
    assert!(
        output.contains("\n19:"),
        "should have Type 19 (display settings)"
    );
    assert!(
        output.contains("\n24:"),
        "should have Type 24 (initial time)"
    );
    assert!(output.contains("\n25:"), "should have Type 25 (final time)");
    assert!(output.contains("\n26:"), "should have Type 26 (time step)");
}

#[test]
fn settings_roundtrip_integration_method() {
    // Write settings, then parse them back and check integration method
    for method in [
        SimMethod::Euler,
        SimMethod::RungeKutta4,
        SimMethod::RungeKutta2,
    ] {
        let mut project = make_project(vec![make_model(vec![])]);
        project.sim_specs.sim_method = method;
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        // Prepend the separator that write_sketch_section normally emits
        let output = format!("///---\\\\\\\n{}", writer.buf);

        let parser = crate::mdl::settings::PostEquationParser::new(&output);
        let settings = parser.parse_settings();
        assert_eq!(
            settings.integration_method, method,
            "integration method should roundtrip for {:?}",
            method
        );
    }
}

#[test]
fn settings_roundtrip_unit_equivalences() {
    let mut project = make_project(vec![make_model(vec![])]);
    project.units = vec![
        Unit {
            name: "Dollar".to_owned(),
            equation: Some("$".to_owned()),
            disabled: false,
            aliases: vec!["Dollars".to_owned()],
        },
        Unit {
            name: "Hour".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["Hours".to_owned(), "Hr".to_owned()],
        },
    ];
    let mut writer = MdlWriter::new();
    writer.write_settings_section(&project);
    // Prepend the separator that write_sketch_section normally emits
    let output = format!("///---\\\\\\\n{}", writer.buf);

    let parser = crate::mdl::settings::PostEquationParser::new(&output);
    let settings = parser.parse_settings();
    assert_eq!(settings.unit_equivs.len(), 2);
    assert_eq!(settings.unit_equivs[0].name, "Dollar");
    assert_eq!(settings.unit_equivs[0].equation, Some("$".to_string()));
    assert_eq!(settings.unit_equivs[0].aliases, vec!["Dollars"]);
    assert_eq!(settings.unit_equivs[1].name, "Hour");
    assert_eq!(settings.unit_equivs[1].equation, None);
    assert_eq!(settings.unit_equivs[1].aliases, vec!["Hours", "Hr"]);
}

// ---- Phase 6 Task 2: Full file assembly ----

#[test]
fn full_assembly_has_all_three_sections() {
    let var = make_aux("x", "5", Some("Units"), "A constant");
    let elements = vec![ViewElement::Aux(view_element::Aux {
        name: "x".to_string(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })];
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: None,
            elements,
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);

    let result = crate::mdl::project_to_mdl(&project);
    assert!(
        result.is_ok(),
        "project_to_mdl should succeed: {:?}",
        result
    );
    let mdl = result.unwrap();

    // Section 1: Equations -- contains variable entry
    assert!(mdl.contains("x = "), "should contain equation for x");
    // Equations terminator
    assert!(
        mdl.contains("\\\\\\---/// Sketch information"),
        "should have equations terminator"
    );

    // Section 2: Sketch -- V300 header and elements
    assert!(mdl.contains("V300"), "should have V300 sketch header");
    assert!(mdl.contains("*View 1"), "should have view title");

    // Section 3: Settings -- marker and type codes
    assert!(mdl.contains(":L\x7F<%^E!@"), "should have settings marker");
    assert!(mdl.contains("15:"), "should have Type 15 line");

    // Sections should be in order: equations, sketch, settings
    let eq_term = mdl.find("\\\\\\---/// Sketch").unwrap();
    let v300 = mdl.find("V300").unwrap();
    let sketch_term = mdl.find("///---\\\\\\").unwrap();
    let settings_marker = mdl.find(":L\x7F<%^E!@").unwrap();
    assert!(eq_term < v300, "equations should come before sketch");
    assert!(
        v300 < sketch_term,
        "V300 should come before sketch terminator"
    );
    assert!(
        sketch_term < settings_marker,
        "sketch terminator should come before settings marker"
    );
}

// ---- Phase 6 Task 3: compat wrapper ----

#[test]
fn compat_to_mdl_matches_project_to_mdl() {
    let var = make_aux("x", "5", Some("Units"), "A constant");
    let project = make_project(vec![make_model(vec![var])]);

    let direct = crate::mdl::project_to_mdl(&project).unwrap();
    let compat = crate::compat::to_mdl(&project).unwrap();
    assert_eq!(
        direct, compat,
        "compat::to_mdl should produce same result as mdl::project_to_mdl"
    );
}

#[test]
fn write_arrayed_with_default_equation_omits_dimension_level_default() {
    // When default_equation is set (from EXCEPT syntax), the writer must
    // NOT emit name[Dim...]=default because that would apply the default
    // equation to excepted elements that should default to 0.
    let var = datamodel::Variable::Aux(datamodel::Aux {
        ident: "cost".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["region".to_string()],
            vec![
                ("north".to_string(), "base+1".to_string(), None, None),
                ("south".to_string(), "base+2".to_string(), None, None),
            ],
            Some("base".to_string()),
            true,
        ),
        documentation: String::new(),
        units: Some("dollars".to_string()),
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &HashMap::new());

    // Must NOT contain dimension-level default (would apply to excepted elements)
    assert!(
        !buf.contains("cost[region]="),
        "should NOT contain dimension-level default equation, got: {buf}"
    );
    // Individual element equations should be present
    assert!(
        buf.contains("cost[north]="),
        "should contain north element equation, got: {buf}"
    );
    assert!(
        buf.contains("cost[south]="),
        "should contain south element equation, got: {buf}"
    );
}

#[test]
fn write_dimension_with_element_level_mapping() {
    let dim = datamodel::Dimension {
        name: "dim_a".to_string(),
        elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
        mappings: vec![datamodel::DimensionMapping {
            target: "dim_b".to_string(),
            element_map: vec![
                ("a1".to_string(), "b2".to_string()),
                ("a2".to_string(), "b1".to_string()),
            ],
        }],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("-> (dim b: b2, b1)"),
        "element-level mapping must use parenthesized syntax, got: {buf}"
    );
}

#[test]
fn write_dimension_with_multi_target_positional_mapping() {
    let dim = datamodel::Dimension {
        name: "dim_a".to_string(),
        elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
        mappings: vec![
            datamodel::DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "dim_c".to_string(),
                element_map: vec![],
            },
        ],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("dim b") && buf.contains("dim c"),
        "both positional mapping targets should be emitted, got: {buf}"
    );
}

#[test]
fn write_dimension_element_mapping_sorted_by_source_position() {
    // element_map entries out of source order should still emit
    // targets in the dimension's element order for correct positional
    // correspondence on re-import.
    let dim = datamodel::Dimension {
        name: "dim_a".to_string(),
        elements: datamodel::DimensionElements::Named(vec![
            "a1".to_string(),
            "a2".to_string(),
            "a3".to_string(),
        ]),
        mappings: vec![datamodel::DimensionMapping {
            target: "dim_b".to_string(),
            element_map: vec![
                ("a3".to_string(), "b3".to_string()),
                ("a1".to_string(), "b1".to_string()),
                ("a2".to_string(), "b2".to_string()),
            ],
        }],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("-> (dim b: b1, b2, b3)"),
        "targets should be in source element order (a1->b1, a2->b2, a3->b3), got: {buf}"
    );
}

#[test]
fn write_dimension_element_mapping_case_insensitive_lookup() {
    // element_map uses canonical (lowercase) keys, but dim.elements
    // may preserve original casing -- the sort must still work.
    let dim = datamodel::Dimension {
        name: "Region".to_string(),
        elements: datamodel::DimensionElements::Named(vec![
            "North".to_string(),
            "South".to_string(),
            "East".to_string(),
        ]),
        mappings: vec![datamodel::DimensionMapping {
            target: "zone".to_string(),
            element_map: vec![
                ("east".to_string(), "z3".to_string()),
                ("north".to_string(), "z1".to_string()),
                ("south".to_string(), "z2".to_string()),
            ],
        }],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("-> (zone: z1, z2, z3)"),
        "targets should be sorted by source element order despite case mismatch, got: {buf}"
    );
}

#[test]
fn write_dimension_element_mapping_underscored_names() {
    // Element names with underscores must be normalized via to_lower_space()
    // to match the canonical form used in element_map keys.
    let dim = datamodel::Dimension {
        name: "Continent".to_string(),
        elements: datamodel::DimensionElements::Named(vec![
            "North_America".to_string(),
            "South_America".to_string(),
            "Europe".to_string(),
        ]),
        mappings: vec![datamodel::DimensionMapping {
            target: "zone".to_string(),
            element_map: vec![
                ("europe".to_string(), "z3".to_string()),
                ("north america".to_string(), "z1".to_string()),
                ("south america".to_string(), "z2".to_string()),
            ],
        }],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("-> (zone: z1, z2, z3)"),
        "underscore element names should match canonical element_map keys, got: {buf}"
    );
}

#[test]
fn write_dimension_one_to_many_falls_back_to_positional() {
    // When a source element maps to multiple targets (from subdimension
    // expansion), the element-level notation can't round-trip correctly.
    // The writer should fall back to a positional dimension-name mapping.
    let dim = datamodel::Dimension {
        name: "dim_b".to_string(),
        elements: datamodel::DimensionElements::Named(vec!["b1".to_string(), "b2".to_string()]),
        mappings: vec![datamodel::DimensionMapping {
            target: "dim_a".to_string(),
            element_map: vec![
                ("b1".to_string(), "a1".to_string()),
                ("b1".to_string(), "a2".to_string()),
                ("b2".to_string(), "a3".to_string()),
            ],
        }],
        parent: None,
    };

    let mut buf = String::new();
    write_dimension_def(&mut buf, &dim);

    assert!(
        buf.contains("-> dim a") && !buf.contains("(dim a:"),
        "one-to-many mapping should fall back to positional notation, got: {buf}"
    );
}

#[test]
fn write_arrayed_with_default_equation_writes_explicit_elements() {
    let mut buf = String::new();
    write_arrayed_entries(
        &mut buf,
        "g",
        &["DimA".to_string()],
        &[
            ("A1".to_string(), "10".to_string(), None, None),
            ("A2".to_string(), "7".to_string(), None, None),
            ("A3".to_string(), "7".to_string(), None, None),
        ],
        &Some("7".to_string()),
        &None,
        "",
    );
    assert!(
        !buf.contains("g[DimA]"),
        "dimension-level default must not be emitted, got: {buf}"
    );
    assert!(
        !buf.contains(":EXCEPT:"),
        "EXCEPT syntax should not be emitted"
    );
    assert!(
        buf.contains("g[A1]"),
        "A1 entry should be written explicitly, got: {buf}"
    );
    assert!(
        buf.contains("g[A2]") && buf.contains("g[A3]"),
        "all explicit array elements should be written, got: {buf}"
    );
}

#[test]
fn write_arrayed_no_default_writes_all_elements() {
    let mut buf = String::new();
    write_arrayed_entries(
        &mut buf,
        "h",
        &["DimA".to_string()],
        &[
            ("A1".to_string(), "8".to_string(), None, None),
            ("A2".to_string(), "0".to_string(), None, None),
        ],
        &None,
        &None,
        "",
    );
    assert!(
        !buf.contains(":EXCEPT:"),
        "should not emit EXCEPT when no default_equation, got: {buf}"
    );
    assert!(buf.contains("h[A1]"), "should write A1 element, got: {buf}");
    assert!(buf.contains("h[A2]"), "should write A2 element, got: {buf}");
}

#[test]
fn write_arrayed_except_no_exceptions_all_default() {
    let mut buf = String::new();
    write_arrayed_entries(
        &mut buf,
        "k",
        &["DimA".to_string()],
        &[
            ("A1".to_string(), "5".to_string(), None, None),
            ("A2".to_string(), "5".to_string(), None, None),
        ],
        &Some("5".to_string()),
        &None,
        "",
    );
    assert!(
        !buf.contains("k[DimA]"),
        "dimension-level default must not be emitted, got: {buf}"
    );
    assert!(buf.contains("k[A1]"), "should write A1 element, got: {buf}");
    assert!(buf.contains("k[A2]"), "should write A2 element, got: {buf}");
    assert!(
        !buf.contains(":EXCEPT:"),
        "EXCEPT syntax should not be emitted, got: {buf}"
    );
}

#[test]
fn write_arrayed_except_with_omitted_elements_avoids_dimension_default() {
    let mut buf = String::new();
    write_arrayed_entries(
        &mut buf,
        "h",
        &["DimA".to_string()],
        &[("A1".to_string(), "8".to_string(), None, None)],
        &Some("8".to_string()),
        &None,
        "",
    );

    assert!(
        !buf.contains("h[DimA]"),
        "dimension-level default would apply to omitted EXCEPT elements, got: {buf}"
    );
    assert!(
        buf.contains("h[A1]"),
        "explicitly present elements must still be emitted, got: {buf}"
    );
}

#[test]
fn compat_get_direct_equation_does_not_produce_backslash_escapes() {
    let compat = Compat {
        data_source: Some(crate::datamodel::DataSource {
            kind: crate::datamodel::DataSourceKind::Constants,
            file: "data/a.csv".to_string(),
            tab_or_delimiter: ",".to_string(),
            row_or_col: "B2".to_string(),
            cell: String::new(),
        }),
        ..Compat::default()
    };
    let eq = compat_get_direct_equation(&compat).expect("should produce equation");
    assert!(
        !eq.contains("\\'"),
        "writer must not emit backslash-escaped quotes (parser treats ' as toggle): {eq}"
    );
    assert!(
        eq.contains("GET DIRECT CONSTANTS"),
        "should produce GET DIRECT CONSTANTS: {eq}"
    );
}

// ---- Multi-view split tests (Phase 3, Tasks 1-2) ----

fn make_view_aux(name: &str, uid: i32) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: name.to_owned(),
        uid,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })
}

fn make_view_stock(name: &str, uid: i32) -> ViewElement {
    ViewElement::Stock(view_element::Stock {
        name: name.to_owned(),
        uid,
        x: 200.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })
}

fn make_view_flow(name: &str, uid: i32) -> ViewElement {
    ViewElement::Flow(view_element::Flow {
        name: name.to_owned(),
        uid,
        x: 150.0,
        y: 150.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![],
        compat: None,
        label_compat: None,
    })
}

fn make_view_group(name: &str, uid: i32) -> ViewElement {
    ViewElement::Group(view_element::Group {
        uid,
        name: name.to_owned(),
        x: 0.0,
        y: 0.0,
        width: 500.0,
        height: 500.0,
        is_mdl_view_marker: true,
    })
}

fn make_xmile_group(name: &str, uid: i32) -> ViewElement {
    ViewElement::Group(view_element::Group {
        uid,
        name: name.to_owned(),
        x: 0.0,
        y: 0.0,
        width: 500.0,
        height: 500.0,
        is_mdl_view_marker: false,
    })
}

fn make_stock_flow(elements: Vec<ViewElement>) -> StockFlow {
    StockFlow {
        name: None,
        elements,
        view_box: Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    }
}

fn sketch_record_uids_for_view(output: &str, view_title: &str) -> Vec<i32> {
    let marker = format!("*{view_title}\n");
    let start = output.find(&marker).expect("view marker should exist");
    let section = &output[start + marker.len()..];
    let end = section
        .find("\\\\\\---/// Sketch information - do not modify anything except names\n")
        .or_else(|| section.find("///---\\\\\\\n"))
        .expect("view should end at the next sketch boundary");
    let section = &section[..end];

    let mut ids = section
        .lines()
        .filter_map(|line| {
            let record_type = line.split(',').next()?;
            matches!(record_type, "1" | "10" | "11" | "12")
                .then(|| line.split(',').nth(1)?.parse::<i32>().ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

#[test]
fn split_view_no_groups_returns_single_segment() {
    let sf = make_stock_flow(vec![
        make_view_aux("price", 1),
        make_view_stock("inventory", 2),
    ]);
    let segments = split_view_on_groups(&sf);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].0, "View 1");
    assert_eq!(segments[0].1.len(), 2);
}

#[test]
fn split_view_no_groups_uses_stockflow_name() {
    let mut sf = make_stock_flow(vec![make_view_aux("price", 1)]);
    sf.name = Some("My Custom View".to_owned());
    let segments = split_view_on_groups(&sf);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].0, "My Custom View");
}

#[test]
fn write_sketch_section_reapplies_segment_offsets() {
    let sf = StockFlow {
        name: None,
        elements: vec![
            make_view_group("1 housing", 100),
            ViewElement::Aux(view_element::Aux {
                name: "First_Aux".to_string(),
                uid: 1,
                x: 120.0,
                y: 130.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            make_view_group("2 investments", 200),
            ViewElement::Aux(view_element::Aux {
                name: "Second_Aux".to_string(),
                uid: 2,
                x: 340.0,
                y: 470.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
        ],
        view_box: Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: Some(view_element::StockFlowSketchCompat {
            segments: vec![
                view_element::SketchSegmentCompat {
                    x_offset: 20.0,
                    y_offset: 30.0,
                },
                view_element::SketchSegmentCompat {
                    x_offset: 240.0,
                    y_offset: 370.0,
                },
            ],
            flows: vec![],
            links: vec![],
        }),
    };
    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&[View::StockFlow(sf)]);
    let output = writer.buf;

    assert!(
        output.contains("*1 housing"),
        "missing first view header: {output}"
    );
    assert!(
        output.contains("*2 investments"),
        "missing second view header: {output}"
    );
    assert!(
        output.contains("10,1,First Aux,100,100,40,20,8,3,0,0,-1,0,0,0"),
        "first segment should subtract its stored offset: {output}"
    );
    assert!(
        output.contains("10,1,Second Aux,100,100,40,20,8,3,0,0,-1,0,0,0"),
        "second segment should subtract its stored offset: {output}"
    );
}

#[test]
fn write_sketch_section_reassigns_dense_uids_per_view() {
    let sf = StockFlow {
        name: None,
        elements: vec![
            make_view_group("1 housing", 100),
            ViewElement::Stock(view_element::Stock {
                name: "Homes".to_string(),
                uid: 10,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Stock(view_element::Stock {
                name: "Inventory".to_string(),
                uid: 20,
                x: 300.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Flow(view_element::Flow {
                name: "Sales".to_string(),
                uid: 60,
                x: 200.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                points: vec![
                    view_element::FlowPoint {
                        x: 122.5,
                        y: 100.0,
                        attached_to_uid: Some(10),
                    },
                    view_element::FlowPoint {
                        x: 277.5,
                        y: 100.0,
                        attached_to_uid: Some(20),
                    },
                ],
                compat: None,
                label_compat: None,
            }),
            ViewElement::Link(view_element::Link {
                uid: 80,
                from_uid: 10,
                to_uid: 60,
                shape: LinkShape::Straight,
                polarity: Some(LinkPolarity::Positive),
            }),
            make_view_group("2 investments", 200),
            ViewElement::Aux(view_element::Aux {
                name: "Risk".to_string(),
                uid: 300,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Flow(view_element::Flow {
                name: "Funding".to_string(),
                uid: 400,
                x: 200.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                points: vec![],
                compat: None,
                label_compat: None,
            }),
        ],
        view_box: Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&[View::StockFlow(sf)]);
    let output = writer.buf;

    let housing_ids = sketch_record_uids_for_view(&output, "1 housing");
    assert_eq!(housing_ids, vec![1, 2, 3, 4, 5, 6, 7]);

    let investment_ids = sketch_record_uids_for_view(&output, "2 investments");
    assert_eq!(investment_ids, vec![1, 2, 3]);
}

#[test]
fn split_view_two_groups_produces_two_segments() {
    let sf = make_stock_flow(vec![
        make_view_group("1 housing", 100),
        make_view_aux("price", 1),
        make_view_stock("inventory", 2),
        make_view_group("2 investments", 200),
        make_view_aux("rate", 3),
        make_view_flow("capital_flow", 4),
    ]);
    let segments = split_view_on_groups(&sf);
    assert_eq!(segments.len(), 2, "expected 2 segments from 2 groups");
    assert_eq!(segments[0].0, "1 housing");
    assert_eq!(segments[0].1.len(), 2, "first segment: price + inventory");
    assert_eq!(segments[1].0, "2 investments");
    assert_eq!(
        segments[1].1.len(),
        2,
        "second segment: rate + capital_flow"
    );
}

#[test]
fn split_view_elements_partitioned_correctly() {
    let sf = make_stock_flow(vec![
        make_view_group("1 housing", 100),
        make_view_aux("price", 1),
        make_view_stock("inventory", 2),
        make_view_group("2 investments", 200),
        make_view_aux("rate", 3),
    ]);
    let segments = split_view_on_groups(&sf);

    // First segment should contain price and inventory
    let seg1_names: Vec<&str> = segments[0].1.iter().filter_map(|e| e.get_name()).collect();
    assert_eq!(seg1_names, vec!["price", "inventory"]);

    // Second segment should contain rate
    let seg2_names: Vec<&str> = segments[1].1.iter().filter_map(|e| e.get_name()).collect();
    assert_eq!(seg2_names, vec!["rate"]);
}

#[test]
fn split_view_modules_filtered_out() {
    let sf = make_stock_flow(vec![
        make_view_aux("price", 1),
        ViewElement::Module(view_element::Module {
            name: "submodel".to_owned(),
            uid: 99,
            x: 0.0,
            y: 0.0,
            label_side: view_element::LabelSide::Bottom,
        }),
    ]);
    let segments = split_view_on_groups(&sf);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].1.len(), 1, "module should be filtered out");
}

#[test]
fn split_view_preserves_font() {
    let mut sf = make_stock_flow(vec![
        make_view_group("view1", 100),
        make_view_aux("x", 1),
        make_view_group("view2", 200),
        make_view_aux("y", 2),
    ]);
    sf.font = Some("192-192-192,0,Verdana|10||0-0-0".to_owned());
    let segments = split_view_on_groups(&sf);
    for (_, _, font) in &segments {
        assert_eq!(
            font.as_deref(),
            Some("192-192-192,0,Verdana|10||0-0-0"),
            "all segments should share the StockFlow font"
        );
    }
}

#[test]
fn multi_view_mdl_output_contains_view_headers() {
    let sf = make_stock_flow(vec![
        make_view_group("1 housing", 100),
        make_view_aux("price", 1),
        make_view_group("2 investments", 200),
        make_view_aux("rate", 2),
    ]);
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    assert!(
        output.contains("*1 housing"),
        "output should contain first view header: {output}"
    );
    assert!(
        output.contains("*2 investments"),
        "output should contain second view header: {output}"
    );
}

#[test]
fn multi_view_mdl_output_has_separators_between_views() {
    let sf = make_stock_flow(vec![
        make_view_group("view1", 100),
        make_view_aux("a", 1),
        make_view_group("view2", 200),
        make_view_aux("b", 2),
    ]);
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    // The second view should have a V300 header
    let v300_count = output.matches("V300").count();
    assert_eq!(
        v300_count, 2,
        "two views should produce two V300 headers: {output}"
    );
}

#[test]
fn single_view_no_groups_mdl_output() {
    let sf = make_stock_flow(vec![make_view_aux("price", 1)]);
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    assert!(
        output.contains("*View 1"),
        "single view should use default name: {output}"
    );
    let v300_count = output.matches("V300").count();
    assert_eq!(
        v300_count, 1,
        "single view should produce one V300 header: {output}"
    );
}

#[test]
fn multi_view_uses_font_when_present() {
    let mut sf = make_stock_flow(vec![make_view_group("view1", 100), make_view_aux("a", 1)]);
    sf.font = Some(
        "192-192-192,0,Verdana|10||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0".to_owned(),
    );
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    assert!(
        output.contains("$192-192-192,0,Verdana|10||"),
        "should use preserved font: {output}"
    );
    assert!(
        !output.contains("Times New Roman"),
        "should not use default font when custom font present: {output}"
    );
}

#[test]
fn single_view_uses_default_font_when_none() {
    let sf = make_stock_flow(vec![make_view_aux("a", 1)]);
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    assert!(
        output.contains("Times New Roman|12"),
        "should use default font when font is None: {output}"
    );
}

// ---- Task 5: compat dimensions in element output ----

#[test]
fn stock_compat_dimensions_emitted() {
    let stock = view_element::Stock {
        name: "Population".to_string(),
        uid: 2,
        x: 300.0,
        y: 150.0,
        label_side: view_element::LabelSide::Top,
        compat: Some(view_element::ViewElementCompat {
            width: 53.0,
            height: 32.0,
            shape: 3,
            bits: 131,
            name_field: None,
            tail: None,
        }),
    };
    let mut buf = String::new();
    write_stock_element(&mut buf, &stock);
    assert!(
        buf.contains(",53,32,3,131,"),
        "stock with compat should emit preserved dimensions: {buf}"
    );
}

#[test]
fn stock_default_dimensions_without_compat() {
    let stock = view_element::Stock {
        name: "Population".to_string(),
        uid: 2,
        x: 300.0,
        y: 150.0,
        label_side: view_element::LabelSide::Top,
        compat: None,
    };
    let mut buf = String::new();
    write_stock_element(&mut buf, &stock);
    assert!(
        buf.contains(",40,20,3,3,"),
        "stock without compat should use default 40,20,3,3: {buf}"
    );
}

#[test]
fn aux_compat_dimensions_emitted() {
    let aux = view_element::Aux {
        name: "Rate".to_string(),
        uid: 1,
        x: 100.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: Some(view_element::ViewElementCompat {
            width: 45.0,
            height: 18.0,
            shape: 8,
            bits: 131,
            name_field: None,
            tail: None,
        }),
    };
    let mut buf = String::new();
    write_aux_element(&mut buf, &aux);
    assert!(
        buf.contains(",45,18,8,131,"),
        "aux with compat should emit preserved dimensions: {buf}"
    );
}

#[test]
fn aux_default_dimensions_without_compat() {
    let aux = view_element::Aux {
        name: "Rate".to_string(),
        uid: 1,
        x: 100.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut buf = String::new();
    write_aux_element(&mut buf, &aux);
    assert!(
        buf.contains(",40,20,8,3,"),
        "aux without compat should use default 40,20,8,3: {buf}"
    );
}

#[test]
fn flow_valve_compat_dimensions_emitted() {
    let flow = view_element::Flow {
        name: "Birth_Rate".to_string(),
        uid: 6,
        x: 295.0,
        y: 191.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![],
        compat: Some(view_element::ViewElementCompat {
            width: 12.0,
            height: 18.0,
            shape: 34,
            bits: 131,
            name_field: None,
            tail: None,
        }),
        label_compat: Some(view_element::ViewElementCompat {
            width: 55.0,
            height: 14.0,
            shape: 40,
            bits: 35,
            name_field: None,
            tail: None,
        }),
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );
    // Valve line should use flow.compat dimensions
    assert!(
        buf.contains(",12,18,34,131,"),
        "valve with compat should emit preserved dimensions: {buf}"
    );
    // Label line should use flow.label_compat dimensions
    assert!(
        buf.contains(",55,14,40,35,"),
        "flow label with label_compat should emit preserved dimensions: {buf}"
    );
}

#[test]
fn flow_default_dimensions_without_compat() {
    let flow = view_element::Flow {
        name: "Birth_Rate".to_string(),
        uid: 6,
        x: 295.0,
        y: 191.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );
    // Valve line should use default dimensions
    assert!(
        buf.contains(",6,8,34,3,"),
        "valve without compat should use default 6,8,34,3: {buf}"
    );
    // Label line should use default dimensions
    assert!(
        buf.contains(",49,8,40,3,"),
        "flow label without label_compat should use default 49,8,40,3: {buf}"
    );
}

#[test]
fn cloud_compat_dimensions_emitted() {
    let cloud = view_element::Cloud {
        uid: 7,
        flow_uid: 6,
        x: 479.0,
        y: 235.0,
        compat: Some(view_element::ViewElementCompat {
            width: 20.0,
            height: 14.0,
            shape: 0,
            bits: 131,
            name_field: None,
            tail: None,
        }),
    };
    let mut buf = String::new();
    write_cloud_element(&mut buf, &cloud);
    assert!(
        buf.contains(",20,14,0,131,"),
        "cloud with compat should emit preserved dimensions: {buf}"
    );
}

#[test]
fn cloud_default_dimensions_without_compat() {
    let cloud = view_element::Cloud {
        uid: 7,
        flow_uid: 6,
        x: 479.0,
        y: 235.0,
        compat: None,
    };
    let mut buf = String::new();
    write_cloud_element(&mut buf, &cloud);
    assert!(
        buf.contains(",10,8,0,3,"),
        "cloud without compat should use default 10,8,0,3: {buf}"
    );
}

#[test]
fn alias_compat_dimensions_emitted() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: Some(view_element::ViewElementCompat {
            width: 45.0,
            height: 18.0,
            shape: 8,
            bits: 66,
            name_field: None,
            tail: None,
        }),
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Growth_Rate");
    let mut buf = String::new();
    write_alias_element(&mut buf, &alias, &name_map);
    assert!(
        buf.contains(",45,18,8,66,"),
        "alias with compat should emit preserved dimensions: {buf}"
    );
}

#[test]
fn alias_default_dimensions_without_compat() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Growth_Rate");
    let mut buf = String::new();
    write_alias_element(&mut buf, &alias, &name_map);
    assert!(
        buf.contains(",40,20,8,2,"),
        "alias without compat should use default 40,20,8,2: {buf}"
    );
}

// ---- Phase 4 Task 3/4: Equation LHS casing from view element names ----

#[test]
fn build_display_name_map_extracts_view_element_names() {
    let views = vec![View::StockFlow(StockFlow {
        name: None,
        elements: vec![
            ViewElement::Aux(view_element::Aux {
                name: "Endogenous Federal Funds Rate".to_owned(),
                uid: 1,
                x: 0.0,
                y: 0.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Stock(view_element::Stock {
                name: "Population Level".to_owned(),
                uid: 2,
                x: 0.0,
                y: 0.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Flow(view_element::Flow {
                name: "Birth Rate".to_owned(),
                uid: 3,
                x: 0.0,
                y: 0.0,
                label_side: view_element::LabelSide::Bottom,
                points: vec![],
                compat: None,
                label_compat: None,
            }),
        ],
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    let map = build_display_name_map(&views);
    assert_eq!(
        map.get("endogenous_federal_funds_rate").map(|s| s.as_str()),
        Some("Endogenous Federal Funds Rate"),
    );
    assert_eq!(
        map.get("population_level").map(|s| s.as_str()),
        Some("Population Level"),
    );
    assert_eq!(
        map.get("birth_rate").map(|s| s.as_str()),
        Some("Birth Rate"),
    );
}

#[test]
fn build_display_name_map_first_occurrence_wins() {
    // If a name appears in multiple views, the first one wins
    let views = vec![View::StockFlow(StockFlow {
        name: None,
        elements: vec![
            ViewElement::Aux(view_element::Aux {
                name: "Growth Rate".to_owned(),
                uid: 1,
                x: 0.0,
                y: 0.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Aux(view_element::Aux {
                name: "growth rate".to_owned(),
                uid: 5,
                x: 0.0,
                y: 0.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
        ],
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    let map = build_display_name_map(&views);
    // The first element's casing wins
    assert_eq!(
        map.get("growth_rate").map(|s| s.as_str()),
        Some("Growth Rate"),
    );
}

#[test]
fn equation_lhs_uses_view_element_casing() {
    let var = make_aux(
        "endogenous_federal_funds_rate",
        "0.05",
        Some("1/Year"),
        "Rate var",
    );
    let views = vec![View::StockFlow(StockFlow {
        name: None,
        elements: vec![ViewElement::Aux(view_element::Aux {
            name: "Endogenous Federal Funds Rate".to_owned(),
            uid: 1,
            x: 0.0,
            y: 0.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        })],
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    let display_names = build_display_name_map(&views);
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &display_names);
    assert!(
        buf.starts_with("Endogenous Federal Funds Rate = "),
        "LHS should use view element casing, got: {buf}"
    );
}

#[test]
fn equation_lhs_fallback_without_view_element() {
    let var = make_aux("unmatched_variable", "42", None, "");
    let display_names = HashMap::new();
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &display_names);
    assert!(
        buf.starts_with("unmatched variable = "),
        "LHS should fall back to format_mdl_ident when no view element matches, got: {buf}"
    );
}

#[test]
fn equation_lhs_casing_for_stock() {
    let var = Variable::Stock(Stock {
        ident: "population_level".to_owned(),
        equation: Equation::Scalar("1000".to_owned()),
        documentation: String::new(),
        units: None,
        inflows: vec!["births".to_owned()],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut display_names = HashMap::new();
    display_names.insert("population_level".to_owned(), "Population Level".to_owned());
    let mut buf = String::new();
    write_variable_entry(&mut buf, &var, &display_names);
    assert!(
        buf.starts_with("Population Level="),
        "Stock LHS should use view element casing, got: {buf}"
    );
}

#[test]
fn equation_lhs_casing_in_full_project_roundtrip() {
    let var = make_aux("growth_rate", "0.05", Some("1/Year"), "Rate");
    let elements = vec![ViewElement::Aux(view_element::Aux {
        name: "Growth Rate".to_owned(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })];
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: None,
            elements,
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
    };
    let project = make_project(vec![model]);
    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
    assert!(
        mdl.contains("Growth Rate = "),
        "Full project MDL should use view element casing on LHS, got: {mdl}"
    );
}

// ---- Phase 5 Subcomponent C: Variable ordering ----

#[test]
fn ungrouped_variables_sorted_alphabetically() {
    // Variables inserted in non-alphabetical order: c, a, b
    let var_c = make_aux("c_var", "3", None, "");
    let var_a = make_aux("a_var", "1", None, "");
    let var_b = make_aux("b_var", "2", None, "");
    let model = make_model(vec![var_c, var_a, var_b]);
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");

    let pos_a = mdl.find("a var = ").expect("should contain a var");
    let pos_b = mdl.find("b var = ").expect("should contain b var");
    let pos_c = mdl.find("c var = ").expect("should contain c var");
    assert!(
        pos_a < pos_b && pos_b < pos_c,
        "ungrouped variables should appear in alphabetical order: a={pos_a}, b={pos_b}, c={pos_c}"
    );
}

#[test]
fn grouped_variables_retain_group_order() {
    // Group members in a specific order: z, m, a -- should NOT be alphabetized
    let var_z = make_aux("z_rate", "10", None, "");
    let var_m = make_aux("m_rate", "20", None, "");
    let var_a = make_aux("a_rate", "30", None, "");
    let var_ungrouped = make_aux("ungrouped_x", "40", None, "");

    let group = datamodel::ModelGroup {
        name: "My Sector".to_owned(),
        doc: Some("Sector docs".to_owned()),
        parent: None,
        members: vec![
            "z_rate".to_owned(),
            "m_rate".to_owned(),
            "a_rate".to_owned(),
        ],
        run_enabled: false,
    };

    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var_z, var_m, var_a, var_ungrouped],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![group],
    };
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");

    // Grouped vars should appear in group order (z, m, a), not alphabetical
    let pos_z = mdl.find("z rate = ").expect("should contain z rate");
    let pos_m = mdl.find("m rate = ").expect("should contain m rate");
    let pos_a = mdl.find("a rate = ").expect("should contain a rate");
    assert!(
        pos_z < pos_m && pos_m < pos_a,
        "grouped variables should retain group order: z={pos_z}, m={pos_m}, a={pos_a}"
    );

    // Ungrouped variables should come after grouped section
    let pos_ungrouped = mdl
        .find("ungrouped x = ")
        .expect("should contain ungrouped x");
    assert!(
        pos_a < pos_ungrouped,
        "ungrouped variables should come after grouped: a={pos_a}, ungrouped={pos_ungrouped}"
    );
}

#[test]
fn xmile_groups_are_not_split_into_separate_views() {
    let sf = make_stock_flow(vec![
        make_xmile_group("Economic Sector", 100),
        make_view_aux("price", 1),
        make_view_stock("inventory", 2),
        make_xmile_group("Social Sector", 200),
        make_view_aux("population", 3),
    ]);
    let segments = split_view_on_groups(&sf);
    assert_eq!(
        segments.len(),
        1,
        "XMILE groups should not trigger view splitting"
    );
    // All elements including groups should be in the single segment
    assert_eq!(
        segments[0].1.len(),
        5,
        "all elements (including XMILE groups) should be in the segment"
    );
}

#[test]
fn mixed_mdl_markers_and_xmile_groups() {
    let sf = make_stock_flow(vec![
        make_view_group("View 1", 100),    // MDL marker
        make_xmile_group("Sector A", 101), // XMILE org group
        make_view_aux("price", 1),
        make_view_group("View 2", 200), // MDL marker
        make_view_aux("rate", 2),
    ]);
    let segments = split_view_on_groups(&sf);
    assert_eq!(
        segments.len(),
        2,
        "should split on MDL markers only, not XMILE groups"
    );
    // First segment: XMILE group + price
    assert_eq!(segments[0].0, "View 1");
    assert_eq!(segments[0].1.len(), 2, "first segment: xmile_group + price");
    // Second segment: rate
    assert_eq!(segments[1].0, "View 2");
    assert_eq!(segments[1].1.len(), 1, "second segment: rate");
}

#[test]
fn empty_views_produce_valid_sketch_section() {
    let views: Vec<View> = vec![];
    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    assert!(
        output.contains("V300"),
        "empty views should still produce a V300 header: {output}"
    );
    assert!(
        output.contains("///---\\\\\\"),
        "should have terminator: {output}"
    );
}

#[test]
fn lookup_sentinel_constant_used_consistently() {
    assert!(is_lookup_only_equation(LOOKUP_SENTINEL));
    assert!(is_lookup_only_equation(""));
    assert!(is_lookup_only_equation("  0+0  "));
    assert!(!is_lookup_only_equation("TIME"));
    assert!(!is_lookup_only_equation("x + y"));
}
