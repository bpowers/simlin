// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The typed builtin-function AST node and the one table of per-builtin facts.
//!
//! [`BuiltinFn::signature`] is the single statement of what each builtin IS --
//! its name and aliases, its source arity, how each argument position takes
//! part in array lowering ([`ArgKind`]), the shape of its result
//! ([`ResultKind`]) and how its value relates to simulation time
//! ([`Invariance`]). [`BuiltinFn::args`] / [`BuiltinFn::args_mut`] give every
//! consumer a uniform view of the argument expressions, so a pass that only
//! needs "visit every argument" or "which positions are arrays" reads the table
//! instead of enumerating the variants. The exhaustive matches that remain
//! elsewhere in the crate each encode per-variant semantics (which opcode,
//! which unit rule, which argument shapes the result) and say so.
//!
//! `BuiltinId::arity()` in `bytecode.rs` is the VM-side twin of this table for
//! the 22 builtins that execute through `Opcode::Apply`.

use std::fmt;
use std::sync::LazyLock;

use smallvec::SmallVec;

use crate::common::IdentMap;

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
/// An unresolved builtin call: the function's lowercased name and its
/// arguments. The arguments are a `Box<[Expr]>` rather than a `Vec` so a
/// node holds exactly the arguments it has: an argument list is fixed once
/// parsed, and a `Vec` grown by `push` keeps `Vec`'s minimum capacity of four
/// slots, which across C-LEARN's LTM parse trees is 45 MiB of empty capacity
/// (`docs/design/engine-performance.md`, C7).
pub struct UntypedBuiltinFn<Expr>(pub String, pub Box<[Expr]>);

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
    // (Python round() / IEEE roundTiesToEven). The XMILE v1.0 spec defines no
    // ROUND builtin (its function catalog stops at INT, which footnote 7
    // mandates as floor). Python's semantics are the REQUIREMENT here, a
    // product decision -- ties-to-even regardless of what other tools do.
    // Disclosure, not a caveat to revisit: Stella also defines ROUND ("rounds
    // expression to its nearest integer value", isee Stella docs, tie rule
    // unspecified), so a Stella-authored model calling ROUND imports and
    // simulates under these semantics instead of failing with UnknownBuiltin;
    // whether Stella agrees at exact .5 ties is unverified, and matching
    // Stella is explicitly a non-goal (if that ever changes, verify its tie
    // rule against ground-truth output first). Vensim defines no ROUND at
    // all, and the MDL writer records an ExportWarning for it
    // (mdl/writer.rs).
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

/// How one argument position takes part in array lowering.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// A scalar value, evaluated once per element of the enclosing equation.
    Scalar,
    /// An array the builtin consumes as one operand: the materializer
    /// (`compiler::array_operand`) moves a computed expression in this position
    /// into a temp, codegen reads the position as a view, and `Expr2` lowering
    /// lets sub-expressions inside it union disjoint named dimensions
    /// (`SUM(a[*] + h[*])` over `a[DimA]` and `h[DimC]` is a cross-product
    /// sum).
    ///
    /// `whole` says how an apply-to-all element reference inside the operand
    /// resolves. `true`: the builtin reads the argument's entire storage
    /// regardless of the enclosing element, so `vals[D]` in an equation over
    /// `D` is promoted back to the full axis (`VECTOR SORT ORDER`, `RANK`,
    /// `VECTOR ELM MAP`, `ALLOCATE *`). `false`: the enclosing element pins the
    /// axes it names and the builtin iterates the rest, so
    /// `SUM(matrix[D, *])` in an equation over `D` sums one row per element
    /// (the reducers, and `VECTOR SELECT`, which reduces the `*`-marked axis
    /// the same way).
    Array { whole: bool },
    /// The graphical-function identity of a `LOOKUP` family call: a reference
    /// to the gf-bearing variable, laid out at compile time, not a runtime
    /// value. The stages treat it as follows, and a stage added later (the
    /// Phase 6 materializer among them) inherits the same rule:
    ///
    /// - pass 0 (`Context::lower_pass0`) resolves a BARE arrayed table
    ///   reference exactly as it resolves a bare arrayed variable -- the
    ///   enclosing apply-to-all element pins the axes it iterates and every
    ///   other axis becomes a wildcard (`make_dimension_subscripts`), so
    ///   `out[COP] = LOOKUP(g, t)` over `g[COP]` applies each element's table
    ///   and `out[COP] = SUM(LOOKUP(g, t))` over `g[COP, ROW]` sums the
    ///   element's row (pinned in `per_element_gf_tests`);
    /// - `compiler::array_operand` reads the LOWERED table's shape to decide
    ///   whether the call is a per-element arrayed-GF apply (a multi-element
    ///   table array) or the ordinary scalar lookup (one table);
    /// - `Context::lower_builtin_expr3` lowers it like a reducer's operand: the
    ///   enclosing element pins the axes it names and a free axis survives as a
    ///   view, which is what makes the apply array-valued;
    /// - `compiler::array_operand::materialize_view_operands` leaves it alone
    ///   (a temp carries no graphical functions);
    /// - codegen reads the table's base off the lowered reference
    ///   (`extract_table_info`, `arrayed_lookup_table_info`);
    /// - dependency and causal walkers see it as
    ///   [`BuiltinContents::LookupTable`], a non-edge.
    Table,
    /// `isModuleInput(x)`'s payload: a bare identifier the parser keeps as a
    /// `String`, never an expression. It counts toward the source arity but
    /// yields no entry from [`BuiltinFn::args`] or [`BuiltinFn::arg_kinds`].
    Ident,
}

/// The shape of a builtin's result, as the apply-to-all hoister and the array
/// materializer read it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    /// One number: the reducers, `VECTOR SELECT`, the lookups, the nullaries.
    Scalar,
    /// The join of its argument shapes: an elementwise builtin is applied
    /// inside a `BeginIter` body, so its result has whatever shape its
    /// arguments carry.
    Elementwise,
    /// An array a dedicated opcode writes into a temp (`VECTOR ELM MAP`,
    /// `VECTOR SORT ORDER`, `RANK`, `ALLOCATE *`); `shape_from` is the
    /// position of the argument whose view sizes that temp.
    Array { shape_from: u8 },
}

/// How a builtin's value relates to simulation time, read by the
/// run-invariance classifier (`compiler::invariance`); `Snapshot` and
/// `Lagged` are the two forms whose arguments the dependency walk records
/// under a lag (`variable::DepLag`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Invariance {
    /// A pure function of its arguments -- invariant iff every argument is.
    /// The fixed time globals `DT`/`INITIAL TIME`/`FINAL TIME` are here too:
    /// they have no arguments and do not change across a run.
    Pure,
    /// Varies with time even when every argument is constant: `TIME`,
    /// `PULSE`, `RAMP`, `STEP`.
    TimeDependent,
    /// Reads the previous step's snapshot (`PREVIOUS`): variant whatever its
    /// argument is, and the argument is not a dt-time read of the variable.
    Lagged,
    /// Reads the initial-values snapshot (`INIT`): invariant whatever its
    /// argument is, because that buffer is frozen after the initials phase,
    /// so the argument is not a dt-time dependency.
    Snapshot,
}

/// Everything the compiler knows about one builtin function that is not the
/// argument expressions themselves. One `static` per [`BuiltinFn`] variant,
/// reached through [`BuiltinFn::signature`] and listed in
/// [`BuiltinSig::ALL`].
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct BuiltinSig {
    /// The canonical lowercase name, as the lexer produces it and as
    /// [`BuiltinFn::name`] reports it.
    pub name: &'static str,
    /// Other spellings the parser accepts for the same builtin.
    pub aliases: &'static [&'static str],
    /// The fewest arguments a call may spell (the source arity; an
    /// [`ArgKind::Ident`] position counts).
    pub min_args: u8,
    /// The most arguments a call may spell; `None` for a variadic builtin.
    pub max_args: Option<u8>,
    /// One kind per position of the call as written, up to `max_args`. A
    /// variadic builtin lists one kind, which every position takes. For a
    /// builtin whose one-argument form is a reduction (`unary_reduces`) these
    /// are the kinds of the n-ary form; [`BuiltinFn::arg_kinds`] applies the
    /// reduction rule for the value at hand.
    pub arg_kinds: &'static [ArgKind],
    /// The one-argument form is an array reduction: its single argument is
    /// `ArgKind::Array { whole: false }` and its result `ResultKind::Scalar`,
    /// whatever `arg_kinds` and `result` say for the n-ary form. XMILE v1.0
    /// section 3.7.1.3 defines `MAX(A)` / `MIN(A)` this way ("extends
    /// MAX(x, y)") and `MEAN(A)` as `SUM(A)/SIZE(A)`; the n-ary forms HERE are
    /// the scalar `MAX(x, y)` / `MIN(x, y)` of section 3.5 and Stella's scalar
    /// mean of the arguments. The spec's mixed form -- 3.7.1.3's second
    /// parameter "any mix of arrays and scalars", `MAX(A, 0)` reducing over `A`
    /// and `0` -- is not implemented (GH #1026).
    pub unary_reduces: bool,
    /// The shape of the result of the n-ary form (see `unary_reduces`).
    pub result: ResultKind,
    pub invariance: Invariance,
}

const SCALAR: ArgKind = ArgKind::Scalar;
/// A reducer's operand: the enclosing apply-to-all element pins its axes.
const REDUCED: ArgKind = ArgKind::Array { whole: false };
/// A vector builtin's operand: read whole, independent of the enclosing element.
const WHOLE: ArgKind = ArgKind::Array { whole: true };

const fn sig(
    name: &'static str,
    min_args: u8,
    max_args: Option<u8>,
    arg_kinds: &'static [ArgKind],
    result: ResultKind,
    invariance: Invariance,
) -> BuiltinSig {
    BuiltinSig {
        name,
        aliases: &[],
        min_args,
        max_args,
        arg_kinds,
        unary_reduces: false,
        result,
        invariance,
    }
}

/// A one-argument elementwise function of a scalar.
const fn unary_math(name: &'static str) -> BuiltinSig {
    sig(
        name,
        1,
        Some(1),
        &[SCALAR],
        ResultKind::Elementwise,
        Invariance::Pure,
    )
}

/// A one-argument array reduction to a scalar.
const fn reducer(name: &'static str) -> BuiltinSig {
    sig(
        name,
        1,
        Some(1),
        &[REDUCED],
        ResultKind::Scalar,
        Invariance::Pure,
    )
}

/// A fixed time global: no arguments, one value for the whole run.
const fn time_global(name: &'static str, aliases: &'static [&'static str]) -> BuiltinSig {
    BuiltinSig {
        name,
        aliases,
        min_args: 0,
        max_args: Some(0),
        arg_kinds: &[],
        unary_reduces: false,
        result: ResultKind::Scalar,
        invariance: Invariance::Pure,
    }
}

static LOOKUP: BuiltinSig = sig(
    "lookup",
    2,
    Some(2),
    &[ArgKind::Table, SCALAR],
    ResultKind::Scalar,
    Invariance::Pure,
);
static LOOKUP_FORWARD: BuiltinSig = sig(
    "lookup_forward",
    2,
    Some(2),
    &[ArgKind::Table, SCALAR],
    ResultKind::Scalar,
    Invariance::Pure,
);
static LOOKUP_BACKWARD: BuiltinSig = sig(
    "lookup_backward",
    2,
    Some(2),
    &[ArgKind::Table, SCALAR],
    ResultKind::Scalar,
    Invariance::Pure,
);
static ABS: BuiltinSig = unary_math("abs");
static ARCCOS: BuiltinSig = unary_math("arccos");
static ARCSIN: BuiltinSig = unary_math("arcsin");
static ARCTAN: BuiltinSig = unary_math("arctan");
static COS: BuiltinSig = unary_math("cos");
static EXP: BuiltinSig = unary_math("exp");
static INF: BuiltinSig = sig("inf", 0, Some(0), &[], ResultKind::Scalar, Invariance::Pure);
static INT: BuiltinSig = unary_math("int");
static IS_MODULE_INPUT: BuiltinSig = sig(
    "ismoduleinput",
    1,
    Some(1),
    &[ArgKind::Ident],
    ResultKind::Scalar,
    Invariance::Pure,
);
static LN: BuiltinSig = unary_math("ln");
static LOG10: BuiltinSig = unary_math("log10");
static MAX: BuiltinSig = BuiltinSig {
    name: "max",
    aliases: &[],
    min_args: 1,
    max_args: Some(2),
    arg_kinds: &[SCALAR, SCALAR],
    unary_reduces: true,
    result: ResultKind::Elementwise,
    invariance: Invariance::Pure,
};
/// n-ary `MEAN` is Stella's scalar mean of its arguments. XMILE v1.0 defines
/// only the one-argument array mean (section 3.7.1.3); the n-ary form's ground
/// truth is in-repo: `test/test-models/tests/builtin_mean/builtin_mean.stmx`
/// (Stella Professional 1.9.4) evaluates `MEAN(1, 2, ..., 9, TIME)` and its
/// Stella-produced `output.tab` gives 4.6 at `TIME = 1`, and
/// `test/modules2/modules2.xmile` (Stella Architect 2.0) uses a two-argument
/// `MEAN`. Codegen sums the arguments as scalars and divides, so the result is
/// `Scalar` at every arity (never the elementwise join an n-ary `MAX` has).
static MEAN: BuiltinSig = BuiltinSig {
    name: "mean",
    aliases: &[],
    min_args: 0,
    max_args: None,
    arg_kinds: &[SCALAR],
    unary_reduces: true,
    result: ResultKind::Scalar,
    invariance: Invariance::Pure,
};
static MIN: BuiltinSig = BuiltinSig {
    name: "min",
    aliases: &[],
    min_args: 1,
    max_args: Some(2),
    arg_kinds: &[SCALAR, SCALAR],
    unary_reduces: true,
    result: ResultKind::Elementwise,
    invariance: Invariance::Pure,
};
static PI: BuiltinSig = sig("pi", 0, Some(0), &[], ResultKind::Scalar, Invariance::Pure);
static PULSE: BuiltinSig = sig(
    "pulse",
    2,
    Some(3),
    &[SCALAR, SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::TimeDependent,
);
static QUANTUM: BuiltinSig = sig(
    "quantum",
    2,
    Some(2),
    &[SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::Pure,
);
static RAMP: BuiltinSig = sig(
    "ramp",
    2,
    Some(3),
    &[SCALAR, SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::TimeDependent,
);
static ROUND: BuiltinSig = unary_math("round");
static SAFEDIV: BuiltinSig = sig(
    "safediv",
    2,
    Some(3),
    &[SCALAR, SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::Pure,
);
static SIGN: BuiltinSig = unary_math("sign");
static SSHAPE: BuiltinSig = sig(
    "sshape",
    3,
    Some(3),
    &[SCALAR, SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::Pure,
);
static SIN: BuiltinSig = unary_math("sin");
static SQRT: BuiltinSig = unary_math("sqrt");
static STEP: BuiltinSig = sig(
    "step",
    2,
    Some(2),
    &[SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::TimeDependent,
);
static TAN: BuiltinSig = unary_math("tan");
static TIME: BuiltinSig = sig(
    "time",
    0,
    Some(0),
    &[],
    ResultKind::Scalar,
    Invariance::TimeDependent,
);
static TIME_STEP: BuiltinSig = time_global("time_step", &["dt"]);
static INITIAL_TIME: BuiltinSig = time_global("initial_time", &["starttime"]);
static FINAL_TIME: BuiltinSig = time_global("final_time", &["stoptime"]);
static RANK: BuiltinSig = sig(
    "rank",
    2,
    Some(2),
    &[WHOLE, SCALAR],
    ResultKind::Array { shape_from: 0 },
    Invariance::Pure,
);
static SIZE: BuiltinSig = reducer("size");
static STDDEV: BuiltinSig = reducer("stddev");
static SUM: BuiltinSig = reducer("sum");
/// `VECTOR SELECT` reduces the range its operands mark to one number, so its two
/// array positions are REDUCED, not WHOLE: the enclosing element pins the axes
/// it names and the builtin iterates the rest. Vensim spells the iterated range
/// with `!` and leaves every other subscript an element -- genuine Vensim output
/// in `test/sdeverywhere/models/vector/` runs
/// `q[DimB] = VECTOR SELECT(e[DimA!,DimB], c[DimA!], 0, VSSUM, VSERRNONE)` and
/// `r[DimA] = VECTOR SELECT(e[DimA,DimB!], d[DimA,DimB!], :NA:, VSMAX, VSERRNONE)`,
/// where the LHS's own dimension is an element of the operand and only the
/// `!`-marked axis is summed. Simlin spells the range `*`, and `q`'s `DimB`
/// resolving to the active element is what makes the two spellings agree.
static VECTOR_SELECT: BuiltinSig = sig(
    "vector_select",
    5,
    Some(5),
    &[REDUCED, REDUCED, SCALAR, SCALAR, SCALAR],
    ResultKind::Scalar,
    Invariance::Pure,
);
static VECTOR_ELM_MAP: BuiltinSig = sig(
    "vector_elm_map",
    2,
    Some(2),
    &[WHOLE, WHOLE],
    ResultKind::Array { shape_from: 1 },
    Invariance::Pure,
);
static VECTOR_SORT_ORDER: BuiltinSig = sig(
    "vector_sort_order",
    2,
    Some(2),
    &[WHOLE, SCALAR],
    ResultKind::Array { shape_from: 0 },
    Invariance::Pure,
);
static ALLOCATE_AVAILABLE: BuiltinSig = sig(
    "allocate_available",
    3,
    Some(3),
    &[WHOLE, WHOLE, SCALAR],
    ResultKind::Array { shape_from: 0 },
    Invariance::Pure,
);
static ALLOCATE_BY_PRIORITY: BuiltinSig = sig(
    "allocate_by_priority",
    5,
    Some(5),
    &[WHOLE, WHOLE, SCALAR, SCALAR, SCALAR],
    ResultKind::Array { shape_from: 0 },
    Invariance::Pure,
);
/// `PREVIOUS(x)` is spelled with one argument and desugared to
/// `PREVIOUS(x, 0)` by `Expr1` lowering, so the typed node always carries two.
static PREVIOUS: BuiltinSig = sig(
    "previous",
    1,
    Some(2),
    &[SCALAR, SCALAR],
    ResultKind::Elementwise,
    Invariance::Lagged,
);
static INIT: BuiltinSig = sig(
    "init",
    1,
    Some(1),
    &[SCALAR],
    ResultKind::Elementwise,
    Invariance::Snapshot,
);

impl BuiltinSig {
    /// Every signature, one per [`BuiltinFn`] variant, in declaration order.
    pub const ALL: [&'static BuiltinSig; 44] = [
        &LOOKUP,
        &LOOKUP_FORWARD,
        &LOOKUP_BACKWARD,
        &ABS,
        &ARCCOS,
        &ARCSIN,
        &ARCTAN,
        &COS,
        &EXP,
        &INF,
        &INT,
        &IS_MODULE_INPUT,
        &LN,
        &LOG10,
        &MAX,
        &MEAN,
        &MIN,
        &PI,
        &PULSE,
        &QUANTUM,
        &RAMP,
        &ROUND,
        &SAFEDIV,
        &SIGN,
        &SSHAPE,
        &SIN,
        &SQRT,
        &STEP,
        &TAN,
        &TIME,
        &TIME_STEP,
        &INITIAL_TIME,
        &FINAL_TIME,
        &RANK,
        &SIZE,
        &STDDEV,
        &SUM,
        &VECTOR_SELECT,
        &VECTOR_ELM_MAP,
        &VECTOR_SORT_ORDER,
        &ALLOCATE_AVAILABLE,
        &ALLOCATE_BY_PRIORITY,
        &PREVIOUS,
        &INIT,
    ];

    /// The signature a (lowercased) source name or alias denotes.
    ///
    /// The name half of the table is stated once, in the signatures; this is
    /// an index over [`Self::ALL`] built on first use, because the parse path
    /// looks every call up here (once per builtin application per variable
    /// parse) and a scan of the table would cost it more than the typed node
    /// it builds.
    pub fn by_name(name: &str) -> Option<&'static BuiltinSig> {
        static BY_NAME: LazyLock<IdentMap<&'static str, &'static BuiltinSig>> =
            LazyLock::new(|| {
                BuiltinSig::ALL
                    .iter()
                    .flat_map(|sig| {
                        std::iter::once(sig.name)
                            .chain(sig.aliases.iter().copied())
                            .map(move |name| (name, *sig))
                    })
                    .collect()
            });
        BY_NAME.get(name).copied()
    }

    /// Whether a call spelled with `n` arguments is well-formed.
    pub fn accepts_arity(&self, n: usize) -> bool {
        n >= self.min_args as usize && self.max_args.is_none_or(|max| n <= max as usize)
    }

    /// The accepted arity as a noun phrase for a diagnostic ("2 arguments",
    /// "1 or 2 arguments", "2 to 4 arguments", "at least 1 argument"). Reads
    /// the same two fields `accepts_arity` does, so the message a rejected
    /// call carries cannot describe a different range than the one that
    /// rejected it; the noun agrees with the last number the phrase names, so
    /// no caller has to spell "argument(s)".
    pub fn arity_phrase(&self) -> String {
        let (last, range) = match self.max_args {
            None => (self.min_args, format!("at least {}", self.min_args)),
            Some(max) if max == self.min_args => (max, max.to_string()),
            Some(max) if max == self.min_args + 1 => (max, format!("{} or {max}", self.min_args)),
            Some(max) => (max, format!("{} to {max}", self.min_args)),
        };
        let plural = if last == 1 { "" } else { "s" };
        format!("{range} argument{plural}")
    }
}

/// The argument expressions of a builtin, in call order, as shared or mutable
/// references. One body serves both: `$iter` is `iter` or `iter_mut` for the
/// variadic `Mean`, and the optional `mut` token selects `&**a` or `&mut **a`
/// for the boxed positions.
macro_rules! builtin_args {
    ($builtin:expr, $iter:ident $(, $m:tt)?) => {{
        use BuiltinFn::*;
        let mut out = SmallVec::new();
        match $builtin {
            Lookup(a, b, _) | LookupForward(a, b, _) | LookupBackward(a, b, _) => {
                out.push(&$($m)? **a);
                out.push(&$($m)? **b);
            }
            Abs(a) | Arccos(a) | Arcsin(a) | Arctan(a) | Cos(a) | Exp(a) | Int(a) | Ln(a)
            | Log10(a) | Round(a) | Sign(a) | Sin(a) | Sqrt(a) | Tan(a) | Size(a) | Stddev(a)
            | Sum(a) | Init(a) => out.push(&$($m)? **a),
            Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {}
            Max(a, b) | Min(a, b) => {
                out.push(&$($m)? **a);
                if let Some(b) = b {
                    out.push(&$($m)? **b);
                }
            }
            Mean(args) => {
                for a in args.$iter() {
                    out.push(a);
                }
            }
            Pulse(a, b, c) | Ramp(a, b, c) | SafeDiv(a, b, c) => {
                out.push(&$($m)? **a);
                out.push(&$($m)? **b);
                if let Some(c) = c {
                    out.push(&$($m)? **c);
                }
            }
            Quantum(a, b) | Step(a, b) | Rank(a, b) | VectorElmMap(a, b) | VectorSortOrder(a, b)
            | Previous(a, b) => {
                out.push(&$($m)? **a);
                out.push(&$($m)? **b);
            }
            Sshape(a, b, c) | AllocateAvailable(a, b, c) => {
                out.push(&$($m)? **a);
                out.push(&$($m)? **b);
                out.push(&$($m)? **c);
            }
            VectorSelect(a, b, c, d, e) | AllocateByPriority(a, b, c, d, e) => {
                out.push(&$($m)? **a);
                out.push(&$($m)? **b);
                out.push(&$($m)? **c);
                out.push(&$($m)? **d);
                out.push(&$($m)? **e);
            }
        }
        out
    }};
}

/// Rebuild a builtin of one expression type from a builtin of another, calling
/// `$f` on every argument. One body serves the by-value and by-reference
/// forms: `$arg`/`$opt`/`$many` say how a required, optional, or variadic
/// position is passed to `$f`, and `$copy`/`$clone` how a `Loc` or the
/// `IsModuleInput` identifier crosses over.
macro_rules! builtin_rebuild {
    ($builtin:expr, $f:ident, $arg:ident, $opt:ident, $many:ident, $copy:ident, $clone:ident) => {{
        use BuiltinFn::*;
        match $builtin {
            Lookup(a, b, loc) => Lookup($arg!($f, a), $arg!($f, b), $copy!(loc)),
            LookupForward(a, b, loc) => LookupForward($arg!($f, a), $arg!($f, b), $copy!(loc)),
            LookupBackward(a, b, loc) => LookupBackward($arg!($f, a), $arg!($f, b), $copy!(loc)),
            Abs(a) => Abs($arg!($f, a)),
            Arccos(a) => Arccos($arg!($f, a)),
            Arcsin(a) => Arcsin($arg!($f, a)),
            Arctan(a) => Arctan($arg!($f, a)),
            Cos(a) => Cos($arg!($f, a)),
            Exp(a) => Exp($arg!($f, a)),
            Inf => Inf,
            Int(a) => Int($arg!($f, a)),
            IsModuleInput(id, loc) => IsModuleInput($clone!(id), $copy!(loc)),
            Ln(a) => Ln($arg!($f, a)),
            Log10(a) => Log10($arg!($f, a)),
            Max(a, b) => Max($arg!($f, a), $opt!($f, b)),
            Mean(args) => Mean($many!($f, args)),
            Min(a, b) => Min($arg!($f, a), $opt!($f, b)),
            Pi => Pi,
            Pulse(a, b, c) => Pulse($arg!($f, a), $arg!($f, b), $opt!($f, c)),
            Quantum(a, b) => Quantum($arg!($f, a), $arg!($f, b)),
            Ramp(a, b, c) => Ramp($arg!($f, a), $arg!($f, b), $opt!($f, c)),
            Round(a) => Round($arg!($f, a)),
            SafeDiv(a, b, c) => SafeDiv($arg!($f, a), $arg!($f, b), $opt!($f, c)),
            Sign(a) => Sign($arg!($f, a)),
            Sshape(a, b, c) => Sshape($arg!($f, a), $arg!($f, b), $arg!($f, c)),
            Sin(a) => Sin($arg!($f, a)),
            Sqrt(a) => Sqrt($arg!($f, a)),
            Step(a, b) => Step($arg!($f, a), $arg!($f, b)),
            Tan(a) => Tan($arg!($f, a)),
            Time => Time,
            TimeStep => TimeStep,
            StartTime => StartTime,
            FinalTime => FinalTime,
            Rank(a, b) => Rank($arg!($f, a), $arg!($f, b)),
            Size(a) => Size($arg!($f, a)),
            Stddev(a) => Stddev($arg!($f, a)),
            Sum(a) => Sum($arg!($f, a)),
            VectorSelect(a, b, c, d, e) => VectorSelect(
                $arg!($f, a),
                $arg!($f, b),
                $arg!($f, c),
                $arg!($f, d),
                $arg!($f, e),
            ),
            VectorElmMap(a, b) => VectorElmMap($arg!($f, a), $arg!($f, b)),
            VectorSortOrder(a, b) => VectorSortOrder($arg!($f, a), $arg!($f, b)),
            AllocateAvailable(a, b, c) => {
                AllocateAvailable($arg!($f, a), $arg!($f, b), $arg!($f, c))
            }
            AllocateByPriority(a, b, c, d, e) => AllocateByPriority(
                $arg!($f, a),
                $arg!($f, b),
                $arg!($f, c),
                $arg!($f, d),
                $arg!($f, e),
            ),
            Previous(a, b) => Previous($arg!($f, a), $arg!($f, b)),
            Init(a) => Init($arg!($f, a)),
        }
    }};
}

// The by-value spellings for `builtin_rebuild!`: arguments are moved out of
// their boxes and `Loc`s / identifiers are moved through.
macro_rules! rebuild_arg_owned {
    ($f:ident, $a:ident) => {
        Box::new($f(*$a)?)
    };
}
macro_rules! rebuild_opt_owned {
    ($f:ident, $a:ident) => {
        match $a {
            Some(a) => Some(Box::new($f(*a)?)),
            None => None,
        }
    };
}
macro_rules! rebuild_many_owned {
    ($f:ident, $a:ident) => {
        $a.into_iter()
            .map(&mut $f)
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
}
macro_rules! rebuild_pass_owned {
    ($a:ident) => {
        $a
    };
}

// The by-reference spellings: arguments are borrowed, `Loc`s copied and the
// identifier cloned.
macro_rules! rebuild_arg_borrowed {
    ($f:ident, $a:ident) => {
        Box::new($f(&**$a)?)
    };
}
macro_rules! rebuild_opt_borrowed {
    ($f:ident, $a:ident) => {
        match $a {
            Some(a) => Some(Box::new($f(&**a)?)),
            None => None,
        }
    };
}
macro_rules! rebuild_many_borrowed {
    ($f:ident, $a:ident) => {
        $a.iter()
            .map(&mut $f)
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
}
macro_rules! rebuild_copy_borrowed {
    ($a:ident) => {
        *$a
    };
}
macro_rules! rebuild_clone_borrowed {
    ($a:ident) => {
        $a.clone()
    };
}

impl<Expr> BuiltinFn<Expr> {
    /// The per-builtin facts for this call. Every consumer that needs a fact
    /// about a builtin -- rather than its arguments -- reads it here.
    pub fn signature(&self) -> &'static BuiltinSig {
        use BuiltinFn::*;
        match self {
            Lookup(_, _, _) => &LOOKUP,
            LookupForward(_, _, _) => &LOOKUP_FORWARD,
            LookupBackward(_, _, _) => &LOOKUP_BACKWARD,
            Abs(_) => &ABS,
            Arccos(_) => &ARCCOS,
            Arcsin(_) => &ARCSIN,
            Arctan(_) => &ARCTAN,
            Cos(_) => &COS,
            Exp(_) => &EXP,
            Inf => &INF,
            Int(_) => &INT,
            IsModuleInput(_, _) => &IS_MODULE_INPUT,
            Ln(_) => &LN,
            Log10(_) => &LOG10,
            Max(_, _) => &MAX,
            Mean(_) => &MEAN,
            Min(_, _) => &MIN,
            Pi => &PI,
            Pulse(_, _, _) => &PULSE,
            Quantum(_, _) => &QUANTUM,
            Ramp(_, _, _) => &RAMP,
            Round(_) => &ROUND,
            SafeDiv(_, _, _) => &SAFEDIV,
            Sign(_) => &SIGN,
            Sshape(_, _, _) => &SSHAPE,
            Sin(_) => &SIN,
            Sqrt(_) => &SQRT,
            Step(_, _) => &STEP,
            Tan(_) => &TAN,
            Time => &TIME,
            TimeStep => &TIME_STEP,
            StartTime => &INITIAL_TIME,
            FinalTime => &FINAL_TIME,
            Rank(_, _) => &RANK,
            Size(_) => &SIZE,
            Stddev(_) => &STDDEV,
            Sum(_) => &SUM,
            VectorSelect(_, _, _, _, _) => &VECTOR_SELECT,
            VectorElmMap(_, _) => &VECTOR_ELM_MAP,
            VectorSortOrder(_, _) => &VECTOR_SORT_ORDER,
            AllocateAvailable(_, _, _) => &ALLOCATE_AVAILABLE,
            AllocateByPriority(_, _, _, _, _) => &ALLOCATE_BY_PRIORITY,
            Previous(_, _) => &PREVIOUS,
            Init(_) => &INIT,
        }
    }

    /// The canonical lowercase name of this builtin.
    pub fn name(&self) -> &'static str {
        self.signature().name
    }

    /// The argument expressions, in call order. An `IsModuleInput`'s
    /// identifier is not an expression and is absent; an absent optional
    /// trailing argument is absent.
    pub fn args(&self) -> SmallVec<[&Expr; 5]> {
        builtin_args!(self, iter)
    }

    /// [`Self::args`], mutably.
    pub fn args_mut(&mut self) -> SmallVec<[&mut Expr; 5]> {
        builtin_args!(self, iter_mut, mut)
    }

    /// Whether this call is the one-argument reduction form of a builtin
    /// whose n-ary form is scalar (`MAX(A)`, `MIN(A)`, `MEAN(A)`).
    pub fn is_unary_reduction(&self) -> bool {
        self.signature().unary_reduces && self.args().len() == 1
    }

    /// One [`ArgKind`] per entry of [`Self::args`], aligned with it.
    pub fn arg_kinds(&self) -> SmallVec<[ArgKind; 5]> {
        let sig = self.signature();
        let n = self.args().len();
        if sig.unary_reduces && n == 1 {
            return smallvec::smallvec![REDUCED];
        }
        // A variadic builtin lists one kind; every position takes it.
        let tail = sig.arg_kinds.last().copied().unwrap_or(ArgKind::Scalar);
        sig.arg_kinds
            .iter()
            .copied()
            .filter(|kind| *kind != ArgKind::Ident)
            .chain(std::iter::repeat(tail))
            .take(n)
            .collect()
    }

    /// [`Self::args`] paired with [`Self::arg_kinds`].
    pub fn args_with_kinds(&self) -> impl Iterator<Item = (&Expr, ArgKind)> {
        self.args().into_iter().zip(self.arg_kinds())
    }

    /// The shape of this call's result (see [`ResultKind`]).
    pub fn result_kind(&self) -> ResultKind {
        if self.is_unary_reduction() {
            ResultKind::Scalar
        } else {
            self.signature().result
        }
    }

    /// Whether any argument position is an array operand.
    pub fn has_array_operand(&self) -> bool {
        self.arg_kinds()
            .iter()
            .any(|kind| matches!(kind, ArgKind::Array { .. }))
    }

    /// Transform all expression arguments in this builtin using the provided function.
    /// Returns an error if any transformation fails. Arguments are visited in
    /// call order, the order [`Self::args`] reports.
    pub fn try_map<F, E2, Err>(self, mut f: F) -> std::result::Result<BuiltinFn<E2>, Err>
    where
        F: FnMut(Expr) -> std::result::Result<E2, Err>,
    {
        Ok(builtin_rebuild!(
            self,
            f,
            rebuild_arg_owned,
            rebuild_opt_owned,
            rebuild_many_owned,
            rebuild_pass_owned,
            rebuild_pass_owned
        ))
    }

    /// [`Self::try_map`] over a borrowed builtin: builds a new builtin of
    /// another expression type from references to this one's arguments,
    /// without cloning them first.
    pub fn try_map_ref<F, E2, Err>(&self, mut f: F) -> std::result::Result<BuiltinFn<E2>, Err>
    where
        F: FnMut(&Expr) -> std::result::Result<E2, Err>,
    {
        Ok(builtin_rebuild!(
            self,
            f,
            rebuild_arg_borrowed,
            rebuild_opt_borrowed,
            rebuild_many_borrowed,
            rebuild_copy_borrowed,
            rebuild_clone_borrowed
        ))
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

    /// Infallible version of [`Self::try_map_ref`].
    pub fn map_ref<F, E2>(&self, mut f: F) -> BuiltinFn<E2>
    where
        F: FnMut(&Expr) -> E2,
    {
        self.try_map_ref(|e| Ok::<_, std::convert::Infallible>(f(e)))
            .unwrap()
    }

    /// [`Self::map`], handing each argument its [`ArgKind`].
    pub fn map_with_kinds<F, E2>(self, mut f: F) -> BuiltinFn<E2>
    where
        F: FnMut(Expr, ArgKind) -> E2,
    {
        let mut kinds = self.arg_kinds().into_iter();
        self.map(|e| {
            let kind = kinds
                .next()
                .unwrap_or_else(|| unreachable!("map visits exactly the positions args() reports"));
            f(e, kind)
        })
    }

    /// [`Self::try_map_ref`], handing each argument its [`ArgKind`].
    pub fn try_map_ref_with_kinds<F, E2, Err>(
        &self,
        mut f: F,
    ) -> std::result::Result<BuiltinFn<E2>, Err>
    where
        F: FnMut(&Expr, ArgKind) -> std::result::Result<E2, Err>,
    {
        let mut kinds = self.arg_kinds().into_iter();
        self.try_map_ref(|e| {
            let kind = kinds
                .next()
                .unwrap_or_else(|| unreachable!("map visits exactly the positions args() reports"));
            f(e, kind)
        })
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

    /// Call a closure on each expression argument by reference, in call order.
    pub fn for_each_expr_ref<F>(&self, mut f: F)
    where
        F: FnMut(&Expr),
    {
        for arg in self.args() {
            f(arg);
        }
    }
}

/// Whether a (lowercased) name or alias denotes a builtin that takes no
/// arguments.
pub fn is_0_arity_builtin_fn(name: &str) -> bool {
    BuiltinSig::by_name(name).is_some_and(|sig| sig.max_args == Some(0))
}

/// ASCII case-insensitive, allocation-free variant of [`is_0_arity_builtin_fn`].
///
/// The 0-arity builtin names are all ASCII, so a name containing any non-ASCII
/// byte cannot match, and ASCII case-folding yields the same membership verdict
/// as Unicode lowercasing for this fixed ASCII set. Used on the hot parse path
/// (`Expr0::reify_0_arity_builtins`), which runs for *every* variable reference
/// and so tests membership against this short fixed list rather than scanning
/// [`BuiltinSig::ALL`]; `nullary_name_list_matches_the_signature_table` pins
/// the list to the table.
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
/// expands to a stdlib module: a name with a
/// [`crate::module_functions::stdlib_descriptor`] plus the alias forms
/// `delay`, `delayn`, and `smthn`, which `builtins_visitor` normalizes to one
/// of those before the descriptor is looked up.
///
/// Defined over the descriptor table rather than `stdlib::MODEL_NAMES` so the
/// per-element expansion decision (`builtins_visitor::per_element_requirements`)
/// and the expansion itself cannot disagree: a `stdlib⁚systems_*` model has no
/// descriptor and is not a callable function.
pub(crate) fn is_stdlib_module_function(func_name: &str) -> bool {
    matches!(func_name, "delay" | "delayn" | "smthn")
        || crate::module_functions::stdlib_descriptor(func_name).is_some()
}

/// Whether a (lowercased) name or alias denotes a builtin function.
pub fn is_builtin_fn(name: &str) -> bool {
    BuiltinSig::by_name(name).is_some()
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
    /// This is [`ArgKind::Table`] as a walker sees it.
    LookupTable(&'a Expr),
}

/// Visit a builtin's contents in call order: the `IsModuleInput` identifier as
/// [`BuiltinContents::Ident`], a lookup's table position as
/// [`BuiltinContents::LookupTable`], and every other argument as
/// [`BuiltinContents::Expr`].
pub(crate) fn walk_builtin_expr<'a, Expr, F>(builtin: &'a BuiltinFn<Expr>, mut cb: F)
where
    F: FnMut(BuiltinContents<'a, Expr>),
{
    if let BuiltinFn::IsModuleInput(id, loc) = builtin {
        cb(BuiltinContents::Ident(id, *loc));
    }
    for (arg, kind) in builtin.args().into_iter().zip(builtin.arg_kinds()) {
        match kind {
            ArgKind::Table => cb(BuiltinContents::LookupTable(arg)),
            ArgKind::Scalar | ArgKind::Array { .. } => cb(BuiltinContents::Expr(arg)),
            ArgKind::Ident => unreachable!("an identifier payload is not an expression argument"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Builtin = BuiltinFn<i32>;

    /// One value of EVERY variant, with distinct, ascending argument values so
    /// argument ORDER is observable. The `_`-less match below makes a newly
    /// added variant a compile error here until it is listed.
    fn every_variant() -> Vec<Builtin> {
        fn b(n: i32) -> Box<i32> {
            Box::new(n)
        }
        let all = vec![
            Builtin::Lookup(b(1), b(2), Loc::new(1, 2)),
            Builtin::LookupForward(b(1), b(2), Loc::new(1, 2)),
            Builtin::LookupBackward(b(1), b(2), Loc::new(1, 2)),
            Builtin::Abs(b(1)),
            Builtin::Arccos(b(1)),
            Builtin::Arcsin(b(1)),
            Builtin::Arctan(b(1)),
            Builtin::Cos(b(1)),
            Builtin::Exp(b(1)),
            Builtin::Inf,
            Builtin::Int(b(1)),
            Builtin::IsModuleInput("x".to_string(), Loc::new(1, 2)),
            Builtin::Ln(b(1)),
            Builtin::Log10(b(1)),
            Builtin::Max(b(1), None),
            Builtin::Mean(vec![1, 2, 3]),
            Builtin::Min(b(1), None),
            Builtin::Pi,
            Builtin::Pulse(b(1), b(2), None),
            Builtin::Quantum(b(1), b(2)),
            Builtin::Ramp(b(1), b(2), None),
            Builtin::Round(b(1)),
            Builtin::SafeDiv(b(1), b(2), None),
            Builtin::Sign(b(1)),
            Builtin::Sshape(b(1), b(2), b(3)),
            Builtin::Sin(b(1)),
            Builtin::Sqrt(b(1)),
            Builtin::Step(b(1), b(2)),
            Builtin::Tan(b(1)),
            Builtin::Time,
            Builtin::TimeStep,
            Builtin::StartTime,
            Builtin::FinalTime,
            Builtin::Rank(b(1), b(2)),
            Builtin::Size(b(1)),
            Builtin::Stddev(b(1)),
            Builtin::Sum(b(1)),
            Builtin::VectorSelect(b(1), b(2), b(3), b(4), b(5)),
            Builtin::VectorElmMap(b(1), b(2)),
            Builtin::VectorSortOrder(b(1), b(2)),
            Builtin::AllocateAvailable(b(1), b(2), b(3)),
            Builtin::AllocateByPriority(b(1), b(2), b(3), b(4), b(5)),
            Builtin::Previous(b(1), b(2)),
            Builtin::Init(b(1)),
        ];
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
        all
    }

    /// The optional-trailing-argument and n-ary forms of the variants whose
    /// shape varies with arity, so both forms of every such variant are
    /// exercised.
    fn arity_variants() -> Vec<Builtin> {
        fn b(n: i32) -> Box<i32> {
            Box::new(n)
        }
        vec![
            Builtin::Max(b(1), Some(b(2))),
            Builtin::Min(b(1), Some(b(2))),
            Builtin::Mean(vec![]),
            Builtin::Mean(vec![1]),
            Builtin::Pulse(b(1), b(2), Some(b(3))),
            Builtin::Ramp(b(1), b(2), Some(b(3))),
            Builtin::SafeDiv(b(1), b(2), Some(b(3))),
        ]
    }

    fn arg_values(v: &Builtin) -> Vec<i32> {
        v.args().into_iter().copied().collect()
    }

    /// compiler-unification.AC2.1: every variant agrees with its signature.
    ///
    /// The rows are the enumeration itself (`every_variant`, plus the
    /// alternate arities), and the assertions are properties of the table and
    /// the accessors rather than a transcription of either: the argument
    /// view, the rebuilders, and the kinds all report the same positions in
    /// the same order, and the source arity admits what the node holds.
    #[test]
    fn compiler_unification_ac2_1_every_variant_agrees_with_its_signature() {
        let mut rows = every_variant();
        let n_variants = rows.len();
        rows.extend(arity_variants());

        for v in &rows {
            let sig = v.signature();
            let name = sig.name;
            let args = arg_values(v);
            let n_ident = sig
                .arg_kinds
                .iter()
                .filter(|k| **k == ArgKind::Ident)
                .count();

            // Source arity admits what the node holds.
            assert!(
                sig.accepts_arity(args.len() + n_ident),
                "{name}: {} expression args + {n_ident} ident args lie outside [{}, {:?}]",
                args.len(),
                sig.min_args,
                sig.max_args
            );
            assert!(
                sig.min_args as usize <= sig.max_args.map_or(usize::MAX, |m| m as usize),
                "{name}: min_args exceeds max_args"
            );

            // args() yields the operands in declaration order: every row is
            // built with ascending values 1..=n.
            assert_eq!(
                args,
                (1..=args.len() as i32).collect::<Vec<_>>(),
                "{name}: args() order"
            );

            // args() and args_mut() see the same expressions in the same order.
            let mut mutable = v.clone();
            let mutable_args: Vec<i32> = mutable.args_mut().into_iter().map(|x| *x).collect();
            assert_eq!(args, mutable_args, "{name}: args_mut disagrees with args");

            // for_each_expr_ref, map and try_map_ref all visit args() in order.
            let mut visited = Vec::new();
            v.for_each_expr_ref(|x| visited.push(*x));
            assert_eq!(args, visited, "{name}: for_each_expr_ref order");
            let mut mapped_order = Vec::new();
            let round_trip = v.clone().map(|x| {
                mapped_order.push(x);
                x
            });
            assert_eq!(&round_trip, v, "{name}: map(identity) is not the identity");
            assert_eq!(args, mapped_order, "{name}: map visit order");
            let by_ref: Result<Builtin, ()> = v.try_map_ref(|x| Ok(*x));
            assert_eq!(
                by_ref.as_ref(),
                Ok(v),
                "{name}: try_map_ref(copy) is not a copy"
            );
            let kinds_seen: Vec<ArgKind> = {
                let mut seen = Vec::new();
                let _ = v.clone().map_with_kinds(|x, k| {
                    seen.push(k);
                    x
                });
                seen
            };
            assert_eq!(
                kinds_seen,
                v.arg_kinds().to_vec(),
                "{name}: map_with_kinds hands out kinds in a different order"
            );

            // One kind per expression argument, none of them Ident.
            let kinds = v.arg_kinds();
            assert_eq!(
                kinds.len(),
                args.len(),
                "{name}: arg_kinds is not aligned with args"
            );
            assert!(
                kinds.iter().all(|k| *k != ArgKind::Ident),
                "{name}: an identifier payload surfaced as an expression argument"
            );
            assert_eq!(
                v.args_with_kinds().count(),
                args.len(),
                "{name}: args_with_kinds is not aligned with args"
            );

            // A reduction has exactly one argument and it is the reduced array.
            if v.is_unary_reduction() {
                assert!(
                    sig.unary_reduces && args.len() == 1,
                    "{name}: reduction shape"
                );
                assert_eq!(kinds.to_vec(), vec![ArgKind::Array { whole: false }]);
                assert_eq!(v.result_kind(), ResultKind::Scalar);
            } else {
                assert_eq!(v.result_kind(), sig.result, "{name}: n-ary result kind");
            }

            // An array-producing builtin takes its shape from an array position.
            if let ResultKind::Array { shape_from } = v.result_kind() {
                let from = shape_from as usize;
                assert!(
                    from < kinds.len(),
                    "{name}: shape_from past the last argument"
                );
                assert!(
                    matches!(kinds[from], ArgKind::Array { .. }),
                    "{name}: shape_from names a non-array position"
                );
            }
            assert_eq!(
                v.has_array_operand(),
                kinds.iter().any(|k| matches!(k, ArgKind::Array { .. })),
                "{name}: has_array_operand"
            );

            // The name half of the table round-trips through the lookup.
            assert_eq!(v.name(), name);
            assert!(
                std::ptr::eq(BuiltinSig::by_name(name).unwrap(), sig),
                "{name}: by_name(name) is a different signature"
            );
            for alias in sig.aliases {
                assert!(
                    std::ptr::eq(BuiltinSig::by_name(alias).unwrap(), sig),
                    "{name}: alias {alias} resolves elsewhere"
                );
            }
            assert!(is_builtin_fn(name));
            assert_eq!(
                is_0_arity_builtin_fn(name),
                sig.max_args == Some(0),
                "{name}: is_0_arity_builtin_fn"
            );

            // walk_builtin_expr reports the same positions, classified by kind.
            let mut walked = Vec::new();
            walk_builtin_expr(v, |c| match c {
                BuiltinContents::Ident(id, _) => walked.push(format!("ident:{id}")),
                BuiltinContents::Expr(e) => walked.push(format!("expr:{e}")),
                BuiltinContents::LookupTable(e) => walked.push(format!("table:{e}")),
            });
            let expected: Vec<String> = {
                let mut exp = Vec::new();
                if let Builtin::IsModuleInput(id, _) = v {
                    exp.push(format!("ident:{id}"));
                }
                for (arg, kind) in v.args_with_kinds() {
                    let tag = if kind == ArgKind::Table {
                        "table"
                    } else {
                        "expr"
                    };
                    exp.push(format!("{tag}:{arg}"));
                }
                exp
            };
            assert_eq!(walked, expected, "{name}: walk_builtin_expr contents");
        }

        // The table is exactly the enumeration: one signature per variant,
        // each reached by exactly one variant, with distinct names.
        assert_eq!(BuiltinSig::ALL.len(), n_variants);
        let mut seen = std::collections::BTreeSet::new();
        for v in every_variant() {
            assert!(
                BuiltinSig::ALL
                    .iter()
                    .any(|s| std::ptr::eq(*s, v.signature())),
                "{}: signature missing from ALL",
                v.name()
            );
            assert!(
                seen.insert(v.name()),
                "two variants share the name {:?}",
                v.name()
            );
        }
        for sig in BuiltinSig::ALL {
            for alias in sig.aliases {
                assert!(seen.insert(alias), "alias {alias:?} collides with a name");
            }
        }

        assert!(!is_builtin_fn("lookupz"));
        assert!(!is_0_arity_builtin_fn("lookup"));
        assert!(is_0_arity_builtin_fn("time"));
    }

    /// The fixed list behind `is_0_arity_builtin_fn_ci` is exactly the set of
    /// nullary names and aliases the table declares.
    #[test]
    fn nullary_name_list_matches_the_signature_table() {
        let from_table: std::collections::BTreeSet<&str> = BuiltinSig::ALL
            .iter()
            .filter(|sig| sig.max_args == Some(0))
            .flat_map(|sig| std::iter::once(sig.name).chain(sig.aliases.iter().copied()))
            .collect();
        let all_names: Vec<&str> = BuiltinSig::ALL
            .iter()
            .flat_map(|sig| std::iter::once(sig.name).chain(sig.aliases.iter().copied()))
            .collect();
        for name in &all_names {
            assert_eq!(
                is_0_arity_builtin_fn_ci(name),
                from_table.contains(name),
                "{name}: the case-insensitive list disagrees with the table"
            );
            assert_eq!(
                is_0_arity_builtin_fn_ci(&name.to_uppercase()),
                from_table.contains(name),
                "{name}: uppercase"
            );
        }
        assert_eq!(from_table.len(), 9, "the fixed list has 9 entries");
    }

    #[test]
    fn test_is_0_arity_builtin_fn_ci() {
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

    /// Rows are the four arms of `BuiltinSig::arity_phrase`, each spelled once
    /// with a plural and (where the arm can name 1 as its last number) once
    /// with a singular, since the noun is chosen from that number. The second
    /// loop derives coverage from the table rather than asserting it: every
    /// signature in `BuiltinSig::ALL` must land in an arm a row names, so a new
    /// builtin whose arity shape none of these covers fails here instead of
    /// silently rendering as something else.
    #[test]
    fn arity_phrase_covers_every_shape_in_the_signature_table() {
        let shape = |min: u8, max: Option<u8>| -> String {
            BuiltinSig {
                min_args: min,
                max_args: max,
                ..ABS
            }
            .arity_phrase()
        };
        // (arm, min, max, rendering)
        let rows: [(&str, u8, Option<u8>, &str); 6] = [
            ("variadic", 0, None, "at least 0 arguments"),
            ("variadic", 1, None, "at least 1 argument"),
            ("exact", 2, Some(2), "2 arguments"),
            ("exact", 1, Some(1), "1 argument"),
            ("one optional", 1, Some(2), "1 or 2 arguments"),
            ("a range", 2, Some(4), "2 to 4 arguments"),
        ];
        for (arm, min, max, expected) in rows {
            assert_eq!(shape(min, max), expected, "{arm}");
        }

        for sig in BuiltinSig::ALL {
            let arm = match sig.max_args {
                None => "variadic",
                Some(max) if max == sig.min_args => "exact",
                Some(max) if max == sig.min_args + 1 => "one optional",
                Some(_) => "a range",
            };
            assert!(
                rows.iter().any(|(row_arm, _, _, _)| *row_arm == arm),
                "{}'s arity shape ({}, {:?}) is an arm no row above covers",
                sig.name,
                sig.min_args,
                sig.max_args
            );
            assert!(
                !sig.arity_phrase().is_empty(),
                "{} must render an arity phrase",
                sig.name
            );
        }
    }

    #[test]
    fn accepts_arity_is_the_closed_source_range() {
        assert!(!MAX.accepts_arity(0));
        assert!(MAX.accepts_arity(1));
        assert!(MAX.accepts_arity(2));
        assert!(!MAX.accepts_arity(3));
        assert!(MEAN.accepts_arity(0));
        assert!(MEAN.accepts_arity(17));
        assert!(INF.accepts_arity(0));
        assert!(!INF.accepts_arity(1));
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
    fn try_map_ref_stops_at_the_first_failure() {
        let builtin: BuiltinFn<i32> = BuiltinFn::Mean(vec![1, 2, 3]);
        let mut calls = 0;
        let result: Result<BuiltinFn<i32>, i32> = builtin.try_map_ref(|x| {
            calls += 1;
            if *x == 2 { Err(*x) } else { Ok(*x) }
        });
        assert_eq!(result, Err(2));
        assert_eq!(calls, 2);
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

    /// The arity-polymorphic variants change kind and result with their
    /// argument count: the one-argument form is XMILE 3.7.1.3's array
    /// reduction (`MAX(A)` "extends MAX(x, y)", `MEAN(A)`), the n-ary form is
    /// Simlin's scalar rule -- section 3.5's `MAX(x, y)` / `MIN(x, y)` and
    /// Stella's n-ary `MEAN`; the spec's mixed `MAX(A, 0)` is not implemented
    /// (GH #1026).
    #[test]
    fn unary_reduction_forms_are_reduced_arrays_and_n_ary_forms_are_scalars() {
        let one = BuiltinFn::<i32>::Max(Box::new(1), None);
        assert!(one.is_unary_reduction());
        assert_eq!(
            one.arg_kinds().to_vec(),
            vec![ArgKind::Array { whole: false }]
        );
        assert_eq!(one.result_kind(), ResultKind::Scalar);

        let two = BuiltinFn::<i32>::Max(Box::new(1), Some(Box::new(2)));
        assert!(!two.is_unary_reduction());
        assert_eq!(
            two.arg_kinds().to_vec(),
            vec![ArgKind::Scalar, ArgKind::Scalar]
        );
        assert_eq!(two.result_kind(), ResultKind::Elementwise);

        let mean1 = BuiltinFn::<i32>::Mean(vec![1]);
        assert!(mean1.is_unary_reduction());
        assert_eq!(
            mean1.arg_kinds().to_vec(),
            vec![ArgKind::Array { whole: false }]
        );

        let mean3 = BuiltinFn::<i32>::Mean(vec![1, 2, 3]);
        assert!(!mean3.is_unary_reduction());
        assert_eq!(mean3.arg_kinds().to_vec(), vec![ArgKind::Scalar; 3]);
        assert_eq!(mean3.result_kind(), ResultKind::Scalar);

        // A one-argument builtin that is not a reduction keeps its kind.
        let abs = BuiltinFn::<i32>::Abs(Box::new(1));
        assert!(!abs.is_unary_reduction());
        assert_eq!(abs.arg_kinds().to_vec(), vec![ArgKind::Scalar]);
    }
}
