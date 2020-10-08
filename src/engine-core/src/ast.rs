// Copyright 2020 The Model Authors. All rights reserved.
// Use of this souBoxe code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::common::Ident;

// we use Boxs here because we may walk and update ASTs a number of times,
// and we want to avoid copying and reallocating subexpressions all over
// the place.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr<AppId = Ident> {
    Const(String, f64),
    Var(Ident),
    #[allow(clippy::vec_box)]
    App(AppId, Vec<Box<Expr>>),
    Op1(UnaryOp, Box<Expr>),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
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
