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
    Lookup(Box<Expr>, Box<Expr>, Loc),
    LookupForward(Box<Expr>, Box<Expr>, Loc),
    LookupBackward(Box<Expr>, Box<Expr>, Loc),
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
    Sign(Box<Expr>),
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
            LookupForward(_, _, _) => "lookup_forward",
            LookupBackward(_, _, _) => "lookup_backward",
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
            Sign(_) => "sign",
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

    /// Transform all expression arguments in this builtin using the provided function.
    /// Returns an error if any transformation fails.
    pub fn try_map<F, E2, Err>(self, mut f: F) -> std::result::Result<BuiltinFn<E2>, Err>
    where
        F: FnMut(Expr) -> std::result::Result<E2, Err>,
    {
        use BuiltinFn::*;
        Ok(match self {
            Lookup(table_expr, index_expr, loc) => {
                Lookup(Box::new(f(*table_expr)?), Box::new(f(*index_expr)?), loc)
            }
            LookupForward(table_expr, index_expr, loc) => {
                LookupForward(Box::new(f(*table_expr)?), Box::new(f(*index_expr)?), loc)
            }
            LookupBackward(table_expr, index_expr, loc) => {
                LookupBackward(Box::new(f(*table_expr)?), Box::new(f(*index_expr)?), loc)
            }
            Abs(a) => Abs(Box::new(f(*a)?)),
            Arccos(a) => Arccos(Box::new(f(*a)?)),
            Arcsin(a) => Arcsin(Box::new(f(*a)?)),
            Arctan(a) => Arctan(Box::new(f(*a)?)),
            Cos(a) => Cos(Box::new(f(*a)?)),
            Exp(a) => Exp(Box::new(f(*a)?)),
            Inf => Inf,
            Int(a) => Int(Box::new(f(*a)?)),
            IsModuleInput(id, loc) => IsModuleInput(id, loc),
            Ln(a) => Ln(Box::new(f(*a)?)),
            Log10(a) => Log10(Box::new(f(*a)?)),
            Max(a, b) => Max(
                Box::new(f(*a)?),
                b.map(|b| f(*b)).transpose()?.map(Box::new),
            ),
            Mean(args) => Mean(
                args.into_iter()
                    .map(&mut f)
                    .collect::<std::result::Result<_, _>>()?,
            ),
            Min(a, b) => Min(
                Box::new(f(*a)?),
                b.map(|b| f(*b)).transpose()?.map(Box::new),
            ),
            Pi => Pi,
            Pulse(a, b, c) => Pulse(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                c.map(|c| f(*c)).transpose()?.map(Box::new),
            ),
            Ramp(a, b, c) => Ramp(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                c.map(|c| f(*c)).transpose()?.map(Box::new),
            ),
            SafeDiv(a, b, c) => SafeDiv(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                c.map(|c| f(*c)).transpose()?.map(Box::new),
            ),
            Sign(a) => Sign(Box::new(f(*a)?)),
            Sin(a) => Sin(Box::new(f(*a)?)),
            Sqrt(a) => Sqrt(Box::new(f(*a)?)),
            Step(a, b) => Step(Box::new(f(*a)?), Box::new(f(*b)?)),
            Tan(a) => Tan(Box::new(f(*a)?)),
            Time => Time,
            TimeStep => TimeStep,
            StartTime => StartTime,
            FinalTime => FinalTime,
            Rank(a, rest) => Rank(
                Box::new(f(*a)?),
                rest.map(|(b, c)| {
                    Ok::<_, Err>((
                        Box::new(f(*b)?),
                        c.map(|c| f(*c)).transpose()?.map(Box::new),
                    ))
                })
                .transpose()?,
            ),
            Size(a) => Size(Box::new(f(*a)?)),
            Stddev(a) => Stddev(Box::new(f(*a)?)),
            Sum(a) => Sum(Box::new(f(*a)?)),
        })
    }

    /// Transform all expression arguments in this builtin using the provided function.
    /// Infallible version of try_map.
    pub fn map<F, E2>(self, mut f: F) -> BuiltinFn<E2>
    where
        F: FnMut(Expr) -> E2,
    {
        self.try_map(|e| Ok::<_, std::convert::Infallible>(f(e)))
            .unwrap()
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
        | "lookup_forward"
        | "lookup_backward"
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
        | "sign"
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
        BuiltinFn::Lookup(table_expr, index_expr, _loc)
        | BuiltinFn::LookupForward(table_expr, index_expr, _loc)
        | BuiltinFn::LookupBackward(table_expr, index_expr, _loc) => {
            cb(BuiltinContents::Expr(table_expr));
            cb(BuiltinContents::Expr(index_expr));
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
        | BuiltinFn::Sign(a)
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

#[test]
fn test_map() {
    // Test that map correctly transforms expression types
    let builtin: BuiltinFn<i32> = BuiltinFn::Abs(Box::new(42));
    let mapped: BuiltinFn<String> = builtin.map(|x| x.to_string());
    assert_eq!(mapped.name(), "abs");
    if let BuiltinFn::Abs(x) = mapped {
        assert_eq!(*x, "42");
    } else {
        panic!("expected Abs variant");
    }
}

#[test]
fn test_map_0_arity() {
    // Test that 0-arity builtins work with map
    let builtin: BuiltinFn<i32> = BuiltinFn::Time;
    let mapped: BuiltinFn<String> = builtin.map(|x| x.to_string());
    assert!(matches!(mapped, BuiltinFn::Time));
}

#[test]
fn test_try_map_success() {
    let builtin: BuiltinFn<i32> = BuiltinFn::Max(Box::new(10), Some(Box::new(20)));
    let result: Result<BuiltinFn<i64>, &str> = builtin.try_map(|x| Ok(x as i64 * 2));
    assert!(result.is_ok());
    if let Ok(BuiltinFn::Max(a, Some(b))) = result {
        assert_eq!(*a, 20);
        assert_eq!(*b, 40);
    } else {
        panic!("expected Max variant with two args");
    }
}

#[test]
fn test_try_map_failure() {
    let builtin: BuiltinFn<i32> = BuiltinFn::Abs(Box::new(42));
    let result: Result<BuiltinFn<i64>, &str> = builtin.try_map(|_| Err("error"));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "error");
}

#[test]
fn test_map_mean_vec() {
    // Test that Mean with Vec<Expr> is correctly transformed
    let builtin: BuiltinFn<i32> = BuiltinFn::Mean(vec![1, 2, 3]);
    let mapped: BuiltinFn<i32> = builtin.map(|x| x * 10);
    if let BuiltinFn::Mean(args) = mapped {
        assert_eq!(args, vec![10, 20, 30]);
    } else {
        panic!("expected Mean variant");
    }
}
