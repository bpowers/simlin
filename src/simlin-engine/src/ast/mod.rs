// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub use crate::builtins::Loc;
use std::collections::HashMap;

use crate::builtins::{BuiltinContents, UntypedBuiltinFn, walk_builtin_expr};
use crate::common::{ElementName, EquationResult};
use crate::datamodel::Dimension;
use crate::model::ScopeStage0;

mod expr0;
mod expr1;
mod expr2;

pub use expr0::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
pub use expr1::Expr1;
#[allow(unused_imports)]
pub use expr2::{ArraySource, ArrayView, Expr2, IndexExpr2, StridedDimension};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<Expr> {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

impl Ast<Expr2> {
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            Ast::Arrayed(_, subscripts) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
        }
    }

    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(_, _) => "TODO(array)".to_owned(),
        }
    }
}

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr2>> {
    match ast {
        Ast::Scalar(expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .and_then(Expr2::from)
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .and_then(Expr2::from)
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr2>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr1::from(expr)
                        .map(|expr| expr.constify_dimensions(scope))
                        .and_then(Expr2::from)
                    {
                        Ok(expr) => Ok((id, expr)),
                        Err(err) => Err(err),
                    }
                })
                .collect();
            match elements {
                Ok(elements) => Ok(Ast::Arrayed(dims, elements)),
                Err(err) => Err(err),
            }
        }
    }
}

/// Visitors walk Expr ASTs.
pub trait Visitor<T> {
    fn walk_index(&mut self, e: &IndexExpr0) -> T;
    fn walk(&mut self, e: &Expr0) -> T;
}

macro_rules! child_needs_parens(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _) | $expr::Subscript(_, _, _) => false,
            $expr::Op1(_, _, _) => matches!($child, $expr::Op2(_, _, _, _)),
            $expr::Op2(parent_op, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _)
                | $expr::App(_, _)
                | $expr::Subscript(_, _, _)
                | $expr::If(_, _, _, _)
                | $expr::Op1(_, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary(parent: &Expr0, child: &Expr0, eqn: String) -> String {
    if child_needs_parens!(Expr0, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

macro_rules! child_needs_parens2(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _, _) | $expr::Subscript(_, _, _, _) => false,
            $expr::Op1(_, _, _, _) => matches!($child, $expr::Op2(_, _, _, _, _)),
            $expr::Op2(parent_op, _, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _, _)
                | $expr::App(_, _, _)
                | $expr::Subscript(_, _, _, _)
                | $expr::If(_, _, _, _, _)
                | $expr::Op1(_, _, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary1(parent: &Expr2, child: &Expr2, eqn: String) -> String {
    if child_needs_parens2!(Expr2, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => format!("*:{}", id),
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{}", n),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => id.clone(),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                match op {
                    UnaryOp::Transpose => {
                        let l = self.walk(l);
                        format!("{}'", l)
                    }
                    _ => {
                        let l = paren_if_necessary(expr, l, self.walk(l));
                        let op: &str = match op {
                            UnaryOp::Positive => "+",
                            UnaryOp::Negative => "-",
                            UnaryOp::Not => "!",
                            UnaryOp::Transpose => unreachable!(), // handled above
                        };
                        format!("{}{}", op, l)
                    }
                }
            }
            Expr0::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "mod",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({}) then ({}) else ({})", cond, t, f)
            }
        }
    }
}

pub fn print_eqn(expr: &Expr0) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    assert_eq!(
        "a + b",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a + b * c",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mul,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a * (b + c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Not,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Positive,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr0::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr0::App(
            UntypedBuiltinFn(
                "lookup".to_string(),
                vec![
                    Expr0::Var("a".to_string(), Loc::new(7, 8)),
                    Expr0::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
                ]
            ),
            Loc::new(0, 14),
        ))
    );
}

struct LatexVisitor {}

impl LatexVisitor {
    fn walk_index(&mut self, expr: &IndexExpr2) -> String {
        match expr {
            IndexExpr2::Wildcard(_) => "*".to_string(),
            IndexExpr2::StarRange(id, _) => format!("*:{}", id),
            IndexExpr2::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr2::DimPosition(n, _) => format!("@{}", n),
            IndexExpr2::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr2) -> String {
        match expr {
            Expr2::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr2::Var(id, _, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr2::App(builtin, _, _) => {
                let mut args: Vec<String> = vec![];
                walk_builtin_expr(builtin, |contents| {
                    let arg = match contents {
                        BuiltinContents::Ident(id, _loc) => format!("\\mathrm{{{}}}", id),
                        BuiltinContents::Expr(expr) => self.walk(expr),
                    };
                    args.push(arg);
                });
                let func = builtin.name();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr2::Subscript(id, args, _, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr2::Op1(op, l, _, _) => {
                match op {
                    UnaryOp::Transpose => {
                        let l = self.walk(l);
                        format!("{}^T", l)
                    }
                    _ => {
                        let l = paren_if_necessary1(expr, l, self.walk(l));
                        let op: &str = match op {
                            UnaryOp::Positive => "+",
                            UnaryOp::Negative => "-",
                            UnaryOp::Not => "\\neg ",
                            UnaryOp::Transpose => unreachable!(), // handled above
                        };
                        format!("{}{}", op, l)
                    }
                }
            }
            Expr2::Op2(op, l, r, _, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let r = paren_if_necessary1(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => {
                        return format!("{}^{{{}}}", l, r);
                    }
                    BinaryOp::Mul => "\\cdot",
                    BinaryOp::Div => {
                        return format!("\\frac{{{}}}{{{}}}", l, r);
                    }
                    BinaryOp::Mod => "%",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr2::If(cond, t, f, _, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);

                format!(
                    "\\begin{{cases}}
                     {} & \\text{{if }} {} \\\\
                     {} & \\text{{else}}
                 \\end{{cases}}",
                    t, cond, f
                )
            }
        }
    }
}

pub fn latex_eqn(expr: &Expr2) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(1, 2))),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(1, 2))),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Op2(
                BinaryOp::Sub,
                Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(0, 0))),
                Box::new(Expr2::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                None,
                Loc::new(0, 0),
            )),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            Box::new(Expr2::Op2(
                BinaryOp::Sub,
                Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(0, 0))),
                Box::new(Expr2::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                None,
                Loc::new(0, 0),
            )),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Negative,
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Positive,
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr2::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr2::App(
            crate::builtins::BuiltinFn::Lookup(
                "a".to_string(),
                Box::new(Expr2::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            None,
            Loc::new(0, 14),
        ))
    );
}
