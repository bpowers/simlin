// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;

/// Loc describes a location in an equation by the starting point and ending point.
/// Equations are strings typed by humans for a single variable -- u16 is long enough.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default, Hash)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub struct UntypedBuiltinFn<Expr>(pub String, pub Vec<Expr>);

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
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
    Quantum(Box<Expr>, Box<Expr>),
    Ramp(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    // ROUND(x): nearest integer, exact .5 ties to the EVEN neighbor
    // (Python round() / IEEE roundTiesToEven). A Simlin extension: the XMILE
    // v1.0 spec defines no ROUND builtin.
    Round(Box<Expr>),
    SafeDiv(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Sign(Box<Expr>),
    Sshape(Box<Expr>, Box<Expr>, Box<Expr>),
    Sin(Box<Expr>),
    Sqrt(Box<Expr>),
    Step(Box<Expr>, Box<Expr>),
    Tan(Box<Expr>),
    Time,
    TimeStep,
    StartTime,
    FinalTime,
    // array-only builtins
    Rank(Box<Expr>, Box<Expr>),
    Size(Box<Expr>),
    Stddev(Box<Expr>),
    Sum(Box<Expr>),
    // VECTOR SELECT(selection_array, expression_array, max_value, action, error_handling)
    VectorSelect(Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>),
    // VECTOR ELM MAP(source_array, offset_array)
    VectorElmMap(Box<Expr>, Box<Expr>),
    // VECTOR SORT ORDER(array, direction)
    VectorSortOrder(Box<Expr>, Box<Expr>),
    // ALLOCATE AVAILABLE(request, priority_profile, avail)
    AllocateAvailable(Box<Expr>, Box<Expr>, Box<Expr>),
    // ALLOCATE BY PRIORITY(request, priority, size, width, supply)
    // Desugars to AllocateAvailable at runtime by constructing rectangular
    // priority profiles: (ptype=1, ppriority=priority[i], pwidth=width, pextra=0)
    AllocateByPriority(Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>),
    // builtins replacing stdlib modules
    Previous(Box<Expr>, Box<Expr>),
    Init(Box<Expr>),
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
            Quantum(_, _) => "quantum",
            Ramp(_, _, _) => "ramp",
            Round(_) => "round",
            SafeDiv(_, _, _) => "safediv",
            Sign(_) => "sign",
            Sshape(_, _, _) => "sshape",
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
            VectorSelect(_, _, _, _, _) => "vector_select",
            VectorElmMap(_, _) => "vector_elm_map",
            VectorSortOrder(_, _) => "vector_sort_order",
            AllocateAvailable(_, _, _) => "allocate_available",
            AllocateByPriority(_, _, _, _, _) => "allocate_by_priority",
            // builtins replacing stdlib modules
            Previous(_, _) => "previous",
            Init(_) => "init",
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
            Quantum(a, b) => Quantum(Box::new(f(*a)?), Box::new(f(*b)?)),
            Ramp(a, b, c) => Ramp(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                c.map(|c| f(*c)).transpose()?.map(Box::new),
            ),
            Round(a) => Round(Box::new(f(*a)?)),
            SafeDiv(a, b, c) => SafeDiv(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                c.map(|c| f(*c)).transpose()?.map(Box::new),
            ),
            Sign(a) => Sign(Box::new(f(*a)?)),
            Sshape(a, b, c) => Sshape(Box::new(f(*a)?), Box::new(f(*b)?), Box::new(f(*c)?)),
            Sin(a) => Sin(Box::new(f(*a)?)),
            Sqrt(a) => Sqrt(Box::new(f(*a)?)),
            Step(a, b) => Step(Box::new(f(*a)?), Box::new(f(*b)?)),
            Tan(a) => Tan(Box::new(f(*a)?)),
            Time => Time,
            TimeStep => TimeStep,
            StartTime => StartTime,
            FinalTime => FinalTime,
            Rank(a, direction) => Rank(Box::new(f(*a)?), Box::new(f(*direction)?)),
            Size(a) => Size(Box::new(f(*a)?)),
            Stddev(a) => Stddev(Box::new(f(*a)?)),
            Sum(a) => Sum(Box::new(f(*a)?)),
            VectorSelect(a, b, c, d, e) => VectorSelect(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                Box::new(f(*c)?),
                Box::new(f(*d)?),
                Box::new(f(*e)?),
            ),
            VectorElmMap(a, b) => VectorElmMap(Box::new(f(*a)?), Box::new(f(*b)?)),
            VectorSortOrder(a, b) => VectorSortOrder(Box::new(f(*a)?), Box::new(f(*b)?)),
            AllocateAvailable(a, b, c) => {
                AllocateAvailable(Box::new(f(*a)?), Box::new(f(*b)?), Box::new(f(*c)?))
            }
            AllocateByPriority(a, b, c, d, e) => AllocateByPriority(
                Box::new(f(*a)?),
                Box::new(f(*b)?),
                Box::new(f(*c)?),
                Box::new(f(*d)?),
                Box::new(f(*e)?),
            ),
            Previous(a, b) => Previous(Box::new(f(*a)?), Box::new(f(*b)?)),
            Init(a) => Init(Box::new(f(*a)?)),
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

    /// Zero every `Loc` this builtin carries ITSELF -- the three lookup forms
    /// and `IsModuleInput` are the only variants with one. Argument
    /// expressions are untouched; a caller normalizing a whole tree strips
    /// those with [`Self::map`].
    ///
    /// Written as an exhaustive match with no catch-all (the `Loc`-free
    /// variants are bound whole through one or-pattern) so a new
    /// `Loc`-carrying variant is a compile error here rather than a silently
    /// retained source position.
    /// NOTE: keep variant coverage in sync with `try_map` above.
    pub(crate) fn strip_own_locs(self) -> Self {
        use BuiltinFn::*;
        match self {
            Lookup(a, b, _) => Lookup(a, b, Loc::default()),
            LookupForward(a, b, _) => LookupForward(a, b, Loc::default()),
            LookupBackward(a, b, _) => LookupBackward(a, b, Loc::default()),
            IsModuleInput(id, _) => IsModuleInput(id, Loc::default()),
            locless @ (Abs(_)
            | Arccos(_)
            | Arcsin(_)
            | Arctan(_)
            | Cos(_)
            | Exp(_)
            | Inf
            | Int(_)
            | Ln(_)
            | Log10(_)
            | Max(_, _)
            | Mean(_)
            | Min(_, _)
            | Pi
            | Pulse(_, _, _)
            | Quantum(_, _)
            | Ramp(_, _, _)
            | Round(_)
            | SafeDiv(_, _, _)
            | Sign(_)
            | Sshape(_, _, _)
            | Sin(_)
            | Sqrt(_)
            | Step(_, _)
            | Tan(_)
            | Time
            | TimeStep
            | StartTime
            | FinalTime
            | Rank(_, _)
            | Size(_)
            | Stddev(_)
            | Sum(_)
            | VectorSelect(_, _, _, _, _)
            | VectorElmMap(_, _)
            | VectorSortOrder(_, _)
            | AllocateAvailable(_, _, _)
            | AllocateByPriority(_, _, _, _, _)
            | Previous(_, _)
            | Init(_)) => locless,
        }
    }

    /// Call a closure on each expression argument by reference.
    /// NOTE: keep variant coverage in sync with `try_map` above.
    pub fn for_each_expr_ref<F>(&self, mut f: F)
    where
        F: FnMut(&Expr),
    {
        use BuiltinFn::*;
        match self {
            Lookup(a, b, _) | LookupForward(a, b, _) | LookupBackward(a, b, _) => {
                f(a);
                f(b);
            }
            Abs(a) | Arccos(a) | Arcsin(a) | Arctan(a) | Cos(a) | Exp(a) | Int(a) | Ln(a)
            | Log10(a) | Round(a) | Sign(a) | Sin(a) | Sqrt(a) | Tan(a) | Size(a) | Stddev(a)
            | Sum(a) | Init(a) => f(a),
            Previous(a, b) => {
                f(a);
                f(b);
            }
            Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {}
            Max(a, b) | Min(a, b) => {
                f(a);
                if let Some(b) = b {
                    f(b);
                }
            }
            Mean(args) => {
                for a in args {
                    f(a);
                }
            }
            Quantum(a, b) => {
                f(a);
                f(b);
            }
            Pulse(a, b, c) | Ramp(a, b, c) | SafeDiv(a, b, c) => {
                f(a);
                f(b);
                if let Some(c) = c {
                    f(c);
                }
            }
            Sshape(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Step(a, b) => {
                f(a);
                f(b);
            }
            Rank(a, direction) => {
                f(a);
                f(direction);
            }
            VectorSelect(a, b, c, d, e) => {
                f(a);
                f(b);
                f(c);
                f(d);
                f(e);
            }
            VectorElmMap(a, b) | VectorSortOrder(a, b) => {
                f(a);
                f(b);
            }
            AllocateAvailable(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            AllocateByPriority(a, b, c, d, e) => {
                f(a);
                f(b);
                f(c);
                f(d);
                f(e);
            }
        }
    }
}

pub fn is_0_arity_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "inf"
            | "pi"
            | "time"
            | "time_step"
            | "dt"
            | "initial_time"
            | "starttime"
            | "final_time"
            | "stoptime"
    )
}

/// ASCII case-insensitive, allocation-free variant of [`is_0_arity_builtin_fn`].
///
/// The 0-arity builtin names are all ASCII, so a name containing any non-ASCII
/// byte cannot match, and ASCII case-folding yields the same membership verdict
/// as Unicode lowercasing for this fixed ASCII set. Used on the hot parse path
/// (`Expr0::reify_0_arity_builtins`), which previously allocated a `String` via
/// `to_lowercase()` for *every* variable reference just to test membership.
pub fn is_0_arity_builtin_fn_ci(name: &str) -> bool {
    const NAMES: [&str; 9] = [
        "inf",
        "pi",
        "time",
        "time_step",
        "dt",
        "initial_time",
        "starttime",
        "final_time",
        "stoptime",
    ];
    NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// Returns true if `func_name` (already lowercased) names a function that
/// expands to a stdlib module: the canonical names in `MODEL_NAMES` plus
/// the alias forms `delay`, `delayn`, and `smthn`.
///
/// This is the authoritative check shared by `equation_is_module_call()`
/// (pre-scan name classification) and `contains_module_call()` (walk-time
/// A2A expansion decision). Each caller adds its own structural logic on
/// top (e.g., PREVIOUS arg-count check, INIT inclusion for A2A).
pub(crate) fn is_stdlib_module_function(func_name: &str) -> bool {
    matches!(func_name, "delay" | "delayn" | "smthn")
        || crate::stdlib::MODEL_NAMES.contains(&func_name)
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
        | "quantum"
        | "ramp"
        | "round"
        | "safediv"
        | "sign"
        | "sin"
        | "sshape"
        | "sqrt"
        | "step"
        | "tan"
        // array-only builtins
        | "rank"
        | "size"
        | "stddev"
        | "sum"
        | "vector_select"
        | "vector_elm_map"
        | "vector_sort_order"
        | "allocate_available"
        | "allocate_by_priority"
        // builtins replacing stdlib modules
        | "previous"
        | "init"
        )
}

pub(crate) enum BuiltinContents<'a, Expr> {
    Ident(&'a str, Loc),
    Expr(&'a Expr),
    /// The table-identity argument of a graphical-function lookup
    /// (`LOOKUP`/`LOOKUP FORWARD`/`LOOKUP BACKWARD`) -- a reference to a
    /// gf-holding variable (a standalone lookup-only table, or a value-bearing
    /// WITH LOOKUP variable's own table), NOT a runtime value input. The static
    /// table data is laid out at compile time independent of the runlist, so
    /// dependency/causal walkers must treat this as a non-edge (a table
    /// reference imposes no runtime data dependency); printing and
    /// source-location walkers treat it like any other argument expression.
    LookupTable(&'a Expr),
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
            cb(BuiltinContents::LookupTable(table_expr));
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
        | BuiltinFn::Round(a)
        | BuiltinFn::Sign(a)
        | BuiltinFn::Sin(a)
        | BuiltinFn::Sqrt(a)
        | BuiltinFn::Tan(a)
        | BuiltinFn::Size(a)
        | BuiltinFn::Stddev(a)
        | BuiltinFn::Sum(a)
        | BuiltinFn::Init(a) => cb(BuiltinContents::Expr(a)),
        BuiltinFn::Previous(a, b) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
        }
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
        BuiltinFn::Quantum(a, b) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            if let Some(c) = c {
                cb(BuiltinContents::Expr(c))
            }
        }
        BuiltinFn::Sshape(a, b, c) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            cb(BuiltinContents::Expr(c));
        }
        BuiltinFn::Rank(a, direction) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(direction));
        }
        BuiltinFn::VectorSelect(a, b, c, d, e) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            cb(BuiltinContents::Expr(c));
            cb(BuiltinContents::Expr(d));
            cb(BuiltinContents::Expr(e));
        }
        BuiltinFn::VectorElmMap(a, b) | BuiltinFn::VectorSortOrder(a, b) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
        }
        BuiltinFn::AllocateAvailable(a, b, c) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            cb(BuiltinContents::Expr(c));
        }
        BuiltinFn::AllocateByPriority(a, b, c, d, e) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            cb(BuiltinContents::Expr(c));
            cb(BuiltinContents::Expr(d));
            cb(BuiltinContents::Expr(e));
        }
    }
}

/// `name()` and `is_builtin_fn` are two halves of one table, and this pins them
/// as an ENUMERATION rather than a sample: every `BuiltinFn` variant is built
/// below, and the `_`-less match makes a newly added variant a COMPILE error
/// until it is listed. The assertions are properties (a name is non-empty, and
/// every name `name()` can emit is one `is_builtin_fn` accepts) rather than a
/// transcription of the match arms, so this cannot rot into a copy of the code
/// under test.
#[test]
fn every_builtin_variant_names_itself_and_is_recognized() {
    type Builtin = BuiltinFn<i32>;
    fn b() -> Box<i32> {
        Box::new(0)
    }

    let all: Vec<Builtin> = vec![
        Builtin::Lookup(b(), b(), Loc::default()),
        Builtin::LookupForward(b(), b(), Loc::default()),
        Builtin::LookupBackward(b(), b(), Loc::default()),
        Builtin::Abs(b()),
        Builtin::Arccos(b()),
        Builtin::Arcsin(b()),
        Builtin::Arctan(b()),
        Builtin::Cos(b()),
        Builtin::Exp(b()),
        Builtin::Inf,
        Builtin::Int(b()),
        Builtin::IsModuleInput("x".to_string(), Loc::default()),
        Builtin::Ln(b()),
        Builtin::Log10(b()),
        Builtin::Max(b(), None),
        Builtin::Mean(vec![]),
        Builtin::Min(b(), None),
        Builtin::Pi,
        Builtin::Pulse(b(), b(), None),
        Builtin::Quantum(b(), b()),
        Builtin::Ramp(b(), b(), None),
        Builtin::Round(b()),
        Builtin::SafeDiv(b(), b(), None),
        Builtin::Sign(b()),
        Builtin::Sshape(b(), b(), b()),
        Builtin::Sin(b()),
        Builtin::Sqrt(b()),
        Builtin::Step(b(), b()),
        Builtin::Tan(b()),
        Builtin::Time,
        Builtin::TimeStep,
        Builtin::StartTime,
        Builtin::FinalTime,
        Builtin::Rank(b(), b()),
        Builtin::Size(b()),
        Builtin::Stddev(b()),
        Builtin::Sum(b()),
        Builtin::VectorSelect(b(), b(), b(), b(), b()),
        Builtin::VectorElmMap(b(), b()),
        Builtin::VectorSortOrder(b(), b()),
        Builtin::AllocateAvailable(b(), b(), b()),
        Builtin::AllocateByPriority(b(), b(), b(), b(), b()),
        Builtin::Previous(b(), b()),
        Builtin::Init(b()),
    ];

    // No `_` arm: this is what turns "every variant is covered" into a property
    // the compiler checks rather than a claim the row count implies.
    for v in &all {
        match v {
            Builtin::Lookup(..)
            | Builtin::LookupForward(..)
            | Builtin::LookupBackward(..)
            | Builtin::Abs(..)
            | Builtin::Arccos(..)
            | Builtin::Arcsin(..)
            | Builtin::Arctan(..)
            | Builtin::Cos(..)
            | Builtin::Exp(..)
            | Builtin::Inf
            | Builtin::Int(..)
            | Builtin::IsModuleInput(..)
            | Builtin::Ln(..)
            | Builtin::Log10(..)
            | Builtin::Max(..)
            | Builtin::Mean(..)
            | Builtin::Min(..)
            | Builtin::Pi
            | Builtin::Pulse(..)
            | Builtin::Quantum(..)
            | Builtin::Ramp(..)
            | Builtin::Round(..)
            | Builtin::SafeDiv(..)
            | Builtin::Sign(..)
            | Builtin::Sshape(..)
            | Builtin::Sin(..)
            | Builtin::Sqrt(..)
            | Builtin::Step(..)
            | Builtin::Tan(..)
            | Builtin::Time
            | Builtin::TimeStep
            | Builtin::StartTime
            | Builtin::FinalTime
            | Builtin::Rank(..)
            | Builtin::Size(..)
            | Builtin::Stddev(..)
            | Builtin::Sum(..)
            | Builtin::VectorSelect(..)
            | Builtin::VectorElmMap(..)
            | Builtin::VectorSortOrder(..)
            | Builtin::AllocateAvailable(..)
            | Builtin::AllocateByPriority(..)
            | Builtin::Previous(..)
            | Builtin::Init(..) => {}
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    for v in &all {
        let name = v.name();
        assert!(!name.is_empty(), "a variant reported an empty name");
        assert!(
            is_builtin_fn(name),
            "name() emits {name:?} but is_builtin_fn rejects it"
        );
        assert!(seen.insert(name), "two variants share the name {name:?}");
    }

    assert!(!is_builtin_fn("lookupz"));
    assert!(!is_0_arity_builtin_fn("lookup"));
    assert!(is_0_arity_builtin_fn("time"));
}

#[test]
fn test_is_0_arity_builtin_fn_ci() {
    const NAMES: [&str; 9] = [
        "inf",
        "pi",
        "time",
        "time_step",
        "dt",
        "initial_time",
        "starttime",
        "final_time",
        "stoptime",
    ];
    for name in NAMES {
        assert!(is_0_arity_builtin_fn_ci(name), "lowercase {name}");
        assert!(
            is_0_arity_builtin_fn_ci(&name.to_uppercase()),
            "uppercase {name}"
        );
    }
    assert!(is_0_arity_builtin_fn_ci("Time"));
    assert!(is_0_arity_builtin_fn_ci("Final_Time"));
    assert!(!is_0_arity_builtin_fn_ci("lookup"));
    assert!(!is_0_arity_builtin_fn_ci("times"));
    assert!(!is_0_arity_builtin_fn_ci(""));
    // A non-ASCII name can never match (every builtin name is ASCII).
    assert!(!is_0_arity_builtin_fn_ci("pï"));

    // Equivalent to to_lowercase() + is_0_arity_builtin_fn for any ASCII input,
    // which is the behavior the hot-path caller relies on.
    for s in [
        "TIME",
        "Pi",
        "Dt",
        "Final_Time",
        "STOPTIME",
        "foo",
        "lookuptable",
        "timestep",
    ] {
        assert_eq!(
            is_0_arity_builtin_fn_ci(s),
            is_0_arity_builtin_fn(&s.to_lowercase()),
            "ci/lowercase mismatch for {s}"
        );
    }
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
