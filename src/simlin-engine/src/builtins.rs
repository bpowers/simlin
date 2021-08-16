// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[derive(PartialEq, Clone, Debug)]
pub struct UntypedBuiltinFn<Expr>(pub String, pub Vec<Expr>);

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
    IsModuleInput(String),
    Ln(Box<Expr>),
    Log10(Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Mean(Vec<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Pi,
    Pulse(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Ramp(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    SafeDiv(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Sin(Box<Expr>),
    Sqrt(Box<Expr>),
    Step(Box<Expr>, Box<Expr>),
    Tan(Box<Expr>),
    Time,
    TimeStep,
    StartTime,
    FinalTime,
}

pub fn is_0_arity_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "inf" | "pi" | "time" | "time_step" | "dt" | "initial_time" | "final_time"
    )
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
            | "ismoduleinput"
            | "ln"
            | "log10"
            | "max"
            | "mean"
            | "min"
            | "pi"
            | "pulse"
            | "ramp"
            | "safediv"
            | "sin"
            | "sqrt"
            | "step"
            | "tan"
            | "time"
            | "time_step"
            | "dt"
            | "initial_time"
            | "final_time"
    )
}

#[test]
fn test_is_builtin_fn() {
    assert!(is_builtin_fn("lookup"));
    assert!(!is_builtin_fn("lookupz"));
    assert!(is_builtin_fn("log10"));
}

#[test]
fn test_is_0_arity_builtin_fn() {
    assert!(!is_0_arity_builtin_fn("lookup"));
    assert!(is_0_arity_builtin_fn("time"));
}
