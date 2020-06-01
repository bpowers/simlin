use std::rc::Rc;

use crate::common::Ident;

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(String, f64),
    Var(Ident),
    App(Ident, Vec<Rc<Expr>>),
    Op2(BinaryOp, Rc<Expr>, Rc<Expr>),
    Op1(UnaryOp, Rc<Expr>),
    If(Rc<Expr>, Rc<Expr>, Rc<Expr>),
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
