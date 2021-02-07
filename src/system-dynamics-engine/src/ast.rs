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
    pub start: u16,
    pub end: u16,
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

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr::Const(_s, _n, _loc) => None,
            Expr::Var(v, loc) if v == ident => Some(*loc),
            Expr::Var(_v, _loc) => None,
            Expr::App(_builtin, args, _loc) => {
                for arg in args.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr::Subscript(id, subscripts, loc) => {
                if id == ident {
                    let start = loc.start as usize;
                    return Some(Loc::new(start, start + id.len()));
                }
                for arg in subscripts.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr::Op1(_op, r, _loc) => r.get_var_loc(ident),
            Expr::Op2(_op, l, r, _loc) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr::If(cond, t, f, _loc) => cond
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
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

impl AST {
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            AST::Scalar(expr) => expr.get_var_loc(ident),
            AST::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            AST::Arrayed(_, subscripts) => {
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
            AST::Scalar(expr) => latex_eqn(expr),
            AST::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            AST::Arrayed(_, _) => "TODO(array)".to_owned(),
        }
    }
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

impl BinaryOp {
    // higher the precedence, the tighter the binding.
    // e.g. Mul.precedence() > Add.precedence()
    fn precedence(&self) -> u8 {
        // matches equation.lalrpop
        match self {
            BinaryOp::Add => 4,
            BinaryOp::Sub => 4,
            BinaryOp::Exp => 6,
            BinaryOp::Mul => 5,
            BinaryOp::Div => 5,
            BinaryOp::Mod => 5,
            BinaryOp::Gt => 3,
            BinaryOp::Lt => 3,
            BinaryOp::Gte => 3,
            BinaryOp::Lte => 3,
            BinaryOp::Eq => 2,
            BinaryOp::Neq => 2,
            BinaryOp::And => 1,
            BinaryOp::Or => 1,
        }
    }
}

fn child_needs_parens(parent: &Expr, child: &Expr) -> bool {
    match parent {
        // no children so doesn't matter
        Expr::Const(_, _, _) | Expr::Var(_, _) => false,
        // children are comma separated, so no ambiguity possible
        Expr::App(_, _, _) | Expr::Subscript(_, _, _) => false,
        Expr::Op1(_, _, _) => matches!(child, Expr::Op2(_, _, _, _)),
        Expr::Op2(parent_op, _, _, _) => match child {
            Expr::Const(_, _, _)
            | Expr::Var(_, _)
            | Expr::App(_, _, _)
            | Expr::Subscript(_, _, _)
            | Expr::If(_, _, _, _)
            | Expr::Op1(_, _, _) => false,
            // 3 * 2 + 1
            Expr::Op2(child_op, _, _, _) => {
                // if we have `3 * (2 + 3)`, the parent's precedence
                // is higher than the child and we need enclosing parens
                parent_op.precedence() > child_op.precedence()
            }
        },
        Expr::If(_, _, _, _) => false,
    }
}

fn paren_if_necessary(parent: &Expr, child: &Expr, eqn: String) -> String {
    if child_needs_parens(parent, child) {
        format!("({})", eqn)
    } else {
        eqn
    }
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
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
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

struct LatexVisitor {}

impl Visitor<String> for LatexVisitor {
    fn walk(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr::App(func, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "\\neg ",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
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
            Expr::If(cond, t, f, _) => {
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

pub fn latex_eqn(expr: &Expr) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr::App(
            "lookup".to_string(),
            vec![
                Expr::Var("a".to_string(), Loc::new(7, 8)),
                Expr::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
            ],
            Loc::new(0, 14),
        ))
    );
}
