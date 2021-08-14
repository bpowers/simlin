// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// equations are strings typed by humans for a single
// variable -- u16 is long enough
#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Loc {
    pub start: u16,
    pub end: u16,
}

impl Loc {
    pub fn new(start: usize, end: usize) -> Self {
        Loc {
            start: start as u16,
            end: end as u16,
        }
    }

    pub fn union(&self, rhs: &Self) -> Self {
        Loc {
            start: self.start.min(rhs.start),
            end: self.end.max(rhs.end),
        }
    }
}

#[test]
fn test_loc_basics() {
    let a = Loc { start: 3, end: 7 };
    assert_eq!(a, Loc::new(3, 7));

    let b = Loc { start: 4, end: 11 };
    assert_eq!(Loc::new(3, 11), a.union(&b));

    let c = Loc { start: 1, end: 5 };
    assert_eq!(Loc::new(1, 7), a.union(&c));
}

pub struct UntypedBuiltinFn<Expr>(String, Vec<Expr>);

#[derive(PartialEq, Clone, Debug)]
pub enum BuiltinFn<Expr> {
    Lookup(String, Loc, Box<Expr>),
    Abs(Box<Expr>),
    Arccos(Box<Expr>),
    Arcsin(Box<Expr>),
    Arctan(Box<Expr>),
    Cos(Box<Expr>),
    Exp(Box<Expr>),
    Inf,
    Int(Box<Expr>),
    IsModuleInput(String, Loc),
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
