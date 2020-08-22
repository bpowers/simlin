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

pub trait Visitor {
    fn visit_const(&mut self, e: Expr) -> Expr;
    fn visit_var(&mut self, e: Expr) -> Expr;
    fn visit_app(&mut self, e: Expr) -> Expr;
    fn visit_op2(&mut self, e: Expr) -> Expr;
    fn visit_op1(&mut self, e: Expr) -> Expr;
    fn visit_if(&mut self, e: Expr) -> Expr;
}

impl Expr {
    pub fn walk(self, v: &mut dyn Visitor) -> Self {
        match self {
            Expr::Const(..) => v.visit_const(self),
            Expr::Var(..) => v.visit_var(self),
            Expr::App(..) => v.visit_app(self),
            Expr::Op2(..) => v.visit_op2(self),
            Expr::Op1(..) => v.visit_op1(self),
            Expr::If(..) => v.visit_if(self),
        }
    }
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
