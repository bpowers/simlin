// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL equation text writer.
//!
//! Converts `Expr0` AST nodes into Vensim MDL-format equation text.
//! The key transformation vs the XMILE printer (`ast::print_eqn`) is
//! converting canonical (underscored, lowercase) identifiers back to
//! MDL-style spaced names and using MDL operator syntax.

use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp, Visitor};
use crate::builtins::UntypedBuiltinFn;

/// Replace underscores with spaces -- the reverse of `space_to_underbar()`.
fn underbar_to_space(name: &str) -> String {
    name.replace('_', " ")
}

/// Parenthesize `eqn` when the child's precedence is lower than the parent's,
/// mirroring `paren_if_necessary()` in `ast/mod.rs`.
fn mdl_paren_if_necessary(parent: &Expr0, child: &Expr0, eqn: String) -> String {
    let needs = match parent {
        Expr0::Const(_, _, _) | Expr0::Var(_, _) => false,
        Expr0::App(_, _) | Expr0::Subscript(_, _, _) => false,
        Expr0::Op1(_, _, _) => matches!(child, Expr0::Op2(_, _, _, _)),
        Expr0::Op2(parent_op, _, _, _) => match child {
            Expr0::Op2(child_op, _, _, _) => parent_op.precedence() > child_op.precedence(),
            _ => false,
        },
        Expr0::If(_, _, _, _) => false,
    };
    if needs { format!("({eqn})") } else { eqn }
}

struct MdlPrintVisitor;

impl Visitor<String> for MdlPrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => {
                format!("*:{}", underbar_to_space(id.as_str()))
            }
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{n}"),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => underbar_to_space(id.as_str()),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func.to_uppercase(), args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", underbar_to_space(id.as_str()), args.join(", "))
            }
            Expr0::Op1(op, l, _) => match op {
                UnaryOp::Transpose => {
                    let l = self.walk(l);
                    format!("{l}'")
                }
                _ => {
                    let l = mdl_paren_if_necessary(expr, l, self.walk(l));
                    let op_str = match op {
                        UnaryOp::Positive => "+",
                        UnaryOp::Negative => "-",
                        UnaryOp::Not => "!",
                        UnaryOp::Transpose => unreachable!(),
                    };
                    format!("{op_str}{l}")
                }
            },
            Expr0::Op2(op, l, r, _) => {
                let l = mdl_paren_if_necessary(expr, l, self.walk(l));
                let r = mdl_paren_if_necessary(expr, r, self.walk(r));
                let op_str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "MOD",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "<>",
                    BinaryOp::And => ":AND:",
                    BinaryOp::Or => ":OR:",
                };
                format!("{l} {op_str} {r}")
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("IF THEN ELSE({cond}, {t}, {f})")
            }
        }
    }
}

/// Convert an `Expr0` AST to MDL-format equation text.
pub fn expr0_to_mdl(expr: &Expr0) -> String {
    let mut visitor = MdlPrintVisitor;
    visitor.walk(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr0;
    use crate::lexer::LexerType;

    /// Parse XMILE equation text to Expr0, then convert to MDL and assert.
    fn assert_mdl(xmile_eqn: &str, expected_mdl: &str) {
        let ast = Expr0::new(xmile_eqn, LexerType::Equation)
            .expect("parse should succeed")
            .expect("expression should not be empty");
        let mdl = expr0_to_mdl(&ast);
        assert_eq!(
            expected_mdl, &mdl,
            "MDL mismatch for XMILE input: {xmile_eqn:?}"
        );
    }

    #[test]
    fn constants() {
        assert_mdl("5", "5");
        assert_mdl("3.14", "3.14");
        assert_mdl("1e3", "1e3");
    }

    #[test]
    fn nan_constant() {
        let ast = Expr0::new("NAN", LexerType::Equation).unwrap().unwrap();
        let mdl = expr0_to_mdl(&ast);
        assert_eq!("NaN", &mdl);
    }

    #[test]
    fn variable_references() {
        assert_mdl("population_growth_rate", "population growth rate");
        assert_mdl("x", "x");
        assert_mdl("a_b_c", "a b c");
    }

    #[test]
    fn arithmetic_operators() {
        assert_mdl("a + b", "a + b");
        assert_mdl("a - b", "a - b");
        assert_mdl("a * b", "a * b");
        assert_mdl("a / b", "a / b");
        assert_mdl("a ^ b", "a ^ b");
    }

    #[test]
    fn precedence_no_extra_parens() {
        assert_mdl("a + b * c", "a + b * c");
    }

    #[test]
    fn precedence_parens_emitted() {
        assert_mdl("(a + b) * c", "(a + b) * c");
    }

    #[test]
    fn nested_precedence() {
        assert_mdl("a * (b + c) / d", "a * (b + c) / d");
    }

    #[test]
    fn unary_operators() {
        assert_mdl("-a", "-a");
        assert_mdl("+a", "+a");
    }
}
