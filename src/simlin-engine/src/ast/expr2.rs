// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{EquationResult, Ident};

/// IndexExpr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr2 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr2, Expr2, Loc),
    DimPosition(u32, Loc),
    Expr(Expr2),
}

impl IndexExpr2 {
    pub(crate) fn from(expr: IndexExpr1) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => IndexExpr2::StarRange(ident, loc),
            IndexExpr1::Range(l, r, loc) => {
                IndexExpr2::Range(Expr2::from(l)?, Expr2::from(r)?, loc)
            }
            IndexExpr1::DimPosition(n, loc) => IndexExpr2::DimPosition(n, loc),
            IndexExpr1::Expr(e) => IndexExpr2::Expr(Expr2::from(e)?),
        };

        Ok(expr)
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr2::Wildcard(_) => None,
            IndexExpr2::StarRange(v, loc) => {
                if v == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr2::Range(l, r, _) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            IndexExpr2::DimPosition(_, _) => None,
            IndexExpr2::Expr(e) => e.get_var_loc(ident),
        }
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[allow(dead_code)]
#[derive(PartialEq, Clone, Debug)]
pub enum Expr2 {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(BuiltinFn<Expr2>, Loc),
    Subscript(Ident, Vec<IndexExpr2>, Loc),
    Op1(UnaryOp, Box<Expr2>, Loc),
    Op2(BinaryOp, Box<Expr2>, Box<Expr2>, Loc),
    If(Box<Expr2>, Box<Expr2>, Box<Expr2>, Loc),
}

impl Expr2 {
    pub(crate) fn from(expr: Expr1) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => Expr2::Var(id, loc),
            Expr1::App(builtin_fn, loc) => {
                use BuiltinFn::*;
                let builtin = match builtin_fn {
                    Lookup(v, e, loc) => Lookup(v, Box::new(Expr2::from(*e)?), loc),
                    Abs(e) => Abs(Box::new(Expr2::from(*e)?)),
                    Arccos(e) => Arccos(Box::new(Expr2::from(*e)?)),
                    Arcsin(e) => Arcsin(Box::new(Expr2::from(*e)?)),
                    Arctan(e) => Arctan(Box::new(Expr2::from(*e)?)),
                    Cos(e) => Cos(Box::new(Expr2::from(*e)?)),
                    Exp(e) => Exp(Box::new(Expr2::from(*e)?)),
                    Inf => Inf,
                    Int(e) => Int(Box::new(Expr2::from(*e)?)),
                    IsModuleInput(s, loc) => IsModuleInput(s, loc),
                    Ln(e) => Ln(Box::new(Expr2::from(*e)?)),
                    Log10(e) => Log10(Box::new(Expr2::from(*e)?)),
                    Max(e1, e2) => Max(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Mean(exprs) => {
                        let exprs: EquationResult<Vec<Expr2>> =
                            exprs.into_iter().map(Expr2::from).collect();
                        Mean(exprs?)
                    }
                    Min(e1, e2) => Min(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Pi => Pi,
                    Pulse(e1, e2, e3) => Pulse(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Ramp(e1, e2, e3) => Ramp(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    SafeDiv(e1, e2, e3) => SafeDiv(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Sin(e) => Sin(Box::new(Expr2::from(*e)?)),
                    Sqrt(e) => Sqrt(Box::new(Expr2::from(*e)?)),
                    Step(e1, e2) => Step(Box::new(Expr2::from(*e1)?), Box::new(Expr2::from(*e2)?)),
                    Tan(e) => Tan(Box::new(Expr2::from(*e)?)),
                    Time => Time,
                    TimeStep => TimeStep,
                    StartTime => StartTime,
                    FinalTime => FinalTime,
                    Rank(e, opt) => Rank(
                        Box::new(Expr2::from(*e)?),
                        opt.map(|(e1, opt_e2)| {
                            Ok::<_, crate::common::EquationError>((
                                Box::new(Expr2::from(*e1)?),
                                opt_e2.map(|e2| Expr2::from(*e2)).transpose()?.map(Box::new),
                            ))
                        })
                        .transpose()?,
                    ),
                    Size(e) => Size(Box::new(Expr2::from(*e)?)),
                    Stddev(e) => Stddev(Box::new(Expr2::from(*e)?)),
                    Sum(e) => Sum(Box::new(Expr2::from(*e)?)),
                };
                Expr2::App(builtin, loc)
            }
            Expr1::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr2>> =
                    args.into_iter().map(IndexExpr2::from).collect();
                let args = args?;
                Expr2::Subscript(id, args, loc)
            }
            Expr1::Op1(op, l, loc) => {
                let l_expr = Expr2::from(*l)?;
                Expr2::Op1(op, Box::new(l_expr), loc)
            }
            Expr1::Op2(op, l, r, loc) => {
                let l_expr = Expr2::from(*l)?;
                let r_expr = Expr2::from(*r)?;
                Expr2::Op2(op, Box::new(l_expr), Box::new(r_expr), loc)
            }
            Expr1::If(cond, t, f, loc) => {
                let cond_expr = Expr2::from(*cond)?;
                let t_expr = Expr2::from(*t)?;
                let f_expr = Expr2::from(*f)?;
                Expr2::If(Box::new(cond_expr), Box::new(t_expr), Box::new(f_expr), loc)
            }
        };
        Ok(expr)
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr2::Const(_, _, loc) => *loc,
            Expr2::Var(_, loc) => *loc,
            Expr2::App(_, loc) => *loc,
            Expr2::Subscript(_, _, loc) => *loc,
            Expr2::Op1(_, _, loc) => *loc,
            Expr2::Op2(_, _, _, loc) => *loc,
            Expr2::If(_, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr2::Const(_s, _n, _loc) => None,
            Expr2::Var(v, loc) if v == ident => Some(*loc),
            Expr2::Var(_v, _loc) => None,
            Expr2::App(builtin, _loc) => {
                let mut loc: Option<Loc> = None;
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(id, id_loc) => {
                        if ident == id {
                            loc = Some(id_loc);
                        }
                    }
                    BuiltinContents::Expr(expr) => {
                        if loc.is_none() {
                            loc = expr.get_var_loc(ident);
                        }
                    }
                });
                loc
            }
            Expr2::Subscript(v, _args, loc) if v == ident => Some(*loc),
            Expr2::Subscript(_v, args, _loc) => {
                for arg in args {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr2::Op1(_op, l, _loc) => l.get_var_loc(ident),
            Expr2::Op2(_op, l, r, _loc) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            Expr2::If(c, t, f, _loc) => {
                if let Some(loc) = c.get_var_loc(ident) {
                    return Some(loc);
                }
                if let Some(loc) = t.get_var_loc(ident) {
                    return Some(loc);
                }
                f.get_var_loc(ident)
            }
        }
    }
}

/// Evaluate a constant expression to an integer value.
/// This is used for array subscripts which must be integer constants.
#[cfg(test)]
fn const_int_eval(ast: &Expr2) -> EquationResult<i32> {
    use crate::eqn_err;
    use float_cmp::approx_eq;
    match ast {
        Expr2::Const(_, n, loc) => {
            if approx_eq!(f64, *n, n.round()) {
                Ok(n.round() as i32)
            } else {
                eqn_err!(ExpectedInteger, loc.start, loc.end)
            }
        }
        Expr2::Var(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::App(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::Subscript(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::Op1(op, expr, loc) => {
            let expr = const_int_eval(expr)?;
            let result = match op {
                UnaryOp::Positive => expr,
                UnaryOp::Negative => -expr,
                UnaryOp::Not => i32::from(expr == 0),
                UnaryOp::Transpose => {
                    // Transpose doesn't make sense for integer evaluation
                    return eqn_err!(ExpectedInteger, loc.start, loc.end);
                }
            };
            Ok(result)
        }
        Expr2::Op2(op, l, r, _) => {
            let l = const_int_eval(l)?;
            let r = const_int_eval(r)?;
            let result = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Exp => l.pow(r as u32),
                BinaryOp::Mul => l * r,
                BinaryOp::Div => {
                    if r == 0 {
                        0
                    } else {
                        l / r
                    }
                }
                BinaryOp::Mod => l % r,
                BinaryOp::Gt => (l > r) as i32,
                BinaryOp::Lt => (l < r) as i32,
                BinaryOp::Gte => (l >= r) as i32,
                BinaryOp::Lte => (l <= r) as i32,
                BinaryOp::Eq => (l == r) as i32,
                BinaryOp::Neq => (l != r) as i32,
                BinaryOp::And => ((l != 0) && (r != 0)) as i32,
                BinaryOp::Or => ((l != 0) || (r != 0)) as i32,
            };
            Ok(result)
        }
        Expr2::If(_, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_const_int_eval() {
        // Helper to create const expression
        fn const_expr(val: f64) -> Expr2 {
            Expr2::Const(val.to_string(), val, Loc::default())
        }

        // Test basic constants
        let const_cases = vec![
            (0.0, 0),
            (1.0, 1),
            (-1.0, -1),
            (42.0, 42),
            (3.0, 3), // Tests rounding
        ];

        for (val, expected) in const_cases {
            assert_eq!(expected, const_int_eval(&const_expr(val)).unwrap());
        }

        // Test error case
        assert!(const_int_eval(&const_expr(3.5)).is_err());
        assert!(const_int_eval(&Expr2::Var("foo".to_string(), Loc::default())).is_err());

        // Test unary operations
        let unary_cases = vec![
            (UnaryOp::Negative, 5, -5),
            (UnaryOp::Positive, 5, 5),
            (UnaryOp::Not, 0, 1),
            (UnaryOp::Not, 5, 0),
        ];

        for (op, input, expected) in unary_cases {
            let expr = Expr2::Op1(op, Box::new(const_expr(input as f64)), Loc::default());
            assert_eq!(expected, const_int_eval(&expr).unwrap());
        }

        // Test binary operations
        struct BinaryTestCase {
            op: BinaryOp,
            left: i32,
            right: i32,
            expected: i32,
        }

        let binary_cases = vec![
            BinaryTestCase {
                op: BinaryOp::Add,
                left: 2,
                right: 3,
                expected: 5,
            },
            BinaryTestCase {
                op: BinaryOp::Sub,
                left: 4,
                right: 1,
                expected: 3,
            },
            BinaryTestCase {
                op: BinaryOp::Mul,
                left: 3,
                right: 4,
                expected: 12,
            },
            BinaryTestCase {
                op: BinaryOp::Div,
                left: 7,
                right: 3,
                expected: 2,
            },
            BinaryTestCase {
                op: BinaryOp::Div,
                left: 7,
                right: 0,
                expected: 0,
            }, // div by zero
            BinaryTestCase {
                op: BinaryOp::Mod,
                left: 15,
                right: 7,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Exp,
                left: 3,
                right: 3,
                expected: 27,
            },
            BinaryTestCase {
                op: BinaryOp::Gt,
                left: 4,
                right: 2,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Lt,
                left: 2,
                right: 4,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Eq,
                left: 3,
                right: 3,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Neq,
                left: 3,
                right: 4,
                expected: 1,
            },
        ];

        for tc in binary_cases {
            let expr = Expr2::Op2(
                tc.op,
                Box::new(const_expr(tc.left as f64)),
                Box::new(const_expr(tc.right as f64)),
                Loc::default(),
            );
            assert_eq!(
                tc.expected,
                const_int_eval(&expr).unwrap(),
                "Failed for {:?} {} {}",
                tc.op,
                tc.left,
                tc.right
            );
        }

        // Test complex expression: (2 * 3) + 1 = 7
        let complex = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Op2(
                BinaryOp::Mul,
                Box::new(const_expr(2.0)),
                Box::new(const_expr(3.0)),
                Loc::default(),
            )),
            Box::new(const_expr(1.0)),
            Loc::default(),
        );
        assert_eq!(7, const_int_eval(&complex).unwrap());
    }
}
