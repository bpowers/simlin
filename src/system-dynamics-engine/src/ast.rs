// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{ElementName, Ident};
use crate::datamodel::Dimension;

// equations are strings typed by humans for a single
// variable -- u16 is long enough
#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Loc {
    start: u16,
    end: u16,
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

// we use Boxs here because we may walk and update ASTs a number of times,
// and we want to avoid copying and reallocating subexpressions all over
// the place.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr<AppId = Ident> {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(AppId, Vec<Expr>, Loc),
    Subscript(Ident, Vec<Expr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
}

impl Expr {
    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr::Const(s, n, _loc) => Expr::Const(s, n, loc),
            Expr::Var(v, _loc) => Expr::Var(v, loc),
            Expr::App(builtin, args, _loc) => Expr::App(
                builtin,
                args.into_iter().map(|arg| arg.strip_loc()).collect(),
                loc,
            ),
            Expr::Subscript(off, subscripts, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr::Subscript(off, subscripts, loc)
            }
            Expr::Op1(op, r, _loc) => Expr::Op1(op, Box::new(r.strip_loc()), loc),
            Expr::Op2(op, l, r, _loc) => {
                Expr::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr::If(cond, t, f, _loc) => Expr::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
        }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum AST {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

pub trait Visitor<T> {
    fn walk(&mut self, e: &Expr) -> T;
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Const(s, _, _) => s.clone(),
            Expr::Var(id, _) => id.clone(),
            Expr::App(func, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l, _) => {
                let l = self.walk(l);
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
                let l = self.walk(l);
                let r = self.walk(r);
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
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
                format!("({} {} {})", l, op, r)
            }
            Expr::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({}) then ({}) else ({})", cond, t, f)
            }
        }
    }
}

pub fn print_eqn(expr: &Expr) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    assert_eq!(
        "(a + b)",
        print_eqn(&Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr::App(
            "lookup".to_string(),
            vec![
                Expr::Var("a".to_string(), Loc::new(7, 8)),
                Expr::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
            ],
            Loc::new(0, 14),
        ))
    );
}
