// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
use crate::ast::literal::Literal;
pub use crate::builtins::Loc;
use crate::builtins::{BuiltinFn, BuiltinSig, UntypedBuiltinFn};
use crate::common::{Canonical, EquationResult, Ident};
use crate::dimensions::DimensionsContext;
use crate::eqn_err;

/// IndexExpr1 represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum IndexExpr1 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident<Canonical>, Loc),
    Range(Expr1, Expr1, Loc),
    DimPosition(u32, Loc),
    Expr(Expr1),
}

impl IndexExpr1 {
    pub(crate) fn from(expr: &IndexExpr0) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr0::Wildcard(loc) => IndexExpr1::Wildcard(*loc),
            IndexExpr0::StarRange(ident, loc) => IndexExpr1::StarRange(ident.canonicalize(), *loc),
            IndexExpr0::Range(l, r, loc) => {
                IndexExpr1::Range(Expr1::from(l)?, Expr1::from(r)?, *loc)
            }
            IndexExpr0::DimPosition(n, loc) => IndexExpr1::DimPosition(*n, *loc),
            IndexExpr0::Expr(e) => IndexExpr1::Expr(Expr1::from(e)?),
        };

        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, dimensions: &DimensionsContext) -> Self {
        match self {
            IndexExpr1::Wildcard(loc) => IndexExpr1::Wildcard(loc),
            IndexExpr1::StarRange(id, loc) => IndexExpr1::StarRange(id, loc),
            IndexExpr1::Range(l, r, loc) => IndexExpr1::Range(
                l.constify_dimensions(dimensions),
                r.constify_dimensions(dimensions),
                loc,
            ),
            IndexExpr1::DimPosition(n, loc) => IndexExpr1::DimPosition(n, loc),
            IndexExpr1::Expr(e) => IndexExpr1::Expr(e.constify_dimensions(dimensions)),
        }
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
///
/// `Eq` is derived for the reason spelled out on [`Expr0`]: it makes
/// "reflexive, therefore backdateable" a compile-checked property, which is what
/// keeps a future float-bearing field from being a bare `f64`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum Expr1 {
    Const(String, Literal, Loc),
    Var(Ident<Canonical>, Loc),
    App(BuiltinFn<Expr1>, Loc),
    Subscript(Ident<Canonical>, Vec<IndexExpr1>, Loc),
    Op1(UnaryOp, Box<Expr1>, Loc),
    Op2(BinaryOp, Box<Expr1>, Box<Expr1>, Loc),
    If(Box<Expr1>, Box<Expr1>, Box<Expr1>, Loc),
}

/// The next positional argument of a call whose arity
/// [`BuiltinSig::accepts_arity`] has already admitted, so a required position
/// is always present.
fn required(args: &mut std::vec::IntoIter<Expr1>) -> Box<Expr1> {
    Box::new(
        args.next()
            .unwrap_or_else(|| unreachable!("arity was checked against the builtin's signature")),
    )
}

/// The next positional argument if the call spelled it.
fn optional(args: &mut std::vec::IntoIter<Expr1>) -> Option<Box<Expr1>> {
    args.next().map(Box::new)
}

impl Expr1 {
    /// Lower a parsed `Expr0` into an `Expr1`, by REFERENCE.
    ///
    /// Deliberately not by value even though every field is either copied or
    /// re-derived here: `Expr1` is a different tree with different identifier
    /// types, so it is built from scratch either way, and taking `&Expr0` is
    /// what lets `lower_ast` read a variable's parsed AST straight out of its
    /// (shared, salsa-cached) home. Consuming it forced the caller to deep-copy
    /// the whole `Expr0` tree -- a `Box` per node and a `String` per identifier
    /// -- purely so this function could destroy it, which was the single
    /// largest source of allocations in a C-LEARN compile.
    pub(crate) fn from(expr: &Expr0) -> EquationResult<Self> {
        let expr = match expr {
            Expr0::Const(s, n, loc) => Expr1::Const(s.clone(), *n, *loc),
            Expr0::Var(id, loc) => Expr1::Var(id.canonicalize(), *loc),
            Expr0::App(UntypedBuiltinFn(id, orig_args), loc) => {
                let loc = *loc;
                let args: EquationResult<Vec<Expr1>> = orig_args.iter().map(Expr1::from).collect();
                let args = args?;

                let Some(sig) = BuiltinSig::by_name(id.as_str()) else {
                    // TODO: this could be a table reference, array reference,
                    //       or module instantiation according to 3.3.2 of the spec
                    return eqn_err!(
                        UnknownBuiltin,
                        loc.start,
                        loc.end,
                        format!("'{}' is not a known function", id.as_str())
                    );
                };
                // The signature is the one statement of each builtin's arity;
                // once it admits the call, the destructuring below is
                // positional and infallible.
                if !sig.accepts_arity(args.len()) {
                    // Name the function the way the CALL spells it, not the
                    // signature's canonical name: a modeler who wrote `DT` is
                    // not helped by a message about `time_step`. The parser
                    // has already lowercased the name (and joined a multi-word
                    // one with `_`), so the display uppercases it -- the
                    // spelling XMILE v1.0 uses for every function it defines
                    // (3.3.2 `ABS(x)`, the tables in 3.5).
                    let given = if args.len() == 1 {
                        "1 was".to_string()
                    } else {
                        format!("{} were", args.len())
                    };
                    return eqn_err!(
                        BadBuiltinArgs,
                        loc.start,
                        loc.end,
                        format!(
                            "{} takes {}, but {given} given",
                            id.as_str().to_uppercase(),
                            sig.arity_phrase()
                        )
                    );
                }
                let mut args = args.into_iter();
                let builtin = match sig.name {
                    "lookup" => BuiltinFn::Lookup(required(&mut args), required(&mut args), loc),
                    "lookup_forward" => {
                        BuiltinFn::LookupForward(required(&mut args), required(&mut args), loc)
                    }
                    "lookup_backward" => {
                        BuiltinFn::LookupBackward(required(&mut args), required(&mut args), loc)
                    }
                    "abs" => BuiltinFn::Abs(required(&mut args)),
                    "arccos" => BuiltinFn::Arccos(required(&mut args)),
                    "arcsin" => BuiltinFn::Arcsin(required(&mut args)),
                    "arctan" => BuiltinFn::Arctan(required(&mut args)),
                    "cos" => BuiltinFn::Cos(required(&mut args)),
                    "exp" => BuiltinFn::Exp(required(&mut args)),
                    "inf" => BuiltinFn::Inf,
                    "int" => BuiltinFn::Int(required(&mut args)),
                    // The one argument is an identifier, kept as text rather
                    // than lowered as an expression.
                    "ismoduleinput" => match args.next() {
                        Some(Expr1::Var(ident, loc)) => {
                            BuiltinFn::IsModuleInput(ident.to_string(), loc)
                        }
                        _ => {
                            return eqn_err!(
                                ExpectedIdent,
                                loc.start,
                                loc.end,
                                "ISMODULEINPUT's argument must be a variable name"
                            );
                        }
                    },
                    "ln" => BuiltinFn::Ln(required(&mut args)),
                    "log10" => BuiltinFn::Log10(required(&mut args)),
                    "max" => BuiltinFn::Max(required(&mut args), optional(&mut args)),
                    "mean" => BuiltinFn::Mean(args.collect()),
                    "min" => BuiltinFn::Min(required(&mut args), optional(&mut args)),
                    "pi" => BuiltinFn::Pi,
                    "pulse" => BuiltinFn::Pulse(
                        required(&mut args),
                        required(&mut args),
                        optional(&mut args),
                    ),
                    "quantum" => BuiltinFn::Quantum(required(&mut args), required(&mut args)),
                    "ramp" => BuiltinFn::Ramp(
                        required(&mut args),
                        required(&mut args),
                        optional(&mut args),
                    ),
                    "round" => BuiltinFn::Round(required(&mut args)),
                    "safediv" => BuiltinFn::SafeDiv(
                        required(&mut args),
                        required(&mut args),
                        optional(&mut args),
                    ),
                    "sign" => BuiltinFn::Sign(required(&mut args)),
                    "sin" => BuiltinFn::Sin(required(&mut args)),
                    "sshape" => BuiltinFn::Sshape(
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                    ),
                    "sqrt" => BuiltinFn::Sqrt(required(&mut args)),
                    "step" => BuiltinFn::Step(required(&mut args), required(&mut args)),
                    "tan" => BuiltinFn::Tan(required(&mut args)),
                    "time" => BuiltinFn::Time,
                    "time_step" => BuiltinFn::TimeStep,
                    "initial_time" => BuiltinFn::StartTime,
                    "final_time" => BuiltinFn::FinalTime,
                    "rank" => BuiltinFn::Rank(required(&mut args), required(&mut args)),
                    "size" => BuiltinFn::Size(required(&mut args)),
                    "stddev" => BuiltinFn::Stddev(required(&mut args)),
                    "sum" => BuiltinFn::Sum(required(&mut args)),
                    "vector_select" => BuiltinFn::VectorSelect(
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                    ),
                    "vector_elm_map" => {
                        BuiltinFn::VectorElmMap(required(&mut args), required(&mut args))
                    }
                    "vector_sort_order" => {
                        BuiltinFn::VectorSortOrder(required(&mut args), required(&mut args))
                    }
                    "allocate_available" => BuiltinFn::AllocateAvailable(
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                    ),
                    "allocate_by_priority" => BuiltinFn::AllocateByPriority(
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                        required(&mut args),
                    ),
                    // Unary PREVIOUS(x) desugars to PREVIOUS(x, 0).
                    // builtins_visitor may have already added the fallback
                    // at Expr0 level, so both 1-arg and 2-arg forms are valid.
                    "previous" => {
                        let a = required(&mut args);
                        let fallback = optional(&mut args).unwrap_or_else(|| {
                            Box::new(Expr1::Const("0".to_string(), Literal::new(0.0), loc))
                        });
                        BuiltinFn::Previous(a, fallback)
                    }
                    "init" => BuiltinFn::Init(required(&mut args)),
                    other => unreachable!("builtin {other} has a signature but no constructor"),
                };
                Expr1::App(builtin, loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr1>> =
                    args.iter().map(IndexExpr1::from).collect();
                Expr1::Subscript(id.canonicalize(), args?, *loc)
            }
            Expr0::Op1(op, l, loc) => Expr1::Op1(*op, Box::new(Expr1::from(l)?), *loc),
            Expr0::Op2(op, l, r, loc) => Expr1::Op2(
                *op,
                Box::new(Expr1::from(l)?),
                Box::new(Expr1::from(r)?),
                *loc,
            ),
            Expr0::If(cond, t, f, loc) => Expr1::If(
                Box::new(Expr1::from(cond)?),
                Box::new(Expr1::from(t)?),
                Box::new(Expr1::from(f)?),
                *loc,
            ),
        };
        Ok(expr)
    }

    // If you use a dimension name, like the "Boston" element from a "Cities" dimension,
    // we will replace that variable-name-looking string with the constant offset of that
    // dimension element.
    pub(crate) fn constify_dimensions(self, dimensions: &DimensionsContext) -> Self {
        match self {
            Expr1::Const(s, n, loc) => Expr1::Const(s, n, loc),
            Expr1::Var(id, loc) => {
                if let Some(off) = dimensions.lookup(id.as_str()) {
                    Expr1::Const(id.to_string(), Literal::new(off as f64), loc)
                } else {
                    Expr1::Var(id, loc)
                }
            }
            Expr1::App(func, loc) => {
                Expr1::App(func.map(|arg| arg.constify_dimensions(dimensions)), loc)
            }
            Expr1::Subscript(id, args, loc) => Expr1::Subscript(
                id,
                args.into_iter()
                    .map(|arg| arg.constify_dimensions(dimensions))
                    .collect(),
                loc,
            ),
            Expr1::Op1(op, l, loc) => {
                Expr1::Op1(op, Box::new(l.constify_dimensions(dimensions)), loc)
            }
            Expr1::Op2(op, l, r, loc) => Expr1::Op2(
                op,
                Box::new(l.constify_dimensions(dimensions)),
                Box::new(r.constify_dimensions(dimensions)),
                loc,
            ),
            Expr1::If(cond, l, r, loc) => Expr1::If(
                Box::new(cond.constify_dimensions(dimensions)),
                Box::new(l.constify_dimensions(dimensions)),
                Box::new(r.constify_dimensions(dimensions)),
                loc,
            ),
        }
    }
}

impl Default for Expr1 {
    fn default() -> Self {
        Expr1::Const("0.0".to_string(), Literal::new(0.0), Loc::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{ErrorCode, RawIdent};

    fn const0(n: usize) -> Expr0 {
        Expr0::Const(n.to_string(), Literal::new(n as f64), Loc::default())
    }

    fn call(name: &str, args: Vec<Expr0>) -> EquationResult<Expr1> {
        Expr1::from(&Expr0::App(
            UntypedBuiltinFn(name.to_string(), args.into()),
            Loc::default(),
        ))
    }

    /// The constructor admits exactly the arities each signature declares, and
    /// the node it builds reports the argument count the call spelled. Rows
    /// are every signature in the table at every admitted arity (a variadic
    /// one up to four), plus one below and one above the range.
    #[test]
    fn constructor_admits_exactly_the_signatures_arity_range() {
        for sig in BuiltinSig::ALL {
            let min = sig.min_args as usize;
            let max = sig.max_args.map_or(min.max(1) + 3, |m| m as usize);
            let ident_arg = sig.arg_kinds.contains(&crate::builtins::ArgKind::Ident);
            for n in min..=max {
                let args: Vec<Expr0> = if ident_arg {
                    (0..n)
                        .map(|_| Expr0::Var(RawIdent::new("x".to_string()), Loc::default()))
                        .collect()
                } else {
                    (1..=n).map(const0).collect()
                };
                let built = call(sig.name, args)
                    .unwrap_or_else(|e| panic!("{}({n} args) must construct, got {e:?}", sig.name));
                let Expr1::App(builtin, _) = built else {
                    panic!("{}: expected an App", sig.name)
                };
                assert!(
                    std::ptr::eq(builtin.signature(), sig),
                    "{}: signature",
                    sig.name
                );
                // `PREVIOUS(x)` desugars to `PREVIOUS(x, 0)`, so the node holds
                // one argument more than a one-argument call spelled.
                let expected_exprs = if sig.name == "previous" {
                    2
                } else {
                    n - usize::from(ident_arg)
                };
                assert_eq!(
                    builtin.args().len(),
                    expected_exprs,
                    "{}({n} args): expression argument count",
                    sig.name
                );
                for alias in sig.aliases {
                    let aliased = call(alias, (1..=n).map(const0).collect()).unwrap();
                    let Expr1::App(aliased, _) = aliased else {
                        unreachable!()
                    };
                    assert!(std::ptr::eq(aliased.signature(), sig), "alias {alias}");
                }
            }
            if min > 0 {
                let err = call(sig.name, (1..min).map(const0).collect()).unwrap_err();
                assert_eq!(err.code, ErrorCode::BadBuiltinArgs, "{}: too few", sig.name);
            }
            if let Some(max) = sig.max_args {
                let too_many = (1..=max as usize + 1).map(const0).collect();
                let err = call(sig.name, too_many).unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::BadBuiltinArgs,
                    "{}: too many",
                    sig.name
                );
            }
        }
    }

    /// The reason a rejected call carries is a sentence a modeler can act on:
    /// it names the function the way the CALL spelled it (an alias stays the
    /// alias), and its two numbers agree with their nouns.
    ///
    /// Rows cover both directions of the `given` agreement and both directions
    /// of the arity phrase's; `builtins::tests::arity_phrase_covers_every_
    /// shape_in_the_signature_table` covers the phrase's four shapes.
    #[test]
    fn a_rejected_call_says_what_it_takes_and_what_it_was_given() {
        // (spelling, argument count, expected reason)
        let rows = [
            ("abs", 2, "ABS takes 1 argument, but 2 were given"),
            ("pulse", 1, "PULSE takes 2 or 3 arguments, but 1 was given"),
            // `dt` is an ALIAS of the `time_step` signature: the message must
            // answer with the name the modeler typed, not the canonical one.
            ("dt", 1, "DT takes 0 arguments, but 1 was given"),
        ];
        for (name, argc, expected) in rows {
            let err = call(name, (1..=argc).map(const0).collect()).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadBuiltinArgs, "{name}");
            assert_eq!(err.details.as_deref(), Some(expected), "{name}");
        }
    }

    #[test]
    fn unknown_names_and_non_identifier_module_input_args_are_rejected() {
        assert_eq!(
            call("lookupz", vec![const0(1)]).unwrap_err().code,
            ErrorCode::UnknownBuiltin
        );
        assert_eq!(
            call("ismoduleinput", vec![const0(1)]).unwrap_err().code,
            ErrorCode::ExpectedIdent
        );
    }

    #[test]
    fn unary_previous_desugars_to_a_zero_fallback() {
        let Expr1::App(BuiltinFn::Previous(_, fallback), _) =
            call("previous", vec![const0(7)]).unwrap()
        else {
            panic!("expected PREVIOUS")
        };
        assert!(matches!(*fallback, Expr1::Const(_, value, _) if value == Literal::new(0.0)));
    }
}
