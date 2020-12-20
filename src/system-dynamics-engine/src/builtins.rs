// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[derive(PartialEq, Clone, Debug)]
pub enum BuiltinFn<Expr> {
    Lookup(String, Box<Expr>),
    Abs(Box<Expr>),
    Arccos(Box<Expr>),
    Arcsin(Box<Expr>),
    Arctan(Box<Expr>),
    Cos(Box<Expr>),
    Exp(Box<Expr>),
    Inf,
    Int(Box<Expr>),
    Ln(Box<Expr>),
    Log10(Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Pi,
    Pulse(Box<Expr>, Box<Expr>, Box<Expr>),
    SafeDiv(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Sin(Box<Expr>),
    Sqrt(Box<Expr>),
    Tan(Box<Expr>),
}

pub fn is_builtin_fn_or_time(name: &str) -> bool {
    is_builtin_fn(name) || matches!(name, "time" | "dt" | "initial_time" | "final_time")
}

pub fn is_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "lookup"
            | "abs"
            | "arccos"
            | "arcsin"
            | "arctan"
            | "cos"
            | "exp"
            | "inf"
            | "int"
            | "ln"
            | "log10"
            | "max"
            | "min"
            | "pi"
            | "pulse"
            | "safediv"
            | "sin"
            | "sqrt"
            | "tan"
    )
}

#[test]
fn test_is_builtin_fn() {
    assert!(is_builtin_fn("lookup"));
    assert!(!is_builtin_fn("lookupz"));
    assert!(is_builtin_fn("log10"));
}
