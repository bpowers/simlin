// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::graph::{CausalGraph, get_variable_dependencies};
use super::indexed::IndexedGraph;
use super::partitions::CyclePartitions;
use super::polarity::{
    analyze_expr_polarity_with_context, analyze_graphical_function_polarity, analyze_link_polarity,
    expr_references_var, flip_polarity, is_negative_constant, is_positive_constant,
};
use super::types::{
    Link, LinkPolarity, Loop, LoopPolarity, ModuleLtmRole, POLARITY_CONFIDENCE_THRESHOLD,
    TruncatedByBudget, classify_module_for_ltm, normalize_module_ref,
};
use crate::ast::BinaryOp;
use crate::common::{Canonical, Ident};
use crate::datamodel::Dimension;
use crate::db::{
    DetectedLoopPolarity, SimlinDb, compute_link_polarities, model_cycle_partitions,
    model_detected_loops, sync_from_datamodel,
};
use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
use crate::variable::Variable;
use std::collections::{HashMap, HashSet};

#[test]
fn test_simple_reinforcing_loop() {
    // Create a simple reinforcing loop: population -> births -> population
    let model = x_model(
        "main",
        vec![
            x_stock("population", "100", &["births"], &[], None),
            x_flow("births", "population * birth_rate", None),
            x_aux("birth_rate", "0.02", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);
    let loops = &detected.loops;
    assert_eq!(loops.len(), 1);

    let loop_item = &loops[0];
    assert!(
        loop_item.variables.contains(&"population".to_string()),
        "Loop should contain population"
    );
    assert_eq!(loop_item.id, "r1");
    assert_eq!(loop_item.polarity, DetectedLoopPolarity::Reinforcing);
}

#[test]
fn test_deterministic_loop_naming() {
    // Create a model with multiple loops to test deterministic naming
    let model = x_model(
        "main",
        vec![
            x_stock("population", "100", &["births"], &["deaths"], None),
            x_flow("births", "population * birth_rate", None),
            x_flow("deaths", "population * death_rate", None),
            x_aux("birth_rate", "0.02", None),
            x_aux("death_rate", "0.01", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project1 = x_project(sim_specs.clone(), std::slice::from_ref(&model));
    let db1 = SimlinDb::default();
    let result1 = sync_from_datamodel(&db1, &datamodel_project1);
    let model1 = result1.models["main"].source;
    let detected1 = model_detected_loops(&db1, model1, result1.project);

    let datamodel_project2 = x_project(sim_specs, &[model]);
    let db2 = SimlinDb::default();
    let result2 = sync_from_datamodel(&db2, &datamodel_project2);
    let model2 = result2.models["main"].source;
    let detected2 = model_detected_loops(&db2, model2, result2.project);

    assert_eq!(detected1.loops.len(), detected2.loops.len());

    for (loop1, loop2) in detected1.loops.iter().zip(detected2.loops.iter()) {
        assert_eq!(loop1.id, loop2.id, "Loop IDs should be deterministic");
        assert_eq!(
            loop1.variables, loop2.variables,
            "Loop variables should be identical"
        );
    }
}

#[test]
fn test_no_loops() {
    // Create a model with no loops
    let model = x_model(
        "main",
        vec![
            x_aux("input", "10", None),
            x_aux("output", "input * 2", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);
    assert!(detected.loops.is_empty());
}

#[test]
fn test_balancing_loop() {
    // Create a balancing loop: goal -> gap -> adjustment -> level -> gap
    // gap = goal - level (negative link from level to gap)
    let model = x_model(
        "main",
        vec![
            x_stock("level", "100", &["adjustment"], &[], None),
            x_flow("adjustment", "gap / adjustment_time", None),
            x_aux("gap", "goal - level", None),
            x_aux("goal", "200", None),
            x_aux("adjustment_time", "5", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert!(!detected.loops.is_empty());

    let has_balancing = detected
        .loops
        .iter()
        .any(|l| l.polarity == DetectedLoopPolarity::Balancing);
    assert!(has_balancing, "Should have detected a balancing loop");
}

#[test]
fn test_module_loops() {
    // Test loop detection with modules
    use crate::testutils::x_module;

    // Create a model that uses a module (like SMOOTH)
    // This simulates a model with a module that might create a feedback loop
    let main_model = x_model(
        "main",
        vec![
            x_stock("inventory", "100", &["production"], &["sales"], None),
            x_flow("production", "desired_production", None),
            x_aux(
                "desired_production",
                "smooth_inventory_gap * adjustment_rate",
                None,
            ),
            x_aux("inventory_gap", "target_inventory - inventory", None),
            x_module(
                "smooth_inventory_gap",
                &[("inventory_gap", "smooth_inventory_gap\u{00B7}input")],
                None,
            ),
            x_aux("target_inventory", "100", None),
            x_aux("adjustment_rate", "0.1", None),
            x_flow("sales", "10", None),
        ],
    );

    // Create the SMOOTH module model (simplified version)
    let smooth_model = x_model(
        "smooth_inventory_gap",
        vec![
            x_aux("input", "0", None),
            x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
            x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
            x_aux("smooth_time", "3", None),
            x_aux("output", "smoothed", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert!(
        !detected.loops.is_empty(),
        "Should find at least one loop through the module"
    );
}

#[test]
fn test_multi_module_loops() {
    // Test loop detection across multiple module instances
    use crate::testutils::x_module;

    // Create a model with two module instances that form a loop together
    let main_model = x_model(
        "main",
        vec![
            x_aux("initial_value", "10", None),
            x_module(
                "processor_a",
                &[("initial_value", "processor_a\u{00B7}input")],
                None,
            ),
            x_aux("intermediate", "processor_a", None),
            x_module(
                "processor_b",
                &[("intermediate", "processor_b\u{00B7}input")],
                None,
            ),
            x_aux("feedback", "processor_b * 0.5", None),
            x_aux("combined", "initial_value + feedback", None),
        ],
    );

    // Create simple processor modules
    let processor_a_model = x_model(
        "processor_a",
        vec![
            x_aux("input", "0", None),
            x_aux("output", "input * 2", None),
        ],
    );

    let processor_b_model = x_model(
        "processor_b",
        vec![
            x_aux("input", "0", None),
            x_aux("output", "input + 1", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(
        sim_specs,
        &[main_model, processor_a_model, processor_b_model],
    );
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    // This model has no feedback loop (initial_value is a constant, no
    // path from output back to input), so no loops should be found.
    assert!(
        detected.loops.is_empty(),
        "Model without feedback should have no loops"
    );
}

#[test]
fn test_link_polarity_detection() {
    // Test polarity detection in simple expressions
    use crate::ast::{Ast, Expr2};

    // Test positive link: y = x * 2
    let x_var = Ident::new("x");
    let expr = Expr2::Op2(
        BinaryOp::Mul,
        Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
        Box::new(Expr2::Const(
            "2".to_string(),
            2.0,
            crate::ast::Loc::default(),
        )),
        None,
        crate::ast::Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    let empty_vars = HashMap::new();
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(polarity, LinkPolarity::Positive);

    // Test negative link: y = -x
    let expr = Expr2::Op2(
        BinaryOp::Sub,
        Box::new(Expr2::Const(
            "0".to_string(),
            0.0,
            crate::ast::Loc::default(),
        )),
        Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
        None,
        crate::ast::Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(polarity, LinkPolarity::Negative);

    // Test negative link via multiplication: y = x * -3
    let expr = Expr2::Op2(
        BinaryOp::Mul,
        Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
        Box::new(Expr2::Const(
            "-3".to_string(),
            -3.0,
            crate::ast::Loc::default(),
        )),
        None,
        crate::ast::Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(polarity, LinkPolarity::Negative);
}

#[test]
fn test_format_path_empty_loop() {
    // Test format_path() with empty links (covers line 44)
    let loop_item = Loop {
        id: "R1".to_string(),
        links: vec![],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
    };

    let path = loop_item.format_path();
    assert_eq!(path, "", "Empty loop should return empty string");
    assert!(path.is_empty(), "Path must be empty for loop with no links");
}

#[test]
fn test_get_variable_dependencies_module() {
    // Test get_variable_dependencies for Module type (covers lines 70-72)
    use crate::variable::ModuleInput;

    let input_var = Ident::new("input_signal");
    let module = Variable::Module {
        ident: Ident::new("processor"),
        model_name: Ident::new("process_model"),
        units: None,
        inputs: vec![
            ModuleInput {
                src: input_var.clone(),
                dst: Ident::new("input"),
            },
            ModuleInput {
                src: Ident::new("control"),
                dst: Ident::new("param"),
            },
        ],
        errors: vec![],
        unit_errors: vec![],
    };

    let deps = get_variable_dependencies(&module);
    assert_eq!(deps.len(), 2, "Module should have 2 dependencies");
    assert!(deps.contains(&input_var), "Should contain input_signal");
    assert!(
        deps.contains(&Ident::new("control")),
        "Should contain control"
    );
}

#[test]
fn test_get_variable_dependencies_no_ast() {
    // Test get_variable_dependencies when AST is None (covers line 83)
    let var = Variable::Var {
        ident: Ident::new("empty_var"),
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

    let deps = get_variable_dependencies(&var);
    assert_eq!(
        deps.len(),
        0,
        "Variable with no AST should have no dependencies"
    );
    assert!(
        deps.is_empty(),
        "Dependencies must be empty for variable without AST"
    );
}

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
fn test_is_positive_constant() {
    // Test is_positive_constant function (covers lines 1058-1062)
    use crate::ast::{Expr2, Loc};

    let pos_const = Expr2::Const("5".to_string(), 5.0, Loc::default());
    assert!(is_positive_constant(&pos_const), "5.0 should be positive");

    let neg_const = Expr2::Const("-5".to_string(), -5.0, Loc::default());
    assert!(
        !is_positive_constant(&neg_const),
        "-5.0 should not be positive"
    );

    let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
    assert!(
        !is_positive_constant(&zero_const),
        "0.0 should not be positive"
    );

    let var_expr = Expr2::Var(Ident::new("x"), None, Loc::default());
    assert!(
        !is_positive_constant(&var_expr),
        "Variable should not be positive constant"
    );
}

#[test]
fn test_is_negative_constant() {
    // Test is_negative_constant function (covers lines 1066-1070)
    use crate::ast::{Expr2, Loc};

    let neg_const = Expr2::Const("-3".to_string(), -3.0, Loc::default());
    assert!(is_negative_constant(&neg_const), "-3.0 should be negative");

    let pos_const = Expr2::Const("3".to_string(), 3.0, Loc::default());
    assert!(
        !is_negative_constant(&pos_const),
        "3.0 should not be negative"
    );

    let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
    assert!(
        !is_negative_constant(&zero_const),
        "0.0 should not be negative"
    );

    let var_expr = Expr2::Var(Ident::new("y"), None, Loc::default());
    assert!(
        !is_negative_constant(&var_expr),
        "Variable should not be negative constant"
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
            Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
            None,
            Loc::default(),
        ),
    );
    elements.insert(
        CanonicalElementName::from_raw("dim2"),
        Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
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
        Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
        Box::new(Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
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
        Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
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
        Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
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
        Box::new(Expr2::Const("100".to_string(), 100.0, Loc::default())),
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
    let direction = Expr2::Const("1".to_string(), 1.0, Loc::default());
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
    let one = || Expr2::Const("1".to_string(), 1.0, Loc::default());

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
    let lit = |n: f64| Expr2::Const(format!("{n}"), n, Loc::default());

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
fn test_lookup_table_polarity_in_links() {
    use crate::datamodel;

    // Create a model with a lookup table
    let mut model_vars = vec![
        x_stock("water", "100", &[], &["outflow"], None),
        x_flow("outflow", "water * lookup(lookup, water)", None),
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

/// A continuous graphical function over x = [0, 1, 2, ...] with the given
/// y-points; used to build per-element GF fixtures for the #502 tests.
#[cfg(test)]
fn continuous_gf(y_points: Vec<f64>) -> crate::datamodel::GraphicalFunction {
    let n = y_points.len();
    let y_min = y_points.iter().copied().fold(f64::INFINITY, f64::min);
    let y_max = y_points.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some((0..n).map(|i| i as f64).collect()),
        y_points,
        x_scale: crate::datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: (n.saturating_sub(1)) as f64,
        },
        y_scale: crate::datamodel::GraphicalFunctionScale {
            min: y_min,
            max: y_max,
        },
    }
}

/// A per-element graphical-function aux: `ident[dim]` whose i-th element has
/// the i-th GF in `element_gfs` (paired with `elements` positionally). Each
/// element's value-equation is the placeholder `"0"` (the GF is what the
/// `LOOKUP` builtin reads, not the value).
#[cfg(test)]
fn per_element_gf_aux(
    ident: &str,
    dim: &str,
    elements: &[&str],
    element_gfs: Vec<crate::datamodel::GraphicalFunction>,
) -> crate::datamodel::Variable {
    let arrayed = elements
        .iter()
        .zip(element_gfs)
        .map(|(elem, gf)| (elem.to_string(), "0".to_string(), None, Some(gf)))
        .collect();
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::Arrayed(vec![dim.to_string()], arrayed, None, false),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

/// An apply-to-all aux `ident[dim] = equation`.
#[cfg(test)]
fn arrayed_aux(ident: &str, dim: &str, equation: &str) -> crate::datamodel::Variable {
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::ApplyToAll(
            vec![dim.to_string()],
            equation.to_string(),
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

/// Build a single-model `datamodel::Project` with one named dimension and the
/// given variables, then sync it and return `compute_link_polarities`'s
/// variable-level `(from, to) -> LinkPolarity` map.
#[cfg(test)]
fn link_polarities_for(
    dim_name: &str,
    elements: &[&str],
    variables: Vec<crate::datamodel::Variable>,
) -> HashMap<(String, String), LinkPolarity> {
    let project = crate::datamodel::Project {
        name: "test".to_string(),
        sim_specs: sim_specs_with_units("months"),
        dimensions: vec![crate::datamodel::Dimension::named(
            dim_name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![x_model("main", variables)],
        source: Default::default(),
        ai_information: None,
    };
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    compute_link_polarities(&db, model, result.project)
}

#[test]
fn test_per_element_gf_link_polarity_agree() {
    // AC7.1: effect[Region] = LOOKUP(curve[Region], dose[Region]) where every
    // region's curve table is monotone increasing -> the dose -> effect link
    // gets Positive (dose's Positive arg polarity composed with the agreeing
    // Positive table polarity), not Unknown.
    let curve = per_element_gf_aux(
        "curve",
        "region",
        &["nyc", "boston"],
        vec![
            continuous_gf(vec![0.0, 1.0, 2.0]),
            continuous_gf(vec![0.0, 2.0, 4.0]),
        ],
    );
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston"],
        vec![
            curve,
            arrayed_aux("dose", "region", "5"),
            arrayed_aux("effect", "region", "lookup(curve[region], dose[region])"),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Positive,
        "per-element GF with monotone-agreeing tables -> Positive link"
    );
}

#[test]
fn test_per_element_gf_link_polarity_disagree() {
    // AC7.2: same model but the regions' tables disagree on direction (NYC
    // increasing, Boston decreasing) -> the per-element fold reports the
    // direction disagreement -> Unknown link polarity.
    let curve = per_element_gf_aux(
        "curve",
        "region",
        &["nyc", "boston"],
        vec![
            continuous_gf(vec![0.0, 1.0, 2.0]),
            continuous_gf(vec![4.0, 2.0, 0.0]),
        ],
    );
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston"],
        vec![
            curve,
            arrayed_aux("dose", "region", "5"),
            arrayed_aux("effect", "region", "lookup(curve[region], dose[region])"),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Unknown,
        "per-element GF with disagreeing table directions -> Unknown link"
    );
}

#[test]
fn test_per_element_gf_link_polarity_fixed_index() {
    // AC7.3: a scalar target indexing one element of the arrayed GF picks that
    // element's specific table. curve[nyc]'s table is decreasing (so the
    // dose -> effect link is Negative); curve[boston]'s is increasing (so the
    // dose -> effect2 link is Positive) -- confirming the FixedIndex resolves
    // the correct element.
    let curve = per_element_gf_aux(
        "curve",
        "region",
        &["nyc", "boston"],
        vec![
            continuous_gf(vec![4.0, 2.0, 0.0]),
            continuous_gf(vec![0.0, 1.0, 2.0]),
        ],
    );
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston"],
        vec![
            curve,
            x_aux("dose", "5", None),
            x_aux("effect", "lookup(curve[nyc], dose)", None),
            x_aux("effect2", "lookup(curve[boston], dose)", None),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Negative,
        "LOOKUP(curve[nyc], dose) -> NYC's decreasing table -> Negative link"
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect2".to_string())],
        LinkPolarity::Positive,
        "LOOKUP(curve[boston], dose) -> Boston's increasing table -> Positive link"
    );
}

#[test]
fn test_per_element_gf_link_polarity_fixed_index_non_sorted_order() {
    // AC8.1 (LTM polarity twin of the per-element-GF mapping fix): the same
    // FixedIndex polarity path as `test_per_element_gf_link_polarity_fixed_index`,
    // but the dimension is declared in NON-alphabetical order (`z_first,
    // a_second`) while the GF `elems` Vec is built in alphabetical order
    // (`a_second, z_first`). `polarity.rs` resolves `curve[z_first]` by
    // `tables.get(dim.get_offset(z_first))` -- get_offset returns z_first's
    // DECLARED index (0), so the table at compiled index 0 MUST be z_first's
    // own. Before the per-element-GF mapping fix, `build_tables` lays tables
    // out by Vec position, so index 0 holds a_second's table and the polarity
    // is silently the wrong element's direction.
    //
    // z_first's table is DECREASING (its link is Negative); a_second's is
    // INCREASING (Positive). A positional layout would swap them.
    let curve = per_element_gf_aux(
        "curve",
        "region",
        // GF Vec built ALPHABETICALLY (a_second, z_first) -- the order the MDL
        // importer's sort produces; differs from the declared order below.
        &["a_second", "z_first"],
        vec![
            continuous_gf(vec![0.0, 1.0, 2.0]), // a_second: increasing
            continuous_gf(vec![4.0, 2.0, 0.0]), // z_first: decreasing
        ],
    );
    let polarities = link_polarities_for(
        "region",
        // Dimension declared NON-alphabetically: z_first (idx 0), a_second (idx 1).
        &["z_first", "a_second"],
        vec![
            curve,
            x_aux("dose", "5", None),
            x_aux("effect_z", "lookup(curve[z_first], dose)", None),
            x_aux("effect_a", "lookup(curve[a_second], dose)", None),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect_z".to_string())],
        LinkPolarity::Negative,
        "LOOKUP(curve[z_first], dose) must resolve z_first's OWN (decreasing) \
         table at its declared index 0 -> Negative; a positional table layout \
         would read a_second's increasing table -> wrong Positive"
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect_a".to_string())],
        LinkPolarity::Positive,
        "LOOKUP(curve[a_second], dose) must resolve a_second's OWN (increasing) \
         table at its declared index 1 -> Positive"
    );
}

#[test]
fn test_per_element_gf_link_polarity_one_nonmonotone_element_ignored() {
    // AC7.5: a per-element GF where one element's table is genuinely
    // non-monotone (Boston: y = [0, 1, 0.3, 0.7] -> Unknown from
    // analyze_graphical_function_polarity) but the rest are Positive. Per the
    // Ast::Arrayed adopt-first-concrete fold semantics that
    // fold_per_element_table_polarity mirrors, an Unknown among concretes is
    // ignored, so the link polarity is Positive (not Unknown). (The "a
    // genuinely non-monotone table still returns Unknown" criterion is about
    // analyze_graphical_function_polarity itself -- the scalar-table case,
    // covered by the #492 task -- not about flipping the whole link.)
    let curve = per_element_gf_aux(
        "curve",
        "region",
        &["nyc", "boston", "la"],
        vec![
            continuous_gf(vec![0.0, 1.0, 2.0]),
            continuous_gf(vec![0.0, 1.0, 0.3, 0.7]),
            continuous_gf(vec![0.0, 3.0, 6.0]),
        ],
    );
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston", "la"],
        vec![
            curve,
            arrayed_aux("dose", "region", "5"),
            arrayed_aux("effect", "region", "lookup(curve[region], dose[region])"),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Positive,
        "one non-monotone element among Positive ones is ignored by the fold -> Positive link"
    );
}

#[test]
fn test_per_element_gf_link_polarity_bare_var_reference() {
    // The "bare A2A written as Var" case: effect[Region] = LOOKUP(curve,
    // dose[Region]) where `curve` is over Region but referenced bare (no
    // subscript). Before this change the Lookup arm used only tables.first()
    // (NYC's table) and would report Positive; aggregating over all element
    // tables sees the NYC-increasing / Boston-decreasing disagreement -> the
    // link polarity is Unknown.
    let curve = per_element_gf_aux(
        "curve",
        "region",
        &["nyc", "boston"],
        vec![
            continuous_gf(vec![0.0, 1.0, 2.0]),
            continuous_gf(vec![4.0, 2.0, 0.0]),
        ],
    );
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston"],
        vec![
            curve,
            arrayed_aux("dose", "region", "5"),
            arrayed_aux("effect", "region", "lookup(curve, dose[region])"),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Unknown,
        "bare per-element-GF reference aggregates all tables -> disagreement -> Unknown"
    );
}

#[test]
fn test_lookup_forward_backward_arm_polarity() {
    // LOOKUP / LOOKUP_FORWARD / LOOKUP_BACKWARD share the
    // `(table_expr, index_expr, loc)` shape and the same monotonicity story,
    // so the Lookup polarity arm covers all three via one `|` pattern. The
    // per-element-GF tests above exercise the `LOOKUP` spelling; this one
    // exercises `lookup_forward` and `lookup_backward` so the merged arm has
    // direct coverage. With both regions' `curve` tables monotone increasing,
    // `dose` enters the lookup-index position as Positive and each table is
    // Positive, so the `dose -> effect` link is Positive (not Unknown).
    for builtin in ["lookup_forward", "lookup_backward"] {
        let curve = per_element_gf_aux(
            "curve",
            "region",
            &["nyc", "boston"],
            vec![
                continuous_gf(vec![0.0, 1.0, 2.0]),
                continuous_gf(vec![0.0, 2.0, 4.0]),
            ],
        );
        let polarities = link_polarities_for(
            "region",
            &["nyc", "boston"],
            vec![
                curve,
                arrayed_aux("dose", "region", "5"),
                arrayed_aux(
                    "effect",
                    "region",
                    &format!("{builtin}(curve[region], dose[region])"),
                ),
            ],
        );
        assert_eq!(
            polarities[&("dose".to_string(), "effect".to_string())],
            LinkPolarity::Positive,
            "{builtin} with monotone-increasing per-element tables -> Positive link",
        );
    }
}

/// Build a minimal `Variable::Var` carrying just the parts the LOOKUP polarity
/// path reads: `tables` (one per element, in element order) and an `ast` whose
/// dimensions `Variable::get_dimensions` reports (an `ApplyToAll` over `dims`).
/// Everything else is the natural default. Used by the focused unit test for
/// `lookup_table_polarity`'s defensive subscript branches.
#[cfg(test)]
fn gf_var_for_test(
    ident: &str,
    dims: Vec<crate::dimensions::Dimension>,
    tables: Vec<crate::variable::Table>,
) -> Variable {
    use crate::ast::{Ast, Expr2, Loc};
    Variable::Var {
        ident: Ident::new(ident),
        ast: Some(Ast::ApplyToAll(
            dims,
            Expr2::Const("0".to_string(), 0.0, Loc::default()),
        )),
        init_ast: None,
        eqn: None,
        units: None,
        tables,
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

#[test]
fn test_lookup_table_polarity_defensive_subscript_branches() {
    // Focused coverage of `lookup_table_polarity`'s defensive subscript
    // branches that the datamodel-fixture tests above don't reach: a
    // `Const` (integer-literal) FixedIndex, a multi-dimensional GF source
    // (the conservative bail), and a `Wildcard` subscript. These are
    // exercised through `analyze_link_polarity` with hand-built `Expr2`
    // ASTs and a minimal var map -- the parser doesn't produce these exact
    // shapes for a well-formed model, but the arm must stay total and
    // classify them correctly (resolvable -> the element's polarity;
    // otherwise -> Unknown).
    use crate::ast::{Ast, Expr2, IndexExpr2, Loc};
    use crate::builtins::BuiltinFn;
    use crate::dimensions::Dimension;
    use crate::variable::Table;
    use LinkPolarity::{Negative, Positive, Unknown};

    let dose = Ident::new("dose");
    let region = Dimension::from(&crate::datamodel::Dimension::named(
        "region".to_string(),
        vec!["nyc".to_string(), "boston".to_string()],
    ));
    let other = Dimension::from(&crate::datamodel::Dimension::named(
        "other".to_string(),
        vec!["a".to_string(), "b".to_string()],
    ));

    // curve[nyc] decreasing, curve[boston] increasing -- one table per element.
    let decreasing = Table::new_for_test(vec![0.0, 1.0, 2.0], vec![4.0, 2.0, 0.0]);
    let increasing = Table::new_for_test(vec![0.0, 1.0, 2.0], vec![0.0, 1.0, 2.0]);

    // `effect = LOOKUP(curve[<idx>], dose)` -- a scalar target. `dose` enters
    // the index position as Positive, so the link polarity equals the polarity
    // of the table `<idx>` resolves to.
    let lookup_curve = |idx: IndexExpr2| -> Ast<Expr2> {
        Ast::Scalar(Expr2::App(
            BuiltinFn::Lookup(
                Box::new(Expr2::Subscript(
                    Ident::new("curve"),
                    vec![idx],
                    None,
                    Loc::default(),
                )),
                Box::new(Expr2::Var(dose.clone(), None, Loc::default())),
                Loc::default(),
            ),
            None,
            Loc::default(),
        ))
    };
    let var = |id: &str| Expr2::Var(Ident::new(id), None, Loc::default());
    let int_const = |n: i64| Expr2::Const(n.to_string(), n as f64, Loc::default());

    // (a) `curve[1]` -- a 1-based integer literal into the single dimension.
    // Picks NYC's (offset 0) decreasing table -> Negative. `curve[2]` picks
    // Boston's increasing table -> Positive, confirming the literal resolves
    // to the right element.
    let one_dim_curve = |tables: Vec<Table>| {
        let mut vars = HashMap::new();
        vars.insert(
            Ident::new("curve"),
            gf_var_for_test("curve", vec![region.clone()], tables),
        );
        vars
    };
    assert_eq!(
        analyze_link_polarity(
            &lookup_curve(IndexExpr2::Expr(int_const(1))),
            &dose,
            &one_dim_curve(vec![decreasing.clone(), increasing.clone()]),
        ),
        Negative,
        "LOOKUP(curve[1], dose) resolves the literal to NYC's decreasing table",
    );
    assert_eq!(
        analyze_link_polarity(
            &lookup_curve(IndexExpr2::Expr(int_const(2))),
            &dose,
            &one_dim_curve(vec![decreasing.clone(), increasing.clone()]),
        ),
        Positive,
        "LOOKUP(curve[2], dose) resolves the literal to Boston's increasing table",
    );
    // An out-of-range literal isn't statically resolvable -> Unknown.
    assert_eq!(
        analyze_link_polarity(
            &lookup_curve(IndexExpr2::Expr(int_const(3))),
            &dose,
            &one_dim_curve(vec![decreasing.clone(), increasing.clone()]),
        ),
        Unknown,
        "LOOKUP(curve[3], dose) -- out of range -> Unknown",
    );

    // (b) A multi-dimensional GF source: resolving a joint table offset would
    // need row-major flattening of the per-element table list, which the LTM
    // polarity cases don't require, so the arm bails to Unknown even when both
    // indices are literal elements that would individually resolve.
    let mut multi_dim_vars = HashMap::new();
    multi_dim_vars.insert(
        Ident::new("curve"),
        gf_var_for_test(
            "curve",
            vec![region.clone(), other.clone()],
            vec![increasing.clone(), increasing.clone()],
        ),
    );
    let multi_dim_lookup = Ast::Scalar(Expr2::App(
        BuiltinFn::Lookup(
            Box::new(Expr2::Subscript(
                Ident::new("curve"),
                vec![IndexExpr2::Expr(var("nyc")), IndexExpr2::Expr(var("a"))],
                None,
                Loc::default(),
            )),
            Box::new(Expr2::Var(dose.clone(), None, Loc::default())),
            Loc::default(),
        ),
        None,
        Loc::default(),
    ));
    assert_eq!(
        analyze_link_polarity(&multi_dim_lookup, &dose, &multi_dim_vars),
        Unknown,
        "a multi-dimensional GF source is the conservative bail -> Unknown",
    );

    // (c) A `Wildcard` subscript can't pick a single element's table
    // statically -> Unknown (even though every element's table is increasing).
    assert_eq!(
        analyze_link_polarity(
            &lookup_curve(IndexExpr2::Wildcard(Loc::default())),
            &dose,
            &one_dim_curve(vec![increasing.clone(), increasing.clone()]),
        ),
        Unknown,
        "LOOKUP(curve[*], dose) -- wildcard subscript -> Unknown",
    );
}

#[test]
fn test_fishbanks_loops() {
    use crate::prost::Message;
    use std::fs;

    let proto_bytes =
        fs::read("../../test/fishbanks.protobin").expect("Failed to read fishbanks.protobin file");
    let project_io = crate::project_io::Project::decode(&proto_bytes[..])
        .expect("Failed to decode fishbanks.protobin");
    let datamodel_project = crate::serde::deserialize(project_io);

    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);

    let model_name = crate::canonicalize(&datamodel_project.models[0].name);
    let model = result.models[model_name.as_ref()].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert_eq!(
        detected.loops.len(),
        3,
        "Fishbanks model should have exactly 3 feedback loops, found: {}",
        detected.loops.len()
    );

    // Find the loop containing harvest_rate and fish_stock
    let harvest_loop = detected
        .loops
        .iter()
        .find(|l| {
            l.variables.contains(&"harvest_rate".to_string())
                && l.variables.contains(&"fish_stock".to_string())
        })
        .expect("Should find loop containing harvest_rate and fish_stock");

    // The loop containing harvest_rate should be Undetermined because some
    // links have unknown polarity (conservative: if ANY link is unknown,
    // the whole loop is Undetermined)
    assert_eq!(
        harvest_loop.polarity,
        DetectedLoopPolarity::Undetermined,
        "Loop containing harvest_rate should be Undetermined (has unknown-polarity links)"
    );

    // Verify per-link polarity separately: harvest_rate -> fish_stock is
    // negative (outflow decreases stock)
    let polarities = compute_link_polarities(&db, model, result.project);
    let harvest_to_stock = polarities
        .get(&("harvest_rate".to_string(), "fish_stock".to_string()))
        .expect("Should have harvest_rate -> fish_stock link");
    assert_eq!(
        *harvest_to_stock,
        LinkPolarity::Negative,
        "harvest_rate -> fish_stock should have negative polarity (outflow decreases stock)"
    );
}

#[test]
fn test_logistic_growth_loops() {
    use crate::prost::Message;
    use std::fs;

    let proto_bytes = fs::read("../../test/logistic-growth.protobin")
        .expect("Failed to read logistic-growth.protobin file");
    let project_io = crate::project_io::Project::decode(&proto_bytes[..])
        .expect("Failed to decode logistic-growth.protobin");
    let datamodel_project = crate::serde::deserialize(project_io);

    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);

    let model_name = crate::canonicalize(&datamodel_project.models[0].name);
    let model = result.models[model_name.as_ref()].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert_eq!(
        detected.loops.len(),
        2,
        "Logistic growth model should have exactly 2 feedback loops, found: {}",
        detected.loops.len()
    );

    let balancing_count = detected
        .loops
        .iter()
        .filter(|l| l.polarity == DetectedLoopPolarity::Balancing)
        .count();
    let undetermined_count = detected
        .loops
        .iter()
        .filter(|l| l.polarity == DetectedLoopPolarity::Undetermined)
        .count();

    assert_eq!(
        balancing_count, 1,
        "Logistic growth model should have exactly 1 balancing loop, found: {}",
        balancing_count
    );
    assert_eq!(
        undetermined_count, 1,
        "Logistic growth model should have exactly 1 undetermined loop, found: {}",
        undetermined_count
    );

    // The carrying capacity loop involves fractional_growth_rate and
    // fraction_of_carrying_capacity_used; it should be balancing
    let carrying_capacity_loop = detected.loops.iter().find(|l| {
        l.variables
            .contains(&"fraction_of_carrying_capacity_used".to_string())
            && l.variables.contains(&"fractional_growth_rate".to_string())
    });

    if let Some(loop_item) = carrying_capacity_loop {
        assert_eq!(
            loop_item.polarity,
            DetectedLoopPolarity::Balancing,
            "The carrying capacity loop should be balancing"
        );
    } else {
        panic!("Could not find the carrying capacity loop in the model");
    }
}

#[test]
fn test_loop_polarity_from_runtime_scores_reinforcing() {
    // All positive scores -> Reinforcing, confidence 1.0
    let scores = vec![f64::NAN, 1.0, 2.0, 3.0, 0.5];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(
        result,
        Some((LoopPolarity::Reinforcing, 1.0)),
        "all-positive valid scores should give a perfectly confident reinforcing classification"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_balancing() {
    // All negative scores -> Balancing, confidence 1.0
    let scores = vec![f64::NAN, -1.0, -2.0, -3.0, -0.5];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(
        result,
        Some((LoopPolarity::Balancing, 1.0)),
        "all-negative valid scores should give a perfectly confident balancing classification"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_mostly_reinforcing() {
    // Mostly positive scores with a tiny negative blip -> MostlyReinforcing.
    // r = 1 + 2 + 3 + 0.5 = 6.5, |b| = 0.01
    // confidence = |6.5 - 0.01| / (6.5 + 0.01) ~= 0.9969 (>= 0.99)
    let scores = vec![f64::NAN, 1.0, 2.0, 3.0, 0.5, -0.01];
    let (polarity, confidence) =
        LoopPolarity::from_runtime_scores(&scores).expect("mixed but valid scores");
    assert_eq!(polarity, LoopPolarity::MostlyReinforcing);
    assert!(
        (POLARITY_CONFIDENCE_THRESHOLD..1.0).contains(&confidence),
        "confidence {confidence} should sit between the threshold and 1.0 for a mostly-R loop"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_mostly_balancing() {
    // Mostly negative scores with a tiny positive blip -> MostlyBalancing.
    let scores = vec![f64::NAN, -1.0, -2.0, -3.0, -0.5, 0.01];
    let (polarity, confidence) =
        LoopPolarity::from_runtime_scores(&scores).expect("mixed but valid scores");
    assert_eq!(polarity, LoopPolarity::MostlyBalancing);
    assert!(
        (POLARITY_CONFIDENCE_THRESHOLD..1.0).contains(&confidence),
        "confidence {confidence} should sit between the threshold and 1.0 for a mostly-B loop"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_undetermined() {
    // Symmetric mix of positive and negative scores -> Undetermined,
    // confidence near 0.0
    let scores = vec![f64::NAN, 1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
    let (polarity, confidence) =
        LoopPolarity::from_runtime_scores(&scores).expect("mixed but valid scores");
    assert_eq!(polarity, LoopPolarity::Undetermined);
    assert!(
        confidence < 1e-9,
        "symmetric mix should produce confidence near zero, got {confidence}"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_dominant_below_threshold_undetermined() {
    // Positive dominates but only weakly -- confidence below 0.99 should
    // fall into Undetermined, not MostlyReinforcing.
    // r = 6, |b| = 4 -> confidence = 0.2
    let scores = vec![f64::NAN, 6.0, -4.0];
    let (polarity, confidence) =
        LoopPolarity::from_runtime_scores(&scores).expect("mixed but valid scores");
    assert_eq!(polarity, LoopPolarity::Undetermined);
    assert!(
        confidence < POLARITY_CONFIDENCE_THRESHOLD,
        "confidence {confidence} below threshold should classify as Undetermined"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_empty() {
    // Empty scores -> None
    let scores: Vec<f64> = vec![];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(result, None);
}

#[test]
fn test_loop_polarity_from_runtime_scores_all_nan() {
    // All NaN scores -> None
    let scores = vec![f64::NAN, f64::NAN, f64::NAN];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(result, None);
}

#[test]
fn test_loop_polarity_from_runtime_scores_all_zero() {
    // All zero scores (after filtering NaN) -> None
    let scores = vec![f64::NAN, 0.0, 0.0, 0.0];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(result, None);
}

#[test]
fn test_loop_polarity_from_runtime_scores_all_infinite() {
    // Pure +Inf / -Inf inputs should be filtered out alongside NaN/zero,
    // leaving no valid scores.  Without the `is_finite()` filter this
    // produced `Inf / Inf = NaN` confidence and a spurious classification.
    let scores = vec![f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(result, None);
}

#[test]
fn test_loop_polarity_from_runtime_scores_inf_with_finite_reinforcing() {
    // A stray Inf must not contaminate `positive_sum`; the finite
    // positive entries should still produce a confident Reinforcing
    // classification.
    let scores = vec![f64::INFINITY, 1.0, 2.0, 3.0];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(
        result,
        Some((LoopPolarity::Reinforcing, 1.0)),
        "Inf must be filtered out so the finite positives drive classification"
    );
}

#[test]
fn test_loop_polarity_from_runtime_scores_neg_inf_with_finite_balancing() {
    // Symmetric coverage: a stray -Inf must not contaminate
    // `negative_sum_abs`; the finite negative entries should still
    // produce a confident Balancing classification.
    let scores = vec![f64::NEG_INFINITY, -1.0, -2.0, -3.0];
    let result = LoopPolarity::from_runtime_scores(&scores);
    assert_eq!(
        result,
        Some((LoopPolarity::Balancing, 1.0)),
        "-Inf must be filtered out so the finite negatives drive classification"
    );
}

#[test]
fn test_loop_polarity_abbreviation() {
    assert_eq!(LoopPolarity::Reinforcing.abbreviation(), "R");
    assert_eq!(LoopPolarity::Balancing.abbreviation(), "B");
    assert_eq!(LoopPolarity::MostlyReinforcing.abbreviation(), "Rux");
    assert_eq!(LoopPolarity::MostlyBalancing.abbreviation(), "Bux");
    assert_eq!(LoopPolarity::Undetermined.abbreviation(), "U");
}

#[test]
fn test_calculate_polarity_all_unknown_links() {
    // When all links have Unknown polarity, the loop should be Undetermined
    let graph = CausalGraph {
        edges: HashMap::new(),
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    let links = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Unknown,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Unknown,
        },
    ];

    let polarity = graph.calculate_polarity(&links);
    assert_eq!(
        polarity,
        LoopPolarity::Undetermined,
        "Loop with all Unknown link polarities should have Undetermined polarity"
    );
}

#[test]
fn test_calculate_polarity_mixed_unknown_and_known() {
    // Conservative approach: if ANY link is unknown, the loop is Undetermined
    let graph = CausalGraph {
        edges: HashMap::new(),
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    // One negative link, one unknown -> should be Undetermined
    let links_one_negative = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Negative,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Unknown,
        },
    ];

    let polarity = graph.calculate_polarity(&links_one_negative);
    assert_eq!(
        polarity,
        LoopPolarity::Undetermined,
        "Loop with any unknown link should be Undetermined"
    );

    // Two positive links, one unknown -> should also be Undetermined
    let links_two_positive = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Positive,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("c"),
            polarity: LinkPolarity::Positive,
        },
        Link {
            from: Ident::new("c"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Unknown,
        },
    ];

    let polarity = graph.calculate_polarity(&links_two_positive);
    assert_eq!(
        polarity,
        LoopPolarity::Undetermined,
        "Loop with any unknown link should be Undetermined"
    );
}

#[test]
fn test_calculate_polarity_all_known_links() {
    // When all links have known polarity, count negative links
    let graph = CausalGraph {
        edges: HashMap::new(),
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    // All positive links -> Reinforcing (even number of negatives: 0)
    let links_all_positive = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Positive,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Positive,
        },
    ];

    let polarity = graph.calculate_polarity(&links_all_positive);
    assert_eq!(
        polarity,
        LoopPolarity::Reinforcing,
        "Loop with all positive links should be Reinforcing"
    );

    // One negative, one positive -> Balancing (odd number of negatives: 1)
    let links_one_negative = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Negative,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Positive,
        },
    ];

    let polarity = graph.calculate_polarity(&links_one_negative);
    assert_eq!(
        polarity,
        LoopPolarity::Balancing,
        "Loop with one negative link should be Balancing"
    );

    // Two negative links -> Reinforcing (even number of negatives: 2)
    let links_two_negatives = vec![
        Link {
            from: Ident::new("a"),
            to: Ident::new("b"),
            polarity: LinkPolarity::Negative,
        },
        Link {
            from: Ident::new("b"),
            to: Ident::new("a"),
            polarity: LinkPolarity::Negative,
        },
    ];

    let polarity = graph.calculate_polarity(&links_two_negatives);
    assert_eq!(
        polarity,
        LoopPolarity::Reinforcing,
        "Loop with two negative links should be Reinforcing"
    );
}

#[test]
fn test_loop_id_assignment_undetermined_polarity() {
    // Loops with Undetermined structural polarity should get "u" prefix
    let graph = CausalGraph {
        edges: HashMap::new(),
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    let mut loops = vec![
        Loop {
            id: String::new(),
            links: vec![
                Link {
                    from: Ident::new("a"),
                    to: Ident::new("b"),
                    polarity: LinkPolarity::Unknown,
                },
                Link {
                    from: Ident::new("b"),
                    to: Ident::new("a"),
                    polarity: LinkPolarity::Unknown,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Undetermined,
            dimensions: vec![],
        },
        Loop {
            id: String::new(),
            links: vec![
                Link {
                    from: Ident::new("x"),
                    to: Ident::new("y"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("y"),
                    to: Ident::new("x"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
        },
    ];

    graph.assign_deterministic_loop_ids(&mut loops);

    // Find the undetermined loop (contains "a" and "b")
    let undetermined_loop = loops
        .iter()
        .find(|l| l.links.iter().any(|link| link.from.as_str() == "a"))
        .expect("Should find undetermined loop");

    assert!(
        undetermined_loop.id.starts_with("u"),
        "Undetermined polarity loop should have 'u' prefix, got: {}",
        undetermined_loop.id
    );

    // Find the reinforcing loop (contains "x" and "y")
    let reinforcing_loop = loops
        .iter()
        .find(|l| l.links.iter().any(|link| link.from.as_str() == "x"))
        .expect("Should find reinforcing loop");

    assert!(
        reinforcing_loop.id.starts_with("r"),
        "Reinforcing polarity loop should have 'r' prefix, got: {}",
        reinforcing_loop.id
    );
}

#[test]
fn test_builtin_polarity_monotone_increasing() {
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
    let empty_vars = HashMap::new();

    // Exp(x), Ln(x), Log10(x), Sqrt(x), Arctan(x), Int(x) all propagate polarity
    let monotone_fns: Vec<(&str, BuiltinFn<Expr2>)> = vec![
        ("Exp", BuiltinFn::Exp(x_expr())),
        ("Ln", BuiltinFn::Ln(x_expr())),
        ("Log10", BuiltinFn::Log10(x_expr())),
        ("Sqrt", BuiltinFn::Sqrt(x_expr())),
        ("Arctan", BuiltinFn::Arctan(x_expr())),
        ("Int", BuiltinFn::Int(x_expr())),
    ];

    for (name, builtin) in monotone_fns {
        let expr = Expr2::App(builtin, None, Loc::default());
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "{name}(x) should propagate positive polarity"
        );
    }
}

#[test]
fn test_builtin_polarity_monotone_negative_inner() {
    use crate::ast::UnaryOp;
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    // -x has Negative polarity
    let neg_x = || {
        Box::new(Expr2::Op1(
            UnaryOp::Negative,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        ))
    };
    let empty_vars = HashMap::new();

    let expr = Expr2::App(BuiltinFn::Exp(neg_x()), None, Loc::default());
    let ast = Ast::Scalar(expr);
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(
        polarity,
        LinkPolarity::Negative,
        "Exp(-x) should have negative polarity"
    );
}

#[test]
fn test_builtin_polarity_non_monotone_returns_unknown() {
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
    let empty_vars = HashMap::new();

    let non_monotone: Vec<(&str, BuiltinFn<Expr2>)> = vec![
        ("Abs", BuiltinFn::Abs(x_expr())),
        ("Sin", BuiltinFn::Sin(x_expr())),
        ("Cos", BuiltinFn::Cos(x_expr())),
        ("Sign", BuiltinFn::Sign(x_expr())),
    ];

    for (name, builtin) in non_monotone {
        let expr = Expr2::App(builtin, None, Loc::default());
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Unknown,
            "{name}(x) should return Unknown polarity"
        );
    }
}

#[test]
fn test_builtin_polarity_max_min_scalar() {
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
    let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
    let empty_vars = HashMap::new();

    // Max(x, 5): only x depends on from_var -> propagate x's polarity
    let expr = Expr2::App(
        BuiltinFn::Max(x_expr(), Some(const_5())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(
        polarity,
        LinkPolarity::Positive,
        "Max(x, 5) should propagate positive polarity"
    );

    // Min(5, x): only x depends on from_var -> propagate x's polarity
    let expr = Expr2::App(
        BuiltinFn::Min(const_5(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
    assert_eq!(
        polarity,
        LinkPolarity::Positive,
        "Min(5, x) should propagate positive polarity"
    );
}

#[test]
fn test_expr_references_var() {
    use crate::ast::{Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let y_var = Ident::new("y");
    let x_expr = || Expr2::Var(x_var.clone(), None, Loc::default());
    let const_5 = || Expr2::Const("5".to_string(), 5.0, Loc::default());

    assert!(!expr_references_var(&const_5(), &x_var));
    assert!(expr_references_var(&x_expr(), &x_var));
    assert!(!expr_references_var(&x_expr(), &y_var));

    // ABS(x) references x
    let abs_x = Expr2::App(BuiltinFn::Abs(Box::new(x_expr())), None, Loc::default());
    assert!(expr_references_var(&abs_x, &x_var));
    assert!(!expr_references_var(&abs_x, &y_var));

    // PULSE(1, 5) doesn't reference x
    let pulse = Expr2::App(
        BuiltinFn::Pulse(Box::new(const_5()), Box::new(const_5()), None),
        None,
        Loc::default(),
    );
    assert!(!expr_references_var(&pulse, &x_var));

    // x + 5 references x
    let add = Expr2::Op2(
        BinaryOp::Add,
        Box::new(x_expr()),
        Box::new(const_5()),
        None,
        Loc::default(),
    );
    assert!(expr_references_var(&add, &x_var));
    assert!(!expr_references_var(&add, &y_var));

    // Subscript: array[x] references x through the index expression
    use crate::ast::IndexExpr2;
    let array_var = Ident::new("array");
    let subscript_with_x = Expr2::Subscript(
        array_var.clone(),
        vec![IndexExpr2::Expr(x_expr())],
        None,
        Loc::default(),
    );
    assert!(
        expr_references_var(&subscript_with_x, &x_var),
        "array[x] should reference x through its index"
    );
    assert!(
        expr_references_var(&subscript_with_x, &array_var),
        "array[x] should reference array as the subscripted variable"
    );
    assert!(
        !expr_references_var(&subscript_with_x, &y_var),
        "array[x] should not reference y"
    );

    // Subscript with range: array[x..5] references x
    let subscript_range = Expr2::Subscript(
        array_var.clone(),
        vec![IndexExpr2::Range(x_expr(), const_5(), Loc::default())],
        None,
        Loc::default(),
    );
    assert!(
        expr_references_var(&subscript_range, &x_var),
        "array[x..5] should reference x through range index"
    );

    // Subscript with constant index: array[5] doesn't reference x
    let subscript_const = Expr2::Subscript(
        array_var.clone(),
        vec![IndexExpr2::Expr(const_5())],
        None,
        Loc::default(),
    );
    assert!(
        !expr_references_var(&subscript_const, &x_var),
        "array[5] should not reference x"
    );
}

#[test]
fn test_max_min_polarity_with_non_monotone_arg() {
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
    let abs_x = || Box::new(Expr2::App(BuiltinFn::Abs(x_expr()), None, Loc::default()));
    let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
    let empty_vars = HashMap::new();

    // MAX(ABS(x), x): ABS(x) is non-monotonically dependent on x,
    // so overall polarity is unknown
    let expr = Expr2::App(
        BuiltinFn::Max(abs_x(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MAX(ABS(x), x) should be Unknown (non-monotonic arg)"
    );

    // MIN(ABS(x), x): same reasoning
    let expr = Expr2::App(
        BuiltinFn::Min(abs_x(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MIN(ABS(x), x) should be Unknown (non-monotonic arg)"
    );

    // MAX(5, x): constant is independent, propagate x's polarity
    let expr = Expr2::App(
        BuiltinFn::Max(const_5(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Positive,
        "MAX(5, x) should be Positive (constant is independent)"
    );

    // MAX(ABS(x), ABS(x)): both non-monotonic
    let expr = Expr2::App(BuiltinFn::Max(abs_x(), Some(abs_x())), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MAX(ABS(x), ABS(x)) should be Unknown"
    );

    // MAX(x, x): both positively dependent → Positive
    let expr = Expr2::App(
        BuiltinFn::Max(x_expr(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Positive,
        "MAX(x, x) should be Positive"
    );

    // MAX(PULSE(1, 5), x): PULSE doesn't depend on x → propagate x's polarity
    let pulse = || {
        Box::new(Expr2::App(
            BuiltinFn::Pulse(const_5(), const_5(), None),
            None,
            Loc::default(),
        ))
    };
    let expr = Expr2::App(
        BuiltinFn::Max(pulse(), Some(x_expr())),
        None,
        Loc::default(),
    );
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Positive,
        "MAX(PULSE(1,5), x) should be Positive (PULSE is independent)"
    );
}

#[test]
fn test_variadic_mean_polarity_with_non_monotone_arg() {
    // Variadic MEAN(a, b, ...) is the scalar form. It must mirror Add's
    // self-reference handling: an arg that returns Unknown *and* references
    // from_var (e.g. ABS(x)) makes the whole mean non-monotone, regardless
    // of order. Without this, MEAN(ABS(x), x) would silently mis-classify
    // as Positive because the second arg overwrites the first arg's
    // Unknown-with-self-reference state.
    use crate::ast::{Ast, Expr2, Loc, UnaryOp};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Expr2::Var(x_var.clone(), None, Loc::default());
    let abs_x = || Expr2::App(BuiltinFn::Abs(Box::new(x_expr())), None, Loc::default());
    let neg_x = || Expr2::Op1(UnaryOp::Negative, Box::new(x_expr()), None, Loc::default());
    let empty_vars = HashMap::new();

    // MEAN(ABS(x), x) -- ABS(x) is non-monotone in x. The mean of a
    // non-monotone term and a monotone term is itself non-monotone.
    let mean_abs_x = Expr2::App(
        BuiltinFn::Mean(vec![abs_x(), x_expr()]),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_link_polarity(&Ast::Scalar(mean_abs_x), &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MEAN(ABS(x), x) should be Unknown (ABS(x) is non-monotone in x)"
    );

    // MEAN(x, ABS(x)) -- order swapped from above; result must still be
    // Unknown. This is the contrapositive of the bug: when ABS(x) is the
    // *second* arg, the original buggy combiner happened to do the right
    // thing because Unknown collapses Unknown; here we make sure it stays
    // Unknown when ABS(x) is the *first* arg.
    let mean_x_abs = Expr2::App(
        BuiltinFn::Mean(vec![x_expr(), abs_x()]),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_link_polarity(&Ast::Scalar(mean_x_abs), &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MEAN(x, ABS(x)) should be Unknown (ABS(x) is non-monotone in x)"
    );

    // MEAN(x, x): both args agree on Positive, mean is Positive.
    let mean_x_x = Expr2::App(
        BuiltinFn::Mean(vec![x_expr(), x_expr()]),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_link_polarity(&Ast::Scalar(mean_x_x), &x_var, &empty_vars),
        LinkPolarity::Positive,
        "MEAN(x, x) should be Positive (both args monotone-positive)"
    );

    // MEAN(x, -x): args have opposite known polarities -> Unknown.
    let mean_x_negx = Expr2::App(
        BuiltinFn::Mean(vec![x_expr(), neg_x()]),
        None,
        Loc::default(),
    );
    assert_eq!(
        analyze_link_polarity(&Ast::Scalar(mean_x_negx), &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "MEAN(x, -x) should be Unknown (mixed polarity)"
    );
}

#[test]
fn test_add_sub_div_polarity_with_non_monotone_arg() {
    use crate::ast::{Ast, Expr2, Loc};
    use crate::builtins::BuiltinFn;

    let x_var = Ident::new("x");
    let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
    let abs_x = || Box::new(Expr2::App(BuiltinFn::Abs(x_expr()), None, Loc::default()));
    let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
    let empty_vars = HashMap::new();

    // x + ABS(x): ABS(x) non-monotonically depends on x → Unknown
    let expr = Expr2::Op2(BinaryOp::Add, x_expr(), abs_x(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "x + ABS(x) should be Unknown"
    );

    // x + 5: 5 is independent → Positive
    let expr = Expr2::Op2(BinaryOp::Add, x_expr(), const_5(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Positive,
        "x + 5 should be Positive"
    );

    // ABS(x) - x: ABS(x) non-monotonically depends on x → Unknown
    let expr = Expr2::Op2(BinaryOp::Sub, abs_x(), x_expr(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "ABS(x) - x should be Unknown"
    );

    // 5 - x: 5 is independent → flip(Positive) = Negative
    let expr = Expr2::Op2(BinaryOp::Sub, const_5(), x_expr(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Negative,
        "5 - x should be Negative"
    );

    // x / ABS(x) = sign(x), non-monotonic → Unknown
    let expr = Expr2::Op2(BinaryOp::Div, x_expr(), abs_x(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Unknown,
        "x / ABS(x) should be Unknown"
    );

    // x / 5: 5 is independent → Positive
    let expr = Expr2::Op2(BinaryOp::Div, x_expr(), const_5(), None, Loc::default());
    let ast = Ast::Scalar(expr);
    assert_eq!(
        analyze_link_polarity(&ast, &x_var, &empty_vars),
        LinkPolarity::Positive,
        "x / 5 should be Positive"
    );
}

#[test]
fn test_all_links() {
    // Create a model with known causal structure:
    // population -> births -> population (reinforcing loop)
    // population -> deaths -> population (balancing loop)
    // birth_rate -> births (external input)
    // death_rate -> deaths (external input)
    let model = x_model(
        "main",
        vec![
            x_stock("population", "100", &["births"], &["deaths"], None),
            x_flow("births", "population * birth_rate", None),
            x_flow("deaths", "population * death_rate", None),
            x_aux("birth_rate", "0.02", None),
            x_aux("death_rate", "0.01", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let polarities = compute_link_polarities(&db, model, result.project);

    // Should have links for:
    // birth_rate -> births
    // births -> population (inflow, positive)
    // death_rate -> deaths
    // deaths -> population (outflow, negative)
    // population -> births (stock to flow)
    // population -> deaths (stock to flow)
    assert_eq!(
        polarities.len(),
        6,
        "Should have exactly 6 causal links, found {}",
        polarities.len()
    );

    // Check specific links exist with correct polarity
    assert_eq!(
        polarities[&("births".to_string(), "population".to_string())],
        LinkPolarity::Positive,
        "Inflow should have positive polarity"
    );
    assert_eq!(
        polarities[&("deaths".to_string(), "population".to_string())],
        LinkPolarity::Negative,
        "Outflow should have negative polarity"
    );

    // Verify deterministic ordering: collect keys, sort, and check sorted
    let mut keys: Vec<_> = polarities.keys().cloned().collect();
    keys.sort();
    for i in 1..keys.len() {
        assert!(
            keys[i - 1] < keys[i],
            "Keys should be sorted: {:?} should come before {:?}",
            keys[i - 1],
            keys[i]
        );
    }
}

#[test]
fn test_normalize_module_ref() {
    // Non-module ref passes through unchanged
    let plain = Ident::new("x");
    assert_eq!(normalize_module_ref(&plain).as_str(), "x");

    // Module·output ref normalized to just the module node
    let module_out = Ident::new("$⁚s⁚0⁚smth1\u{00B7}output");
    assert_eq!(normalize_module_ref(&module_out).as_str(), "$⁚s⁚0⁚smth1");

    // Ident with ⁚ but no · passes through unchanged
    let internal = Ident::new("$⁚ltm⁚link_score⁚x→y");
    assert_eq!(
        normalize_module_ref(&internal).as_str(),
        "$⁚ltm⁚link_score⁚x→y"
    );
}

#[test]
fn test_module_output_dep_normalized() {
    use crate::test_common::TestProject;

    // s = SMTH1(x, 5) creates an implicit module "$⁚s⁚0⁚smth1".
    // s's equation becomes a reference to "$⁚s⁚0⁚smth1·output".
    // After normalization, the edge should go from the module node to s
    // (stripping the ·output suffix).
    let project = TestProject::new("test_mod_norm")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("x", "10", None)
        .aux("s", "SMTH1(x, 5)", None)
        .aux("y", "s * 2", None)
        .compile()
        .expect("should compile");

    let main_ident: Ident<Canonical> = Ident::new("main");
    let model = &project.models[&main_ident];
    let graph = CausalGraph::from_model(model, &project).unwrap();

    let smth1_var = model
        .variables
        .keys()
        .find(|k| k.as_str().contains("smth1"))
        .expect("should have smth1 module variable");

    let s_ident = Ident::new("s");
    let has_module_to_s = graph
        .edges
        .get(smth1_var)
        .map(|targets| targets.contains(&s_ident))
        .unwrap_or(false);

    assert!(
        has_module_to_s,
        "Should have an edge from smth1 module to s (after normalization). \
         Edges: {:?}",
        graph.edges
    );

    // Also verify there's NO phantom "module·output" node in the graph
    let has_phantom = graph.edges.keys().any(|k| k.as_str().contains('\u{00B7}'));
    assert!(
        !has_phantom,
        "Should not have any module·output phantom nodes in edges"
    );
}

#[test]
fn test_module_polarity_through_output_ref() {
    use crate::test_common::TestProject;

    // s = SMTH1(x, 5) creates module ref "$⁚s⁚0⁚smth1·output" in s's AST.
    // The polarity of module -> s should be positive (s = module·output, identity).
    // The polarity of s -> y should be positive (y = s * 2).
    let project = TestProject::new("test_mod_pol")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("x", "10", None)
        .aux("s", "SMTH1(x, 5)", None)
        .aux("y", "s * 2", None)
        .compile()
        .expect("should compile");

    let main_ident: Ident<Canonical> = Ident::new("main");
    let model = &project.models[&main_ident];
    let graph = CausalGraph::from_model(model, &project).unwrap();

    let smth1_var = model
        .variables
        .keys()
        .find(|k| k.as_str().contains("smth1"))
        .expect("should have smth1 module variable");

    let s_ident = Ident::new("s");
    let polarity = graph.get_link_polarity(smth1_var, &s_ident);

    assert_eq!(
        polarity,
        LinkPolarity::Positive,
        "Polarity from smth1 module to s should be positive (s references module·output), got {:?}",
        polarity
    );
}

#[test]
fn test_regression_causal_graph_after_implicit_instantiation() {
    use crate::test_common::TestProject;

    let project = TestProject::new("test_implicit_inst")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("x", "10", None)
        .aux("s", "SMTH1(x, 5)", None)
        .compile()
        .expect("should compile");

    let main_ident: Ident<Canonical> = Ident::new("main");
    let model = &project.models[&main_ident];

    // After compilation, the parent model should have a Module variable for smth1
    let has_module = model
        .variables
        .values()
        .any(|v| matches!(v, Variable::Module { .. }));

    assert!(
        has_module,
        "Parent model should contain a Module variable for the smth1 instance"
    );
}

#[test]
fn test_classify_smth1_as_dynamic() {
    use crate::test_common::TestProject;

    let project = TestProject::new("test_classify_smth1")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("x", "10", None)
        .aux("s", "SMTH1(x, 5)", None)
        .compile()
        .expect("should compile");

    let smth1_ident = Ident::new("stdlib⁚smth1");
    let smth1_model = project
        .models
        .get(&smth1_ident)
        .expect("should have stdlib⁚smth1 model");

    assert_eq!(
        classify_module_for_ltm(smth1_model),
        ModuleLtmRole::DynamicModule
    );
}

// --- Cycle Partition tests ---

#[test]
fn test_cycle_partitions_single_stock_self_loop() {
    let model = x_model(
        "main",
        vec![
            x_stock("stock", "100", &["flow"], &[], None),
            x_flow("flow", "stock * 0.1", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(partitions.partitions.len(), 1);
    assert_eq!(partitions.partitions[0].len(), 1);
    assert_eq!(partitions.partitions[0][0], "stock");
    assert_eq!(partitions.stock_partition["stock"], 0);
}

#[test]
fn test_cycle_partitions_two_independent_stocks() {
    let model = x_model(
        "main",
        vec![
            x_stock("alpha", "50", &["flow_a"], &[], None),
            x_flow("flow_a", "alpha * 0.1", None),
            x_stock("beta", "10", &["flow_b"], &[], None),
            x_flow("flow_b", "beta * 0.2", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(partitions.partitions.len(), 2);
    assert_eq!(partitions.partitions[0], vec!["alpha"]);
    assert_eq!(partitions.partitions[1], vec!["beta"]);
    assert_ne!(
        partitions.stock_partition["alpha"],
        partitions.stock_partition["beta"]
    );
}

#[test]
fn test_cycle_partitions_two_mutually_reachable_stocks() {
    // prey <-> predators through flows
    let model = x_model(
        "main",
        vec![
            x_stock("prey", "100", &["prey_births"], &["prey_deaths"], None),
            x_flow("prey_births", "prey * 0.1", None),
            x_flow("prey_deaths", "prey * predators * 0.01", None),
            x_stock("predators", "10", &["pred_births"], &["pred_deaths"], None),
            x_flow("pred_births", "predators * prey * 0.001", None),
            x_flow("pred_deaths", "predators * 0.05", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(
        partitions.partitions.len(),
        1,
        "Mutually-reachable stocks should be in one partition"
    );
    assert_eq!(partitions.partitions[0].len(), 2);
    assert_eq!(partitions.partitions[0][0], "predators");
    assert_eq!(partitions.partitions[0][1], "prey");
    assert_eq!(
        partitions.stock_partition["prey"],
        partitions.stock_partition["predators"]
    );
}

#[test]
fn test_cycle_partitions_three_stocks_two_partitions() {
    // a <-> b (coupled), c independent
    let model = x_model(
        "main",
        vec![
            x_stock("stock_a", "50", &["flow_ab"], &[], None),
            x_flow("flow_ab", "stock_b * 0.1", None),
            x_stock("stock_b", "30", &["flow_ba"], &[], None),
            x_flow("flow_ba", "stock_a * 0.2", None),
            x_stock("stock_c", "10", &["flow_c"], &[], None),
            x_flow("flow_c", "stock_c * 0.05", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(partitions.partitions.len(), 2);
    let coupled = &partitions.partitions[0];
    let independent = &partitions.partitions[1];
    assert_eq!(coupled.len(), 2);
    assert_eq!(coupled[0], "stock_a");
    assert_eq!(coupled[1], "stock_b");
    assert_eq!(independent.len(), 1);
    assert_eq!(independent[0], "stock_c");
}

#[test]
fn test_cycle_partitions_three_stock_chain_scc() {
    // A -> B -> C -> A: all three mutually reachable through the chain
    let model = x_model(
        "main",
        vec![
            x_stock("stock_a", "50", &["flow_ab"], &[], None),
            x_flow("flow_ab", "stock_c * 0.1", None), // C -> A
            x_stock("stock_b", "30", &["flow_bc"], &[], None),
            x_flow("flow_bc", "stock_a * 0.2", None), // A -> B
            x_stock("stock_c", "10", &["flow_ca"], &[], None),
            x_flow("flow_ca", "stock_b * 0.05", None), // B -> C
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(
        partitions.partitions.len(),
        1,
        "3-stock chain should form one SCC"
    );
    assert_eq!(partitions.partitions[0].len(), 3);
    assert_eq!(partitions.partitions[0][0], "stock_a");
    assert_eq!(partitions.partitions[0][1], "stock_b");
    assert_eq!(partitions.partitions[0][2], "stock_c");
}

#[test]
fn test_cycle_partitions_one_way_path() {
    // a -> b but not b -> a: two separate partitions
    let model = x_model(
        "main",
        vec![
            x_stock("stock_a", "50", &["flow_a"], &[], None),
            x_flow("flow_a", "stock_a * 0.1", None),
            x_stock("stock_b", "30", &["flow_b"], &[], None),
            x_flow("flow_b", "stock_a * 0.2", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;

    let partitions = model_cycle_partitions(&db, model, result.project);
    assert_eq!(
        partitions.partitions.len(),
        2,
        "One-way path should yield two separate partitions"
    );
    assert_ne!(
        partitions.stock_partition["stock_a"],
        partitions.stock_partition["stock_b"]
    );
}

#[test]
fn test_cycle_partitions_determinism() {
    let model = x_model(
        "main",
        vec![
            x_stock("stock_a", "50", &["flow_ab"], &[], None),
            x_flow("flow_ab", "stock_b * 0.1", None),
            x_stock("stock_b", "30", &["flow_ba"], &[], None),
            x_flow("flow_ba", "stock_a * 0.2", None),
            x_stock("stock_c", "10", &["flow_c"], &[], None),
            x_flow("flow_c", "stock_c * 0.05", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs.clone(), std::slice::from_ref(&model));
    let db1 = SimlinDb::default();
    let result1 = sync_from_datamodel(&db1, &datamodel_project);
    let model1 = result1.models["main"].source;
    let p1 = model_cycle_partitions(&db1, model1, result1.project);

    let datamodel_project2 = x_project(sim_specs, std::slice::from_ref(&model));
    let db2 = SimlinDb::default();
    let result2 = sync_from_datamodel(&db2, &datamodel_project2);
    let model2 = result2.models["main"].source;
    let p2 = model_cycle_partitions(&db2, model2, result2.project);

    assert_eq!(p1.partitions.len(), p2.partitions.len());
    for (a, b) in p1.partitions.iter().zip(p2.partitions.iter()) {
        assert_eq!(a, b);
    }
}

#[test]
fn test_cycle_partitions_partition_for_loop() {
    let model = x_model(
        "main",
        vec![
            x_stock("stock_a", "50", &["flow_a"], &[], None),
            x_flow("flow_a", "stock_a * 0.1", None),
            x_stock("stock_b", "10", &["flow_b"], &[], None),
            x_flow("flow_b", "stock_b * 0.2", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let partitions = model_cycle_partitions(&db, model, result.project);

    // stock_a and stock_b should each map to a partition
    assert!(partitions.stock_partition.contains_key("stock_a"));
    assert!(partitions.stock_partition.contains_key("stock_b"));
    assert_ne!(
        partitions.stock_partition["stock_a"],
        partitions.stock_partition["stock_b"]
    );

    // Verify that detected loops reference stocks that map to partitions
    let detected = model_detected_loops(&db, model, result.project);
    assert_eq!(detected.loops.len(), 2);
    for detected_loop in &detected.loops {
        let has_partition = detected_loop
            .variables
            .iter()
            .any(|v| partitions.stock_partition.contains_key(v.as_str()));
        assert!(
            has_partition,
            "Loop {} should have at least one stock in the partition map",
            detected_loop.id
        );
    }
}

// -- `partition_for_loop` per-slot resolution (GH #487) --
//
// `partition_for_loop` returns one `Option<usize>` per conceptual slot:
// a singleton for scalar / cross-element / mixed loops, and one entry per
// element of the dimension space for A2A loops.  These tests build a
// `CyclePartitions` and a `Loop` directly so the per-slot behavior is
// exercised independently of the salsa pipeline.

/// Build a `CyclePartitions` from `(stock_name, partition_index)` pairs.
/// The `partitions` outer Vec is filled out enough to be self-consistent
/// (it isn't read by `partition_for_loop`, only `stock_partition` is).
fn cycle_partitions_from(pairs: &[(&str, usize)]) -> CyclePartitions {
    let stock_partition: HashMap<Ident<Canonical>, usize> = pairs
        .iter()
        .map(|(name, p)| (Ident::new(name), *p))
        .collect();
    let max_p = pairs
        .iter()
        .map(|(_, p)| *p)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let mut partitions: Vec<Vec<Ident<Canonical>>> = vec![Vec::new(); max_p];
    for (name, p) in pairs {
        partitions[*p].push(Ident::new(name));
    }
    CyclePartitions {
        partitions,
        stock_partition,
    }
}

#[test]
fn test_partition_for_loop_scalar_singleton() {
    // A scalar loop's stocks are plain names; result is a singleton.
    let partitions = cycle_partitions_from(&[("stock_a", 0), ("stock_b", 1)]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![Ident::new("stock_a")],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
    };
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &[]),
        vec![Some(0)]
    );
}

#[test]
fn test_partition_for_loop_cross_element_singleton() {
    // A cross-element loop has empty `dimensions` and element-level stocks
    // (it may traverse the same stock variable at several elements);
    // they're all in one SCC, so the result is still a singleton.
    let partitions = cycle_partitions_from(&[("pop[nyc]", 2), ("pop[boston]", 2)]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![Ident::new("pop[nyc]"), Ident::new("pop[boston]")],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
    };
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &[]),
        vec![Some(2)]
    );
}

#[test]
fn test_partition_for_loop_below_parent_graph_is_none() {
    // A loop whose stocks aren't in the partition map (e.g. a pure
    // module-internal loop) resolves to a single `None`.
    let partitions = cycle_partitions_from(&[("stock_a", 0)]);
    let loop_item = Loop {
        id: "u1".to_string(),
        links: vec![],
        stocks: vec![Ident::new("smooth·smoothed")],
        polarity: LoopPolarity::Undetermined,
        dimensions: vec![],
    };
    assert_eq!(partitions.partition_for_loop(&loop_item, &[]), vec![None]);
}

#[test]
fn test_partition_for_loop_a2a_uncoupled_distinct_partitions() {
    // A pure-A2A loop over a 3-element dimension whose three element-stocks
    // sit in three *distinct* partitions (element-wise-uncoupled dynamics):
    // `partition_for_loop` returns three distinct entries, one per slot, in
    // declared-element row-major order.
    let dims = vec![Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
    )];
    let partitions = cycle_partitions_from(&[("pop[nyc]", 5), ("pop[boston]", 7), ("pop[la]", 9)]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![
            Ident::new("pop[nyc]"),
            Ident::new("pop[boston]"),
            Ident::new("pop[la]"),
        ],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["Region".to_string()],
    };
    // Row-major over the dimension's *declared* order (NYC, Boston, LA),
    // which is NOT lexical (Boston < LA < NYC) -- so this also pins that
    // the slot order follows declaration, matching `LoopElementIndex`.
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &dims),
        vec![Some(5), Some(7), Some(9)]
    );
}

#[test]
fn test_partition_for_loop_a2a_coupled_partitions_coincide() {
    // A pure-A2A loop over a 3-element dimension whose element-stocks are
    // all in the *same* partition (element-wise-coupled dynamics, e.g. a
    // shared aggregate couples every element): the three slot entries
    // coincide.
    let dims = vec![Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
    )];
    let partitions = cycle_partitions_from(&[("pop[nyc]", 3), ("pop[boston]", 3), ("pop[la]", 3)]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![
            Ident::new("pop[nyc]"),
            Ident::new("pop[boston]"),
            Ident::new("pop[la]"),
        ],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["Region".to_string()],
    };
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &dims),
        vec![Some(3), Some(3), Some(3)]
    );
}

#[test]
fn test_partition_for_loop_a2a_two_dim_row_major() {
    // A 2-D A2A loop: slot order is row-major (first dim slowest, last dim
    // fastest), matching `LoopElementIndex::resolve` -- so the slots are
    // (NYC,adult), (NYC,child), (Boston,adult), (Boston,child).
    let dims = vec![
        Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        ),
        Dimension::named(
            "Age".to_string(),
            vec!["adult".to_string(), "child".to_string()],
        ),
    ];
    let partitions = cycle_partitions_from(&[
        ("pop[nyc,adult]", 0),
        ("pop[nyc,child]", 1),
        ("pop[boston,adult]", 2),
        ("pop[boston,child]", 3),
    ]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![
            Ident::new("pop[nyc,adult]"),
            Ident::new("pop[nyc,child]"),
            Ident::new("pop[boston,adult]"),
            Ident::new("pop[boston,child]"),
        ],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["Region".to_string(), "Age".to_string()],
    };
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &dims),
        vec![Some(0), Some(1), Some(2), Some(3)]
    );
}

#[test]
fn test_partition_for_loop_a2a_indexed_dimension() {
    // An Indexed A2A dimension: element subscripts are 1-based integers,
    // so the stock node names are `q[1]`, `q[2]`, `q[3]` and the slot
    // order follows 1..=size.
    let dims = vec![Dimension::indexed("Periods".to_string(), 3)];
    let partitions = cycle_partitions_from(&[("q[1]", 4), ("q[2]", 4), ("q[3]", 8)]);
    let loop_item = Loop {
        id: "b1".to_string(),
        links: vec![],
        stocks: vec![Ident::new("q[1]"), Ident::new("q[2]"), Ident::new("q[3]")],
        polarity: LoopPolarity::Balancing,
        dimensions: vec!["Periods".to_string()],
    };
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &dims),
        vec![Some(4), Some(4), Some(8)]
    );
}

#[test]
fn test_partition_for_loop_a2a_unresolved_dim_falls_back_to_present_suffixes() {
    // When the project's dim list doesn't cover the loop's declared
    // dimension (a mid-edit inconsistency), `partition_for_loop` falls
    // back to the slot suffixes actually present on the loop's stocks,
    // sorted for determinism.
    let partitions = cycle_partitions_from(&[("pop[boston]", 1), ("pop[nyc]", 2)]);
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![],
        stocks: vec![Ident::new("pop[nyc]"), Ident::new("pop[boston]")],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec!["Region".to_string()],
    };
    // No `Region` in `dims` -> fall back to sorted suffixes: "boston" < "nyc".
    assert_eq!(
        partitions.partition_for_loop(&loop_item, &[]),
        vec![Some(1), Some(2)]
    );
}

#[test]
fn test_loop_through_module_has_internal_stocks() {
    use crate::testutils::x_module;

    // Parent model: inventory -> production -> desired_production ->
    //   smooth_inventory_gap (module) -> inventory_gap -> inventory
    let main_model = x_model(
        "main",
        vec![
            x_stock("inventory", "100", &["production"], &["sales"], None),
            x_flow("production", "desired_production", None),
            x_aux(
                "desired_production",
                "smooth_inventory_gap * adjustment_rate",
                None,
            ),
            x_aux("inventory_gap", "target_inventory - inventory", None),
            x_module(
                "smooth_inventory_gap",
                &[("inventory_gap", "smooth_inventory_gap\u{00B7}input")],
                None,
            ),
            x_aux("target_inventory", "100", None),
            x_aux("adjustment_rate", "0.1", None),
            x_flow("sales", "10", None),
        ],
    );

    // SMOOTH-like module with an internal stock
    let smooth_model = x_model(
        "smooth_inventory_gap",
        vec![
            x_aux("input", "0", None),
            x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
            x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
            x_aux("smooth_time", "3", None),
            x_aux("output", "smoothed", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert!(
        !detected.loops.is_empty(),
        "Should detect at least one loop through the module"
    );

    // The salsa path treats modules as black-box nodes in the parent
    // graph, so the loop includes the module node and the parent stock
    // but not the module-internal stock name.
    let has_inventory = detected
        .loops
        .iter()
        .any(|l| l.variables.contains(&"inventory".to_string()));
    assert!(
        has_inventory,
        "Should find a loop containing the parent stock 'inventory'. Found: {:?}",
        detected
            .loops
            .iter()
            .map(|l| &l.variables)
            .collect::<Vec<_>>()
    );

    let has_module_node = detected
        .loops
        .iter()
        .any(|l| l.variables.contains(&"smooth_inventory_gap".to_string()));
    assert!(
        has_module_node,
        "Loop should include the module node 'smooth_inventory_gap'. Found: {:?}",
        detected
            .loops
            .iter()
            .map(|l| &l.variables)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_loop_through_modules_with_intermediate_variables() {
    use crate::testutils::x_module;

    // stock -> flow -> module_a -> aux_x -> aux_y -> module_b -> result -> stock
    let main_model = x_model(
        "main",
        vec![
            x_stock("tank", "100", &["inflow"], &[], None),
            x_flow("inflow", "result", None),
            x_module("module_a", &[("tank", "module_a\u{00B7}input")], None),
            x_aux("aux_x", "module_a * 2", None),
            x_aux("aux_y", "aux_x + 1", None),
            x_module("module_b", &[("aux_y", "module_b\u{00B7}input")], None),
            x_aux("result", "module_b * 0.5", None),
        ],
    );

    let module_a_model = x_model(
        "module_a",
        vec![
            x_aux("input", "0", None),
            x_stock("buffer_a", "0", &["fill_a"], &[], None),
            x_flow("fill_a", "(input - buffer_a) / 2", None),
            x_aux("output", "buffer_a", None),
        ],
    );

    let module_b_model = x_model(
        "module_b",
        vec![
            x_aux("input", "0", None),
            x_stock("buffer_b", "0", &["fill_b"], &[], None),
            x_flow("fill_b", "(input - buffer_b) / 3", None),
            x_aux("output", "buffer_b", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[main_model, module_a_model, module_b_model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert!(
        !detected.loops.is_empty(),
        "Should detect a loop through two modules with intermediate variables"
    );

    // The salsa path treats modules as black-box nodes: loop variables
    // include the module names and the parent stock, not sub-model internals.
    let all_vars: HashSet<&str> = detected
        .loops
        .iter()
        .flat_map(|l| l.variables.iter().map(|s| s.as_str()))
        .collect();

    assert!(
        all_vars.contains("tank"),
        "Should include parent stock 'tank'. Found: {all_vars:?}"
    );
    assert!(
        all_vars.contains("module_a"),
        "Should include module_a node. Found: {all_vars:?}"
    );
    assert!(
        all_vars.contains("module_b"),
        "Should include module_b node. Found: {all_vars:?}"
    );
}

#[test]
fn test_loop_through_three_modules() {
    use crate::testutils::x_module;

    // stock -> module_a -> aux1 -> module_b -> aux2 -> module_c -> result -> stock
    let main_model = x_model(
        "main",
        vec![
            x_stock("level", "50", &["adjustment"], &[], None),
            x_flow("adjustment", "output_c", None),
            x_module("module_a", &[("level", "module_a\u{00B7}input")], None),
            x_aux("mid1", "module_a", None),
            x_module("module_b", &[("mid1", "module_b\u{00B7}input")], None),
            x_aux("mid2", "module_b", None),
            x_module("module_c", &[("mid2", "module_c\u{00B7}input")], None),
            x_aux("output_c", "module_c * 0.1", None),
        ],
    );

    let make_module = |name: &str, stock_name: &str| {
        x_model(
            name,
            vec![
                x_aux("input", "0", None),
                x_stock(stock_name, "0", &["fill"], &[], None),
                x_flow("fill", &format!("(input - {stock_name}) / 2"), None),
                x_aux("output", stock_name, None),
            ],
        )
    };

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(
        sim_specs,
        &[
            main_model,
            make_module("module_a", "buf_a"),
            make_module("module_b", "buf_b"),
            make_module("module_c", "buf_c"),
        ],
    );
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    assert!(
        !detected.loops.is_empty(),
        "Should detect a loop through three modules"
    );

    // The salsa path treats modules as black-box nodes: loop variables
    // include the module names and the parent stock.
    let all_vars: HashSet<&str> = detected
        .loops
        .iter()
        .flat_map(|l| l.variables.iter().map(|s| s.as_str()))
        .collect();

    assert!(
        all_vars.contains("level"),
        "Should include parent stock. Found: {all_vars:?}"
    );
    assert!(
        all_vars.contains("module_a"),
        "Should include module_a node. Found: {all_vars:?}"
    );
    assert!(
        all_vars.contains("module_b"),
        "Should include module_b node. Found: {all_vars:?}"
    );
    assert!(
        all_vars.contains("module_c"),
        "Should include module_c node. Found: {all_vars:?}"
    );
}

#[test]
fn test_internal_module_loops_not_in_parent() {
    use crate::testutils::x_module;

    // A model with a module that has internal feedback,
    // but no feedback loop in the parent model.
    let main_model = x_model(
        "main",
        vec![
            x_aux("input_signal", "10", None),
            x_module(
                "smoother",
                &[("input_signal", "smoother\u{00B7}input")],
                None,
            ),
            x_aux("result", "smoother", None),
        ],
    );

    // Module with internal feedback: smoothed -> change_in_smooth -> smoothed
    let smooth_model = x_model(
        "smoother",
        vec![
            x_aux("input", "0", None),
            x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
            x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
            x_aux("smooth_time", "3", None),
            x_aux("output", "smoothed", None),
        ],
    );

    let sim_specs = sim_specs_with_units("years");
    let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &datamodel_project);
    let model = result.models["main"].source;
    let detected = model_detected_loops(&db, model, result.project);

    // No feedback loop exists in the parent model (no path from result back
    // to input_signal). The module's INTERNAL feedback loop should NOT be
    // reported at the parent level.
    assert!(
        detected.loops.is_empty(),
        "Internal module loops should not appear in parent. Found: {:?}",
        detected
            .loops
            .iter()
            .map(|l| &l.variables)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_enumerate_pathways_to_outputs_non_standard_output() {
    // Module graph with output named "result" instead of "output"
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    edges.insert(Ident::new("input_val"), vec![Ident::new("intermediate")]);
    edges.insert(Ident::new("intermediate"), vec![Ident::new("result")]);

    let graph = CausalGraph {
        edges,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    // enumerate_module_pathways with hard-coded "output" finds nothing
    let pathways_old = graph.enumerate_module_pathways(&Ident::new("output"));
    assert!(
        pathways_old.is_empty(),
        "Hard-coded 'output' should find no pathways when output is named 'result'"
    );

    // With explicit output ports, pathways are found correctly
    let pathways = graph.enumerate_pathways_to_outputs(&[Ident::new("result")]);
    assert!(
        !pathways.is_empty(),
        "Explicit output port should find pathways to 'result'"
    );
    assert!(
        pathways.contains_key(&Ident::new("input_val")),
        "Should find pathway from input_val to the sink"
    );

    // Auto-detection also works: "result" is a sink (no outgoing edges)
    let pathways_auto = graph.enumerate_pathways_to_outputs(&[]);
    assert!(
        !pathways_auto.is_empty(),
        "Auto-detected sink should find pathways to 'result'"
    );
}

#[test]
fn test_enumerate_pathways_to_outputs_standard_output() {
    // Module graph with output named "output" (standard case)
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    edges.insert(Ident::new("input"), vec![Ident::new("output")]);

    let graph = CausalGraph {
        edges,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    // Auto-detection: "output" node is a sink, so it's found automatically
    let pathways = graph.enumerate_pathways_to_outputs(&[]);
    assert!(
        !pathways.is_empty(),
        "Should find pathways with standard 'output' name"
    );
    assert!(pathways.contains_key(&Ident::new("input")));
}

#[test]
fn test_enumerate_module_pathways_deeper_than_legacy_cap() {
    // Build a strictly-linear module graph deeper than the legacy
    // hard-coded depth=20 truncation: input -> n0 -> n1 -> ... -> n19
    // -> output, which is 22 distinct nodes and 21 edges.  A simple
    // path can visit at most N distinct nodes (the visited set
    // already enforces that), so the principled DFS cap is the
    // module-graph node count.  A 21-element chain is enough to
    // prove the old cap is gone without blowing past the per-test
    // time budget.
    const INTERMEDIATE_NODES: usize = 20;

    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();

    // input -> n0
    edges.insert(Ident::new("input"), vec![Ident::new("n0")]);
    // n_i -> n_{i+1}
    for i in 0..INTERMEDIATE_NODES - 1 {
        edges.insert(
            Ident::new(&format!("n{i}")),
            vec![Ident::new(&format!("n{}", i + 1))],
        );
    }
    // n_{LAST} -> output
    edges.insert(
        Ident::new(&format!("n{}", INTERMEDIATE_NODES - 1)),
        vec![Ident::new("output")],
    );

    let graph = CausalGraph {
        edges,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    let pathways = graph.enumerate_module_pathways(&Ident::new("output"));
    let from_input = pathways.get(&Ident::new("input")).expect(
        "input -> output pathway should be enumerated regardless of \
         chain depth; a hardcoded depth-20 cap silently drops chains \
         this long",
    );
    assert_eq!(
        from_input.len(),
        1,
        "expected exactly one simple input -> output pathway"
    );
    // 22 nodes -> 21 directed links along the chain.
    assert_eq!(
        from_input[0].len(),
        INTERMEDIATE_NODES + 1,
        "pathway should traverse every link in the chain"
    );
}

/// Construct a three-node reinforcing cycle (A → B → C → A) for
/// budget-exhaustion tests.  Tiny by design: the graph has exactly one
/// elementary circuit, so a budget of zero MUST trip and a budget of
/// one MUST succeed.
fn tiny_cycle_graph() -> CausalGraph {
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    edges.insert(Ident::new("a"), vec![Ident::new("b")]);
    edges.insert(Ident::new("b"), vec![Ident::new("c")]);
    edges.insert(Ident::new("c"), vec![Ident::new("a")]);
    CausalGraph {
        edges,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    }
}

#[test]
fn find_circuit_node_lists_bails_out_past_budget() {
    let graph = tiny_cycle_graph();
    // Budget of zero: the very first circuit push should trip the
    // bail-out and return TruncatedByBudget.
    let err = graph
        .find_circuit_node_lists_with_limit(0)
        .expect_err("budget of 0 must bail");
    assert_eq!(err, TruncatedByBudget);
}

#[test]
fn find_circuit_node_lists_succeeds_within_budget() {
    let graph = tiny_cycle_graph();
    let circuits = graph
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("one-circuit graph must not exhaust the budget");
    assert_eq!(
        circuits.len(),
        1,
        "a three-node directed cycle has exactly one elementary circuit"
    );
}

#[test]
fn find_loops_bails_out_past_budget() {
    let graph = tiny_cycle_graph();
    // A budget of 0 is the smallest value that must trigger bail-out:
    // the first circuit push fails the budget check.  Production code
    // passes usize::MAX (no truncation), but callers that want to
    // observe the signal still use `find_loops_with_limit` directly.
    let err = graph
        .find_loops_with_limit(0)
        .expect_err("budget of 0 must bail");
    assert_eq!(err, TruncatedByBudget);
}

// --- IndexedGraph + SCC-restricted enumeration tests ---
//
// These validate the refactor from `HashSet<Ident>` to integer-indexed
// DFS.  The goal is to pin down the invariants the public contract
// depends on so later optimization passes can't silently break them:
// (1) the node-ordering round-trip, (2) SCC decomposition behavior,
// (3) self-loop exclusion at length 1, and (4) budget semantics.

fn build_causal_graph(edges: &[(&str, &[&str])]) -> CausalGraph {
    let mut map: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    for (from, tos) in edges {
        let from_id = Ident::new(from);
        map.entry(from_id)
            .or_default()
            .extend(tos.iter().map(|t| Ident::new(t)));
    }
    CausalGraph {
        edges: map,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    }
}

fn circuits_as_sorted_name_sets(circuits: &[Vec<Ident<Canonical>>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = circuits
        .iter()
        .map(|c| {
            let mut names: Vec<String> = c.iter().map(|n| n.as_str().to_string()).collect();
            names.sort();
            names
        })
        .collect();
    out.sort();
    out
}

#[test]
fn indexed_graph_empty_round_trip() {
    let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    let graph = IndexedGraph::from_edges(&edges);
    assert_eq!(graph.len(), 0);
    assert!(graph.nodes.is_empty());
    assert!(graph.succ.is_empty());
    assert!(graph.node_to_idx.is_empty());

    let cg = CausalGraph {
        edges,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("empty graph must not trip the budget");
    assert!(circuits.is_empty(), "empty graph has no circuits");
}

#[test]
fn indexed_graph_two_node_back_edge() {
    // A <-> B: one elementary circuit A -> B -> A.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
    let graph = IndexedGraph::from_edges(&cg.edges);

    // Nodes must be sorted lex so small-start invariant matches index ordering.
    assert_eq!(
        graph.nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    // Round-trip: every node's index resolves back to the same ident.
    for (i, n) in graph.nodes.iter().enumerate() {
        assert_eq!(graph.node_to_idx[n], i as u32);
    }

    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("small graph must not exhaust budget");
    assert_eq!(
        circuits_as_sorted_name_sets(&circuits),
        vec![vec!["a".to_string(), "b".to_string()]]
    );
}

#[test]
fn indexed_graph_three_node_cycle() {
    // A -> B -> C -> A: exactly one elementary circuit.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("tiny cycle must not exhaust budget");
    assert_eq!(
        circuits_as_sorted_name_sets(&circuits),
        vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]
    );
}

#[test]
fn indexed_graph_two_disjoint_three_cycles() {
    // Two completely disjoint cycles: {a,b,c} and {x,y,z}.
    // Each forms its own SCC so we must find exactly two circuits.
    let cg = build_causal_graph(&[
        ("a", &["b"]),
        ("b", &["c"]),
        ("c", &["a"]),
        ("x", &["y"]),
        ("y", &["z"]),
        ("z", &["x"]),
    ]);
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("small disjoint graphs must not exhaust budget");
    let sorted = circuits_as_sorted_name_sets(&circuits);
    assert_eq!(
        sorted,
        vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
        ]
    );
}

#[test]
fn indexed_graph_two_cycle_and_self_loop_node() {
    // A <-> B (one 2-cycle), and separately S -> S (self-loop).
    // Pure self-loops are intentionally excluded (circuit.len() > 1),
    // so only the A<->B cycle is returned.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"]), ("s", &["s"])]);
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("small graph must not exhaust budget");
    assert_eq!(
        circuits_as_sorted_name_sets(&circuits),
        vec![vec!["a".to_string(), "b".to_string()]]
    );
}

#[test]
fn indexed_graph_scc_pure_dag() {
    // Pure DAG: a -> b -> c, no cycles.  Tarjan must return only
    // trivial (size-1, no self-loop) SCCs.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"])]);
    let graph = IndexedGraph::from_edges(&cg.edges);
    let sccs = graph.tarjan_scc();
    assert_eq!(sccs.len(), 3, "three nodes -> three trivial SCCs");
    for scc in &sccs {
        assert_eq!(scc.len(), 1, "DAG must have only singleton SCCs");
        let v = scc[0];
        assert!(
            !graph.succ[v as usize].contains(&v),
            "no self-loops in this DAG"
        );
    }
    // And `find_circuit_node_lists_with_limit` agrees: zero circuits.
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert!(circuits.is_empty());
}

#[test]
fn indexed_graph_scc_two_disjoint_cycles() {
    // Two disjoint 3-cycles produce two non-trivial SCCs of size 3 each.
    let cg = build_causal_graph(&[
        ("a", &["b"]),
        ("b", &["c"]),
        ("c", &["a"]),
        ("x", &["y"]),
        ("y", &["z"]),
        ("z", &["x"]),
    ]);
    let graph = IndexedGraph::from_edges(&cg.edges);
    let sccs = graph.tarjan_scc();
    let non_trivial: Vec<_> = sccs
        .iter()
        .filter(|s| s.len() > 1 || graph.succ[s[0] as usize].contains(&s[0]))
        .collect();
    assert_eq!(
        non_trivial.len(),
        2,
        "two disjoint 3-cycles -> two non-trivial SCCs"
    );
    assert!(non_trivial.iter().all(|s| s.len() == 3));
}

#[test]
fn indexed_graph_scc_figure_eight_single_scc() {
    // Figure-8: two cycles sharing node `m`.  Cycle 1: a -> m -> b -> a,
    // Cycle 2: c -> m -> d -> c.  All five nodes are mutually reachable
    // so Tarjan must return a single non-trivial SCC of size 5.
    let cg = build_causal_graph(&[
        ("a", &["m"]),
        ("m", &["b", "d"]),
        ("b", &["a"]),
        ("c", &["m"]),
        ("d", &["c"]),
    ]);
    let graph = IndexedGraph::from_edges(&cg.edges);
    let sccs = graph.tarjan_scc();
    let non_trivial: Vec<_> = sccs
        .iter()
        .filter(|s| s.len() > 1 || graph.succ[s[0] as usize].contains(&s[0]))
        .collect();
    assert_eq!(non_trivial.len(), 1, "figure-8 shares a node -> single SCC");
    assert_eq!(non_trivial[0].len(), 5);
}

#[test]
fn indexed_graph_self_loop_only_yields_no_circuit() {
    // Graph with just A -> A must yield zero circuits because pure
    // self-loops (circuit.len() == 1) are intentionally excluded.
    let cg = build_causal_graph(&[("a", &["a"])]);
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("tiny graph must not exhaust budget");
    assert!(
        circuits.is_empty(),
        "pure self-loop must NOT produce a circuit"
    );
}

#[test]
fn indexed_graph_zero_budget_nonempty_graph_truncates() {
    // Non-empty cycle + zero budget -> immediate TruncatedByBudget.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
    let err = cg
        .find_circuit_node_lists_with_limit(0)
        .expect_err("zero budget on a cycle must truncate");
    assert_eq!(err, TruncatedByBudget);
}

#[test]
fn indexed_graph_tiny_in_budget_succeeds() {
    // Matching positive control for the budget test: same cycle but
    // with an ample budget must succeed and return exactly one circuit.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
    let circuits = cg
        .find_circuit_node_lists_with_limit(usize::MAX)
        .expect("ample budget must succeed");
    assert_eq!(circuits.len(), 1);
}

// ------------------------------------------------------------------
// Johnson 1975 circuit-enumeration tests
// ------------------------------------------------------------------
//
// The tests below exercise the production Johnson's enumerator on
// targeted graph shapes and cross-check it against the Tiernan oracle
// retained under cfg(test) in the main IndexedGraph impl.  The
// invariants we care about match the LTM public contract:
//
//   (1) exactly the same set of circuits as Tiernan (after
//       canonicalizing each circuit's rotation),
//   (2) each circuit emitted rotated to start at its lex-smallest
//       node,
//   (3) pure self-loops excluded (circuit.len() > 1 contract),
//   (4) budget semantics: TruncatedByBudget on exactly the
//       max_circuits + 1st circuit,
//   (5) cross-SCC edges never traversed.

/// Canonicalize a list of circuits by mapping each to its canonical
/// edge-sequence rotation (see [`super::canonical_rotation`]) and
/// deduping.  LTM semantics keep distinct directed cycles as separate
/// loops (see issue #308).  Johnson's already emits at most one
/// rotation per directed cycle via inline canonical-rotation dedup;
/// Tiernan emits multiple rotations of each cycle but each rotation
/// canonicalizes to the same key, so reducing both sides to the
/// canonicalized + deduped form puts them on equal footing for
/// equivalence comparisons.
fn canonicalize_circuits(circuits: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
    let mut keys: Vec<Vec<u32>> = circuits
        .into_iter()
        .map(|c| super::canonical_rotation(&c))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Run both Johnson's (production) and Tiernan (oracle) on a graph
/// and assert their canonical-rotation circuit coverage is equal.
fn assert_johnson_matches_tiernan(cg: &CausalGraph) {
    let graph = IndexedGraph::from_edges(cg.edges());
    let sccs = graph.tarjan_scc();

    let mut johnson_circuits: Vec<Vec<u32>> = Vec::new();
    let mut budget_j = usize::MAX;
    // Cross-SCC scratch: see `enumerate_indexed_circuits` for the
    // ownership rationale.  Tests reuse the same map across SCCs to
    // mirror production behaviour.
    let mut g2l: Vec<i32> = vec![-1; graph.nodes.len()];
    for scc in &sccs {
        let mut part = graph
            .enumerate_circuits_in_scc(scc, &mut budget_j, &mut g2l)
            .unwrap();
        johnson_circuits.append(&mut part);
    }

    let mut tiernan_circuits: Vec<Vec<u32>> = Vec::new();
    let mut budget_t = usize::MAX;
    for scc in &sccs {
        let mut part = graph
            .enumerate_circuits_in_scc_tiernan(scc, &mut budget_t)
            .unwrap();
        tiernan_circuits.append(&mut part);
    }

    let johnson_canon = canonicalize_circuits(johnson_circuits);
    let tiernan_canon = canonicalize_circuits(tiernan_circuits);
    assert_eq!(
        johnson_canon,
        tiernan_canon,
        "Johnson's and Tiernan canonical-rotation coverage disagrees on edges {:?}",
        cg.edges()
    );
}

#[test]
fn johnson_empty_graph_no_circuits() {
    let cg = CausalGraph {
        edges: HashMap::new(),
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert!(circuits.is_empty(), "empty graph must have zero circuits");
}

#[test]
fn johnson_pure_dag_no_circuits() {
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("a", &["c"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert!(circuits.is_empty(), "pure DAG has no circuits");
}

#[test]
fn johnson_single_self_loop_excluded() {
    // Pure self-loop A -> A: path.len() == 1 is filtered.
    let cg = build_causal_graph(&[("a", &["a"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert!(circuits.is_empty(), "pure self-loop must not be emitted");
}

#[test]
fn johnson_two_node_back_edge_single_circuit() {
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(circuits.len(), 1);
    assert_eq!(
        circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"],
        "circuit must be rotated to start at lex-smallest node"
    );
}

#[test]
fn johnson_three_node_cycle_single_circuit() {
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(circuits.len(), 1);
    assert_eq!(
        circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

#[test]
fn johnson_two_disjoint_cycles_two_circuits() {
    let cg = build_causal_graph(&[
        ("a", &["b"]),
        ("b", &["c"]),
        ("c", &["a"]),
        ("x", &["y"]),
        ("y", &["z"]),
        ("z", &["x"]),
    ]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let names = circuits_as_sorted_name_sets(&circuits);
    assert_eq!(
        names,
        vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
        ]
    );
}

#[test]
fn johnson_figure_8_shared_vertex_two_circuits() {
    // Two cycles sharing node `m`:
    //   cycle 1: a -> m -> b -> a
    //   cycle 2: c -> m -> d -> c
    let cg = build_causal_graph(&[
        ("a", &["m"]),
        ("m", &["b", "d"]),
        ("b", &["a"]),
        ("c", &["m"]),
        ("d", &["c"]),
    ]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let names = circuits_as_sorted_name_sets(&circuits);
    assert_eq!(
        names,
        vec![
            vec!["a".to_string(), "b".to_string(), "m".to_string()],
            vec!["c".to_string(), "d".to_string(), "m".to_string()],
        ]
    );
}

#[test]
fn johnson_complete_k3_all_directed_cycles() {
    // K3 with all 6 directed edges.  Elementary directed cycles:
    //   3 two-cycles: {a,b}, {a,c}, {b,c}
    //   2 three-cycles: a -> b -> c -> a, a -> c -> b -> a
    // Both directions of the 3-cycle traverse the same node set
    // but represent distinct elementary circuits with potentially
    // different polarity products.  Issue #308 keeps them as
    // separate loops, so the public API reports 5 circuits.
    let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(
        circuits.len(),
        5,
        "K3 has 3 two-cycles + 2 three-cycles (forward + reverse)"
    );
    let names = circuits_as_sorted_name_sets(&circuits);
    // After sorting by node set, the two three-cycles collapse so
    // we still see exactly 4 distinct node sets even though the
    // circuit list has 5 entries.  The next test
    // (`johnson_multidigraph_keeps_distinct_directed_cycles`)
    // pins that the two 3-cycles really are separate.
    assert_eq!(
        names,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec!["a".to_string(), "c".to_string()],
            vec!["b".to_string(), "c".to_string()],
        ]
    );
}

#[test]
fn johnson_zero_budget_bails_immediately() {
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
    let err = cg
        .find_circuit_node_lists_with_limit(0)
        .expect_err("zero budget on a cycle must truncate");
    assert_eq!(err, TruncatedByBudget);
}

#[test]
fn johnson_respects_shared_budget_across_sccs() {
    // Two disjoint 2-cycles and a generous-but-finite budget.  Both
    // cycles fit; budget remains positive at the end.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"]), ("c", &["d"]), ("d", &["c"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(2).unwrap();
    assert_eq!(circuits.len(), 2);

    // Budget of 1 can emit only one of the two cycles before bailing.
    let err = cg
        .find_circuit_node_lists_with_limit(1)
        .expect_err("budget of 1 cannot fit both 2-cycles");
    assert_eq!(err, TruncatedByBudget);
}

#[test]
fn johnson_multidigraph_keeps_distinct_directed_cycles() {
    // On a multidigraph SCC where multiple distinct elementary
    // directed cycles share a node set, the canonical-rotation
    // dedup retains them as separate loops -- matching the
    // elementary-circuit identity used in the LTM literature
    // (issue #308).  Each surviving circuit is rotated to start
    // at its lex-smallest node, and the canonical rotation
    // distinguishes the two 3-cycles via their second-position
    // node (b vs c).
    //
    // K3 with all 6 directed edges has exactly two 3-cycles over
    // {a,b,c}: a -> b -> c -> a and a -> c -> b -> a.  Both
    // surface from start='a' (the cycle's lex-smallest node) so
    // each canonicalizes to itself; both must appear in the
    // output.
    let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let three_cycles: Vec<Vec<&str>> = circuits
        .iter()
        .filter(|c| c.len() == 3)
        .map(|c| c.iter().map(|n| n.as_str()).collect())
        .collect();
    assert_eq!(
        three_cycles.len(),
        2,
        "K3 retains both 3-node directed cycles as distinct loops"
    );
    let mut sorted: Vec<Vec<&str>> = three_cycles.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![vec!["a", "b", "c"], vec!["a", "c", "b"]],
        "the two surviving 3-cycles must be the forward and reverse traversals"
    );
}

#[test]
fn canonical_rotation_picks_lex_smallest_start() {
    // Smoke test for the helper: every rotation of the same
    // directed cycle canonicalizes to the same output.
    let inputs = [vec![1u32, 2, 3], vec![2, 3, 1], vec![3, 1, 2]];
    let canonical: Vec<Vec<u32>> = inputs
        .iter()
        .map(|c| super::canonical_rotation(c))
        .collect();
    for c in &canonical {
        assert_eq!(c, &vec![1u32, 2, 3]);
    }
}

#[test]
fn canonical_rotation_distinguishes_opposite_directions() {
    // Two distinct directed cycles over the same node set must
    // canonicalize to **different** outputs so the dedup keeps
    // both.
    let forward = super::canonical_rotation::<u32>(&[1, 2, 3]);
    let reverse = super::canonical_rotation::<u32>(&[1, 3, 2]);
    assert_eq!(forward, vec![1u32, 2, 3]);
    assert_eq!(reverse, vec![1u32, 3, 2]);
    assert_ne!(forward, reverse);
}

#[test]
fn canonical_rotation_handles_repeated_starting_element() {
    // Defensive check for the `(node, next_node)` tiebreaker
    // described in `docs/design-plans/2026-05-06-ltm-308-canonical-cycle-dedup.md`.
    // Elementary cycles never have repeated nodes, but the helper
    // is generic enough to handle non-elementary closed walks.
    // Two equally-small starting elements (both `1`) are
    // disambiguated by the second-position element.
    let canonical = super::canonical_rotation::<u32>(&[1, 5, 1, 3]);
    // Rotations:
    //   start=0: [1, 5, 1, 3]
    //   start=1: [5, 1, 3, 1]
    //   start=2: [1, 3, 1, 5]
    //   start=3: [3, 1, 5, 1]
    // Lex-smallest: [1, 3, 1, 5] (the second-position 3 < 5
    // breaks the tie between the two start-with-1 candidates).
    assert_eq!(canonical, vec![1u32, 3, 1, 5]);
}

#[test]
fn canonical_rotation_empty_input_returns_empty() {
    let out: Vec<u32> = super::canonical_rotation(&[]);
    assert!(out.is_empty());
}

#[test]
fn strip_subscript_truncates_at_last_open_bracket() {
    assert_eq!(super::strip_subscript("population"), "population");
    assert_eq!(super::strip_subscript("population[nyc]"), "population");
    assert_eq!(super::strip_subscript("m[nyc,boston]"), "m");
}

#[test]
fn split_node_subscript_returns_variable_name_and_optional_subscript() {
    assert_eq!(
        super::split_node_subscript("population"),
        ("population", None)
    );
    assert_eq!(
        super::split_node_subscript("population[nyc]"),
        ("population", Some("nyc"))
    );
    assert_eq!(
        super::split_node_subscript("m[nyc,boston]"),
        ("m", Some("nyc,boston"))
    );
}

#[test]
fn deduplicate_keeps_both_directed_three_cycles_in_multidigraph() {
    // Direct regression test for issue #308.  Construct a 3-node
    // SCC where every pair has bidirectional edges -- the smallest
    // graph that surfaces both directions of a directed 3-cycle.
    // `find_loops_with_limit` must produce TWO loops with the same
    // node set but different link orderings, not one.
    let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
    let loops = cg.find_loops_with_limit(usize::MAX).unwrap();

    let three_loops: Vec<&Loop> = loops.iter().filter(|l| l.links.len() == 3).collect();
    assert_eq!(
        three_loops.len(),
        2,
        "multidigraph must yield both directions of the 3-cycle as distinct Loops"
    );

    // Each loop's link sequence determines a unique directed
    // traversal; the two loops must differ in their second-
    // position node (b vs c) when both start at the lex-smallest
    // node 'a'.
    let mut seconds: Vec<&str> = three_loops.iter().map(|l| l.links[0].to.as_str()).collect();
    seconds.sort();
    assert_eq!(
        seconds,
        vec!["b", "c"],
        "the two 3-cycle loops must be a -> b -> ... and a -> c -> ..."
    );
}

#[test]
fn johnson_budget_bounds_unique_directed_cycles() {
    // Complete directed graph on 4 nodes (K4) with every pair
    // bidirectional.  Each elementary directed cycle is emitted
    // once by Johnson's (the `w >= start` gate forces emission
    // from the lex-smallest start), and the canonical-rotation
    // dedup keeps every distinct directed cycle as its own loop
    // (issue #308):
    //   6 two-cycles: {a,b}, {a,c}, {a,d}, {b,c}, {b,d}, {c,d}.
    //     Each two-cycle has only one elementary directed form
    //     (forward and reverse rotations of [a,b] both
    //     canonicalize to [a,b]).
    //   8 three-cycles: 4 node sets x 2 directions each.  Each
    //     direction canonicalizes distinctly because the
    //     second-position node differs.
    //   6 four-cycles: 1 node set ({a,b,c,d}) x 6 directed
    //     orderings (4!/4 = 6 distinct directed Hamilton cycles
    //     starting at the lex-smallest node).
    // Total: 6 + 8 + 6 = 20 distinct directed cycles.
    //
    // The budget decrement fires before the dedup `seen.insert`
    // check -- a defense-in-depth so a future change that
    // accidentally weakens dedup cannot make the DFS run for
    // longer than the cap implies.  After the canonical-rotation
    // change, raw emissions and unique survivors coincide for
    // elementary cycles on a graph (Johnson's gate prevents the
    // same directed cycle from being re-emitted from a non-lex
    // start), so the budget that fits unique output also fits
    // raw work for K4 specifically.
    let cg = build_causal_graph(&[
        ("a", &["b", "c", "d"]),
        ("b", &["a", "c", "d"]),
        ("c", &["a", "b", "d"]),
        ("d", &["a", "b", "c"]),
    ]);
    let full = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(
        full.len(),
        20,
        "K4 has exactly 20 distinct directed cycles after canonical-rotation dedup"
    );

    // Budget exactly fitting the unique count must succeed.
    let exact = cg.find_circuit_node_lists_with_limit(20).unwrap();
    assert_eq!(exact.len(), 20);

    // Budget one short must trip.
    let err = cg
        .find_circuit_node_lists_with_limit(19)
        .expect_err("budget of 19 must trip because K4 has 20 distinct directed cycles");
    assert_eq!(err, TruncatedByBudget);
}

#[test]
fn johnson_circuit_emitted_from_lex_smallest_node() {
    // Construct a cycle where the lex-smallest node is NOT the first
    // in the edge declarations so we can confirm rotation handling.
    // Edges listed starting from "c": c -> a -> b -> c.  The cycle's
    // lex-min is "a", so the emitted circuit is [a, b, c].
    let cg = build_causal_graph(&[("c", &["a"]), ("a", &["b"]), ("b", &["c"])]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(circuits.len(), 1);
    assert_eq!(
        circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"],
        "circuit must be rotated to start at 'a'"
    );
}

#[test]
fn johnson_unblock_transitive_chain() {
    // Graph designed to exercise the B[] chain unblocking.
    //
    //   a -> b
    //   b -> c, b -> e
    //   c -> a        (closes 1st cycle via b->c)
    //   c -> d
    //   d -> e        (d/e won't close by themselves from start=a)
    //   e -> a        (closes 2nd cycle via b->e)
    //
    // When DFS from start=a descends a->b->c->d->e, e has no cycle
    // back in its initial exploration; e registers as waiter of a in
    // its B[]. Later when b->e is explored and closes via e->a,
    // unblock cascades and correct cycle enumeration is preserved.
    let cg = build_causal_graph(&[
        ("a", &["b"]),
        ("b", &["c", "e"]),
        ("c", &["a", "d"]),
        ("d", &["e"]),
        ("e", &["a"]),
    ]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let names = circuits_as_sorted_name_sets(&circuits);
    // Elementary directed cycles:
    //   a -> b -> c -> a            -> {a,b,c}
    //   a -> b -> e -> a            -> {a,b,e}
    //   a -> b -> c -> d -> e -> a  -> {a,b,c,d,e}
    assert_eq!(
        names,
        vec![
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string()
            ],
            vec!["a".to_string(), "b".to_string(), "e".to_string()],
        ]
    );
}

#[test]
fn johnson_cross_scc_edges_not_traversed() {
    // Two SCCs connected by a cross-SCC edge.
    //   SCC 1: a <-> b
    //   SCC 2: x <-> y
    //   cross: b -> x (one-way; does not close any cycle)
    // Elementary circuits: {a,b} and {x,y}; the cross edge b->x
    // must NOT be traversed into SCC 2 when enumerating from SCC 1.
    let cg = build_causal_graph(&[
        ("a", &["b"]),
        ("b", &["a", "x"]),
        ("x", &["y"]),
        ("y", &["x"]),
    ]);
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let names = circuits_as_sorted_name_sets(&circuits);
    assert_eq!(
        names,
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["x".to_string(), "y".to_string()],
        ]
    );
}

#[test]
fn johnson_multi_scc_in_large_node_space_golden() {
    // Build a graph with several SCCs of mixed sizes embedded in a
    // larger node space (with plenty of acyclic feeder/sink nodes
    // padding the index range).  This is the golden fixture for the
    // per-SCC allocation refactor: emitted circuits MUST remain
    // byte-identical to the pre-fix output, including:
    //   * use of GLOBAL node indices (callers depend on this)
    //   * each cycle rotated to start at its lex-smallest member
    //   * dedup across multidigraph rotations (K3-with-all-edges
    //     contributes a single node-set after dedup)
    //
    // The pre-fix implementation sized JohnsonState to the whole graph;
    // the post-fix implementation sizes per-SCC with a global<->local
    // remap.  Both must produce the same emitted circuits.
    //
    // Topology (named so lexicographic order is predictable):
    //   * trivial feeders feed01..feed20 (no edges -> only acyclic role)
    //   * 2-cycle SCC: scc2_a <-> scc2_b
    //   * 3-cycle SCC: scc3_a -> scc3_b -> scc3_c -> scc3_a
    //   * K3 multidigraph SCC: scc_k3_a/b/c with all 6 directed edges
    //   * 4-cycle SCC with chord: scc4_a -> scc4_b -> scc4_c -> scc4_d -> scc4_a
    //                             plus scc4_a -> scc4_c (creates 3-cycle inside)
    //   * cross-SCC feeder edge: feed01 -> scc2_a (must not affect cycle output)
    //   * trailing sink nodes sink01..sink20 to pad out the index range
    let mut edges: Vec<(String, Vec<String>)> = vec![
        ("scc2_a".to_string(), vec!["scc2_b".to_string()]),
        ("scc2_b".to_string(), vec!["scc2_a".to_string()]),
        ("scc3_a".to_string(), vec!["scc3_b".to_string()]),
        ("scc3_b".to_string(), vec!["scc3_c".to_string()]),
        ("scc3_c".to_string(), vec!["scc3_a".to_string()]),
        (
            "scc_k3_a".to_string(),
            vec!["scc_k3_b".to_string(), "scc_k3_c".to_string()],
        ),
        (
            "scc_k3_b".to_string(),
            vec!["scc_k3_a".to_string(), "scc_k3_c".to_string()],
        ),
        (
            "scc_k3_c".to_string(),
            vec!["scc_k3_a".to_string(), "scc_k3_b".to_string()],
        ),
        (
            "scc4_a".to_string(),
            vec!["scc4_b".to_string(), "scc4_c".to_string()],
        ),
        ("scc4_b".to_string(), vec!["scc4_c".to_string()]),
        ("scc4_c".to_string(), vec!["scc4_d".to_string()]),
        ("scc4_d".to_string(), vec!["scc4_a".to_string()]),
    ];
    // Cross-SCC feeder + sink padding to ensure global node count
    // greatly exceeds individual SCC sizes.
    for i in 1..=20 {
        edges.push((format!("feed{i:02}"), vec!["scc2_a".to_string()]));
        edges.push((format!("sink{i:02}_src"), vec![format!("sink{i:02}_dst")]));
    }
    let edge_refs: Vec<(&str, Vec<&str>)> = edges
        .iter()
        .map(|(f, ts)| (f.as_str(), ts.iter().map(|s| s.as_str()).collect()))
        .collect();
    // Convert to the &[(&str, &[&str])] shape build_causal_graph wants.
    let edge_slices: Vec<(&str, &[&str])> = edge_refs
        .iter()
        .map(|(f, ts)| (*f, ts.as_slice()))
        .collect();
    let cg = build_causal_graph(&edge_slices);

    // Total node count is >60 (20 feeders + 20 sink-src + 20 sink-dst +
    // SCC members), so the pre-fix JohnsonState would allocate buffers
    // sized to ~70 even though the largest SCC is only 4 nodes.
    let graph = IndexedGraph::from_edges(cg.edges());
    assert!(
        graph.len() > 30,
        "test fixture must have a large total node count to exercise the regression"
    );

    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let got = circuits_as_sorted_name_sets(&circuits);

    // Expected node-sets per SCC:
    //   scc2: {scc2_a, scc2_b}
    //   scc3: {scc3_a, scc3_b, scc3_c}
    //   scc_k3 (K3 with all 6 directed edges): three 2-cycles (one per
    //     bidirectional pair) plus two 3-cycles (forward + reverse
    //     traversal of {a,b,c}).  Both 3-cycles have the same sorted
    //     node-set so they appear as duplicate entries in the
    //     name-set view, but the canonical-rotation dedup keeps them
    //     as distinct loops -- see issue #308 and
    //     `johnson_multidigraph_keeps_distinct_directed_cycles`.
    //   scc4 (rim a->b->c->d->a plus chord a->c): the 4-cycle around
    //     the rim and the 3-cycle skipping b via the chord.  No third
    //     cycle exists because b is only reachable from a and only
    //     reaches c, so the only path through b returns via the rim.
    let mut expected: Vec<Vec<String>> = vec![
        vec!["scc2_a".to_string(), "scc2_b".to_string()],
        vec![
            "scc3_a".to_string(),
            "scc3_b".to_string(),
            "scc3_c".to_string(),
        ],
        vec!["scc_k3_a".to_string(), "scc_k3_b".to_string()],
        vec!["scc_k3_a".to_string(), "scc_k3_c".to_string()],
        vec!["scc_k3_b".to_string(), "scc_k3_c".to_string()],
        vec![
            "scc_k3_a".to_string(),
            "scc_k3_b".to_string(),
            "scc_k3_c".to_string(),
        ],
        vec![
            "scc_k3_a".to_string(),
            "scc_k3_b".to_string(),
            "scc_k3_c".to_string(),
        ],
        vec![
            "scc4_a".to_string(),
            "scc4_b".to_string(),
            "scc4_c".to_string(),
            "scc4_d".to_string(),
        ],
        vec![
            "scc4_a".to_string(),
            "scc4_c".to_string(),
            "scc4_d".to_string(),
        ],
    ];
    expected.sort();
    assert_eq!(
        got, expected,
        "multi-SCC enumeration must match pre-refactor golden output"
    );

    // Cross-check against Tiernan oracle directly on the IndexedGraph
    // -- this catches any drift in the per-SCC enumerator that the
    // public dedup might mask.
    assert_johnson_matches_tiernan(&cg);
}

#[test]
fn dedup_deterministic_across_calls() {
    // Multidigraph (K3 with all 6 directed edges) yields two
    // distinct directed 3-cycles sharing node set {a,b,c}; LTM
    // semantics keep them as separate loops (issue #308) but each
    // raw cycle has multiple rotational representations
    // (`a -> b -> c -> a` is the same cycle as `b -> c -> a -> b`).
    // Inline canonical-rotation dedup folds rotations of the same
    // directed cycle into one survivor.  A non-deterministic hasher
    // could pick different representatives between calls on rare
    // collisions, which would silently invalidate the salsa
    // LoopCircuitsResult cache.  The rapidhash fingerprint is
    // content-addressed and deterministic (fixed seed, fixed
    // secret); calling twice on the same graph must produce
    // byte-identical output.
    let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
    let r1 = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    let r2 = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(
        r1, r2,
        "repeated enumeration of the same graph must be byte-identical"
    );

    // The indexed form must also be byte-identical, including the
    // trimmed name table and compact indices.
    let i1 = cg.find_indexed_circuits_with_limit(usize::MAX).unwrap();
    let i2 = cg.find_indexed_circuits_with_limit(usize::MAX).unwrap();
    assert_eq!(
        i1, i2,
        "repeated indexed enumeration must be byte-identical"
    );
}

#[test]
fn find_indexed_circuits_trims_names_to_cycle_participants() {
    // Graph: non-cyclic feeder -> entry -> [ entry <-> loop_a <-> loop_b ]
    // feeder is acyclic and must NOT appear in the names table.
    // Keeping it would invalidate salsa caching whenever an acyclic
    // variable elsewhere in the project is renamed, even though the
    // loop structure is unchanged.
    let cg = build_causal_graph(&[
        ("feeder", &["entry"]),
        ("entry", &["loop_a"]),
        ("loop_a", &["loop_b"]),
        ("loop_b", &["entry"]),
    ]);
    let (names, circuits) = cg.find_indexed_circuits_with_limit(usize::MAX).unwrap();
    assert_eq!(circuits.len(), 1, "exactly one elementary cycle");
    assert!(
        !names.iter().any(|n| n == "feeder"),
        "non-cyclic feeder must be excluded from the names table: {names:?}"
    );
    let mut expected = vec![
        "entry".to_string(),
        "loop_a".to_string(),
        "loop_b".to_string(),
    ];
    expected.sort();
    let mut got = names.clone();
    got.sort();
    assert_eq!(
        got, expected,
        "names table must contain exactly the cycle participants"
    );

    // Compact indices must all resolve to valid names-table entries.
    for c in &circuits {
        for &idx in c {
            assert!(
                (idx as usize) < names.len(),
                "compact index {idx} out of range"
            );
        }
    }
}

#[test]
fn find_indexed_circuits_empty_on_dag_returns_empty_names() {
    // Pure DAG has no circuits; the (names, circuits) pair must both
    // be empty so salsa sees a stable "no LTM" result across any
    // rename/reshape of the DAG.
    let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"])]);
    let (names, circuits) = cg.find_indexed_circuits_with_limit(usize::MAX).unwrap();
    assert!(circuits.is_empty(), "DAG has no circuits");
    assert!(
        names.is_empty(),
        "empty circuits must produce empty names table: {names:?}"
    );
}

#[test]
fn find_loops_and_find_circuit_node_lists_agree_on_count() {
    // Both public APIs share `enumerate_indexed_circuits`, so their
    // circuit counts must remain in lock-step.  This guards against
    // accidental drift if future refactors thread a separate dedup
    // or filter through one path but not the other.
    let cg = build_causal_graph(&[
        ("a", &["b", "c"]),
        ("b", &["a", "c"]),
        ("c", &["a", "b"]),
        ("x", &["y"]),
        ("y", &["x"]),
    ]);
    let loops = cg.find_loops_with_limit(usize::MAX).unwrap();
    let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
    assert_eq!(
        loops.len(),
        circuits.len(),
        "find_loops_with_limit and find_circuit_node_lists_with_limit must produce the same count"
    );
}

#[test]
fn johnson_matches_tiernan_on_fixture_corpus() {
    // Hand-curated corpus of graphs that exercise the Johnson/Tiernan
    // equivalence invariant.  Each entry is a list of (from, [to,...])
    // edge-list fragments, compared after canonicalization.
    let corpus: Vec<Vec<(&str, &[&str])>> = vec![
        // Empty graph
        vec![],
        // Single cycle
        vec![("a", &["b"]), ("b", &["c"]), ("c", &["a"])],
        // Two 2-cycles
        vec![("a", &["b"]), ("b", &["a"]), ("c", &["d"]), ("d", &["c"])],
        // Figure-8
        vec![
            ("a", &["m"]),
            ("m", &["b", "d"]),
            ("b", &["a"]),
            ("c", &["m"]),
            ("d", &["c"]),
        ],
        // K3 with all edges (multi-digraph, dedup exercises)
        vec![("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])],
        // Bowtie: two triangles sharing a single vertex
        vec![
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &["a"]),
            ("c", &["d"]),
            ("d", &["e"]),
            ("e", &["c"]),
        ],
        // Self-loop + non-trivial SCC (excluded self-loop)
        vec![("a", &["a"]), ("b", &["c"]), ("c", &["b"])],
        // Long chain with side spurs that don't close
        vec![
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &["d"]),
            ("d", &["a", "e"]),
            ("e", &["f"]),
            ("f", &["g"]),
        ],
        // Graph with many dead-end branches forcing Johnson's blocking to matter
        vec![
            ("a", &["b"]),
            ("b", &["c", "d", "e"]),
            ("c", &["a"]),
            ("d", &["f"]),
            ("e", &["g"]),
            ("f", &["h"]),
            ("g", &["h"]),
            ("h", &["a"]),
        ],
        // Arms race style: three-clique (every pair bidirectional)
        vec![
            ("alpha", &["beta", "gamma"]),
            ("beta", &["alpha", "gamma"]),
            ("gamma", &["alpha", "beta"]),
        ],
    ];

    for edges in &corpus {
        let cg = build_causal_graph(edges);
        assert_johnson_matches_tiernan(&cg);
    }
}

// ------------------------------------------------------------------
// Property-based equivalence test: random small graphs.
//
// For each randomly generated graph, assert Johnson's and Tiernan
// agree on the canonicalized set of elementary circuits.  The tight
// bound (nodes <= 8, edges up to 16) keeps enumeration cheap while
// giving the generator room to produce interesting SCC structures,
// bidirectional edges, and self-loops.
// ------------------------------------------------------------------

use proptest::prelude::*;

fn build_graph_from_pairs(n: usize, pairs: &[(u8, u8)]) -> CausalGraph {
    // Node names are "v0", "v1", ...  Use two-digit zero-padded
    // names so lex order matches numeric order (v01 < v02 < ... < v10).
    let names: Vec<String> = (0..n).map(|i| format!("v{i:02}")).collect();
    let mut edge_pairs: Vec<(String, String)> = Vec::new();
    for &(from_raw, to_raw) in pairs {
        let from = from_raw as usize % n;
        let to = to_raw as usize % n;
        edge_pairs.push((names[from].clone(), names[to].clone()));
    }
    // Deduplicate: HashMap::entry will overwrite but we need to
    // aggregate the adjacency list instead.
    let mut map: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (f, t) in edge_pairs {
        if seen.insert((f.clone(), t.clone())) {
            map.entry(Ident::new(&f)).or_default().push(Ident::new(&t));
        }
    }
    CausalGraph {
        edges: map,
        stocks: HashSet::new(),
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Johnson's must agree with Tiernan on any random small graph.
    /// The generator draws a node count 2..=8 and up to 16 directed
    /// edges (possibly self-loops, possibly duplicates that we
    /// deduplicate).  Canonicalized circuit lists must match exactly.
    #[test]
    fn johnson_matches_tiernan_on_random_small_graphs(
        n in 2usize..=8,
        edges in prop::collection::vec((any::<u8>(), any::<u8>()), 0..=16),
    ) {
        let cg = build_graph_from_pairs(n, &edges);
        // Exercise via the CausalGraph public API in addition to the
        // direct IndexedGraph inspection, to catch divergences at the
        // API boundary (rotation, SCC ordering, empty-SCC handling).
        assert_johnson_matches_tiernan(&cg);
    }
}
