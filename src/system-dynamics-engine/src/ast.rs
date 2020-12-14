// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{ElementName, Ident};
use crate::datamodel::Dimension;

// we use Boxs here because we may walk and update ASTs a number of times,
// and we want to avoid copying and reallocating subexpressions all over
// the place.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr<AppId = Ident> {
    Const(String, f64),
    Var(Ident),
    App(AppId, Vec<Expr>),
    Subscript(Ident, Vec<Expr>),
    Op1(UnaryOp, Box<Expr>),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Const("0.0".to_string(), 0.0)
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
            Expr::Const(s, _) => s.clone(),
            Expr::Var(id) => id.clone(),
            Expr::App(func, args) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr::Subscript(id, args) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l) => {
                let l = self.walk(l);
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r) => {
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
            Expr::If(cond, t, f) => {
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
            Box::new(Expr::Var("a".to_string())),
            Box::new(Expr::Var("b".to_string()))
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string()))
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string()))
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string()))
        ))
    );
    assert_eq!("4.7", print_eqn(&Expr::Const("4.7".to_string(), 4.7)));
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr::App(
            "lookup".to_string(),
            vec![
                Expr::Var("a".to_string()),
                Expr::Const("1.0".to_string(), 1.0)
            ]
        ))
    );
}
