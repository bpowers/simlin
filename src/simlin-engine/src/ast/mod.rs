// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub mod expr0;
pub mod expr1;
pub mod expr2;

// Re-export all public types for backward compatibility
pub use expr0::{Expr0, IndexExpr0};
pub use expr1::{Expr1, IndexExpr1};
pub use expr2::{BinaryOp, UnaryOp};

// Re-export Loc for convenience
pub use crate::builtins::Loc;
use crate::builtins::UntypedBuiltinFn;
use crate::common::{ElementName, EquationResult};
use crate::datamodel::Dimension;
use crate::model::ScopeStage0;
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<T> {
    Scalar(T),
    ApplyToAll(Vec<Dimension>, T),
    Arrayed(Vec<Dimension>, HashMap<ElementName, T>),
}

impl Ast<Expr1> {
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

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr1>> {
    match ast {
        Ast::Scalar(expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr1>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr1::from(expr).map(|expr| expr.constify_dimensions(scope)) {
                        Ok(expr) => Ok((id, expr)),
                        Err(err) => Err(err),
                    }
                })
                .collect();
            elements.map(|elements| Ast::Arrayed(dims, elements))
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

fn paren_if_necessary1(parent: &Expr1, child: &Expr1, eqn: String) -> String {
    if child_needs_parens!(Expr1, parent, child, eqn) {
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
                let l = paren_if_necessary(expr, l, self.walk(l));
                match op {
                    UnaryOp::Positive => format!("+{}", l),
                    UnaryOp::Negative => format!("-{}", l),
                    UnaryOp::Not => format!("!{}", l),
                    UnaryOp::Transpose => format!("{}'", l),
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

struct LatexVisitor {}

impl LatexVisitor {
    fn walk_index(&mut self, expr: &IndexExpr1) -> String {
        match expr {
            IndexExpr1::Wildcard(_) => "*".to_string(),
            IndexExpr1::StarRange(id, _) => format!("*:{}", id),
            IndexExpr1::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr1::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr1) -> String {
        use crate::builtins::{BuiltinContents, walk_builtin_expr};
        match expr {
            Expr1::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{NaN}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr1::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr1::App(builtin, _) => {
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
            Expr1::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr1::Op1(op, l, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                match op {
                    UnaryOp::Positive => format!("+{}", l),
                    UnaryOp::Negative => format!("-{}", l),
                    UnaryOp::Not => format!("\\neg {}", l),
                    UnaryOp::Transpose => format!("{}'", l),
                }
            }
            Expr1::Op2(op, l, r, _) => {
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
            Expr1::If(cond, t, f, _) => {
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

pub fn latex_eqn(expr: &Expr1) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn test_transpose_precedence() {
        // Test that transpose has highest precedence
        assert_eq!(
            "a' + b",
            print_eqn(&Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Op1(
                    UnaryOp::Transpose,
                    Box::new(Expr0::Var("a".to_string(), Loc::default())),
                    Loc::default()
                )),
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Loc::default(),
            ))
        );
    }
}
