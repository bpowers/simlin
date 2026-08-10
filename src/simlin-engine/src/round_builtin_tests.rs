// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end contract for the `ROUND(x)` builtin.
//!
//! ROUND is a Simlin extension: the XMILE v1.0 spec defines no ROUND function
//! (verified against `docs/reference/xmile-v1.0.html`, which contains zero
//! occurrences of the word). Its semantics are therefore ours to define, and
//! they are Python's `round()` / IEEE 754 roundTiesToEven: round to the
//! nearest integer, with an exact .5 tie going to the EVEN neighbor. That is
//! `f64::round_ties_even` in the VM and the single `f64.nearest` instruction
//! in the wasm backend, so the two backends agree bit for bit by construction.
//!
//! The tie-to-even cases are the whole point of the contract -- `f64::round`
//! (ties away from zero) agrees with `round_ties_even` on every non-tie input,
//! so a test suite without exact .5 ties would pass with the wrong function.

use crate::test_common::TestProject;

/// `(input literal, expected)` rows for scalar ROUND, exercised through the
/// full parse -> lower -> compile -> VM pipeline.
///
/// Expected values are Python 3 `round()` outputs (spot-checked directly
/// against CPython): ties go to the even neighbor, non-ties to the nearest
/// integer.
const ROUND_CASES: &[(&str, f64)] = &[
    // Exact .5 ties -> even neighbor (the cases that distinguish
    // round-half-even from round-half-away-from-zero).
    ("0.5", 0.0),
    ("1.5", 2.0),
    ("2.5", 2.0),
    ("3.5", 4.0),
    // IEEE roundTiesToEven preserves the sign of zero, so round(-0.5) is
    // NEGATIVE zero -- the same value Python's float rounding produces
    // (`round(-0.5, 0) == -0.0`; the bare `round(-0.5) == 0` only because
    // Python ints have no signed zero). Numerically equal to 0 everywhere.
    ("-0.5", -0.0),
    ("-1.5", -2.0),
    ("-2.5", -2.0),
    ("-3.5", -4.0),
    // Non-ties round to nearest.
    ("2.4", 2.0),
    ("2.6", 3.0),
    ("-2.4", -2.0),
    ("-2.6", -3.0),
    ("0.4999", 0.0),
    ("7", 7.0),
    ("-7", -7.0),
    ("0", 0.0),
    // The double closest to but below 0.5: rounds to 0, and would expose an
    // implementation that rounded the DECIMAL spelling instead of the binary
    // value.
    ("0.49999999999999994", 0.0),
    // At 2^52 every double is already an integer; ROUND must be the identity
    // there rather than losing precision.
    ("4503599627370496", 4503599627370496.0),
    // The largest representable x.5 tie below 2^52: 2^52 - 0.5 rounds to the
    // even 2^52.
    ("4503599627370495.5", 4503599627370496.0),
];

#[test]
fn round_scalar_ties_to_even_through_the_vm() {
    for (input, expected) in ROUND_CASES {
        let got = TestProject::new("round_scalar")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("y", &format!("round({input})"), None)
            .vm_result("y");
        assert_eq!(
            got[0].to_bits(),
            expected.to_bits(),
            "round({input}): got {}, expected {expected}",
            got[0]
        );
    }
}

#[test]
fn round_of_a_variable_reference_is_not_constant_folded_away() {
    // Through a variable reference the argument reaches the VM's Apply opcode
    // (a literal argument could in principle be folded at compile time), so
    // this pins the runtime dispatch itself.
    let got = TestProject::new("round_var")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("x", "2.5", None)
        .aux("y", "round(x)", None)
        .vm_result("y");
    assert_eq!(got[0], 2.0);
}

#[test]
fn round_is_case_insensitive_like_every_builtin() {
    let got = TestProject::new("round_case")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("y", "ROUND(1.5)", None)
        .vm_result("y");
    assert_eq!(got[0], 2.0);
}

#[test]
fn round_propagates_nan_and_infinity() {
    let got = TestProject::new("round_nan")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("x", "nan", None)
        .aux("y", "round(x)", None)
        .vm_result("y");
    assert!(got[0].is_nan(), "round(NaN) must be NaN, got {}", got[0]);

    let got = TestProject::new("round_inf")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("y", "round(inf)", None)
        .vm_result("y");
    assert_eq!(got[0], f64::INFINITY);

    let got = TestProject::new("round_neg_inf")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("y", "round(-inf)", None)
        .vm_result("y");
    assert_eq!(got[0], f64::NEG_INFINITY);
}

#[test]
fn round_applies_elementwise_over_arrays() {
    let got = TestProject::new("round_array")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("d", &["a", "b", "c", "e"])
        .array_with_ranges(
            "x[d]",
            vec![("a", "0.5"), ("b", "1.5"), ("c", "2.5"), ("e", "2.6")],
        )
        .array_aux("y[d]", "round(x[d])")
        .run_vm()
        .expect("VM should run");
    for (elem, expected) in [("a", 0.0), ("b", 2.0), ("c", 2.0), ("e", 3.0)] {
        let series = got
            .get(&format!("y[{elem}]"))
            .unwrap_or_else(|| panic!("missing y[{elem}] in {:?}", got.keys()));
        assert_eq!(series[0], expected, "y[{elem}]");
    }
}

#[test]
fn round_inside_a_reducer_compiles_and_reduces() {
    // ROUND under SUM exercises the array-operand materialization path (an
    // elementwise computed operand is hoisted into a temp).
    let got = TestProject::new("round_reduce")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("d", &["a", "b"])
        .array_with_ranges("x[d]", vec![("a", "1.5"), ("b", "2.5")])
        .aux("y", "sum(round(x[*]))", None)
        .vm_result("y");
    assert_eq!(got[0], 4.0); // round(1.5) + round(2.5) = 2 + 2
}

#[test]
fn round_wrong_arity_is_a_compile_error() {
    for eqn in ["round()", "round(1, 2)"] {
        let result = TestProject::new("round_arity")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("y", eqn, None)
            .run_vm();
        assert!(result.is_err(), "{eqn} must fail to compile");
    }
}

#[test]
fn round_preserves_units() {
    // ROUND is units-preserving (like INT/ABS): round(x widgets) is widgets.
    TestProject::new("round_units_ok")
        .with_sim_time(0.0, 1.0, 1.0)
        .with_time_units("seconds")
        .unit("widgets", None)
        .unit("seconds", None)
        .aux_with_units("x", "2.5", Some("widgets"))
        .aux_with_units("y", "round(x)", Some("widgets"))
        .assert_compiles_incremental()
        .assert_no_unit_diagnostics();

    // And the preserved units are x's, not "anything": declaring the result
    // as a different unit is a mismatch.
    TestProject::new("round_units_bad")
        .with_sim_time(0.0, 1.0, 1.0)
        .with_time_units("seconds")
        .unit("widgets", None)
        .unit("gadgets", None)
        .unit("seconds", None)
        .aux_with_units("x", "2.5", Some("widgets"))
        .aux_with_units("y", "round(x)", Some("gadgets"))
        .assert_unit_error_vm();
}

#[test]
fn round_survives_print_and_reparse() {
    // The patch path re-prints equations (expr2 -> expr0 -> text); a builtin
    // missing from that printer would corrupt any model edited through it.
    use crate::datamodel::Equation;
    use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};

    let mut project = TestProject::new("round_reprint")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("x", "2.5", None)
        .aux("y", "round(x)", None)
        .build_datamodel();

    let patch = ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops: vec![ModelOperation::RenameVariable {
                from: "x".to_string(),
                to: "x2".to_string(),
            }],
        }],
    };
    apply_patch(&mut project, patch).expect("rename applies");

    let model = &project.models[0];
    let y = model
        .variables
        .iter()
        .find(|v| v.get_ident() == "y")
        .expect("y exists");
    let eqn = match y.get_equation() {
        Some(Equation::Scalar(text, ..)) => text.clone(),
        other => panic!("expected scalar equation, got {other:?}"),
    };
    assert_eq!(eqn, "round(x2)");
}
