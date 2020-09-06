// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::rc::Rc;

use crate::common::Ident;

// we use Rcs here because we may walk and update ASTs a number of times,
// and we want to avoid copying and reallocating subexpressions all over
// the place.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr<AppId = Ident> {
    Const(String, f64),
    Var(Ident),
    App(AppId, Vec<Rc<Expr>>),
    Op1(UnaryOp, Rc<Expr>),
    Op2(BinaryOp, Rc<Expr>, Rc<Expr>),
    If(Rc<Expr>, Rc<Expr>, Rc<Expr>),
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

#[test]
fn test_rcs() {
    fn rewrite_op1(e: Rc<Expr>) -> Rc<Expr> {
        let mut e = e;
        let e_mut = Rc::make_mut(&mut e);

        if let Expr::Op1(op, ..) = e_mut {
            *e_mut = Expr::Op1(*op, Rc::new(Expr::Const("3".to_string(), 3.0)))
        }

        e
    }

    let var_a = Rc::new(Expr::Var("a".to_string()));
    let e = Rc::new(Expr::Op1(UnaryOp::Positive, var_a));

    let e = rewrite_op1(e);

    let e2 = Expr::Op1(
        UnaryOp::Positive,
        Rc::new(Expr::Const("3".to_string(), 3.0)),
    );
    assert_eq!(e.as_ref(), &e2);
}
