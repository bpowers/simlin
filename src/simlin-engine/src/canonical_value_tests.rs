// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! That the engine computes the same numbers for an equation and for its
//! canonical form (`ast::CanonicalEqn` under `Aliases::Engine`): the form is the
//! engine's statement that two equations say one thing, and only a run says
//! whether they do.

use simlin_base::ast::{
    Aliases, BinaryOp, CanonicalEqn, Expr0, constant_for_call, operator_for_call,
};
use simlin_base::builtins::BuiltinSig;
use simlin_base::common::Ident;
use simlin_base::lexer::LexerType;

use crate::test_common::TestProject;

fn engine(text: &str) -> CanonicalEqn {
    Expr0::new(text, LexerType::Equation)
        .unwrap_or_else(|e| panic!("`{text}` does not parse: {e:?}"))
        .unwrap_or_else(|| panic!("`{text}` is empty"))
        .canonical(Aliases::Engine {
            is_macro: &|_| false,
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
    let at = results.offsets[&Ident::new("x")];
    results.iter().next().expect("a first step")[at]
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
