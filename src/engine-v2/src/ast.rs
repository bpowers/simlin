use sd::common::Ident;

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64),
    Var(Ident),
    App(Ident, Vec<Box<Expr>>),
    Op2(Op2, Box<Expr>, Box<Expr>),
    Op1(Op1, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(PartialEq, Clone, Debug)]
pub enum Cmd {
    Skip,
    Abort,
    While(Box<Expr>, Box<Cmd>),
    Seq(Box<Cmd>, Box<Cmd>),
    Assign(Ident, Box<Expr>),
    Call(Box<Expr>), // app
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum Op2 {
    Exp,
    Mul,
    Div,
    Mod,
    Add,
    Sub,
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
pub enum Op1 {
    Positive,
    Negative,
    Not,
}
