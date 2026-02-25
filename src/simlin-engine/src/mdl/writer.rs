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

/// Map XMILE canonical function names back to their Vensim MDL equivalents.
/// This inverts the `format_function_name()` table in `xmile_compat.rs`.
/// The input is expected to already be lowercase (as stored in `Expr0::App`).
fn xmile_to_mdl_function_name(xmile_name: &str) -> String {
    match xmile_name {
        "smth1" => "SMOOTH".to_owned(),
        "smth3" => "SMOOTH3".to_owned(),
        "delay" => "DELAY FIXED".to_owned(),
        "delay1" => "DELAY1".to_owned(),
        "delay3" => "DELAY3".to_owned(),
        "delayn" => "DELAY N".to_owned(),
        "smthn" => "SMOOTH N".to_owned(),
        "init" => "ACTIVE INITIAL".to_owned(),
        "int" => "INTEGER".to_owned(),
        "lookupinv" => "LOOKUP INVERT".to_owned(),
        "uniform" => "RANDOM UNIFORM".to_owned(),
        "safediv" => "ZIDZ".to_owned(),
        "forcst" => "FORECAST".to_owned(),
        "normalpink" => "RANDOM PINK NOISE".to_owned(),
        "normal" => "RANDOM NORMAL".to_owned(),
        "lookup" => "LOOKUP".to_owned(),
        "integ" => "INTEG".to_owned(),
        _ => underbar_to_space(xmile_name).to_uppercase(),
    }
}

/// Reorder arguments for functions whose XMILE and MDL arg orders differ.
fn reorder_args(mdl_name: &str, mut args: Vec<String>) -> Vec<String> {
    match mdl_name {
        // XMILE: delayn(input, dt, n, init) -> MDL: DELAY N(input, dt, init, n)
        // XMILE: smthn(input, dt, n, init) -> MDL: SMOOTH N(input, dt, init, n)
        "DELAY N" | "SMOOTH N" => {
            if args.len() >= 4 {
                args.swap(2, 3);
            }
            args
        }
        // XMILE: normal(mean, sd, seed, min, max) -> MDL: RANDOM NORMAL(min, max, mean, sd, seed)
        "RANDOM NORMAL" => {
            if args.len() >= 5 {
                let mean = args[0].clone();
                let sd = args[1].clone();
                let seed = args[2].clone();
                let min = args[3].clone();
                let max = args[4].clone();
                args[0] = min;
                args[1] = max;
                args[2] = mean;
                args[3] = sd;
                args[4] = seed;
            }
            args
        }
        _ => args,
    }
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
                let mdl_name = xmile_to_mdl_function_name(func);
                let converted: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                let reordered = reorder_args(&mdl_name, converted);
                format!("{}({})", mdl_name, reordered.join(", "))
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
                    match op {
                        UnaryOp::Positive => format!("+{l}"),
                        UnaryOp::Negative => format!("-{l}"),
                        // MDL uses the keyword form with a trailing space before the operand
                        UnaryOp::Not => format!(":NOT: {l}"),
                        UnaryOp::Transpose => unreachable!(),
                    }
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
        // XMILE uses `not` keyword; MDL uses `:NOT:` with a trailing space before the operand
        assert_mdl("not a", ":NOT: a");
    }

    #[test]
    fn function_rename_smooth() {
        assert_mdl("smth1(x, 5)", "SMOOTH(x, 5)");
    }

    #[test]
    fn function_rename_smooth3() {
        assert_mdl("smth3(x, 5)", "SMOOTH3(x, 5)");
    }

    #[test]
    fn function_rename_safediv() {
        assert_mdl("safediv(a, b)", "ZIDZ(a, b)");
    }

    #[test]
    fn function_rename_init() {
        assert_mdl("init(x, 10)", "ACTIVE INITIAL(x, 10)");
    }

    #[test]
    fn function_rename_int() {
        assert_mdl("int(x)", "INTEGER(x)");
    }

    #[test]
    fn function_rename_uniform() {
        assert_mdl("uniform(0, 1)", "RANDOM UNIFORM(0, 1)");
    }

    #[test]
    fn function_rename_forcst() {
        assert_mdl("forcst(x, 5, 0)", "FORECAST(x, 5, 0)");
    }

    #[test]
    fn function_rename_delay() {
        assert_mdl("delay(x, 5, 0)", "DELAY FIXED(x, 5, 0)");
    }

    #[test]
    fn function_rename_delay1() {
        assert_mdl("delay1(x, 5)", "DELAY1(x, 5)");
    }

    #[test]
    fn function_rename_delay3() {
        assert_mdl("delay3(x, 5)", "DELAY3(x, 5)");
    }

    #[test]
    fn function_rename_integ() {
        assert_mdl(
            "integ(inflow - outflow, 100)",
            "INTEG(inflow - outflow, 100)",
        );
    }

    #[test]
    fn function_rename_lookupinv() {
        assert_mdl("lookupinv(tbl, 0.5)", "LOOKUP INVERT(tbl, 0.5)");
    }

    #[test]
    fn function_rename_normalpink() {
        assert_mdl("normalpink(x, 5)", "RANDOM PINK NOISE(x, 5)");
    }

    #[test]
    fn function_rename_lookup() {
        assert_mdl("lookup(tbl, x)", "LOOKUP(tbl, x)");
    }

    #[test]
    fn function_unknown_uppercased() {
        assert_mdl("abs(x)", "ABS(x)");
        assert_mdl("ln(x)", "LN(x)");
        assert_mdl("max(a, b)", "MAX(a, b)");
    }

    #[test]
    fn arg_reorder_delay_n() {
        // XMILE: delayn(input, delay_time, n, init) -> MDL: DELAY N(input, delay_time, init, n)
        assert_mdl(
            "delayn(input, delay_time, 3, init_val)",
            "DELAY N(input, delay time, init val, 3)",
        );
    }

    #[test]
    fn arg_reorder_smooth_n() {
        // XMILE: smthn(input, delay_time, n, init) -> MDL: SMOOTH N(input, delay_time, init, n)
        assert_mdl(
            "smthn(input, delay_time, 3, init_val)",
            "SMOOTH N(input, delay time, init val, 3)",
        );
    }

    #[test]
    fn arg_reorder_random_normal() {
        // XMILE: normal(mean, sd, seed, min, max) -> MDL: RANDOM NORMAL(min, max, mean, sd, seed)
        assert_mdl(
            "normal(mean, sd, seed, min_val, max_val)",
            "RANDOM NORMAL(min val, max val, mean, sd, seed)",
        );
    }
}
