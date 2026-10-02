// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;

use proptest::prelude::*;

use super::*;
use crate::ast::expr0_strategy::expr_strategy;
use crate::ast::print_eqn;
use crate::test_common::TestProject;

fn parse(text: &str) -> Expr0 {
    Expr0::new(text, LexerType::Equation)
        .unwrap_or_else(|e| panic!("`{text}` does not parse: {e:?}"))
        .unwrap_or_else(|| panic!("`{text}` is empty"))
}

fn no_macro(_: &str) -> bool {
    false
}

fn spelling(text: &str) -> CanonicalEqn {
    parse(text).canonical(Aliases::Spelling)
}

fn engine(text: &str) -> CanonicalEqn {
    parse(text).canonical(Aliases::Engine {
        is_macro: &no_macro,
    })
}

/// What the engine computes for `equation`, at the first step of a run.
fn value_of(equation: &str) -> f64 {
    let project = TestProject::new("p")
        .aux("a", "7", None)
        .aux("b", "-3", None)
        .aux("x", equation, None)
        .build_datamodel();
    let mut vm = crate::queue_compile::build_vm(&project, "main").expect("the model compiles");
    vm.run_to_end().expect("the model runs");
    let results = vm.into_results();
    let at = results.offsets[&crate::common::Ident::new("x")];
    results.iter().next().expect("a first step")[at]
}

/// Spellings a reader of the text would call one equation are one canonical
/// form under either mode; the engine's rewrites are one form only under
/// `Aliases::Engine`.
#[test]
fn spellings_of_one_equation_share_a_canonical_form() {
    // (a, b, the same under Spelling, the same under Engine)
    let rows = [
        ("600000", "6e+05", true, true),
        (".12", "0.12", true, true),
        ("0.0001", "1e-04", true, true),
        ("\"Heat Loss\" + 1", "heat_loss + 1", true, true),
        ("Heat_Loss + 1", "heat_loss + 1", true, true),
        ("x[A1] * 2", "x[a1] * 2", true, true),
        ("x[*:Dim]", "x[*:dim]", true, true),
        ("MAX(a, b)", "max(a,b)", true, true),
        ("VECTOR ELM MAP(a, b)", "vector_elm_map(a, b)", true, true),
        ("a // b", "safediv(a, b)", true, true),
        ("IF a THEN b ELSE 1", "if (a) then (b) else (1)", true, true),
        ("a   +   b", "a + b", true, true),
        // The engine's own rewrites.
        ("MODULO(a, 3)", "a mod 3", false, true),
        ("PI()", "3.141592653589793", false, true),
        ("SIN(PI())", "sin(3.141592653589793)", false, true),
        ("INF()", "1e400", false, true),
        ("inf", "1e400", false, true),
        ("STARTTIME", "INITIAL_TIME", false, true),
        ("DT * 2", "time_step * 2", false, true),
        // Never the same: the engine computes these differently, or reads a
        // different thing.
        ("a + b", "b + a", false, false),
        ("1 * INT(a / 1)", "QUANTUM(a, 1)", false, false),
        ("0.1 + 0.2", "0.30000000000000004", false, false),
        ("1e400", "\"inf\"", false, false),
        ("PI()", "\"pi\"", false, false),
        ("NaN", "\"nan\"", false, false),
        ("a.b", "\"a.b\"", false, false),
        ("-a", "0 - a", false, false),
        ("a - (b - 1)", "a - b - 1", false, false),
    ];
    for (a, b, same_spelling, same_engine) in rows {
        assert_eq!(spelling(a) == spelling(b), same_spelling, "{a} and {b}");
        assert_eq!(engine(a) == engine(b), same_engine, "{a} and {b}");
    }
}

/// A call the project defines a macro for is the macro's, which the engine
/// expands instead of rewriting: it is left as written.
#[test]
fn a_call_a_macro_takes_is_left_as_written() {
    let is_macro = |name: &str| matches!(name, "modulo" | "inf" | "pi" | "starttime");
    let with_macros = |text: &str| {
        parse(text).canonical(Aliases::Engine {
            is_macro: &is_macro,
        })
    };
    for (a, b) in [
        ("MODULO(a, 3)", "a mod 3"),
        ("INF()", "1e400"),
        ("PI()", "3.141592653589793"),
        ("STARTTIME()", "INITIAL_TIME()"),
    ] {
        assert!(engine(a) == engine(b), "{a} and {b} with no macro");
        assert!(
            with_macros(a) != with_macros(b),
            "{a} and {b} beside a macro of the name"
        );
    }
    // A macro named for the builtin an alias stands for takes the builtin's
    // own name and leaves the alias the builtin: `DT` is the time step and
    // `TIME_STEP` the macro's value, so neither is respelled as the other.
    for sig in BuiltinSig::ALL {
        let target_is_macro = |name: &str| name == sig.name;
        let args = vec!["a"; sig.min_args as usize].join(", ");
        for alias in sig.aliases {
            let (as_alias, as_name) = (format!("{alias}({args})"), format!("{}({args})", sig.name));
            let beside_macro = |text: &str| {
                parse(text).canonical(Aliases::Engine {
                    is_macro: &target_is_macro,
                })
            };
            assert!(
                beside_macro(&as_alias) != beside_macro(&as_name),
                "{alias} and {} beside a macro named {}",
                sig.name,
                sig.name
            );
        }
    }
}

/// Inside a subscript the engine reads an index literal otherwise than a
/// call, so no call is folded to a constant or an operator there; a number is
/// still its value and a builtin's alias still the builtin.
#[test]
fn no_call_is_folded_inside_a_subscript() {
    assert!(engine("x[1e400]") != engine("x[INF()]"));
    assert!(engine("x[3.141592653589793]") != engine("x[PI()]"));
    assert!(engine("x[MODULO(i, 3)]") != engine("x[i mod 3]"));
    assert!(engine("x[MAX(1, INF())]") != engine("x[MAX(1, 1e400)]"));
    assert!(engine("x[6e+05]") == engine("x[600000]"));
    assert!(engine("x[STARTTIME]") == engine("x[INITIAL_TIME]"));
}

/// The same number, a NaN the same as a NaN.
fn same_number(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Every rewrite `Aliases::Engine` makes computes what the spelling it
/// rewrites computes: the constants `PI()` and `INF()` fold to, the operator
/// `MODULO` folds to (over divisors of either sign and zero), and every alias
/// in the builtin table as its builtin. Each side runs through the VM, the
/// rewritten side as the canonical form prints.
#[test]
fn the_engines_rewrites_are_what_the_engine_computes() {
    for (call, folded) in [
        ("PI()", constant_for_call("pi", 0)),
        ("INF()", constant_for_call("inf", 0)),
    ] {
        let folded = folded.expect("a constant call");
        assert_eq!(value_of(call).to_bits(), folded.to_bits(), "{call}");
    }
    assert!(matches!(
        operator_for_call("modulo", vec![1, 2]),
        Ok((BinaryOp::Mod, 1, 2))
    ));
    let mut rewritten = vec![
        "MODULO(a, 3)".to_string(),
        "MODULO(b, 2)".to_string(),
        "MODULO(a, b)".to_string(),
        "MODULO(7.5, 2)".to_string(),
        "MODULO(7.5, -2)".to_string(),
        "MODULO(-7.5, 2)".to_string(),
        "MODULO(-7.5, -2)".to_string(),
        "MODULO(a, 0)".to_string(),
        "MODULO(0, b)".to_string(),
        "MODULO(a + 1, b - 1)".to_string(),
    ];
    for sig in BuiltinSig::ALL {
        let args = vec!["a"; sig.min_args as usize].join(", ");
        rewritten.extend(sig.aliases.iter().map(|alias| format!("{alias}({args})")));
    }
    for spelling in &rewritten {
        let canonical = engine(spelling).to_string();
        assert_ne!(&canonical, spelling, "{spelling} is rewritten");
        let (was, is) = (value_of(spelling), value_of(&canonical));
        assert!(
            same_number(was, is),
            "{spelling} is {was}, {canonical} is {is}"
        );
    }
    assert_eq!(
        operator_for_call("modulo", vec![1, 2, 3]),
        Err(vec![1, 2, 3])
    );
    assert_eq!(operator_for_call("max", vec![1, 2]), Err(vec![1, 2]));
    assert_eq!(constant_for_call("pi", 1), None);
}

/// Every alias in the builtin table is its builtin under `Aliases::Engine`,
/// and stays its own spelling under `Aliases::Spelling`.
#[test]
fn every_builtin_alias_is_its_builtin() {
    let mut aliases = 0;
    for sig in BuiltinSig::ALL {
        for alias in sig.aliases {
            aliases += 1;
            let args = vec!["a"; sig.min_args as usize].join(", ");
            let (as_alias, as_name) = (format!("{alias}({args})"), format!("{}({args})", sig.name));
            assert!(
                engine(&as_alias) == engine(&as_name),
                "{alias} and {}",
                sig.name
            );
            assert!(
                spelling(&as_alias) != spelling(&as_name),
                "{alias} and {}",
                sig.name
            );
        }
    }
    assert!(aliases > 0, "the builtin table declares aliases");
}

/// A canonical form prints through the equation printer, a number as the one
/// spelling of its value.
#[test]
fn a_canonical_form_prints_as_an_equation() {
    for (text, printed) in [
        ("\"Heat Loss\"  *  6e+05", "heat_loss * 600000"),
        ("MAX( A , 1e-7 )", "max(a, 1e-7)"),
        ("PI() + INF()", "3.141592653589793 + Inf"),
        ("NAN", "NaN"),
        ("MODULO(a, 3)", "a mod 3"),
        ("x[\"A 1\", *:Dim, 2.0]", "x[a_1, *:dim, 2]"),
        ("1e16 + 0.00001 + 1e-6", "1e16 + 0.00001 + 1e-6"),
    ] {
        assert_eq!(engine(text).to_string(), printed, "{text}");
    }
    assert_eq!(spelling("PI() + 1").to_string(), "pi() + 1");
    assert!(CanonicalEqn::parse("a +", Aliases::Spelling).is_err());
    assert!(matches!(
        CanonicalEqn::parse("  ", Aliases::Spelling),
        Ok(None)
    ));
    assert_eq!(
        CanonicalEqn::parse("A + 1", Aliases::Spelling)
            .expect("it parses")
            .expect("it is not empty")
            .expr(),
        spelling("a + 1").expr()
    );
}

/// Equal canonical forms hash alike, so a set of them holds one of each
/// equation.
#[test]
fn equal_canonical_forms_hash_alike() {
    let forms: HashSet<CanonicalEqn> = ["a + 6e+05", "A + 600000", "\"a\"+600000.0", "a + 1"]
        .into_iter()
        .map(spelling)
        .collect();
    assert_eq!(forms.len(), 2);
}

proptest! {
    /// `Expr0::eq_ignoring_loc` is canonical equality under
    /// `Aliases::Spelling`, walked without building the trees: on an
    /// equation and its own printed-and-reparsed respelling (always the
    /// same), and on two independent equations (rarely the same).
    #[test]
    fn eq_ignoring_loc_is_canonical_equality(a in expr_strategy(), b in expr_strategy()) {
        let respelled = parse(&print_eqn(&a));
        prop_assert!(a.eq_ignoring_loc(&respelled), "{}", print_eqn(&a));
        prop_assert!(a.canonical(Aliases::Spelling) == respelled.canonical(Aliases::Spelling));
        prop_assert_eq!(
            a.eq_ignoring_loc(&b),
            a.canonical(Aliases::Spelling) == b.canonical(Aliases::Spelling),
            "{} and {}",
            print_eqn(&a),
            print_eqn(&b)
        );
    }

    /// The canonical form is a fixed point: the canonical form of its own
    /// tree is itself, under either mode.
    #[test]
    fn a_canonical_form_is_its_own_canonical_form(expr in expr_strategy()) {
        let engine = Aliases::Engine { is_macro: &no_macro };
        for aliases in [Aliases::Spelling, engine] {
            let once = expr.canonical(aliases);
            prop_assert!(once.expr().canonical(aliases) == once, "{}", once);
        }
    }
}
