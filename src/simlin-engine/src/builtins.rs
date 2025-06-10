// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;

/// Loc describes a location in an equation by the starting point and ending point.
/// Equations are strings typed by humans for a single variable -- u16 is long enough.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Default, Hash)]
pub struct Loc {
    pub start: u16,
    pub end: u16,
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.start, self.end)
    }
}

impl Loc {
    pub fn new(start: usize, end: usize) -> Self {
        Loc {
            start: start as u16,
            end: end as u16,
        }
    }

    /// union takes a second Loc and returns the inclusive range from the
    /// start of the earlier token to the end of the later token.
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

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct UntypedBuiltinFn<Expr>(pub String, pub Vec<Expr>);

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum BuiltinFn<Expr> {
    Lookup(String, Box<Expr>, Loc),
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
    // max takes 2 scalar args OR 1-2 args for an array
    Max(Box<Expr>, Option<Box<Expr>>),
    Mean(Vec<Expr>),
    // max takes 2 scalar args OR 1-2 args for an array
    Min(Box<Expr>, Option<Box<Expr>>),
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
    // array-only builtins
    Rank(Box<Expr>, Option<(Box<Expr>, Option<Box<Expr>>)>),
    Size(Box<Expr>),
    Stddev(Box<Expr>),
    Sum(Box<Expr>),
}

impl<Expr> BuiltinFn<Expr> {
    pub fn name(&self) -> &'static str {
        use BuiltinFn::*;
        match self {
            Lookup(_, _, _) => "lookup",
            Abs(_) => "abs",
            Arccos(_) => "arccos",
            Arcsin(_) => "arcsin",
            Arctan(_) => "arctan",
            Cos(_) => "cos",
            Exp(_) => "exp",
            Inf => "inf",
            Int(_) => "int",
            IsModuleInput(_, _) => "ismoduleinput",
            Ln(_) => "ln",
            Log10(_) => "log10",
            Max(_, _) => "max",
            Mean(_) => "mean",
            Min(_, _) => "min",
            Pi => "pi",
            Pulse(_, _, _) => "pulse",
            Ramp(_, _, _) => "ramp",
            SafeDiv(_, _, _) => "safediv",
            Sin(_) => "sin",
            Sqrt(_) => "sqrt",
            Step(_, _) => "step",
            Tan(_) => "tan",
            Time => "time",
            TimeStep => "time_step",
            StartTime => "initial_time",
            FinalTime => "final_time",
            // array only builtins
            Rank(_, _) => "rank",
            Size(_) => "size",
            Stddev(_) => "stddev",
            Sum(_) => "sum",
        }
    }
}

pub fn is_0_arity_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "inf" | "pi" | "time" | "time_step" | "dt" | "initial_time" | "final_time"
    )
}

pub fn is_builtin_fn(name: &str) -> bool {
    is_0_arity_builtin_fn(name)
        || matches!(
            name,
            // scalar builtins
            "lookup"
        | "abs"
        | "arccos"
        | "arcsin"
        | "arctan"
        | "cos"
        | "exp"
        | "int"
        | "ismoduleinput"
        | "ln"
        | "log10"
        | "max"
        | "mean"
        | "min"
        | "pulse"
        | "ramp"
        | "safediv"
        | "sin"
        | "sqrt"
        | "step"
        | "tan"
        // array-only builtins
        | "rank"
        | "size"
        | "stddev"
        | "sum"
        )
}

pub(crate) enum BuiltinContents<'a, Expr> {
    Ident(&'a str, Loc),
    Expr(&'a Expr),
}

pub(crate) fn walk_builtin_expr<'a, Expr, F>(builtin: &'a BuiltinFn<Expr>, mut cb: F)
where
    F: FnMut(BuiltinContents<'a, Expr>),
{
    match builtin {
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime => {}
        BuiltinFn::IsModuleInput(id, loc) => cb(BuiltinContents::Ident(id, *loc)),
        BuiltinFn::Lookup(id, a, loc) => {
            cb(BuiltinContents::Ident(id, *loc));
            cb(BuiltinContents::Expr(a));
        }
        BuiltinFn::Abs(a)
        | BuiltinFn::Arccos(a)
        | BuiltinFn::Arcsin(a)
        | BuiltinFn::Arctan(a)
        | BuiltinFn::Cos(a)
        | BuiltinFn::Exp(a)
        | BuiltinFn::Int(a)
        | BuiltinFn::Ln(a)
        | BuiltinFn::Log10(a)
        | BuiltinFn::Sin(a)
        | BuiltinFn::Sqrt(a)
        | BuiltinFn::Tan(a)
        | BuiltinFn::Size(a)
        | BuiltinFn::Stddev(a)
        | BuiltinFn::Sum(a) => cb(BuiltinContents::Expr(a)),
        BuiltinFn::Mean(args) => {
            args.iter().for_each(|a| cb(BuiltinContents::Expr(a)));
        }
        BuiltinFn::Step(a, b) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
        }
        BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
            cb(BuiltinContents::Expr(a));
            if let Some(b) = b {
                cb(BuiltinContents::Expr(b));
            }
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            if let Some(c) = c {
                cb(BuiltinContents::Expr(c))
            }
        }
        BuiltinFn::Rank(a, rest) => {
            cb(BuiltinContents::Expr(a));
            if let Some((b, c)) = rest {
                cb(BuiltinContents::Expr(b));
                if let Some(c) = c {
                    cb(BuiltinContents::Expr(c));
                }
            }
        }
    }
}

#[test]
fn test_is_builtin_fn() {
    assert!(is_builtin_fn("lookup"));
    assert!(!is_builtin_fn("lookupz"));
    assert!(is_builtin_fn("log10"));
    assert!(is_builtin_fn("sum"));
    assert!(is_builtin_fn("rank"));
    assert!(is_builtin_fn("size"));
    assert!(is_builtin_fn("stddev"));
}

#[test]
fn test_is_0_arity_builtin_fn() {
    assert!(!is_0_arity_builtin_fn("lookup"));
    assert!(is_0_arity_builtin_fn("time"));
}

#[test]
fn test_name() {
    enum TestExpr {}
    type Builtin = BuiltinFn<TestExpr>;

    assert_eq!("inf", Builtin::Inf.name());
    assert_eq!("time", Builtin::Time.name());
}
