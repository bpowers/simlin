// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::BinaryOp;

use super::dimensions::UnaryOp;
use super::expr::{BuiltinFn, Expr, SubscriptIndex};

fn child_needs_parens(parent: &Expr, child: &Expr) -> bool {
    match parent {
        // no children so doesn't matter
        Expr::Const(_, _) | Expr::Var(_, _) => false,
        // children are comma separated, so no ambiguity possible
        Expr::App(_, _)
        | Expr::Subscript(_, _, _, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _) => false,
        // these don't need it
        Expr::Dt(_)
        | Expr::EvalModule(_, _, _, _)
        | Expr::ModuleInput(_, _)
        | Expr::AssignCurr(_, _)
        | Expr::AssignNext(_, _)
        | Expr::AssignTemp(_, _, _) => false,
        Expr::Op1(_, _, _) => matches!(child, Expr::Op2(_, _, _, _)),
        Expr::Op2(parent_op, _, _, _) => match child {
            Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::App(_, _)
            | Expr::Subscript(_, _, _, _)
            | Expr::StaticSubscript(_, _, _)
            | Expr::TempArray(_, _, _)
            | Expr::TempArrayElement(_, _, _, _)
            | Expr::If(_, _, _, _)
            | Expr::Dt(_)
            | Expr::EvalModule(_, _, _, _)
            | Expr::ModuleInput(_, _)
            | Expr::AssignCurr(_, _)
            | Expr::AssignNext(_, _)
            | Expr::AssignTemp(_, _, _)
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
        format!("({eqn})")
    } else {
        eqn
    }
}

fn pretty_subscript_index(idx: &SubscriptIndex) -> String {
    match idx {
        SubscriptIndex::Single(e) => pretty(e),
        SubscriptIndex::Range(start, end) => format!("{}:{}", pretty(start), pretty(end)),
    }
}

#[allow(dead_code)]
pub fn pretty(expr: &Expr) -> String {
    match expr {
        Expr::Const(n, _) => format!("{n}"),
        Expr::Var(off, _) => format!("curr[{off}]"),
        Expr::StaticSubscript(off, view, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            let strides: Vec<_> = view.strides.iter().map(|s| format!("{s}")).collect();
            format!(
                "curr[{off} + view(dims: [{}], strides: [{}], offset: {})]",
                dims.join(", "),
                strides.join(", "),
                view.offset
            )
        }
        Expr::TempArray(id, view, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            let strides: Vec<_> = view.strides.iter().map(|s| format!("{s}")).collect();
            format!(
                "temp[{id}] + view(dims: [{}], strides: [{}], offset: {})",
                dims.join(", "),
                strides.join(", "),
                view.offset
            )
        }
        Expr::TempArrayElement(id, view, idx, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            format!("temp[{id}][{idx}] (dims: [{}])", dims.join(", "))
        }
        Expr::Subscript(off, args, bounds, _) => {
            let args: Vec<_> = args.iter().map(pretty_subscript_index).collect();
            let string_args = args.join(", ");
            let bounds: Vec<_> = bounds.iter().map(|bounds| format!("{bounds}")).collect();
            let string_bounds = bounds.join(", ");
            format!("curr[{off} + (({string_args}) - 1); bounds: {string_bounds}]")
        }
        Expr::Dt(_) => "dt".to_string(),
        Expr::App(builtin, _) => match builtin {
            BuiltinFn::Time => "time".to_string(),
            BuiltinFn::TimeStep => "time_step".to_string(),
            BuiltinFn::StartTime => "initial_time".to_string(),
            BuiltinFn::FinalTime => "final_time".to_string(),
            BuiltinFn::Lookup(table, idx, _loc) => {
                format!("lookup({}, {})", pretty(table), pretty(idx))
            }
            BuiltinFn::LookupForward(table, idx, _loc) => {
                format!("lookup_forward({}, {})", pretty(table), pretty(idx))
            }
            BuiltinFn::LookupBackward(table, idx, _loc) => {
                format!("lookup_backward({}, {})", pretty(table), pretty(idx))
            }
            BuiltinFn::Abs(l) => format!("abs({})", pretty(l)),
            BuiltinFn::Arccos(l) => format!("arccos({})", pretty(l)),
            BuiltinFn::Arcsin(l) => format!("arcsin({})", pretty(l)),
            BuiltinFn::Arctan(l) => format!("arctan({})", pretty(l)),
            BuiltinFn::Cos(l) => format!("cos({})", pretty(l)),
            BuiltinFn::Exp(l) => format!("exp({})", pretty(l)),
            BuiltinFn::Inf => "\u{221e}".to_string(),
            BuiltinFn::Int(l) => format!("int({})", pretty(l)),
            BuiltinFn::IsModuleInput(ident, _loc) => format!("isModuleInput({ident})"),
            BuiltinFn::Ln(l) => format!("ln({})", pretty(l)),
            BuiltinFn::Log10(l) => format!("log10({})", pretty(l)),
            BuiltinFn::Max(l, r) => {
                if let Some(r) = r {
                    format!("max({}, {})", pretty(l), pretty(r))
                } else {
                    format!("max({})", pretty(l))
                }
            }
            BuiltinFn::Mean(args) => {
                let args: Vec<_> = args.iter().map(pretty).collect();
                let string_args = args.join(", ");
                format!("mean({string_args})")
            }
            BuiltinFn::Min(l, r) => {
                if let Some(r) = r {
                    format!("min({}, {})", pretty(l), pretty(r))
                } else {
                    format!("min({})", pretty(l))
                }
            }
            BuiltinFn::Pi => "\u{1D70B}".to_string(),
            BuiltinFn::Pulse(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("pulse({}, {}, {})", pretty(a), pretty(b), c)
            }
            BuiltinFn::Ramp(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("ramp({}, {}, {})", pretty(a), pretty(b), c)
            }
            BuiltinFn::SafeDiv(a, b, c) => format!(
                "safediv({}, {}, {})",
                pretty(a),
                pretty(b),
                c.as_ref()
                    .map(|expr| pretty(expr))
                    .unwrap_or_else(|| "<None>".to_string())
            ),
            BuiltinFn::Sign(l) => format!("sign({})", pretty(l)),
            BuiltinFn::Sin(l) => format!("sin({})", pretty(l)),
            BuiltinFn::Sqrt(l) => format!("sqrt({})", pretty(l)),
            BuiltinFn::Step(a, b) => {
                format!("step({}, {})", pretty(a), pretty(b))
            }
            BuiltinFn::Tan(l) => format!("tan({})", pretty(l)),
            BuiltinFn::Rank(a, b) => {
                if let Some((b, c)) = b {
                    if let Some(c) = c {
                        format!("rank({}, {}, {})", pretty(a), pretty(b), pretty(c))
                    } else {
                        format!("rank({}, {})", pretty(a), pretty(b))
                    }
                } else {
                    format!("rank({})", pretty(a))
                }
            }
            BuiltinFn::Size(a) => format!("size({})", pretty(a)),
            BuiltinFn::Stddev(a) => format!("stddev({})", pretty(a)),
            BuiltinFn::Sum(a) => format!("sum({})", pretty(a)),
        },
        Expr::EvalModule(module, model_name, _input_set, args) => {
            let args: Vec<_> = args.iter().map(pretty).collect();
            let string_args = args.join(", ");
            format!("eval<{module}::{model_name}>({string_args})")
        }
        Expr::ModuleInput(a, _) => format!("mi<{a}>"),
        Expr::Op2(op, l, r, _) => {
            let op: &str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Exp => "^",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::Gt => ">",
                BinaryOp::Gte => ">=",
                BinaryOp::Lt => "<",
                BinaryOp::Lte => "<=",
                BinaryOp::Eq => "==",
                BinaryOp::Neq => "!=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
            };

            format!(
                "{} {} {}",
                paren_if_necessary(expr, l, pretty(l)),
                op,
                paren_if_necessary(expr, r, pretty(r))
            )
        }
        Expr::Op1(op, l, _) => {
            let op: &str = match op {
                UnaryOp::Not => "!",
                UnaryOp::Transpose => "'",
            };
            format!("{}{}", op, paren_if_necessary(expr, l, pretty(l)))
        }
        Expr::If(cond, l, r, _) => {
            format!("if {} then {} else {}", pretty(cond), pretty(l), pretty(r))
        }
        Expr::AssignCurr(off, rhs) => format!("curr[{}] := {}", off, pretty(rhs)),
        Expr::AssignNext(off, rhs) => format!("next[{}] := {}", off, pretty(rhs)),
        Expr::AssignTemp(id, expr, view) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            format!("temp[{id}][{}] <- {}", dims.join(", "), pretty(expr))
        }
    }
}
